"""The Dependabot record refresh stays limited to the two generated files."""

from __future__ import annotations

from pathlib import Path
import re
import unittest

from scripts.refresh_dependency_records import normalize_inventory


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = REPOSITORY_ROOT / ".github" / "workflows" / "dependabot-records.yml"
SHA = re.compile(r"@[0-9a-f]{40}\b")


class DependabotRecordTests(unittest.TestCase):
    """A dependency bump can refresh records without running pull-request code."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW.read_text(encoding="utf-8").replace("\r\n", "\n")

    def test_inventory_newlines_are_normalized(self) -> None:
        self.assertEqual(normalize_inventory("a\r\nb\rc"), "a\nb\nc\n")
        self.assertEqual(normalize_inventory("a\n"), "a\n")
        self.assertEqual(normalize_inventory(""), "")

    def test_workflow_uses_the_base_scripts_and_dependabot_only(self) -> None:
        self.assertIn("pull_request_target:", self.workflow)
        self.assertIn("github.actor == 'dependabot[bot]'", self.workflow)
        self.assertIn(
            "github.event.pull_request.head.repo.full_name == github.repository",
            self.workflow,
        )
        self.assertIn("ref: ${{ github.event.pull_request.base.sha }}", self.workflow)
        self.assertIn("persist-credentials: false", self.workflow)
        self.assertIn("python3 -B scripts/refresh_dependency_records.py", self.workflow)
        self.assertIn(
            "git add -- packaging/flatpak/cargo-sources.json THIRD_PARTY_LICENSES.txt",
            self.workflow,
        )
        self.assertNotIn("pull_request:", self.workflow)
        for line in self.workflow.splitlines():
            stripped = line.strip()
            if stripped.startswith("uses:"):
                self.assertIsNotNone(
                    SHA.search(stripped),
                    f"action is not pinned to a full commit: {stripped}",
                )
