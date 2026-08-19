"""Contract tests for the CI package-install retry script."""

from __future__ import annotations

from pathlib import Path
import re
import unittest


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
SCRIPT = REPOSITORY_ROOT / "scripts" / "ci-install-packages.sh"
WORKFLOWS = (
    REPOSITORY_ROOT / ".github" / "workflows" / "ci.yml",
    REPOSITORY_ROOT / ".github" / "workflows" / "fuzz.yml",
    REPOSITORY_ROOT / ".github" / "workflows" / "release.yml",
)


class CiInstallPackagesTests(unittest.TestCase):
    """Prove hangs become failed attempts and still fit the step ceiling."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.script = SCRIPT.read_text(encoding="utf-8")

    def test_script_rejects_an_empty_package_list(self) -> None:
        self.assertIn('if [ "$#" -eq 0 ]; then', self.script)
        self.assertIn("usage: ci-install-packages.sh", self.script)
        self.assertIn("exit 2", self.script)

    def test_each_attempt_is_bounded_so_a_hang_can_retry(self) -> None:
        attempts = int(re.search(r"^attempts=(\d+)$", self.script, re.M).group(1))
        attempt_seconds = int(
            re.search(r"^attempt_seconds=(\d+)$", self.script, re.M).group(1)
        )
        term_grace_seconds = int(
            re.search(r"^term_grace_seconds=(\d+)$", self.script, re.M).group(1)
        )
        self.assertEqual(attempts, 3)
        self.assertGreaterEqual(attempt_seconds, 60)
        self.assertGreaterEqual(term_grace_seconds, 1)
        self.assertIn("timeout", self.script)
        self.assertIn("--signal=TERM", self.script)
        self.assertIn("--kill-after=", self.script)
        self.assertIn("Acquire::http::Timeout=30", self.script)
        self.assertIn("Acquire::https::Timeout=30", self.script)

        delays = [attempt * 15 for attempt in range(1, attempts)]
        worst_case = attempts * (attempt_seconds + term_grace_seconds) + sum(delays)
        self.assertLess(
            worst_case,
            15 * 60,
            "three bounded attempts plus backoff must fit a 15-minute step",
        )

    def test_install_steps_leave_room_for_retries(self) -> None:
        install_timeouts: list[int] = []
        for workflow in WORKFLOWS:
            text = workflow.read_text(encoding="utf-8")
            self.assertIn("bash scripts/ci-install-packages.sh", text)
            for match in re.finditer(
                r"timeout-minutes:\s*(\d+)\n(?:.*\n){0,6}?"
                r"bash scripts/ci-install-packages\.sh",
                text,
            ):
                install_timeouts.append(int(match.group(1)))
        self.assertGreaterEqual(len(install_timeouts), 6)
        self.assertTrue(
            all(timeout >= 15 for timeout in install_timeouts),
            f"install steps too short for retries: {install_timeouts}",
        )
