from __future__ import annotations

import hashlib
import io
import json
import os
from pathlib import Path
import struct
import tempfile
import unittest
from unittest import mock
import zipfile
import zlib

from scripts import release_artifact


SOURCE_DATE_EPOCH = 1_788_000_000
PROJECT_ROOT = Path(__file__).resolve().parents[2]
EXPECTED_DOCUMENTATION_PATHS = {
    "CHANGELOG.md",
    "CONTRIBUTING.md",
    "NOTICE",
    "README.md",
    "SECURITY.md",
    "THIRD_PARTY_LICENSES.txt",
    "assets/icon.svg",
    "assets/linux/viewr.desktop",
    "docs/ACCESSIBILITY.md",
    "docs/ARCHITECTURE.md",
    "docs/DESIGN.md",
    "docs/FORMATS.md",
    "docs/INSTALL.md",
    "docs/LOCAL-INTELLIGENCE.md",
    "docs/PERFORMANCE.md",
    "docs/PUBLISHING.md",
    "docs/PRIVACY.md",
    "docs/README.md",
    "docs/RATINGS.md",
    "docs/releases/v0.1.0.md",
    "docs/releases/v0.1.1.md",
    "docs/releases/v0.1.2.md",
    "docs/ROADMAP.md",
    "docs/SANDBOX_PLAN.md",
    "docs/screenshots/viewr-console-example.png",
    "docs/STACK.md",
    "docs/STANDARDS.md",
    "docs/VERIFY.md",
}


class ReleaseArtifactTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary_directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary_directory.cleanup)
        self.repository = Path(self.temporary_directory.name)
        (self.repository / "target" / "release").mkdir(parents=True)
        (self.repository / "Cargo.toml").write_text(
            '[workspace.package]\nversion = "1.2.3"\n',
            encoding="utf-8",
        )
        (self.repository / "rust-toolchain.toml").write_text(
            '[toolchain]\nchannel = "1.96.0"\n',
            encoding="utf-8",
        )
        for relative_path in EXPECTED_DOCUMENTATION_PATHS:
            documentation = self.repository / relative_path
            documentation.parent.mkdir(parents=True, exist_ok=True)
            if relative_path in release_artifact.BINARY_DOCUMENTATION_PATHS:
                documentation.write_bytes(self.png_bytes())
            else:
                documentation.write_text(
                    (
                        "# viewr\n\n"
                        "[Security](SECURITY.md)\n"
                        "[Design](docs/DESIGN.md)\n"
                        "[License](LICENSE)\n"
                        "[Section](#supported)\n"
                        "[Website](https://example.invalid/viewr)\n"
                        if relative_path == "README.md"
                        else f"# {documentation.stem}\n"
                    ),
                    encoding="utf-8",
                )
        (self.repository / "LICENSE").write_bytes(b"license\n")

    @staticmethod
    def png_bytes() -> bytes:
        def chunk(kind: bytes, data: bytes) -> bytes:
            checksum = zlib.crc32(kind + data) & 0xFFFFFFFF
            return (
                struct.pack(">I", len(data)) + kind + data + struct.pack(">I", checksum)
            )

        header = struct.pack(">IIBBBBB", 1, 1, 8, 6, 0, 0, 0)
        pixels = zlib.compress(b"\x00\x00\x00\x00\xff")
        return (
            b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", header)
            + chunk(b"IDAT", pixels)
            + chunk(b"IEND", b"")
        )

    def write_binaries(self, target: str) -> Path:
        binary_directory = self.repository / "target" / "release"
        suffix = ".exe" if target.endswith("-windows-msvc") else ""
        (binary_directory / f"viewr{suffix}").write_bytes(
            self.binary_bytes(target, b"main")
        )
        (binary_directory / f"viewr-decode{suffix}").write_bytes(
            self.binary_bytes(target, b"worker")
        )
        return binary_directory

    @staticmethod
    def binary_bytes(target: str, payload: bytes) -> bytes:
        if target == "x86_64-pc-windows-msvc":
            header = bytearray(512)
            header[:2] = b"MZ"
            header[60:64] = (64).to_bytes(4, "little")
            header[64:70] = b"PE\0\0\x64\x86"
            header[70:72] = (1).to_bytes(2, "little")
            header[84:86] = (112).to_bytes(2, "little")
            header[86:88] = (2).to_bytes(2, "little")
            header[88:90] = (0x020B).to_bytes(2, "little")
            header[104:108] = (0x1000).to_bytes(4, "little")
            header[200:208] = b".text\0\0\0"
            header[208:212] = len(payload).to_bytes(4, "little")
            header[212:216] = (0x1000).to_bytes(4, "little")
            header[216:220] = len(payload).to_bytes(4, "little")
            header[220:224] = len(header).to_bytes(4, "little")
            header[236:240] = (0x60000020).to_bytes(4, "little")
            return bytes(header) + payload
        if target == "x86_64-unknown-linux-gnu":
            header = bytearray(120)
            header[:7] = b"\x7fELF\x02\x01\x01"
            header[16:18] = (3).to_bytes(2, "little")
            header[18:20] = (0x3E).to_bytes(2, "little")
            header[20:24] = (1).to_bytes(4, "little")
            header[24:32] = (0x400078).to_bytes(8, "little")
            header[32:40] = (64).to_bytes(8, "little")
            header[52:54] = (64).to_bytes(2, "little")
            header[54:56] = (56).to_bytes(2, "little")
            header[56:58] = (1).to_bytes(2, "little")
            header[64:68] = (1).to_bytes(4, "little")
            header[68:72] = (5).to_bytes(4, "little")
            header[72:80] = (0).to_bytes(8, "little")
            header[80:88] = (0x400000).to_bytes(8, "little")
            file_size = len(header) + len(payload)
            header[96:104] = file_size.to_bytes(8, "little")
            header[104:112] = file_size.to_bytes(8, "little")
            header[112:120] = (0x1000).to_bytes(8, "little")
            return bytes(header) + payload
        cpu = 0x0100000C if target == "aarch64-apple-darwin" else 0x01000007
        header = bytearray(128)
        header[:4] = b"\xcf\xfa\xed\xfe"
        header[4:8] = cpu.to_bytes(4, "little")
        header[12:16] = (2).to_bytes(4, "little")
        header[16:20] = (2).to_bytes(4, "little")
        header[20:24] = (96).to_bytes(4, "little")
        header[32:36] = (0x19).to_bytes(4, "little")
        header[36:40] = (72).to_bytes(4, "little")
        header[40:56] = b"__TEXT".ljust(16, b"\0")
        header[56:64] = (0x100000000).to_bytes(8, "little")
        file_size = len(header) + len(payload)
        header[64:72] = file_size.to_bytes(8, "little")
        header[72:80] = (0).to_bytes(8, "little")
        header[80:88] = file_size.to_bytes(8, "little")
        header[88:92] = (7).to_bytes(4, "little")
        header[92:96] = (5).to_bytes(4, "little")
        header[104:108] = (0x80000028).to_bytes(4, "little")
        header[108:112] = (24).to_bytes(4, "little")
        header[112:120] = len(header).to_bytes(8, "little")
        return bytes(header) + payload

    def build(self, target: str = "x86_64-pc-windows-msvc") -> Path:
        return release_artifact.build_release_artifact(
            repository_root=self.repository,
            target=target,
            binary_directory=self.write_binaries(target),
            source_date_epoch=SOURCE_DATE_EPOCH,
        )

    def make_directory_symlink(self, link: Path, target: Path) -> None:
        try:
            link.symlink_to(target, target_is_directory=True)
        except (NotImplementedError, OSError) as error:
            self.skipTest(f"directory symlinks are unavailable: {error}")
        self.addCleanup(link.unlink)

    def test_binary_directory_allows_a_host_alias_above_target(self) -> None:
        alias = self.repository.parent / f"{self.repository.name}-alias"
        self.make_directory_symlink(alias, self.repository)

        resolved = release_artifact._require_directory_below_target(
            self.repository.resolve(), alias / "target" / "release"
        )

        self.assertEqual(resolved, (self.repository / "target" / "release").resolve())

    def test_binary_directory_rejects_a_link_inside_target(self) -> None:
        linked_release = self.repository / "target" / "linked-release"
        self.make_directory_symlink(
            linked_release, self.repository / "target" / "release"
        )

        with self.assertRaisesRegex(
            release_artifact.ReleaseError, "link or reparse point"
        ):
            release_artifact._require_directory_below_target(
                self.repository.resolve(), linked_release
            )

    def test_build_is_deterministic_and_archive_contract_verifies(self) -> None:
        archive = self.build()
        first_bytes = archive.read_bytes()
        first_sidecar = archive.with_suffix(archive.suffix + ".sha256").read_bytes()

        os.utime(self.repository / "target" / "release" / "viewr.exe", None)
        rebuilt = self.build()

        self.assertEqual(rebuilt, archive)
        self.assertEqual(rebuilt.read_bytes(), first_bytes)
        self.assertEqual(
            rebuilt.with_suffix(rebuilt.suffix + ".sha256").read_bytes(),
            first_sidecar,
        )
        manifest = release_artifact.verify_release_artifact(rebuilt)
        self.assertEqual(manifest["version"], "1.2.3")
        self.assertEqual(manifest["target"], "x86_64-pc-windows-msvc")
        self.assertEqual(manifest["rust_toolchain"], "1.96.0")
        self.assertEqual(manifest["source_date_epoch"], SOURCE_DATE_EPOCH)

        prefix = "viewr-1.2.3-x86_64-pc-windows-msvc"
        with zipfile.ZipFile(rebuilt) as archive_file:
            self.assertEqual(
                set(archive_file.namelist()),
                {
                    f"{prefix}/LICENSE",
                    f"{prefix}/bin/viewr-decode.exe",
                    f"{prefix}/bin/viewr.exe",
                    f"{prefix}/release-manifest.json",
                    *(f"{prefix}/{path}" for path in EXPECTED_DOCUMENTATION_PATHS),
                },
            )
            manifest_bytes = archive_file.read(f"{prefix}/release-manifest.json")
            self.assertTrue(manifest_bytes.endswith(b"\n"))
            self.assertEqual(json.loads(manifest_bytes), manifest)

    def test_build_rejects_an_unresolved_local_readme_link(self) -> None:
        (self.repository / "README.md").write_text(
            "# viewr\n\n[Missing](docs/MISSING.md)\n", encoding="utf-8"
        )
        with self.assertRaisesRegex(
            release_artifact.ReleaseError, "unresolved local link"
        ):
            self.build()

    def test_build_rejects_an_invalid_documentation_asset(self) -> None:
        screenshot = self.repository / "docs/screenshots/viewr-console-example.png"
        screenshot.write_bytes(b"not a PNG")
        with self.assertRaisesRegex(release_artifact.ReleaseError, "valid PNG"):
            self.build()

    def test_build_rejects_a_corrupt_documentation_asset(self) -> None:
        screenshot = self.repository / "docs/screenshots/viewr-console-example.png"
        corrupt = bytearray(self.png_bytes())
        corrupt[24] ^= 1
        screenshot.write_bytes(corrupt)
        with self.assertRaisesRegex(release_artifact.ReleaseError, "PNG checksum"):
            self.build()

    def test_build_rejects_an_oversized_documentation_asset(self) -> None:
        screenshot = self.repository / "docs/screenshots/viewr-console-example.png"
        with screenshot.open("wb") as output:
            output.seek(release_artifact.MAX_DOCUMENTATION_ASSET_BYTES)
            output.write(b"\0")
        with self.assertRaisesRegex(release_artifact.ReleaseError, "size limit"):
            self.build()

    def test_verify_release_set_requires_every_supported_target_and_no_extras(
        self,
    ) -> None:
        for target in release_artifact.SUPPORTED_TARGETS:
            self.build(target)
        release_directory = self.repository / "target" / "release-artifacts"

        manifests = release_artifact.verify_release_set(release_directory, "v1.2.3")
        self.assertEqual(set(manifests), release_artifact.SUPPORTED_TARGETS)

        extra = release_directory / "unexpected.txt"
        extra.write_text("unexpected\n", encoding="utf-8")
        with self.assertRaisesRegex(
            release_artifact.ReleaseError, "release asset set mismatch"
        ):
            release_artifact.verify_release_set(release_directory, "v1.2.3")
        extra.unlink()

        missing_sidecar = next(release_directory.glob("*.zip.sha256"))
        missing_sidecar.unlink()
        with self.assertRaisesRegex(
            release_artifact.ReleaseError, "release asset set mismatch"
        ):
            release_artifact.verify_release_set(release_directory, "v1.2.3")

        with self.assertRaisesRegex(release_artifact.ReleaseError, "semantic version"):
            release_artifact.verify_release_set(release_directory, "1.2.3")

    def test_build_rejects_missing_worker_and_mismatched_tag(self) -> None:
        binary_directory = self.repository / "target" / "release"
        (binary_directory / "viewr.exe").write_bytes(
            self.binary_bytes("x86_64-pc-windows-msvc", b"main")
        )
        with self.assertRaisesRegex(release_artifact.ReleaseError, "viewr-decode.exe"):
            release_artifact.build_release_artifact(
                repository_root=self.repository,
                target="x86_64-pc-windows-msvc",
                binary_directory=binary_directory,
                source_date_epoch=SOURCE_DATE_EPOCH,
            )

        (binary_directory / "viewr-decode.exe").write_bytes(
            self.binary_bytes("x86_64-pc-windows-msvc", b"worker")
        )
        with self.assertRaisesRegex(release_artifact.ReleaseError, "v1.2.3"):
            release_artifact.build_release_artifact(
                repository_root=self.repository,
                target="x86_64-pc-windows-msvc",
                binary_directory=binary_directory,
                source_date_epoch=SOURCE_DATE_EPOCH,
                expected_tag="v1.2.4",
            )

    def test_build_rejects_binary_directory_outside_target(self) -> None:
        external = self.repository / "external"
        external.mkdir()
        with self.assertRaisesRegex(release_artifact.ReleaseError, "target directory"):
            release_artifact.build_release_artifact(
                repository_root=self.repository,
                target="x86_64-unknown-linux-gnu",
                binary_directory=external,
                source_date_epoch=SOURCE_DATE_EPOCH,
            )

    def test_build_rejects_binary_format_that_disagrees_with_target(self) -> None:
        binary_directory = self.repository / "target" / "release"
        linux_binary = self.binary_bytes("x86_64-unknown-linux-gnu", b"mislabeled")
        (binary_directory / "viewr").write_bytes(linux_binary)
        (binary_directory / "viewr-decode").write_bytes(linux_binary)

        with self.assertRaisesRegex(release_artifact.ReleaseError, "Mach-O"):
            release_artifact.build_release_artifact(
                repository_root=self.repository,
                target="x86_64-apple-darwin",
                binary_directory=binary_directory,
                source_date_epoch=SOURCE_DATE_EPOCH,
            )

    def test_target_validation_requires_executable_mapped_entry_points(self) -> None:
        cases = (
            ("x86_64-pc-windows-msvc", 236, 0x40000020, 4),
            ("x86_64-unknown-linux-gnu", 68, 4, 4),
            ("x86_64-apple-darwin", 92, 1, 4),
            ("aarch64-apple-darwin", 92, 1, 4),
        )
        for target, protection_offset, non_executable, width in cases:
            with self.subTest(target=target, failure="non-executable segment"):
                binary = bytearray(self.binary_bytes(target, b"payload"))
                binary[protection_offset : protection_offset + width] = (
                    non_executable.to_bytes(width, "little")
                )
                with self.assertRaisesRegex(
                    release_artifact.ReleaseError, "entry point"
                ):
                    release_artifact._require_binary_target_bytes(
                        bytes(binary), len(binary), target, "test binary"
                    )

            with self.subTest(target=target, failure="unmapped entry point"):
                binary = bytearray(self.binary_bytes(target, b"payload"))
                if target == "x86_64-pc-windows-msvc":
                    binary[104:108] = (0x2000).to_bytes(4, "little")
                elif target == "x86_64-unknown-linux-gnu":
                    binary[24:32] = (0x500000).to_bytes(8, "little")
                else:
                    binary[112:120] = len(binary).to_bytes(8, "little")
                with self.assertRaisesRegex(
                    release_artifact.ReleaseError, "entry point"
                ):
                    release_artifact._require_binary_target_bytes(
                        bytes(binary), len(binary), target, "test binary"
                    )

    def test_target_validation_rejects_malformed_executable_structures(self) -> None:
        def assert_invalid(
            target: str,
            binary: bytes | bytearray,
            message: str,
            *,
            file_size: int | None = None,
        ) -> None:
            payload = bytes(binary)
            with self.assertRaisesRegex(release_artifact.ReleaseError, message):
                release_artifact._require_binary_target_bytes(
                    payload,
                    len(payload) if file_size is None else file_size,
                    target,
                    "test binary",
                )

        windows = "x86_64-pc-windows-msvc"
        assert_invalid(windows, b"MZ", "Windows PE executable")
        invalid_offset = bytearray(64)
        invalid_offset[:2] = b"MZ"
        invalid_offset[60:64] = (1024 * 1024 + 1).to_bytes(4, "little")
        assert_invalid(windows, invalid_offset, "header offset")
        invalid_machine = bytearray(self.binary_bytes(windows, b"payload"))
        invalid_machine[68:70] = (0x014C).to_bytes(2, "little")
        assert_invalid(windows, invalid_machine, "structurally valid")
        invalid_section = bytearray(self.binary_bytes(windows, b"payload"))
        invalid_section[220:224] = len(invalid_section).to_bytes(4, "little")
        assert_invalid(windows, invalid_section, "out-of-bounds")

        linux = "x86_64-unknown-linux-gnu"
        assert_invalid(linux, b"\x7fELF", "complete x86-64 ELF")
        invalid_machine = bytearray(self.binary_bytes(linux, b"payload"))
        invalid_machine[18:20] = (0xB7).to_bytes(2, "little")
        assert_invalid(linux, invalid_machine, "structurally valid")
        invalid_segment = bytearray(self.binary_bytes(linux, b"payload"))
        invalid_size = len(invalid_segment) + 1
        invalid_segment[96:104] = invalid_size.to_bytes(8, "little")
        invalid_segment[104:112] = invalid_size.to_bytes(8, "little")
        assert_invalid(linux, invalid_segment, "out-of-bounds")

        macos = "x86_64-apple-darwin"
        assert_invalid(macos, b"", "thin 64-bit Mach-O")
        invalid_command = bytearray(self.binary_bytes(macos, b"payload"))
        invalid_command[36:40] = (4).to_bytes(4, "little")
        assert_invalid(macos, invalid_command, "invalid Mach-O load command")
        truncated_segment = bytearray(self.binary_bytes(macos, b"payload"))
        truncated_segment[36:40] = (64).to_bytes(4, "little")
        assert_invalid(macos, truncated_segment, "truncated Mach-O segment")
        invalid_segment = bytearray(self.binary_bytes(macos, b"payload"))
        invalid_segment[96:100] = (1).to_bytes(4, "little")
        assert_invalid(macos, invalid_segment, "invalid Mach-O segment")
        truncated_entry = bytearray(self.binary_bytes(macos, b"payload"))
        truncated_entry[108:112] = (16).to_bytes(4, "little")
        assert_invalid(macos, truncated_entry, "truncated Mach-O entry-point")
        truncated_commands = bytearray(self.binary_bytes(macos, b"payload"))
        truncated_commands[20:24] = (16).to_bytes(4, "little")
        truncated_commands[32:36] = (0).to_bytes(4, "little")
        truncated_commands[36:40] = (12).to_bytes(4, "little")
        assert_invalid(macos, truncated_commands, "truncated Mach-O load command")
        malformed_commands = bytearray(self.binary_bytes(macos, b"payload"))
        malformed_commands.extend(b"\0" * 8)
        malformed_commands[20:24] = (104).to_bytes(4, "little")
        assert_invalid(macos, malformed_commands, "malformed Mach-O load commands")

    def test_verifier_revalidates_exact_archived_binary_formats(self) -> None:
        target = "x86_64-unknown-linux-gnu"
        archive = self.build(target)
        manifest = release_artifact.verify_release_artifact(archive)
        prefix = "viewr-1.2.3-x86_64-unknown-linux-gnu"
        with zipfile.ZipFile(archive) as source:
            payloads = {
                record["path"]: source.read(f"{prefix}/{record['path']}")
                for record in manifest["files"]
            }

        wrong_binary = self.binary_bytes("x86_64-pc-windows-msvc", b"mislabeled")
        payloads["bin/viewr"] = wrong_binary
        payloads["bin/viewr-decode"] = wrong_binary
        for record in manifest["files"]:
            payload = payloads[record["path"]]
            record["size"] = len(payload)
            record["sha256"] = hashlib.sha256(payload).hexdigest()
        entries = [
            release_artifact.ArchiveEntry(
                record["path"],
                int(record["mode"], 8),
                payloads[record["path"]],
            )
            for record in manifest["files"]
        ]
        release_artifact._write_archive(
            archive,
            prefix,
            entries,
            manifest,
            release_artifact._zip_timestamp(SOURCE_DATE_EPOCH),
        )
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        sidecar = archive.with_suffix(archive.suffix + ".sha256")
        sidecar.write_text(f"{digest}  {archive.name}\n", encoding="ascii")

        with self.assertRaisesRegex(release_artifact.ReleaseError, "ELF executable"):
            release_artifact.verify_release_artifact(archive)

    def test_verifier_rejects_tampering_before_extracting_members(self) -> None:
        archive = self.build("x86_64-unknown-linux-gnu")
        data = bytearray(archive.read_bytes())
        data[len(data) // 2] ^= 1
        archive.write_bytes(data)

        with self.assertRaisesRegex(release_artifact.ReleaseError, "checksum"):
            release_artifact.verify_release_artifact(archive)

    def test_verifier_rejects_unsafe_member_with_recomputed_sidecar(self) -> None:
        archive = self.build("x86_64-unknown-linux-gnu")
        with zipfile.ZipFile(archive, mode="a", compression=zipfile.ZIP_STORED) as file:
            file.writestr("../escape", b"not allowed")
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        sidecar = archive.with_suffix(archive.suffix + ".sha256")
        sidecar.write_text(f"{digest}  {archive.name}\n", encoding="ascii")

        with self.assertRaisesRegex(release_artifact.ReleaseError, "unsafe path"):
            release_artifact.verify_release_artifact(archive)

    def test_verifier_rejects_trailing_data_with_recomputed_sidecar(self) -> None:
        archive = self.build("x86_64-unknown-linux-gnu")
        with archive.open("ab") as output:
            output.write(b"trailing payload")
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        sidecar = archive.with_suffix(archive.suffix + ".sha256")
        sidecar.write_text(f"{digest}  {archive.name}\n", encoding="ascii")

        with self.assertRaisesRegex(release_artifact.ReleaseError, "ZIP envelope"):
            release_artifact.verify_release_artifact(archive)

    def test_sidecar_is_standard_sha256_format(self) -> None:
        archive = self.build("aarch64-apple-darwin")
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        sidecar = archive.with_suffix(archive.suffix + ".sha256")
        self.assertEqual(
            sidecar.read_text(encoding="ascii"), f"{digest}  {archive.name}\n"
        )

    def test_cli_builds_verifies_and_reports_contract_errors(self) -> None:
        target = "x86_64-pc-windows-msvc"
        binary_directory = self.write_binaries(target)
        output = io.StringIO()
        with (
            mock.patch.object(release_artifact, "REPOSITORY_ROOT", self.repository),
            mock.patch.dict(
                os.environ,
                {"VIEWR_RELEASE_TAG": "v1.2.3"},
                clear=True,
            ),
            mock.patch("sys.stdout", output),
        ):
            exit_code = release_artifact.main(
                [
                    "build",
                    "--target",
                    target,
                    "--binary-dir",
                    str(binary_directory),
                    "--source-date-epoch",
                    str(SOURCE_DATE_EPOCH),
                    "--expected-tag-from-env",
                ]
            )
        self.assertEqual(exit_code, 0)
        self.assertIn("created and verified", output.getvalue())

        archive = (
            self.repository
            / "target"
            / "release-artifacts"
            / "viewr-1.2.3-x86_64-pc-windows-msvc.zip"
        )
        output = io.StringIO()
        with (
            mock.patch.object(release_artifact, "REPOSITORY_ROOT", self.repository),
            mock.patch("sys.stdout", output),
        ):
            exit_code = release_artifact.main(["verify", str(archive)])
        self.assertEqual(exit_code, 0)
        self.assertIn("verified", output.getvalue())

        error_output = io.StringIO()
        with (
            mock.patch.object(release_artifact, "REPOSITORY_ROOT", self.repository),
            mock.patch.dict(
                os.environ,
                {"VIEWR_RELEASE_TAG": "v1.2.3;Write-Output injected"},
                clear=True,
            ),
            mock.patch("sys.stderr", error_output),
        ):
            exit_code = release_artifact.main(
                [
                    "build",
                    "--target",
                    target,
                    "--binary-dir",
                    str(binary_directory),
                    "--source-date-epoch",
                    str(SOURCE_DATE_EPOCH),
                    "--expected-tag-from-env",
                ]
            )
        self.assertEqual(exit_code, 1)
        self.assertIn("release tag must be v1.2.3", error_output.getvalue())

    def test_release_workflow_passes_tag_as_data_not_shell_source(self) -> None:
        workflow = (PROJECT_ROOT / ".github" / "workflows" / "release.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("VIEWR_RELEASE_TAG: ${{ github.ref_name }}", workflow)
        self.assertIn("--expected-tag-from-env", workflow)
        self.assertNotIn("--expected-tag ${{", workflow)

    def test_manual_release_workflow_runs_can_never_publish(self) -> None:
        workflow = (PROJECT_ROOT / ".github" / "workflows" / "release.yml").read_text(
            encoding="utf-8"
        )

        self.assertIn("workflow_dispatch:", workflow)
        self.assertIn(
            "if: github.event_name == 'push' && github.ref_type == 'tag'", workflow
        )
        self.assertNotIn("\n    if: github.ref_type == 'tag'\n", workflow)

    def test_release_workflow_uses_reviewed_notes_and_immutable_installers(
        self,
    ) -> None:
        workflow = (PROJECT_ROOT / ".github" / "workflows" / "release.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn('notes_file="docs/releases/${tag}.md"', workflow)
        self.assertIn('--notes-file "$notes_file"', workflow)
        self.assertNotIn("--generate-notes", workflow)
        self.assertIn("install.ps1.sha256", workflow)
        self.assertIn("install.sh.sha256", workflow)
        self.assertIn("Attest every release asset", workflow)
        self.assertIn("Release assets do not match the exact expected set", workflow)

    def test_public_installer_commands_never_execute_moving_branch_content(
        self,
    ) -> None:
        surfaces = [
            PROJECT_ROOT / "README.md",
            PROJECT_ROOT / "docs" / "INSTALL.md",
            PROJECT_ROOT / "crates" / "viewr" / "src" / "cli.rs",
        ]
        combined = "\n".join(path.read_text(encoding="utf-8") for path in surfaces)
        self.assertNotIn("/main/install.ps1", combined)
        self.assertNotIn("/main/install.sh", combined)
        self.assertNotIn("/master/install.ps1", combined)
        self.assertNotIn("/master/install.sh", combined)
        self.assertIn("/releases/download/v0.1.2/install.ps1", combined)
        self.assertIn("/releases/download/v0.1.2/install.sh", combined)

    def test_supply_chain_audit_denies_unreviewed_warnings(self) -> None:
        workflow = (PROJECT_ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )
        audit_policy = (PROJECT_ROOT / ".cargo" / "audit.toml").read_text(
            encoding="utf-8"
        )
        self.assertIn("cargo audit -D warnings", workflow)
        self.assertIn("cargo audit --file fuzz/Cargo.lock -D warnings", workflow)
        self.assertIn('"RUSTSEC-2024-0436"', audit_policy)
        self.assertNotIn('"RUSTSEC-2026-0221"', audit_policy)

    def test_source_date_epoch_has_explicit_environment_and_git_fallbacks(self) -> None:
        with mock.patch.dict(os.environ, {"SOURCE_DATE_EPOCH": "123"}, clear=True):
            self.assertEqual(
                release_artifact._resolve_source_date_epoch(self.repository, 456),
                456,
            )
            self.assertEqual(
                release_artifact._resolve_source_date_epoch(self.repository, None),
                123,
            )

        with mock.patch.dict(os.environ, {"SOURCE_DATE_EPOCH": "invalid"}, clear=True):
            with self.assertRaisesRegex(release_artifact.ReleaseError, "base-10"):
                release_artifact._resolve_source_date_epoch(self.repository, None)

        completed = mock.Mock(stdout="789\n")
        with (
            mock.patch.dict(os.environ, {}, clear=True),
            mock.patch("subprocess.run", return_value=completed) as run,
        ):
            self.assertEqual(
                release_artifact._resolve_source_date_epoch(self.repository, None),
                789,
            )
        run.assert_called_once()

    def test_build_rejects_unpinned_identity_and_invalid_epoch(self) -> None:
        binary_directory = self.write_binaries("x86_64-unknown-linux-gnu")
        (self.repository / "rust-toolchain.toml").write_text(
            '[toolchain]\nchannel = "stable"\n',
            encoding="utf-8",
        )
        with self.assertRaisesRegex(release_artifact.ReleaseError, "exact stable"):
            release_artifact.build_release_artifact(
                repository_root=self.repository,
                target="x86_64-unknown-linux-gnu",
                binary_directory=binary_directory,
                source_date_epoch=SOURCE_DATE_EPOCH,
            )

        (self.repository / "rust-toolchain.toml").write_text(
            '[toolchain]\nchannel = "1.96.0"\n',
            encoding="utf-8",
        )
        with self.assertRaisesRegex(release_artifact.ReleaseError, "non-negative"):
            release_artifact.build_release_artifact(
                repository_root=self.repository,
                target="x86_64-unknown-linux-gnu",
                binary_directory=binary_directory,
                source_date_epoch=-1,
            )

    def test_contract_rejects_invalid_configuration_and_container_edges(self) -> None:
        self.assertEqual(
            release_artifact._zip_timestamp(0),
            (1980, 1, 1, 0, 0, 0),
        )
        self.assertEqual(
            release_artifact._zip_timestamp(4_400_000_000),
            (2107, 12, 31, 23, 59, 58),
        )

        with self.assertRaisesRegex(release_artifact.ReleaseError, "unsupported"):
            release_artifact.build_release_artifact(
                repository_root=self.repository,
                target="mips-unknown-none",
                binary_directory=self.repository / "target" / "release",
                source_date_epoch=SOURCE_DATE_EPOCH,
            )

        (self.repository / "Cargo.toml").write_text(
            "not valid TOML [", encoding="utf-8"
        )
        with self.assertRaisesRegex(
            release_artifact.ReleaseError, "invalid release identity"
        ):
            release_artifact._load_identity(self.repository)

        wrong_archive = self.repository / "target" / "release-artifacts.tar"
        wrong_archive.write_bytes(b"not a zip")
        with self.assertRaisesRegex(release_artifact.ReleaseError, r"\.zip format"):
            release_artifact.verify_release_artifact(wrong_archive)


if __name__ == "__main__":
    unittest.main()
