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
ASSIGNMENT = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)=(\d+)$")


class CiInstallPackagesTests(unittest.TestCase):
    """Prove hangs become failed attempts and still fit the step ceiling."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.script = SCRIPT.read_text(encoding="utf-8").replace("\r\n", "\n")
        cls.assignments = {
            match.group(1): int(match.group(2))
            for line in cls.script.splitlines()
            if (match := ASSIGNMENT.match(line))
        }

    def test_script_rejects_an_empty_package_list(self) -> None:
        self.assertIn('if [ "$#" -eq 0 ]; then', self.script)
        self.assertIn("usage: ci-install-packages.sh", self.script)
        self.assertIn("exit 2", self.script)

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
        self.assertIn("Acquire::http::Timeout=30", self.script)
        self.assertIn("Acquire::https::Timeout=30", self.script)
        self.assertIn("NEEDRESTART_SUSPEND=1", self.script)
        self.assertIn("NEEDRESTART_MODE=l", self.script)
        self.assertIn("DPkg::Lock::Timeout=60", self.script)

        delays = [attempt * 15 for attempt in range(1, attempts)]
        worst_case = attempts * (attempt_seconds + term_grace_seconds) + sum(delays)
        self.assertLess(
            worst_case,
            15 * 60,
            "three bounded attempts plus backoff must fit a 15-minute step",
        )

    def test_install_steps_leave_room_for_retries(self) -> None:
        install_timeouts: list[int] = []
        timeout_line = re.compile(r"timeout-minutes:\s*(\d+)\s*$")
        for workflow in WORKFLOWS:
            lines = (
                workflow.read_text(encoding="utf-8").replace("\r\n", "\n").splitlines()
            )
            self.assertTrue(
                any("bash scripts/ci-install-packages.sh" in line for line in lines),
                f"{workflow.name} does not call ci-install-packages.sh",
            )
            last_timeout: int | None = None
            for line in lines:
                if match := timeout_line.search(line):
                    last_timeout = int(match.group(1))
                if "bash scripts/ci-install-packages.sh" in line:
                    self.assertIsNotNone(
                        last_timeout,
                        f"{workflow.name} install step has no timeout-minutes",
                    )
                    install_timeouts.append(last_timeout)
        self.assertGreaterEqual(len(install_timeouts), 6)
        self.assertTrue(
            all(timeout >= 15 for timeout in install_timeouts),
            f"install steps too short for retries: {install_timeouts}",
        )
