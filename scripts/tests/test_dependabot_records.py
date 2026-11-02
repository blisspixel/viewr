"""The Dependabot record refresh stays limited to the two generated files."""

from __future__ import annotations

from pathlib import Path
from tempfile import TemporaryDirectory
import json
import re
import subprocess
import sys
import unittest

from scripts.refresh_dependency_records import normalize_inventory
from scripts.report_dependabot_required_checks import (
    REQUIRED_CHECKS,
    decisions_for,
    require_repository,
    require_sha,
)


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = REPOSITORY_ROOT / ".github" / "workflows" / "dependabot-records.yml"
REPORT = REPOSITORY_ROOT / "scripts" / "report_dependabot_required_checks.py"
SHA = re.compile(r"@[0-9a-f]{40}\b")
HEAD_SHA = "a" * 40
REPOSITORY = "blisspixel/viewr"


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
        self.assertIn(
            'if [[ ! "$sha" =~ ^[0-9a-f]{40}$ ]]; then',
            self.workflow,
        )
        self.assertNotIn("secrets: inherit", self.workflow)
        self.assertEqual(self.workflow.count("contents: write"), 1)
        self.assertEqual(self.workflow.count("checks: write"), 1)
        self.assertEqual(self.workflow.count("id-token: write"), 1)
        self.assertNotIn("conclusion: success", self.workflow)
        for line in self.workflow.splitlines():
            stripped = line.strip()
            if not stripped.startswith("uses:"):
                continue
            if "./.github/workflows/" in stripped:
                self.assertTrue(
                    stripped.startswith("uses: ./.github/workflows/"),
                    stripped,
                )
                self.assertTrue(stripped.endswith(".yml"), stripped)
                continue
            self.assertIsNotNone(
                SHA.search(stripped),
                f"action is not pinned to a full commit: {stripped}",
            )

    def test_records_run_calls_trusted_ci_and_reports_on_the_new_commit(self) -> None:
        caller = self.workflow.split("  required-checks:", 1)[1].split("  fuzz:", 1)[0]
        self.assertIn("needs.refresh.outputs.head_sha", caller)
        self.assertIn("uses: ./.github/workflows/ci.yml", caller)
        self.assertIn("pin: ${{ needs.refresh.outputs.head_sha }}", caller)
        self.assertNotIn("checkout-ref", caller)
        self.assertIn("cache-mode: read", caller)
        self.assertIn("contents: read", caller)
        self.assertNotIn("contents: write", caller)
        self.assertNotIn("checks: write", caller)
        self.assertNotIn("secrets:", caller)

        fuzz = self.workflow.split("  fuzz:", 1)[1].split("  report:", 1)[0]
        self.assertIn("uses: ./.github/workflows/fuzz.yml", fuzz)
        self.assertIn("pin: ${{ needs.refresh.outputs.head_sha }}", fuzz)
        self.assertNotIn("checkout-ref", fuzz)
        self.assertIn("cache-mode: read", fuzz)
        self.assertIn("permissions:\n      contents: read\n", fuzz)
        self.assertNotIn("id-token:", fuzz)
        self.assertNotIn("secrets:", fuzz)

        report = self.workflow.split("  report:", 1)[1]
        self.assertIn("ref: ${{ github.sha }}", report)
        self.assertIn("persist-credentials: false", report)
        self.assertNotIn("head.sha", report)
        self.assertNotIn("head.ref", report)
        self.assertIn("scripts/report_dependabot_required_checks.py", report)
        self.assertIn("--publish", report)
        self.assertIn("always() && needs.refresh.outputs.head_sha != ''", report)

        release = (REPOSITORY_ROOT / ".github" / "workflows" / "release.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("uses: ./.github/workflows/ci.yml", release)
        self.assertNotIn("checkout-ref", release)

    def test_called_workflows_checkout_the_requested_commit(self) -> None:
        # Code scanning classifies a checkout ref expression that contains
        # ref, head, sha, commit, or branch as an untrusted pull-request
        # checkout. These workflows also run on schedule and workflow_dispatch.
        forbidden = re.compile(r"head|branch|ref|sha|commit", re.IGNORECASE)
        for name in ("ci.yml", "fuzz.yml"):
            text = (
                (REPOSITORY_ROOT / ".github" / "workflows" / name)
                .read_text(encoding="utf-8")
                .replace("\r\n", "\n")
            )
            self.assertIn("\n      pin:\n", text)
            self.assertNotIn("checkout-ref", text)
            self.assertNotIn("inputs[", text)
            pin_refs = [
                line.strip()
                for line in text.splitlines()
                if line.strip().startswith("ref:")
            ]
            self.assertTrue(pin_refs, name)
            for line in pin_refs:
                self.assertEqual(line, "ref: ${{ inputs.pin }}")
                self.assertIsNone(forbidden.search(line.split("ref:", 1)[1]), line)
            self.assertEqual(text.count("uses: actions/checkout@"), len(pin_refs) * 2)
            self.assertEqual(text.count("if: ${{ !inputs.pin }}"), len(pin_refs))
            self.assertEqual(text.count("if: ${{ inputs.pin }}"), len(pin_refs) * 2)
            self.assertEqual(
                text.count('if [[ ! "$PIN" =~ ^[0-9a-f]{40}$ ]]; then'),
                len(pin_refs),
            )

        verify = (REPOSITORY_ROOT / "docs" / "VERIFY.md").read_text(encoding="utf-8")
        for name in REQUIRED_CHECKS:
            self.assertIn(f"`{name}`", verify)

    def test_required_check_results_fail_closed(self) -> None:
        nested = f"required-checks / {REQUIRED_CHECKS[0]}"
        jobs = [
            {
                "name": nested,
                "conclusion": "success",
                "html_url": f"https://github.com/{REPOSITORY}/actions/runs/1/job/2",
            },
            {
                "name": REQUIRED_CHECKS[1],
                "conclusion": "skipped",
                "html_url": "https://example.invalid/job",
            },
            {"name": REQUIRED_CHECKS[2], "conclusion": "failure"},
            {"name": f"wrapper / {REQUIRED_CHECKS[3]}", "conclusion": "success"},
            {"name": f"other / {REQUIRED_CHECKS[3]}", "conclusion": "failure"},
        ]
        decisions = {
            item["name"]: item for item in decisions_for(jobs, repository=REPOSITORY)
        }
        self.assertEqual(
            [item["name"] for item in decisions.values()], list(REQUIRED_CHECKS)
        )
        self.assertEqual(decisions[REQUIRED_CHECKS[0]]["conclusion"], "success")
        self.assertEqual(
            decisions[REQUIRED_CHECKS[0]]["details_url"],
            f"https://github.com/{REPOSITORY}/actions/runs/1/job/2",
        )
        self.assertEqual(decisions[REQUIRED_CHECKS[1]]["conclusion"], "failure")
        self.assertNotIn("details_url", decisions[REQUIRED_CHECKS[1]])
        self.assertEqual(decisions[REQUIRED_CHECKS[2]]["conclusion"], "failure")
        self.assertEqual(decisions[REQUIRED_CHECKS[3]]["conclusion"], "failure")
        for name in REQUIRED_CHECKS[4:]:
            self.assertEqual(decisions[name]["conclusion"], "failure")
            self.assertIn(
                "missing, skipped, or unsuccessful", decisions[name]["summary"]
            )

        almost = f"prefix {REQUIRED_CHECKS[0]}"
        lone = decisions_for(
            [{"name": almost, "conclusion": "success"}],
            repository=REPOSITORY,
        )
        self.assertEqual(lone[0]["conclusion"], "failure")
        foreign_repo = decisions_for(
            [
                {
                    "name": REQUIRED_CHECKS[0],
                    "conclusion": "success",
                    "html_url": "https://github.com/other/viewr/actions/runs/1",
                }
            ],
            repository=REPOSITORY,
        )
        self.assertEqual(foreign_repo[0]["conclusion"], "success")
        self.assertNotIn("details_url", foreign_repo[0])

    def test_cli_rejects_a_bad_sha_and_a_failed_job_without_publishing(self) -> None:
        with self.assertRaises(SystemExit):
            require_sha("main")
        with self.assertRaises(SystemExit):
            require_repository("blisspixel/viewr/extra")
        with TemporaryDirectory() as tmp:
            path = Path(tmp) / "jobs.json"
            path.write_text(
                json.dumps([{"name": REQUIRED_CHECKS[0], "conclusion": "success"}]),
                encoding="utf-8",
            )
            completed = subprocess.run(
                [
                    sys.executable,
                    "-B",
                    str(REPORT),
                    "--jobs-file",
                    str(path),
                    "--head-sha",
                    HEAD_SHA,
                    "--repository",
                    REPOSITORY,
                ],
                cwd=REPOSITORY_ROOT,
                capture_output=True,
                text=True,
                check=False,
            )
        self.assertEqual(completed.returncode, 1)
        self.assertIn('"conclusion": "failure"', completed.stdout)
        self.assertNotIn("gh api", completed.stderr)
        rejected = subprocess.run(
            [
                sys.executable,
                "-B",
                str(REPORT),
                "--jobs-file",
                str(path),
                "--head-sha",
                "not-a-sha",
            ],
            cwd=REPOSITORY_ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotEqual(rejected.returncode, 0)
