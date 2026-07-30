"""Integration tests for the public installer contracts."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest
import zipfile


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
INSTALLER = REPOSITORY_ROOT / "install.sh"
VERSION = "1.2.3"
TARGET = "x86_64-unknown-linux-gnu"


@unittest.skipIf(os.name == "nt", "the POSIX installer runs on Unix CI hosts")
class UnixInstallerTests(unittest.TestCase):
    """Run the real shell installer against a hermetic fake GitHub release."""

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.home = self.root / "home"
        self.install_root = self.root / "install"
        self.bin_dir = self.root / "bin"
        self.temp_dir = self.root / "tmp"
        self.fake_bin = self.root / "fake-bin"
        for directory in (
            self.home,
            self.bin_dir,
            self.temp_dir,
            self.fake_bin,
        ):
            directory.mkdir()
        self.archive, self.sidecar = self.make_release()
        self.write_fake_commands()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    @staticmethod
    def executable(contents: str) -> bytes:
        return contents.encode("utf-8")

    def make_release(
        self,
        *,
        doctor_passes: bool = True,
        extra_member: bool = False,
    ) -> tuple[Path, Path]:
        prefix = f"viewr-{VERSION}-{TARGET}"
        viewr = """#!/bin/sh
case "${1:-}" in
    --version) printf 'viewr 1.2.3\\n' ;;
    doctor) exit DOCTOR_EXIT ;;
    *) exit 1 ;;
esac
""".replace("DOCTOR_EXIT", "0" if doctor_passes else "1")
        payloads = {
            "LICENSE": b"test license\n",
            "NOTICE": b"test notice\n",
            "README.md": b"# test release\n",
            "THIRD_PARTY_LICENSES.html": b"<p>test licenses</p>\n",
            "assets/icon.svg": b"<svg xmlns='http://www.w3.org/2000/svg'/>\n",
            "assets/linux/viewr.desktop": b"[Desktop Entry]\nName=viewr\n",
            "bin/viewr": self.executable(viewr),
            "bin/viewr-decode": self.executable("#!/bin/sh\nexit 0\n"),
        }
        files = []
        for path, payload in sorted(payloads.items()):
            files.append(
                {
                    "mode": "0755" if path.startswith("bin/") else "0644",
                    "path": path,
                    "sha256": hashlib.sha256(payload).hexdigest(),
                    "size": len(payload),
                }
            )
        manifest = {
            "archive_format": "zip-stored-v1",
            "files": files,
            "package_name": "viewr",
            "rust_toolchain": "1.96.0",
            "schema_version": 1,
            "source_date_epoch": 1_700_000_000,
            "target": TARGET,
            "version": VERSION,
        }
        payloads["release-manifest.json"] = (
            json.dumps(manifest, indent=2, sort_keys=True) + "\n"
        ).encode()
        if extra_member:
            payloads["unexpected.txt"] = b"not in the manifest\n"

        archive = self.root / f"{prefix}.zip"
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_STORED) as output:
            for path, payload in sorted(payloads.items()):
                info = zipfile.ZipInfo(f"{prefix}/{path}")
                mode = 0o755 if path.startswith("bin/") else 0o644
                info.create_system = 3
                info.external_attr = (stat.S_IFREG | mode) << 16
                output.writestr(info, payload)
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        sidecar = archive.with_suffix(".zip.sha256")
        sidecar.write_text(f"{digest}  {archive.name}\n", encoding="ascii")
        return archive, sidecar

    def write_fake_commands(self) -> None:
        curl = self.fake_bin / "curl"
        curl.write_text(
            """#!/bin/sh
output=
effective=0
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) output=$2; shift 2 ;;
        -w) effective=1; shift 2 ;;
        *) shift ;;
    esac
done
if [ "$effective" -eq 1 ]; then
    printf 'https://github.com/blisspixel/viewr/releases/tag/v1.2.3'
