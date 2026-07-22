#!/usr/bin/env python3
"""Build and verify deterministic, checksummed viewr release archives."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from datetime import datetime, timezone
import hashlib
import hmac
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import struct
import subprocess
import sys
import tempfile
import tomllib
from typing import BinaryIO
import zipfile


REPOSITORY_ROOT = Path(__file__).resolve().parent.parent
SUPPORTED_TARGETS = frozenset(
    {
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "x86_64-pc-windows-msvc",
        "x86_64-unknown-linux-gnu",
    }
)
MANIFEST_NAME = "release-manifest.json"
MANIFEST_SCHEMA_VERSION = 1
ARCHIVE_FORMAT = "zip-store-v1"
MAX_MANIFEST_BYTES = 1024 * 1024
MAX_BINARY_HEADER_BYTES = 1024 * 1024 + 512
ZIP_END_RECORD = struct.Struct("<4s4H2LH")
VERSION_PATTERN = re.compile(
    r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:[-+][0-9A-Za-z.-]+)?"
)
SHA256_PATTERN = re.compile(r"[0-9a-f]{64}")
TOOLCHAIN_PATTERN = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")


class ReleaseError(ValueError):
    """A release artifact violated the repository's packaging contract."""


@dataclass(frozen=True)
class ArchiveEntry:
    """One immutable payload entry and its Unix archive mode."""

    relative_path: str
    mode: int
    payload: Path | bytes

    @property
    def size(self) -> int:
        if isinstance(self.payload, bytes):
            return len(self.payload)
        return self.payload.stat().st_size

    def digest(self) -> str:
        if isinstance(self.payload, bytes):
            return hashlib.sha256(self.payload).hexdigest()
        return sha256_file(self.payload)

    def copy_to(self, destination: BinaryIO) -> None:
        if isinstance(self.payload, bytes):
            destination.write(self.payload)
            return
        with self.payload.open("rb") as source:
            shutil.copyfileobj(source, destination, length=1024 * 1024)


def sha256_file(path: Path) -> str:
    """Return a lower-case SHA-256 digest without loading the file into memory."""
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _is_reparse_point(path: Path) -> bool:
    if path.is_symlink():
        return True
    attributes = getattr(path.lstat(), "st_file_attributes", 0)
    reparse_attribute = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)
    return bool(attributes & reparse_attribute)


def _require_regular_file(path: Path, description: str) -> None:
    try:
        if _is_reparse_point(path) or not path.is_file():
            raise ReleaseError(f"{description} must be a regular non-link file: {path}")
    except FileNotFoundError as error:
        raise ReleaseError(f"missing {description}: {path}") from error


def _require_directory_below_target(repository_root: Path, directory: Path) -> Path:
    target_root = (repository_root / "target").resolve(strict=True)
    candidate = directory if directory.is_absolute() else repository_root / directory
    unresolved = candidate.absolute()
    current = unresolved
    while current != repository_root:
        if current.exists() and _is_reparse_point(current):
            raise ReleaseError(
                f"binary directory contains a link or reparse point: {current}"
            )
        if current == current.parent:
            break
        current = current.parent

    try:
        resolved = candidate.resolve(strict=True)
        resolved.relative_to(target_root)
    except (FileNotFoundError, ValueError) as error:
        raise ReleaseError(
            "binary directory must be an existing directory below the target directory"
        ) from error
    if not resolved.is_dir():
        raise ReleaseError(
            "binary directory must be an existing directory below the target directory"
        )
    return resolved


