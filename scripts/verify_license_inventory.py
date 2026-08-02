"""Compare generated third-party license inventories by release meaning."""

from __future__ import annotations

from dataclasses import dataclass
from hashlib import sha256
from pathlib import Path
import sys


class InventoryError(ValueError):
    """Raised when an inventory is malformed or differs from the baseline."""


@dataclass(frozen=True, order=True)
class LicenseSection:
    """One license text and the release packages that use it."""

    license_id: str
    license_name: str
    packages: tuple[str, ...]
    text: str

    def label(self) -> str:
        digest = sha256(self.text.encode("utf-8")).hexdigest()[:12]
        packages = ", ".join(self.packages[:5])
        if len(self.packages) > 5:
            packages += f", +{len(self.packages) - 5} more"
        return f"{self.license_id} ({digest}): {packages}"


@dataclass(frozen=True)
class LicenseInventory:
    """Semantic content extracted from the plain-text cargo-about report."""

    summary: tuple[str, ...]
    sections: tuple[LicenseSection, ...]


def _normalized_newlines(text: str) -> str:
    return text.replace("\r\n", "\n").replace("\r", "\n")


def parse_inventory(path: Path) -> LicenseInventory:
    text = _normalized_newlines(path.read_text(encoding="utf-8"))
    lines = text.split("\n")

    try:
        summary_start = lines.index("## License summary")
        sections_start = lines.index("## License texts and packages")
    except ValueError as error:
        raise InventoryError(
            "license inventory is missing required section headings"
        ) from error

    if sections_start <= summary_start:
        raise InventoryError("license inventory section order is invalid")

    summary: list[str] = []
    for line in lines[summary_start + 1 : sections_start]:
        stripped = line.strip()
        if not stripped:
            continue
        if not stripped.startswith("- "):
            raise InventoryError(f"invalid summary line: {stripped}")
        summary.append(stripped[2:].strip())

    sections: list[LicenseSection] = []
    index = sections_start + 1
    while index < len(lines):
        line = lines[index]
        if not line.strip():
            index += 1
            continue
        if not line.startswith("### "):
            raise InventoryError(f"expected license section heading, found: {line}")
        license_id = line[4:].strip()
        if not license_id:
            raise InventoryError("license section is missing its id")
        index += 1

        if index >= len(lines) or not lines[index].startswith("name: "):
            raise InventoryError(
                f"license section {license_id} is missing its display name"
            )
        license_name = lines[index][len("name: ") :].strip()
        if not license_name:
            raise InventoryError(f"license section {license_id} has an empty name")
        index += 1

        packages: list[str] = []
        while index < len(lines):
            current = lines[index]
            if current == "used_by:":
                index += 1
                while index < len(lines):
                    package_line = lines[index]
                    if package_line.startswith("  ") and package_line.strip():
                        packages.append(package_line.strip())
                        index += 1
                        continue
                    break
                continue
            if current == "text:":
                index += 1
                break
            if current.startswith("### "):
                break
            if not current.strip():
                index += 1
                continue
            raise InventoryError(
                f"license section {license_id} has unexpected content: {current}"
            )

        if not packages:
            raise InventoryError(f"license section {license_id} has no packages")
        if index >= len(lines) or lines[index] != "<<<":
            raise InventoryError(
                f"license section {license_id} is missing its text fence"
            )
        index += 1

        body_lines: list[str] = []
        closed = False
        while index < len(lines):
            if lines[index] == ">>>":
                closed = True
                index += 1
                break
            body_lines.append(lines[index])
            index += 1
        if not closed:
            raise InventoryError(
                f"license section {license_id} has an unclosed text fence"
            )

        section_text = "\n".join(body_lines).strip("\n")
        if not section_text:
            raise InventoryError(f"license section {license_id} has no license text")

        sections.append(
            LicenseSection(
                license_id,
                license_name,
                tuple(sorted(packages)),
                section_text,
            )
        )

    if not summary:
        raise InventoryError("license inventory has no summary")
    if not sections:
        raise InventoryError("license inventory has no license sections")
    return LicenseInventory(tuple(sorted(summary)), tuple(sorted(sections)))


def verify_inventories(committed: Path, generated: Path) -> None:
    expected = parse_inventory(committed)
    actual = parse_inventory(generated)
    problems: list[str] = []
    if expected.summary != actual.summary:
        problems.append("license summary differs")
    if expected.sections != actual.sections:
        missing = sorted(set(expected.sections) - set(actual.sections))
        added = sorted(set(actual.sections) - set(expected.sections))
        if missing:
            problems.append("missing: " + "; ".join(item.label() for item in missing))
        if added:
            problems.append("unexpected: " + "; ".join(item.label() for item in added))
        if not missing and not added:
            problems.append("license section multiplicity differs")
    if problems:
        raise InventoryError(
            "third-party license inventory drift: " + " | ".join(problems)
        )


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print(
            "usage: verify_license_inventory.py COMMITTED_INVENTORY GENERATED_INVENTORY",
            file=sys.stderr,
        )
        return 2
    try:
        verify_inventories(Path(argv[1]), Path(argv[2]))
    except (InventoryError, OSError, UnicodeError) as error:
        print(error, file=sys.stderr)
        return 1
    print("third-party license inventory matches the locked release graph")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