elif [ "${output##*.}" = "sha256" ]; then
    cp "$FAKE_SIDECAR" "$output"
else
    cp "$FAKE_ARCHIVE" "$output"
fi
""",
            encoding="utf-8",
        )
        uname = self.fake_bin / "uname"
        uname.write_text(
            """#!/bin/sh
case "${1:-}" in
    -s) printf 'Linux\\n' ;;
    -m) printf 'x86_64\\n' ;;
    *) exit 1 ;;
esac
""",
            encoding="utf-8",
        )
        unzip = self.fake_bin / "unzip"
        unzip.write_text(
            """#!/usr/bin/env python3
import pathlib
import sys
import zipfile

if len(sys.argv) == 3 and sys.argv[1] == "-Z1":
    with zipfile.ZipFile(sys.argv[2]) as archive:
        for name in archive.namelist():
            print(name)
elif len(sys.argv) == 5 and sys.argv[1] == "-q" and sys.argv[3] == "-d":
    destination = pathlib.Path(sys.argv[4])
    with zipfile.ZipFile(sys.argv[2]) as archive:
        archive.extractall(destination)
else:
    raise SystemExit(2)
""",
            encoding="utf-8",
        )
        curl.chmod(0o755)
        uname.chmod(0o755)
        unzip.chmod(0o755)

    def run_installer(
        self,
        *,
        archive: Path | None = None,
        sidecar: Path | None = None,
        check: bool = True,
    ) -> subprocess.CompletedProcess[str]:
        environment = os.environ.copy()
        environment.update(
            {
                "FAKE_ARCHIVE": str(archive or self.archive),
                "FAKE_SIDECAR": str(sidecar or self.sidecar),
                "HOME": str(self.home),
                "PATH": f"{self.fake_bin}{os.pathsep}{environment['PATH']}",
                "TMPDIR": str(self.temp_dir),
                "VIEWR_BIN_DIR": str(self.bin_dir),
                "VIEWR_INSTALL_ROOT": str(self.install_root),
            }
        )
        return subprocess.run(
            ["sh", str(INSTALLER)],
            check=check,
            capture_output=True,
            env=environment,
            text=True,
        )

    def test_installs_and_replaces_only_a_verified_release(self) -> None:
        first = self.run_installer()
        self.assertIn("Installed viewr 1.2.3", first.stdout)
        release = self.install_root / "releases" / "v1.2.3"
        self.assertEqual(
            (release / ".viewr-install").read_text(encoding="utf-8"),
            "repository=blisspixel/viewr\n"
            "version=1.2.3\n"
            "target=x86_64-unknown-linux-gnu\n",
        )
        self.assertEqual((self.bin_dir / "viewr").resolve(), release / "viewr")

        second = self.run_installer()
        self.assertIn("Installed viewr 1.2.3", second.stdout)
        self.assertEqual(list((self.install_root / "releases").glob(".backup-*")), [])
        self.assertEqual(
            list((self.install_root / "releases").glob(".installing-*")), []
        )

    def test_rejects_an_archive_member_outside_the_manifest(self) -> None:
        archive, sidecar = self.make_release(extra_member=True)
        result = self.run_installer(archive=archive, sidecar=sidecar, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("file set does not match its manifest", result.stderr)
        self.assertFalse(self.install_root.exists())

    def test_failed_staged_doctor_preserves_the_installed_release(self) -> None:
        self.run_installer()
        release = self.install_root / "releases" / "v1.2.3"
        original = (release / "viewr").read_bytes()
        archive, sidecar = self.make_release(doctor_passes=False)

        result = self.run_installer(archive=archive, sidecar=sidecar, check=False)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("did not pass viewr doctor", result.stderr)
        self.assertEqual((release / "viewr").read_bytes(), original)
        self.assertEqual(
            list((self.install_root / "releases").glob(".installing-*")), []
        )


if __name__ == "__main__":
    unittest.main()