def _load_identity(repository_root: Path) -> tuple[str, str]:
    cargo_path = repository_root / "Cargo.toml"
    toolchain_path = repository_root / "rust-toolchain.toml"
    _require_regular_file(cargo_path, "workspace manifest")
    _require_regular_file(toolchain_path, "toolchain manifest")
    try:
        cargo = tomllib.loads(cargo_path.read_text(encoding="utf-8"))
        toolchain = tomllib.loads(toolchain_path.read_text(encoding="utf-8"))
        version = cargo["workspace"]["package"]["version"]
        channel = toolchain["toolchain"]["channel"]
    except (KeyError, tomllib.TOMLDecodeError) as error:
        raise ReleaseError(
            f"invalid release identity configuration: {error}"
        ) from error
    if not isinstance(version, str) or VERSION_PATTERN.fullmatch(version) is None:
        raise ReleaseError("workspace version must be an explicit semantic version")
    if not isinstance(channel, str) or TOOLCHAIN_PATTERN.fullmatch(channel) is None:
        raise ReleaseError(
            "release toolchain must be pinned to an exact stable version"
        )
    return version, channel


def _canonical_text(path: Path, description: str) -> bytes:
    _require_regular_file(path, description)
    try:
        text = path.read_text(encoding="utf-8")
    except UnicodeDecodeError as error:
        raise ReleaseError(f"{description} must be UTF-8 text: {path}") from error
    return text.replace("\r\n", "\n").replace("\r", "\n").encode("utf-8")


def _binary_names(target: str) -> tuple[str, str]:
    suffix = ".exe" if target.endswith("-windows-msvc") else ""
    return f"viewr{suffix}", f"viewr-decode{suffix}"


