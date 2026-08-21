"""Contract tests for the CI Flatpak SDK install retry script."""

from __future__ import annotations

from pathlib import Path
import re
import unittest


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
SCRIPT = REPOSITORY_ROOT / "scripts" / "ci-install-flatpak-sdk.sh"
WORKFLOW = REPOSITORY_ROOT / ".github" / "workflows" / "ci.yml"
MANIFEST = REPOSITORY_ROOT / "packaging" / "flatpak" / "com.github.blisspixel.viewr.yml"
TOOLCHAIN = REPOSITORY_ROOT / "rust-toolchain.toml"
ASSIGNMENT = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)=(\d+)$")
CHANNEL = re.compile(r'^channel\s*=\s*"([^"]+)"\s*$', re.MULTILINE)


class CiInstallFlatpakSdkTests(unittest.TestCase):
    """Prove hangs and republish 404s become failed attempts that still retry."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.script = SCRIPT.read_text(encoding="utf-8").replace("\r\n", "\n")
        cls.workflow = WORKFLOW.read_text(encoding="utf-8").replace("\r\n", "\n")
        cls.manifest = MANIFEST.read_text(encoding="utf-8").replace("\r\n", "\n")
        cls.assignments = {
            match.group(1): int(match.group(2))
            for line in cls.script.splitlines()
            if (match := ASSIGNMENT.match(line))
        }

    def test_script_installs_the_manifest_runtime(self) -> None:
        self.assertIn("runtime-version: '25.08'", self.manifest)
        self.assertIn("org.freedesktop.Sdk.Extension.rust-stable", self.manifest)
        self.assertIn("org.freedesktop.Platform//25.08", self.script)
        self.assertIn("org.freedesktop.Sdk//25.08", self.script)
        self.assertIn("org.freedesktop.Sdk.Extension.rust-stable//25.08", self.script)

    def test_each_attempt_is_bounded_so_a_hang_can_retry(self) -> None:
        attempts = self.assignments["attempts"]
        attempt_seconds = self.assignments["attempt_seconds"]
        term_grace_seconds = self.assignments["term_grace_seconds"]
        self.assertEqual(attempts, 3)
        self.assertGreaterEqual(attempt_seconds, 240)
        self.assertGreaterEqual(term_grace_seconds, 1)
        self.assertIn("timeout", self.script)
        self.assertIn("--signal=TERM", self.script)
        self.assertIn("--kill-after=", self.script)
        self.assertIn("flatpak remote-add --user --if-not-exists flathub", self.script)
        self.assertIn(
            "flatpak install --user --assumeyes --noninteractive flathub", self.script
        )

        delays = [attempt * 15 for attempt in range(1, attempts)]
        worst_case = attempts * (attempt_seconds + term_grace_seconds) + sum(delays)
        self.assertLess(
            worst_case,
            15 * 60,
            "three bounded attempts plus backoff must fit a 15-minute step",
        )

    def test_workflow_calls_the_retry_script_with_room_for_retries(self) -> None:
        self.assertIn("bash scripts/ci-install-flatpak-sdk.sh", self.workflow)
        self.assertNotIn(
            "flatpak install --user --assumeyes --noninteractive flathub \\",
            self.workflow,
        )
        timeout_line = re.compile(r"timeout-minutes:\s*(\d+)\s*$")
        last_timeout: int | None = None
        found = False
        for line in self.workflow.splitlines():
            if match := timeout_line.search(line):
                last_timeout = int(match.group(1))
            if "bash scripts/ci-install-flatpak-sdk.sh" in line:
                self.assertEqual(last_timeout, 20)
                found = True
        self.assertTrue(found)

    def test_flathub_miss_falls_back_to_the_pinned_toolchain(self) -> None:
        match = CHANNEL.search(TOOLCHAIN.read_text(encoding="utf-8"))
        self.assertIsNotNone(match)
        channel = match.group(1)
        self.assertEqual(channel, "1.96.0")
        self.assertIn(f"RUST_VERSION={channel}", self.script)
        self.assertIn("RUST_DIST_DATE=2026-05-28", self.script)
        self.assertIn(
            'RUST_URL="https://static.rust-lang.org/dist/${RUST_DIST_DATE}/${RUST_TARBALL}"',
            self.script,
        )
        self.assertIn(
            "c295047583a56238ea06b43f849f4b877fa12bfd4c7103f8d9a74c94c9c4e108",
            self.script,
        )
        self.assertIn("curl --fail --location --proto =https --tlsv1.2", self.script)
        self.assertIn("sha256sum -c", self.script)
        self.assertIn("build-extension: true", self.script)
        self.assertIn("--disable-download", self.script)
        self.assertIn("install_flathub_rust_stable", self.script)
        self.assertIn("install_pinned_rust_stable_extension", self.script)
