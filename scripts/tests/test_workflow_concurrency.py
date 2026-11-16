"""Workflow concurrency must keep queued runs instead of replacing them."""

from __future__ import annotations

import re
import unittest
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = PROJECT_ROOT / ".github" / "workflows"
CONCURRENCY = re.compile(r"(?m)^[ \t]*concurrency:[ \t]*$")
CANCEL = re.compile(r"(?m)^[ \t]*cancel-in-progress:[ \t]*(.*)$")
QUEUE_MAX = re.compile(r"(?m)^[ \t]*queue:[ \t]*max[ \t]*$")
SERIALIZED = {
    "release.yml": "release-artifacts-",
    "dependabot-ci.yml": "dependabot-required-",
    "dependabot-fuzz.yml": "dependabot-fuzz-",
    "dependabot-records.yml": "dependabot-records-",
}


class WorkflowConcurrencyTests(unittest.TestCase):
    def test_concurrency_groups_queue_without_cancelling(self) -> None:
        paths = sorted(WORKFLOWS.glob("*.yml")) + sorted(WORKFLOWS.glob("*.yaml"))
        names = {path.name for path in paths}
        self.assertTrue(SERIALIZED.keys() <= names)

        for path in paths:
            text = path.read_text(encoding="utf-8")
            self.assertEqual(
                len(CONCURRENCY.findall(text)),
                len(QUEUE_MAX.findall(text)),
                f"{path.name} has a concurrency group without queue: max",
            )
            for value in CANCEL.findall(text):
                self.assertEqual(
                    value.strip(),
                    "false",
                    f"{path.name} sets cancel-in-progress to {value.strip()!r}",
                )

        for name, group in SERIALIZED.items():
            text = (WORKFLOWS / name).read_text(encoding="utf-8")
            self.assertEqual(text.count("concurrency:"), 1)
            self.assertIn(group, text)
            self.assertIn("cancel-in-progress: false", text)
            self.assertIn("queue: max", text)