def _require_binary_target_bytes(
    header: bytes, file_size: int, target: str, description: str
) -> None:
    if target == "x86_64-pc-windows-msvc":
        if len(header) < 64 or header[:2] != b"MZ":
            raise ReleaseError(f"binary is not a Windows PE executable: {description}")
        pe_offset = int.from_bytes(header[60:64], "little")
        if pe_offset > 1024 * 1024 or pe_offset + 44 > len(header):
            raise ReleaseError(
                f"binary has an invalid Windows PE header offset: {description}"
            )
        pe_header = header[pe_offset : pe_offset + 44]
        section_count = int.from_bytes(pe_header[6:8], "little")
        optional_header_size = int.from_bytes(pe_header[20:22], "little")
        optional_header_offset = pe_offset + 24
        section_table_offset = optional_header_offset + optional_header_size
        section_table_end = section_table_offset + section_count * 40
        if (
            pe_header[:6] != b"PE\0\0\x64\x86"
            or section_count == 0
            or optional_header_size < 112
            or int.from_bytes(pe_header[22:24], "little") & 0x0002 == 0
            or int.from_bytes(pe_header[24:26], "little") != 0x020B
            or section_table_end > len(header)
            or section_table_end > file_size
        ):
            raise ReleaseError(
                f"binary is not a structurally valid x86-64 Windows PE executable: {description}"
            )
        entry_rva = int.from_bytes(
            header[optional_header_offset + 16 : optional_header_offset + 20],
            "little",
        )
        entry_is_executable = False
        for index in range(section_count):
            section_offset = section_table_offset + index * 40
            section = header[section_offset : section_offset + 40]
            virtual_size = int.from_bytes(section[8:12], "little")
            virtual_address = int.from_bytes(section[12:16], "little")
            raw_size = int.from_bytes(section[16:20], "little")
            raw_offset = int.from_bytes(section[20:24], "little")
            characteristics = int.from_bytes(section[36:40], "little")
            mapped_size = max(virtual_size, raw_size)
            if raw_size > 0 and raw_offset + raw_size > file_size:
                raise ReleaseError(
                    f"binary has an out-of-bounds Windows PE section: {description}"
                )
            if (
                characteristics & 0x20000020 == 0x20000020
                and raw_size > 0
                and virtual_address <= entry_rva
                and entry_rva < virtual_address + min(mapped_size, raw_size)
            ):
                entry_is_executable = True
        if entry_rva == 0 or not entry_is_executable:
            raise ReleaseError(
                f"binary entry point is not mapped by executable Windows PE code: {description}"
            )
        return

    if target == "x86_64-unknown-linux-gnu":
        if len(header) < 64:
            raise ReleaseError(
                f"binary is not a complete x86-64 ELF executable: {description}"
            )
        program_offset = int.from_bytes(header[32:40], "little")
        program_entry_size = int.from_bytes(header[54:56], "little")
        program_count = int.from_bytes(header[56:58], "little")
        program_table_end = program_offset + program_entry_size * program_count
        if (
            header[:7] != b"\x7fELF\x02\x01\x01"
            or int.from_bytes(header[16:18], "little") not in {2, 3}
            or int.from_bytes(header[18:20], "little") != 0x3E
            or int.from_bytes(header[20:24], "little") != 1
            or int.from_bytes(header[52:54], "little") != 64
            or program_offset < 64
            or program_entry_size < 56
            or program_count == 0
            or program_table_end > len(header)
            or program_table_end > file_size
        ):
            raise ReleaseError(
                f"binary is not a structurally valid x86-64 little-endian ELF executable: {description}"
            )
        entry_address = int.from_bytes(header[24:32], "little")
        entry_is_executable = False
        for index in range(program_count):
            entry_offset = program_offset + index * program_entry_size
            program = header[entry_offset : entry_offset + 56]
            program_type = int.from_bytes(program[:4], "little")
            flags = int.from_bytes(program[4:8], "little")
            file_offset = int.from_bytes(program[8:16], "little")
            virtual_address = int.from_bytes(program[16:24], "little")
            file_segment_size = int.from_bytes(program[32:40], "little")
            memory_segment_size = int.from_bytes(program[40:48], "little")
            if program_type == 1 and (
                file_segment_size > memory_segment_size
                or file_offset + file_segment_size > file_size
            ):
                raise ReleaseError(
                    f"binary has an out-of-bounds ELF load segment: {description}"
                )
            if (
                program_type == 1
                and flags & 0x1 != 0
                and file_segment_size > 0
                and virtual_address <= entry_address
                and entry_address < virtual_address + file_segment_size
            ):
                entry_is_executable = True
        if entry_address == 0 or not entry_is_executable:
            raise ReleaseError(
                f"binary entry point is not mapped by an executable ELF load segment: {description}"
            )
        return

    expected_cpu = 0x0100000C if target == "aarch64-apple-darwin" else 0x01000007
    command_count = int.from_bytes(header[16:20], "little") if len(header) >= 24 else 0
    command_bytes = int.from_bytes(header[20:24], "little") if len(header) >= 24 else 0
    commands_end = 32 + command_bytes
    if (
        len(header) < 32
        or header[:4] != b"\xcf\xfa\xed\xfe"
        or int.from_bytes(header[4:8], "little") != expected_cpu
        or int.from_bytes(header[12:16], "little") != 2
        or command_count == 0
        or command_bytes < command_count * 8
        or commands_end > len(header)
        or commands_end > file_size
    ):
        raise ReleaseError(
            f"binary is not a structurally valid thin 64-bit Mach-O for {target}: {description}"
        )
    command_offset = 32
    executable_segments: list[tuple[int, int]] = []
    entry_file_offset: int | None = None
    for _ in range(command_count):
        if command_offset + 8 > commands_end:
            raise ReleaseError(
                f"binary has a truncated Mach-O load command: {description}"
            )
        command = int.from_bytes(header[command_offset : command_offset + 4], "little")
        command_size = int.from_bytes(
            header[command_offset + 4 : command_offset + 8], "little"
        )
        next_command = command_offset + command_size
        if command_size < 8 or next_command > commands_end:
            raise ReleaseError(
                f"binary has an invalid Mach-O load command: {description}"
            )
        if command == 0x19:
            if command_size < 72:
                raise ReleaseError(
                    f"binary has a truncated Mach-O segment command: {description}"
                )
            file_offset = int.from_bytes(
                header[command_offset + 40 : command_offset + 48], "little"
            )
            file_segment_size = int.from_bytes(
                header[command_offset + 48 : command_offset + 56], "little"
            )
            init_protection = int.from_bytes(
                header[command_offset + 60 : command_offset + 64], "little"
            )
            section_count = int.from_bytes(
                header[command_offset + 64 : command_offset + 68], "little"
            )
            if (
                command_size < 72 + section_count * 80
                or file_offset + file_segment_size > file_size
            ):
                raise ReleaseError(
                    f"binary has an invalid Mach-O segment command: {description}"
                )
            if init_protection & 0x4 != 0 and file_segment_size > 0:
                executable_segments.append(
                    (file_offset, file_offset + file_segment_size)
                )
        elif command == 0x80000028:
            if command_size < 24:
                raise ReleaseError(
                    f"binary has a truncated Mach-O entry-point command: {description}"
                )
            entry_file_offset = int.from_bytes(
                header[command_offset + 8 : command_offset + 16], "little"
            )
        command_offset = next_command
    if command_offset != commands_end:
        raise ReleaseError(f"binary has malformed Mach-O load commands: {description}")
    if entry_file_offset is None or not any(
        start <= entry_file_offset < end for start, end in executable_segments
    ):
        raise ReleaseError(
            f"binary entry point is not mapped by an executable Mach-O segment: {description}"
        )


