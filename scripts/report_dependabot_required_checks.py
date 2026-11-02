#!/usr/bin/env python3
"""Report required CI checks on a Dependabot records commit.

The records workflow pushes with the job token, so the new pull request runs
wait for approval and do not post these checks. This command reads the CI jobs
from the records run, which tested that commit, and creates one check for each
required name. A missing, skipped, or unsuccessful job is failure.
"""

from __future__ import annotations

from pathlib import Path
import argparse
import json
import re
import subprocess
import sys
from datetime import UTC, datetime
from urllib.parse import urlsplit


REQUIRED_CHECKS = (
    "fmt / clippy / test (ubuntu-latest)",
    "fmt / clippy / test (macos-latest)",
    "fmt / clippy / test (windows-latest)",
    "coverage (>= 85% on logic)",
    "C decoders under default-deny seccomp",
    "GUI performance budgets",
    "deny + audit (privacy invariant)",
)

_SHA = re.compile(r"[0-9a-f]{40}\Z")
_REPOSITORY = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+\Z")
_RUN_ID = re.compile(r"[0-9]+\Z")
_SUCCESS = "success"
_FAILURE = "failure"


def require_sha(value: str) -> str:
    """Reject anything other than the commit the records job just created."""

    if _SHA.fullmatch(value) is None:
        raise SystemExit("head SHA must be 40 lowercase hex characters")
    return value


def require_repository(value: str) -> str:
    """Keep check-run requests on an owner/name repository path."""

    if _REPOSITORY.fullmatch(value) is None:
        raise SystemExit("repository must be owner/name")
    return value


def reported_conclusion(job_conclusion: object) -> str:
    """Pass only a completed successful job through to branch protection."""

    if job_conclusion == _SUCCESS:
        return _SUCCESS
    return _FAILURE


def _job_name(job: dict[str, object]) -> str:
    name = job.get("name", "")
    return name.strip() if isinstance(name, str) else ""


def _matches(job_name: str, required: str) -> bool:
    return job_name == required or job_name.endswith(f" / {required}")


def _details_url(job: dict[str, object], repository: str) -> str:
    url = job.get("html_url", "")
    if not isinstance(url, str):
        return ""
    parts = urlsplit(url)
    prefix = f"/{repository}/"
    if (
        parts.scheme == "https"
        and parts.hostname == "github.com"
        and parts.path.startswith(prefix)
    ):
        return url
    return ""


def decisions_for(
    jobs: list[dict[str, object]],
    *,
    repository: str,
) -> list[dict[str, str]]:
    """Choose one fail-closed result for each required check name."""

    decisions: list[dict[str, str]] = []
    observed = sorted({_job_name(job) for job in jobs if _job_name(job)})
    for required in REQUIRED_CHECKS:
        matches = [job for job in jobs if _matches(_job_name(job), required)]
        conclusions = {reported_conclusion(job.get("conclusion")) for job in matches}
        if conclusions == {_SUCCESS}:
            conclusion = _SUCCESS
            summary = f"CI job succeeded for `{required}` on this commit."
        else:
            conclusion = _FAILURE
            visible = ", ".join(observed[:30]) or "(no jobs)"
            summary = (
                f"CI job for `{required}` was missing, skipped, or unsuccessful "
                f"on this commit. Jobs in the records run: {visible}."
            )
        details = ""
        for job in matches:
            details = _details_url(job, repository) or details
        decision = {
            "name": required,
            "conclusion": conclusion,
            "summary": summary,
        }
        if details:
            decision["details_url"] = details
        decisions.append(decision)
    return decisions


def _load_jobs(path: Path) -> list[dict[str, object]]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, list):
        raise SystemExit("jobs file must be a JSON list")
    jobs: list[dict[str, object]] = []
    for item in payload:
        if not isinstance(item, dict):
            raise SystemExit("each job must be a JSON object")
        jobs.append(item)
    return jobs


def fetch_jobs(repository: str, run_id: str) -> list[dict[str, object]]:
    """Read every job in this records run, including reusable workflow jobs."""

    if _RUN_ID.fullmatch(run_id) is None:
        raise SystemExit("run id must be digits")
    jobs: list[dict[str, object]] = []
    for page in range(1, 11):
        completed = subprocess.run(
            [
                "gh",
                "api",
                f"repos/{repository}/actions/runs/{run_id}/jobs?per_page=100&page={page}",
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        if completed.returncode != 0:
            if completed.stderr:
                sys.stderr.write(completed.stderr[-2000:])
            raise SystemExit(
                f"listing CI jobs failed with status {completed.returncode}"
            )
        payload = json.loads(completed.stdout)
        batch = payload.get("jobs")
        if not isinstance(batch, list):
            raise SystemExit("jobs response did not contain a list")
        for item in batch:
            if not isinstance(item, dict):
                raise SystemExit("each job must be a JSON object")
            jobs.append(item)
        if len(batch) < 100:
            return jobs
    raise SystemExit("CI job list exceeded 1000 rows")


def publish_decision(
    decision: dict[str, str],
    *,
    repository: str,
    head_sha: str,
    completed_at: str,
) -> None:
    """Create the check run for one required name."""

    payload: dict[str, object] = {
        "name": decision["name"],
        "head_sha": head_sha,
        "status": "completed",
        "conclusion": decision["conclusion"],
        "completed_at": completed_at,
        "output": {
            "title": decision["conclusion"],
            "summary": decision["summary"],
        },
    }
    details = decision.get("details_url", "")
    if details:
        payload["details_url"] = details
    completed = subprocess.run(
        [
            "gh",
            "api",
            "--method",
            "POST",
            f"repos/{repository}/check-runs",
            "--input",
            "-",
        ],
        input=json.dumps(payload),
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        if completed.stderr:
            sys.stderr.write(completed.stderr[-2000:])
        raise SystemExit(
            f"creating check {decision['name']} failed with status {completed.returncode}"
        )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--head-sha", required=True)
    parser.add_argument("--repository", default="blisspixel/viewr")
    parser.add_argument("--jobs-file", type=Path)
    parser.add_argument("--run-id")
    parser.add_argument("--publish", action="store_true")
    args = parser.parse_args(argv)
    head_sha = require_sha(args.head_sha)
    repository = require_repository(args.repository)
    if args.jobs_file is not None:
        jobs = _load_jobs(args.jobs_file)
    elif args.run_id is not None:
        jobs = fetch_jobs(repository, args.run_id)
    else:
        raise SystemExit("pass --jobs-file or --run-id")
    decisions = decisions_for(jobs, repository=repository)
    if args.publish:
        completed_at = datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
        for decision in decisions:
            publish_decision(
                decision,
                repository=repository,
                head_sha=head_sha,
                completed_at=completed_at,
            )
    json.dump(decisions, sys.stdout, indent=2)
    sys.stdout.write("\n")
    if any(decision["conclusion"] != _SUCCESS for decision in decisions):
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
