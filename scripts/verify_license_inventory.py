"""Compare generated third-party license inventories by release meaning."""

from __future__ import annotations

from dataclasses import dataclass
from hashlib import sha256
from html.parser import HTMLParser
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
    """Semantic content extracted from cargo-about's HTML report."""

    summary: tuple[str, ...]
    sections: tuple[LicenseSection, ...]


def _normalized_text(parts: list[str]) -> str:
    return "".join(parts).replace("\r\n", "\n").replace("\r", "\n").strip("\n")


class _InventoryParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.summary: list[str] = []
        self.sections: list[LicenseSection] = []
        self._heading_tag: str | None = None
        self._heading_id = ""
        self._heading_parts: list[str] = []
        self._summary_area = False
        self._license_area = False
        self._summary_parts: list[str] | None = None
        self._in_section = False
        self._in_packages = False
        self._package_parts: list[str] | None = None
        self._packages: list[str] = []
        self._pre_parts: list[str] | None = None
        self._section_text: str | None = None
        self._license_id = ""
        self._license_name = ""

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        attributes = dict(attrs)
        if tag in {"h2", "h3"}:
            self._heading_tag = tag
            self._heading_id = attributes.get("id") or ""
            self._heading_parts = []
        elif tag == "li" and self._summary_area:
            self._summary_parts = []
        elif tag == "section" and self._license_area:
            if self._in_section:
                raise InventoryError("nested license sections are not supported")
            self._in_section = True
            self._packages = []
            self._section_text = None
        elif (
            tag == "div"
            and self._in_section
            and "packages" in (attributes.get("class") or "").split()
        ):
            self._in_packages = True
        elif tag == "a" and self._in_packages:
            self._package_parts = []
        elif tag == "pre" and self._in_section:
            if self._pre_parts is not None or self._section_text is not None:
                raise InventoryError("a license section must contain exactly one text")
            self._pre_parts = []

    def handle_data(self, data: str) -> None:
        if self._heading_tag is not None:
            self._heading_parts.append(data)
        if self._summary_parts is not None:
            self._summary_parts.append(data)
        if self._package_parts is not None:
            self._package_parts.append(data)
        if self._pre_parts is not None:
            self._pre_parts.append(data)

    def handle_endtag(self, tag: str) -> None:
        if tag == self._heading_tag:
            heading = _normalized_text(self._heading_parts).strip()
            if tag == "h2":
                self._summary_area = heading == "License summary"
                self._license_area = heading == "License texts and packages"
            elif tag == "h3" and self._in_section:
                self._license_id = self._heading_id
                self._license_name = heading
            self._heading_tag = None
            self._heading_id = ""
            self._heading_parts = []
        elif tag == "li" and self._summary_parts is not None:
            self.summary.append(_normalized_text(self._summary_parts).strip())
            self._summary_parts = None
        elif tag == "a" and self._package_parts is not None:
            package = _normalized_text(self._package_parts).strip()
            if package:
                self._packages.append(package)
            self._package_parts = None
        elif tag == "div" and self._in_packages:
            self._in_packages = False
        elif tag == "pre" and self._pre_parts is not None:
            self._section_text = _normalized_text(self._pre_parts)
            self._pre_parts = None
        elif tag == "section" and self._in_section:
            if not self._license_id or not self._license_name:
                raise InventoryError("license section is missing its license heading")
            if not self._packages:
                raise InventoryError("license section has no packages")
            if not self._section_text:
                raise InventoryError("license section has no license text")
            self.sections.append(
                LicenseSection(
                    self._license_id,
                    self._license_name,
                    tuple(sorted(self._packages)),
                    self._section_text,
                )
            )
            self._in_section = False

    def inventory(self) -> LicenseInventory:
        if self._in_section or self._pre_parts is not None:
            raise InventoryError("license inventory ended inside a section")
        if not self.summary:
            raise InventoryError("license inventory has no summary")
        if not self.sections:
            raise InventoryError("license inventory has no license sections")
        return LicenseInventory(
            tuple(sorted(self.summary)), tuple(sorted(self.sections))
        )


def parse_inventory(path: Path) -> LicenseInventory:
    parser = _InventoryParser()
    parser.feed(path.read_text(encoding="utf-8"))
    parser.close()
    return parser.inventory()


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
            "usage: verify_license_inventory.py COMMITTED_HTML GENERATED_HTML",
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