def _require_binary_target(path: Path, target: str) -> None:
    """Reject an executable whose structure does not match its target label."""
    file_size = path.stat().st_size
    with path.open("rb") as binary:
        header = binary.read(MAX_BINARY_HEADER_BYTES)
    _require_binary_target_bytes(header, file_size, target, str(path))


def _zip_timestamp(source_date_epoch: int) -> tuple[int, int, int, int, int, int]:
    if isinstance(source_date_epoch, bool) or source_date_epoch < 0:
        raise ReleaseError("SOURCE_DATE_EPOCH must be a non-negative integer")
    try:
        timestamp = datetime.fromtimestamp(source_date_epoch, timezone.utc)
    except (OSError, OverflowError, ValueError) as error:
        raise ReleaseError(
            "SOURCE_DATE_EPOCH is outside the supported date range"
        ) from error
    if timestamp.year < 1980:
        return (1980, 1, 1, 0, 0, 0)
    if timestamp.year > 2107:
        return (2107, 12, 31, 23, 59, 58)
    return (
        timestamp.year,
        timestamp.month,
        timestamp.day,
        timestamp.hour,
        timestamp.minute,
        timestamp.second - timestamp.second % 2,
    )


def _zip_info(
    name: str, mode: int, timestamp: tuple[int, int, int, int, int, int]
) -> zipfile.ZipInfo:
    info = zipfile.ZipInfo(name, date_time=timestamp)
    info.compress_type = zipfile.ZIP_STORED
    info.create_system = 3
    info.external_attr = (stat.S_IFREG | mode) << 16
    return info


