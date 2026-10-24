#!/usr/bin/env python3
"""Regenerate the checked-in dependency records from the locked graph.

Dependabot updates Cargo.lock without these two files. CI then rejects the
pull request. This command rewrites only:

- packaging/flatpak/cargo-sources.json
- THIRD_PARTY_LICENSES.txt
"""

from __future__ import annotations

from pathlib import Path
import subprocess
import sys


REPOSITORY_ROOT = Path(__file__).resolve().parent.parent
CARGO_SOURCES = REPOSITORY_ROOT / "packaging" / "flatpak" / "cargo-sources.json"
LICENSE_INVENTORY = REPOSITORY_ROOT / "THIRD_PARTY_LICENSES.txt"
GENERATED_INVENTORY = REPOSITORY_ROOT / "target" / "THIRD_PARTY_LICENSES.txt"


def normalize_inventory(text: str) -> str:
    """Use LF endings and one trailing newline, independent of the host."""

    normalized = text.replace("\r\n", "\n").replace("\r", "\n")
    if normalized and not normalized.endswith("\n"):
        normalized += "\n"
    return normalized


def _run(command: list[str]) -> None:
    completed = subprocess.run(command, cwd=REPOSITORY_ROOT, check=False)
    if completed.returncode != 0:
        raise SystemExit(completed.returncode)


def main() -> None:
    _run([sys.executable, "-B", "scripts/generate-flatpak-cargo-sources.py"])
    _run(["cargo", "fetch", "--locked"])
    GENERATED_INVENTORY.parent.mkdir(parents=True, exist_ok=True)
    _run(
        [
            "cargo",
            "about",
            "generate",
            "about.hbs",
            "--workspace",
            "--locked",
            "--offline",
            "--fail",
            "--output-file",
            str(GENERATED_INVENTORY),
        ]
    )
    LICENSE_INVENTORY.write_text(
        normalize_inventory(GENERATED_INVENTORY.read_text(encoding="utf-8")),
        encoding="utf-8",
        newline="\n",
    )
    if not CARGO_SOURCES.is_file():
        raise SystemExit("Flatpak Cargo source map was not written")


if __name__ == "__main__":
    main()
