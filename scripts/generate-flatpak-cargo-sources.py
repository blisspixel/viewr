#!/usr/bin/env python3
"""Generate checksum-pinned Flatpak Cargo sources from Cargo.lock."""

from __future__ import annotations

import json
from pathlib import Path
import tomllib


REPOSITORY_ROOT = Path(__file__).resolve().parent.parent
LOCKFILE = REPOSITORY_ROOT / "Cargo.lock"
OUTPUT = REPOSITORY_ROOT / "packaging" / "flatpak" / "cargo-sources.json"
CRATES_IO_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"


def crate_sources(name: str, version: str, checksum: str) -> list[dict[str, str]]:
    archive = f"{name}-{version}.crate"
    destination = f"cargo/vendor/{name}-{version}"
    return [
        {
            "type": "archive",
            "archive-type": "tar-gzip",
            "url": f"https://static.crates.io/crates/{name}/{archive}",
            "sha256": checksum,
            "dest": destination,
        },
        {
            "type": "inline",
            "contents": json.dumps({"package": checksum, "files": {}}),
            "dest": destination,
            "dest-filename": ".cargo-checksum.json",
        },
    ]


def main() -> None:
    lock = tomllib.loads(LOCKFILE.read_text(encoding="utf-8"))
    packages: list[tuple[str, str, str]] = []
    seen: set[tuple[str, str]] = set()

    for package in lock["package"]:
        source = package.get("source")
        if source is None:
            continue
        if source != CRATES_IO_SOURCE:
            raise ValueError(f"unsupported Cargo source for {package['name']}: {source}")
        key = (package["name"], package["version"])
        if key in seen:
            continue
        seen.add(key)
        checksum = package.get("checksum")
        if not checksum:
            raise ValueError(f"missing checksum for {package['name']} {package['version']}")
        packages.append((*key, checksum))

    sources: list[dict[str, str]] = []
    for package in sorted(packages):
        sources.extend(crate_sources(*package))
    sources.append(
        {
            "type": "inline",
            "contents": (
                "[source.crates-io]\n"
                'replace-with = "vendored-sources"\n\n'
                "[source.vendored-sources]\n"
                'directory = "vendor"\n'
            ),
            "dest": "cargo",
            "dest-filename": "config.toml",
        }
    )
    with OUTPUT.open("w", encoding="utf-8", newline="\n") as output:
        output.write(json.dumps(sources, indent=2) + "\n")


if __name__ == "__main__":
    main()