def _serialize_manifest(manifest: dict[str, object]) -> bytes:
    return (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode("utf-8")


def _write_archive(
    destination: Path,
    prefix: str,
    entries: list[ArchiveEntry],
    manifest: dict[str, object],
    timestamp: tuple[int, int, int, int, int, int],
) -> None:
    manifest_entry = ArchiveEntry(
        relative_path=MANIFEST_NAME,
        mode=0o644,
        payload=_serialize_manifest(manifest),
    )
    with zipfile.ZipFile(
        destination, mode="w", compression=zipfile.ZIP_STORED, allowZip64=True
    ) as archive:
        for entry in sorted(
            [*entries, manifest_entry], key=lambda item: item.relative_path
        ):
            member_name = f"{prefix}/{entry.relative_path}"
            info = _zip_info(member_name, entry.mode, timestamp)
            with archive.open(info, mode="w", force_zip64=True) as output:
                entry.copy_to(output)


def _validate_member_name(name: str) -> None:
    if "\\" in name:
        raise ReleaseError(f"archive member uses a non-portable separator: {name}")
    path = PurePosixPath(name)
    if path.is_absolute() or any(part in {"", ".", ".."} for part in path.parts):
        raise ReleaseError(f"archive member has an unsafe path: {name}")


def _require_canonical_zip_envelope(archive_path: Path) -> None:
    size = archive_path.stat().st_size
    if size < ZIP_END_RECORD.size:
        raise ReleaseError("release archive is too short to be a canonical ZIP")
    with archive_path.open("rb") as archive:
        if archive.read(4) != b"PK\x03\x04":
            raise ReleaseError("release archive has data before its first ZIP member")
        archive.seek(-ZIP_END_RECORD.size, os.SEEK_END)
        end_record = ZIP_END_RECORD.unpack(archive.read(ZIP_END_RECORD.size))
    (
        signature,
        disk_number,
        central_directory_disk,
        entries_on_disk,
        total_entries,
        central_directory_size,
        central_directory_offset,
        comment_length,
    ) = end_record
    if (
        signature != b"PK\x05\x06"
        or disk_number != 0
        or central_directory_disk != 0
        or entries_on_disk != total_entries
        or total_entries == 0
        or comment_length != 0
        or central_directory_offset + central_directory_size
        != size - ZIP_END_RECORD.size
    ):
        raise ReleaseError("release archive has a non-canonical ZIP envelope")


def _validated_manifest(
    archive: zipfile.ZipFile, archive_name: str
) -> dict[str, object]:
    if archive.comment:
        raise ReleaseError("release archive must not contain an archive comment")
    infos = archive.infolist()
    names = [info.filename for info in infos]
    if len(names) != len(set(names)):
        raise ReleaseError("archive contains duplicate member names")
    for info in infos:
        _validate_member_name(info.filename)
        if info.is_dir() or info.compress_type != zipfile.ZIP_STORED:
            raise ReleaseError(
                f"archive member must be a stored regular file: {info.filename}"
            )
        if info.flag_bits & 1:
            raise ReleaseError(f"archive member must not be encrypted: {info.filename}")
        if info.comment or info.extra:
            raise ReleaseError(
                f"archive member contains non-canonical metadata: {info.filename}"
            )
        archived_mode = info.external_attr >> 16
        if not stat.S_ISREG(archived_mode):
            raise ReleaseError(f"archive member is not a regular file: {info.filename}")

    manifest_infos = [
        info for info in infos if PurePosixPath(info.filename).name == MANIFEST_NAME
    ]
    if len(manifest_infos) != 1:
        raise ReleaseError("archive must contain exactly one release manifest")
    manifest_info = manifest_infos[0]
    if manifest_info.file_size > MAX_MANIFEST_BYTES:
        raise ReleaseError("release manifest exceeds its safety limit")
    try:
        manifest_bytes = archive.read(manifest_info)
        manifest = json.loads(manifest_bytes)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ReleaseError("release manifest is not valid UTF-8 JSON") from error
    if (
        not isinstance(manifest, dict)
        or _serialize_manifest(manifest) != manifest_bytes
    ):
        raise ReleaseError("release manifest is not in canonical form")

    expected_keys = {
        "archive_format",
        "files",
        "package_name",
        "rust_toolchain",
        "schema_version",
        "source_date_epoch",
        "target",
        "version",
    }
    if set(manifest) != expected_keys:
        raise ReleaseError("release manifest has an unexpected field set")
    if (
        manifest["schema_version"] != MANIFEST_SCHEMA_VERSION
        or manifest["archive_format"] != ARCHIVE_FORMAT
    ):
        raise ReleaseError("release manifest uses an unsupported schema")
    if manifest["package_name"] != "viewr":
        raise ReleaseError("release manifest has an unexpected package name")
    version = manifest["version"]
    target = manifest["target"]
    toolchain = manifest["rust_toolchain"]
    source_date_epoch = manifest["source_date_epoch"]
    if not isinstance(version, str) or VERSION_PATTERN.fullmatch(version) is None:
        raise ReleaseError("release manifest has an invalid version")
    if not isinstance(target, str) or target not in SUPPORTED_TARGETS:
        raise ReleaseError("release manifest has an unsupported target")
    if not isinstance(toolchain, str) or TOOLCHAIN_PATTERN.fullmatch(toolchain) is None:
        raise ReleaseError("release manifest has an unpinned toolchain")
    if (
        isinstance(source_date_epoch, bool)
        or not isinstance(source_date_epoch, int)
        or source_date_epoch < 0
    ):
        raise ReleaseError("release manifest has an invalid SOURCE_DATE_EPOCH")

    prefix = f"viewr-{version}-{target}"
    if (
        archive_name != f"{prefix}.zip"
        or manifest_info.filename != f"{prefix}/{MANIFEST_NAME}"
    ):
        raise ReleaseError(
            "archive name, target, version, and manifest prefix do not agree"
        )
    files = manifest["files"]
    if not isinstance(files, list):
        raise ReleaseError("release manifest files must be a list")
    main_binary, worker_binary = _binary_names(target)
    expected_paths = {
        "LICENSE",
        "README.md",
        f"bin/{main_binary}",
        f"bin/{worker_binary}",
    }
    expected_members = {f"{prefix}/{path}" for path in expected_paths}
    expected_members.add(f"{prefix}/{MANIFEST_NAME}")
    if set(names) != expected_members:
        raise ReleaseError("archive member set does not match the release contract")
    expected_timestamp = _zip_timestamp(source_date_epoch)
    for info in infos:
        if info.date_time != expected_timestamp:
            raise ReleaseError(
                f"archive member has a non-reproducible timestamp: {info.filename}"
            )
    manifest_mode = manifest_info.external_attr >> 16
    if stat.S_IMODE(manifest_mode) != 0o644:
        raise ReleaseError("release manifest must use mode 0644")

    seen_paths: set[str] = set()
    for file_record in files:
        if not isinstance(file_record, dict) or set(file_record) != {
            "mode",
            "path",
            "sha256",
            "size",
        }:
            raise ReleaseError("release manifest contains an invalid file record")
        path = file_record["path"]
        digest = file_record["sha256"]
        size = file_record["size"]
        mode = file_record["mode"]
        if (
            not isinstance(path, str)
            or path not in expected_paths
            or path in seen_paths
        ):
            raise ReleaseError(
                "release manifest contains an unexpected or duplicate path"
            )
        if not isinstance(digest, str) or SHA256_PATTERN.fullmatch(digest) is None:
            raise ReleaseError(
                f"release manifest contains an invalid checksum for {path}"
            )
        if isinstance(size, bool) or not isinstance(size, int) or size < 0:
            raise ReleaseError(f"release manifest contains an invalid size for {path}")
        expected_mode = "0755" if path.startswith("bin/") else "0644"
        if mode != expected_mode:
            raise ReleaseError(f"release manifest contains an invalid mode for {path}")

        member = archive.getinfo(f"{prefix}/{path}")
        archived_mode = member.external_attr >> 16
        if (
            stat.S_IMODE(archived_mode) != int(expected_mode, 8)
            or member.file_size != size
        ):
            raise ReleaseError(
                f"archive metadata does not match the manifest for {path}"
            )
        member_digest = hashlib.sha256()
        member_header = bytearray()
        with archive.open(member) as source:
            for chunk in iter(lambda: source.read(1024 * 1024), b""):
                member_digest.update(chunk)
                remaining_header = MAX_BINARY_HEADER_BYTES - len(member_header)
                if path.startswith("bin/") and remaining_header > 0:
                    member_header.extend(chunk[:remaining_header])
        if not hmac.compare_digest(member_digest.hexdigest(), digest):
            raise ReleaseError(
                f"archive member checksum does not match the manifest for {path}"
            )
        if path.startswith("bin/"):
            _require_binary_target_bytes(
                bytes(member_header), member.file_size, target, f"archive member {path}"
            )
        seen_paths.add(path)
    if seen_paths != expected_paths:
        raise ReleaseError("release manifest does not describe every required file")
    return manifest


def _verify_archive_contract(
    archive_path: Path, archive_name: str
) -> dict[str, object]:
    try:
        _require_canonical_zip_envelope(archive_path)
        with zipfile.ZipFile(archive_path) as archive:
            return _validated_manifest(archive, archive_name)
    except (OSError, zipfile.BadZipFile, zipfile.LargeZipFile) as error:
        raise ReleaseError(f"invalid release archive: {error}") from error


def build_release_artifact(
    *,
    repository_root: Path,
    target: str,
    binary_directory: Path,
    source_date_epoch: int,
    expected_tag: str | None = None,
) -> Path:
    """Build a deterministic archive and sidecar from prebuilt workspace binaries."""
    repository_root = repository_root.resolve(strict=True)
    if target not in SUPPORTED_TARGETS:
        raise ReleaseError(f"unsupported release target: {target}")
    version, toolchain = _load_identity(repository_root)
    if expected_tag is not None and expected_tag != f"v{version}":
        raise ReleaseError(f"release tag must be v{version}, got {expected_tag}")
    binary_directory = _require_directory_below_target(
        repository_root, binary_directory
    )
    main_binary, worker_binary = _binary_names(target)
    main_path = binary_directory / main_binary
    worker_path = binary_directory / worker_binary
    _require_regular_file(main_path, main_binary)
    _require_regular_file(worker_path, worker_binary)
    _require_binary_target(main_path, target)
    _require_binary_target(worker_path, target)

    entries = [
        ArchiveEntry(
            "LICENSE",
            0o644,
            _canonical_text(repository_root / "LICENSE", "license"),
        ),
        ArchiveEntry(
            "README.md",
            0o644,
            _canonical_text(repository_root / "README.md", "readme"),
        ),
        ArchiveEntry(f"bin/{main_binary}", 0o755, main_path),
        ArchiveEntry(f"bin/{worker_binary}", 0o755, worker_path),
    ]
    manifest: dict[str, object] = {
        "archive_format": ARCHIVE_FORMAT,
        "files": [
            {
                "mode": f"{entry.mode:04o}",
                "path": entry.relative_path,
                "sha256": entry.digest(),
                "size": entry.size,
            }
            for entry in sorted(entries, key=lambda item: item.relative_path)
        ],
        "package_name": "viewr",
        "rust_toolchain": toolchain,
        "schema_version": MANIFEST_SCHEMA_VERSION,
        "source_date_epoch": source_date_epoch,
        "target": target,
        "version": version,
    }
    prefix = f"viewr-{version}-{target}"
    archive_name = f"{prefix}.zip"
    target_root = repository_root / "target"
    if _is_reparse_point(target_root):
        raise ReleaseError("target directory must not be a link or reparse point")
    output_directory = target_root / "release-artifacts"
    output_directory.mkdir(exist_ok=True)
    if _is_reparse_point(output_directory) or not output_directory.is_dir():
        raise ReleaseError("release artifact directory must be a regular directory")
    archive_path = output_directory / archive_name
    sidecar_path = archive_path.with_suffix(archive_path.suffix + ".sha256")
    for output_path in (archive_path, sidecar_path):
        if output_path.exists() and _is_reparse_point(output_path):
            raise ReleaseError(
                f"release output must not be a link or reparse point: {output_path}"
            )

    temporary_archive: Path | None = None
    temporary_sidecar: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            dir=output_directory,
            prefix=f".{archive_name}.",
            suffix=".tmp",
            delete=False,
        ) as temporary:
            temporary_archive = Path(temporary.name)
        _write_archive(
            temporary_archive,
            prefix,
            entries,
            manifest,
            _zip_timestamp(source_date_epoch),
        )
        _verify_archive_contract(temporary_archive, archive_name)
        archive_digest = sha256_file(temporary_archive)
        sidecar_bytes = f"{archive_digest}  {archive_name}\n".encode("ascii")
        with tempfile.NamedTemporaryFile(
            dir=output_directory,
            prefix=f".{archive_name}.sha256.",
            suffix=".tmp",
            delete=False,
        ) as temporary:
            temporary.write(sidecar_bytes)
            temporary_sidecar = Path(temporary.name)
        os.replace(temporary_archive, archive_path)
        temporary_archive = None
        os.replace(temporary_sidecar, sidecar_path)
        temporary_sidecar = None
    finally:
        for temporary_path in (temporary_archive, temporary_sidecar):
            if temporary_path is not None:
                temporary_path.unlink(missing_ok=True)

    verify_release_artifact(archive_path)
    return archive_path


