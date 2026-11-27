#!/usr/bin/env python3
"""Measure GLIBC symbol versions on Linux release binaries.

The release job builds the Linux archive on Ubuntu 24.04 and refuses a
binary whose dynamic symbols need a newer glibc than 2.39. That is the
floor Ubuntu 24.04 LTS and Debian 13 can run. The published v0.7.8
archive was linked on Ubuntu 26.04 and needs glibc 2.43.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import subprocess
import sys


GLIBC_SYMBOL = re.compile(r"\bGLIBC_([0-9]+)\.([0-9]+)\b")
MAXIMUM = (2, 39)


def highest_glibc_version(objdump_text: str) -> tuple[int, int] | None:
    """Return the newest GLIBC_ symbol version in objdump -T text."""

    found = [
        (int(major), int(minor)) for major, minor in GLIBC_SYMBOL.findall(objdump_text)
    ]
    if not found:
        return None
    return max(found)


def format_glibc_version(version: tuple[int, int]) -> str:
    """Format a major.minor glibc version without extra zeros."""

    return f"{version[0]}.{version[1]}"


def parse_glibc_version(value: str) -> tuple[int, int]:
    """Parse a major.minor glibc version."""

    if GLIBC_SYMBOL.fullmatch(f"GLIBC_{value}") is None:
        raise ValueError(value)
    major, minor = value.split(".")
    return int(major), int(minor)


def measure_binary(path: Path) -> tuple[int, int]:
    """Run objdump -T and return that binary's highest GLIBC version."""

    completed = subprocess.run(
        ["objdump", "-T", str(path)],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() or f"objdump exited {completed.returncode}"
        raise SystemExit(f"{path}: {detail}")
    highest = highest_glibc_version(completed.stdout)
    if highest is None:
        raise SystemExit(f"{path}: no GLIBC symbol versions")
    return highest


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binaries", nargs="+", type=Path)
    parser.add_argument("--maximum", default=format_glibc_version(MAXIMUM))
    args = parser.parse_args(argv)
    try:
        maximum = parse_glibc_version(args.maximum)
    except ValueError:
        print(f"invalid glibc maximum: {args.maximum}", file=sys.stderr)
        return 2
    highest = max(measure_binary(path) for path in args.binaries)
    if highest > maximum:
        print(
            "glibc "
            f"{format_glibc_version(highest)} is above "
            f"{format_glibc_version(maximum)}",
            file=sys.stderr,
        )
        return 1
    print(format_glibc_version(highest))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
