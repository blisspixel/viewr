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
REQUIRED_JOBS = (
    "quality",
    "performance",
    "c-decoder-policy",
    "coverage",
    "supply-chain",
)


def _workflow(name: str) -> str:
    path = REPOSITORY_ROOT / ".github" / "workflows" / name
    return path.read_text(encoding="utf-8").replace("\r\n", "\n")


def _jobs(text: str) -> dict[str, str]:
    body = text.split("\njobs:\n", 1)[1]
    parts = re.split(r"\n(?=  [a-z0-9-]+:\n)", "\n" + body)
    jobs: dict[str, str] = {}
    for part in parts:
        part = part.strip("\n")
        if not part.strip():
            continue
        name = part.split(":", 1)[0].strip()
        jobs[name] = part
    return jobs


def _run_blocks(job: str) -> list[str]:
    lines = job.splitlines()
    blocks: list[str] = []
    index = 0
    while index < len(lines):
        stripped = lines[index].lstrip()
        indent = len(lines[index]) - len(stripped)
        if not stripped.startswith("run:"):
            index += 1
            continue
        block = [stripped]
        index += 1
        while index < len(lines):
            current = lines[index]
            if current.strip() == "":
                following = index + 1
                if (
                    following < len(lines)
                    and lines[following].strip()
                    and len(lines[following]) - len(lines[following].lstrip()) > indent
                ):
                    index += 1
                    continue
                break
            if len(current) - len(current.lstrip()) <= indent:
                break
            block.append(current.strip())
            index += 1
        blocks.append("\n".join(block))
    return blocks


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
        self.assertNotIn("github-actions[bot]", self.workflow)
        self.assertIn('git config user.name "Nick Seal"', self.workflow)
        self.assertIn(
            'git config user.email "32712898+blisspixel@users.noreply.github.com"',
            self.workflow,
        )
        self.assertEqual(self.workflow.count("contents: write"), 1)
        self.assertEqual(self.workflow.count("checks: write"), 1)
        self.assertEqual(self.workflow.count("id-token: write"), 0)
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
        self.assertIn("uses: ./.github/workflows/dependabot-ci.yml", caller)
        self.assertNotIn("uses: ./.github/workflows/ci.yml", caller)
        self.assertIn("pin: ${{ needs.refresh.outputs.head_sha }}", caller)
        self.assertNotIn("checkout-ref", caller)
        self.assertIn("cache-mode: read", caller)
        self.assertIn("actions: read", caller)
        self.assertIn("contents: read", caller)
        self.assertNotIn("contents: write", caller)
        self.assertNotIn("checks: write", caller)
        self.assertNotIn("id-token:", caller)
        self.assertNotIn("attestations:", caller)
        self.assertNotIn("secrets:", caller)

        fuzz = self.workflow.split("  fuzz:", 1)[1].split("  report:", 1)[0]
        self.assertIn("uses: ./.github/workflows/dependabot-fuzz.yml", fuzz)
        self.assertNotIn("uses: ./.github/workflows/fuzz.yml", fuzz)
        self.assertIn("pin: ${{ needs.refresh.outputs.head_sha }}", fuzz)
        self.assertNotIn("checkout-ref", fuzz)
        self.assertIn("cache-mode: read", fuzz)
        self.assertIn("actions: read", fuzz)
        self.assertIn("contents: read", fuzz)
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
        self.assertNotIn("dependabot-ci.yml", release)
        self.assertNotIn("inputs.pin", release)

    def test_called_workflows_checkout_the_requested_commit(self) -> None:
        for name in ("ci.yml", "fuzz.yml"):
            text = _workflow(name)
            triggers = text.split("\npermissions:", 1)[0]
            self.assertIn("\n  schedule:\n", triggers)
            self.assertIn("\n  workflow_dispatch:\n", triggers)
            self.assertNotIn("inputs.pin", text)

        guard = (
            "github.actor == 'dependabot[bot]' && "
            "github.event.pull_request.head.repo.full_name == github.repository"
        )
        pairs = (
            ("ci.yml", "dependabot-ci.yml", REQUIRED_JOBS),
            ("fuzz.yml", "dependabot-fuzz.yml", ("fuzz",)),
        )
        for source_name, called_name, job_ids in pairs:
            called = _workflow(called_name)
            triggers = called.split("\non:", 1)[1].split("\npermissions:", 1)[0]
            self.assertIn("workflow_call:", triggers)
            self.assertNotIn("schedule:", triggers)
            self.assertNotIn("workflow_dispatch:", triggers)
            self.assertNotIn("pull_request:", triggers)
            self.assertNotIn("id-token:", called)
            self.assertIn("actions: read", called)
            self.assertIn("\n      pin:\n", called)
            for line in called.splitlines():
                stripped = line.strip()
                if not stripped.startswith("uses:"):
                    continue
                self.assertIsNotNone(
                    SHA.search(stripped),
                    f"action is not pinned to a full commit: {stripped}",
                )
            refs = [
                line.strip()
                for line in called.splitlines()
                if line.strip().startswith("ref:")
            ]
            self.assertTrue(refs, called_name)
            self.assertTrue(all(line == "ref: ${{ inputs.pin }}" for line in refs))
            source_jobs = _jobs(_workflow(source_name))
            called_jobs = _jobs(called)
            self.assertEqual(tuple(called_jobs), job_ids)
            for job_id in job_ids:
                self.assertIn(guard, called_jobs[job_id])
                source_runs = _run_blocks(source_jobs[job_id])
                called_runs = _run_blocks(called_jobs[job_id])
                self.assertTrue(called_runs[0].startswith("run: |"))
                self.assertIn('"$PIN" =~ ^[0-9a-f]{40}$', called_runs[0])
                self.assertEqual(called_runs[1:], source_runs, job_id)

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