def verify_release_artifact(archive_path: Path) -> dict[str, object]:
    """Verify a sidecar checksum and every member described by the manifest."""
    archive_path = archive_path.resolve(strict=True)
    _require_regular_file(archive_path, "release archive")
    if archive_path.suffix != ".zip":
        raise ReleaseError("release archive must use the .zip format")
    sidecar_path = archive_path.with_suffix(archive_path.suffix + ".sha256")
    _require_regular_file(sidecar_path, "release checksum sidecar")
    try:
        sidecar = sidecar_path.read_text(encoding="ascii")
    except UnicodeDecodeError as error:
        raise ReleaseError("release checksum sidecar must be ASCII") from error
    match = re.fullmatch(r"([0-9a-f]{64})  ([^/\\\r\n]+)\n", sidecar)
    if match is None or match.group(2) != archive_path.name:
        raise ReleaseError("release checksum sidecar has an invalid format or filename")
    actual_digest = sha256_file(archive_path)
    if not hmac.compare_digest(match.group(1), actual_digest):
        raise ReleaseError("release archive checksum does not match its sidecar")
    return _verify_archive_contract(archive_path, archive_path.name)


def _resolve_source_date_epoch(repository_root: Path, explicit: int | None) -> int:
    if explicit is not None:
        return explicit
    environment_value = os.environ.get("SOURCE_DATE_EPOCH")
    if environment_value is not None:
        try:
            return int(environment_value, 10)
        except ValueError as error:
            raise ReleaseError(
                "SOURCE_DATE_EPOCH must contain a base-10 integer"
            ) from error
    try:
        result = subprocess.run(
            ["git", "log", "-1", "--format=%ct"],
            cwd=repository_root,
            check=True,
            capture_output=True,
            text=True,
        )
        return int(result.stdout.strip(), 10)
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        raise ReleaseError(
            "cannot determine SOURCE_DATE_EPOCH from the environment or Git"
        ) from error


def _argument_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    build = subparsers.add_parser("build", help="package prebuilt release binaries")
    build.add_argument("--target", required=True, choices=sorted(SUPPORTED_TARGETS))
    build.add_argument("--binary-dir", required=True, type=Path)
    tag_source = build.add_mutually_exclusive_group()
    tag_source.add_argument("--expected-tag")
    tag_source.add_argument("--expected-tag-from-env", action="store_true")
    build.add_argument("--source-date-epoch", type=int)
    verify = subparsers.add_parser(
        "verify", help="verify an archive and its checksum sidecar"
    )
    verify.add_argument("archive", type=Path)
    return parser


def main(arguments: list[str] | None = None) -> int:
    args = _argument_parser().parse_args(arguments)
    try:
        if args.command == "build":
            expected_tag = args.expected_tag
            if args.expected_tag_from_env:
                expected_tag = os.environ.get("VIEWR_RELEASE_TAG")
                if expected_tag is None:
                    raise ReleaseError(
                        "VIEWR_RELEASE_TAG is required with --expected-tag-from-env"
                    )
            archive = build_release_artifact(
                repository_root=REPOSITORY_ROOT,
                target=args.target,
                binary_directory=args.binary_dir,
                source_date_epoch=_resolve_source_date_epoch(
                    REPOSITORY_ROOT, args.source_date_epoch
                ),
                expected_tag=expected_tag,
            )
            print(f"created and verified {archive}")
        else:
            verify_release_artifact(args.archive)
            print(f"verified {args.archive}")
        return 0
    except (OSError, ReleaseError) as error:
        print(f"release-artifact: error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
