"""Validate artifact-bound v0.6 product-quality evidence records."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import hmac
import json
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping, Sequence


REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
MATRIX_PATH = REPOSITORY_ROOT / "docs" / "PRODUCT-QUALITY.md"
REPOSITORY = "blisspixel/viewr"
WORKFLOW_NAME = "Release artifacts"
PLATFORM_TARGETS = {
    "windows": ("x86_64-pc-windows-msvc",),
    "macos": ("aarch64-apple-darwin", "x86_64-apple-darwin"),
    "linux": ("x86_64-unknown-linux-gnu",),
}
FIXTURE_ARTIFACT = "product-quality-fixtures"
FIXTURE_CONTENT_PATHS = frozenset(
    {
        "browse/1-red.png",
        "browse/2-green.png",
        "browse/10-blue.png",
        "editing/replacement.png",
        "editing/source.png",
        "failure/malformed.png",
        "failure/unsupported.txt",
        "fixture-manifest.txt",
        "sequences/two-frame.gif",
        "sequences/two-frame.png",
        "sequences/two-frame.webp",
        "sequences/two-page.tiff",
        "sequences/two-size.ico",
        "visual/large.png",
        "visual/small.png",
    }
)
FIXTURE_CHECKSUMS = "fixture-sha256.txt"
FIXTURE_PATHS = FIXTURE_CONTENT_PATHS.union({FIXTURE_CHECKSUMS})
REQUIRED_FIELDS = (
    "Version",
    "Candidate commit",
    "Candidate workflow run",
    "Fixture artifact",
    "Fixture manifest SHA-256",
    "Artifact filename",
    "Artifact SHA-256",
    "Package type",
    "Operating system",
    "Display scale",
    "Graphics adapter",
    "Run date",
)
RESULTS = frozenset({"Pass", "Fail", "Approved exception"})
MATRIX_ID_PATTERN = re.compile(r"PQ-[A-Z]{2}-[0-9]{2}")
VERSION_PATTERN = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)")
COMMIT_PATTERN = re.compile(r"[0-9a-f]{40}")
SHA256_PATTERN = re.compile(r"[0-9a-f]{64}")
RUN_URL_PATTERN = re.compile(
    r"https://github\.com/blisspixel/viewr/actions/runs/([1-9][0-9]*)"
)
ISSUE_URL_PATTERN = re.compile(
    r"https://github\.com/blisspixel/viewr/issues/[1-9][0-9]*"
)
PLACEHOLDER_PATTERN = re.compile(
    r"(?:\bTBD\b|\bTODO\b|\bplaceholder\b|\bnot yet recorded\b)", re.IGNORECASE
)


class EvidenceError(ValueError):
    """A product-quality evidence record violated its contract."""


@dataclass(frozen=True)
class Result:
    """One manual matrix result."""

    outcome: str
    observation: str


@dataclass(frozen=True)
class Record:
    """One complete platform evidence record."""

    path: Path
    platform: str
    fields: dict[str, str]
    results: dict[str, Result]


def _table_cells(line: str) -> list[str] | None:
    stripped = line.strip()
    if not stripped.startswith("|") or not stripped.endswith("|"):
        return None

    cells: list[str] = []
    current: list[str] = []
    escaped = False
    for character in stripped[1:-1]:
        if escaped:
            current.append(character)
            escaped = False
        elif character == "\\":
            escaped = True
        elif character == "|":
            cells.append("".join(current).strip())
            current = []
        else:
            current.append(character)
    if escaped:
        current.append("\\")
    cells.append("".join(current).strip())
    return cells


def load_matrix_ids(path: Path = MATRIX_PATH) -> tuple[str, ...]:
    """Return the ordered check identifiers from the canonical matrix."""
    identifiers: list[str] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        cells = _table_cells(line)
        if cells and MATRIX_ID_PATTERN.fullmatch(cells[0]):
            identifiers.append(cells[0])
    if not identifiers:
        raise EvidenceError(f"{path}: product-quality matrix has no check identifiers")
    if len(identifiers) != len(set(identifiers)):
        raise EvidenceError(f"{path}: product-quality matrix has duplicate identifiers")
    return tuple(identifiers)


def _require_value(path: Path, field: str, value: str) -> None:
    if not value or PLACEHOLDER_PATTERN.search(value):
        raise EvidenceError(f"{path}: {field} must contain a recorded value")


def _validate_fields(path: Path, platform: str, fields: dict[str, str]) -> None:
    missing = [field for field in REQUIRED_FIELDS if field not in fields]
    unexpected = sorted(set(fields).difference(REQUIRED_FIELDS))
    if missing:
        raise EvidenceError(f"{path}: missing metadata fields: {', '.join(missing)}")
    if unexpected:
        raise EvidenceError(
            f"{path}: unexpected metadata fields: {', '.join(unexpected)}"
        )
    for field in REQUIRED_FIELDS:
        _require_value(path, field, fields[field])

    version = fields["Version"]
    if not VERSION_PATTERN.fullmatch(version):
        raise EvidenceError(f"{path}: Version must be a stable semantic version")
    if not COMMIT_PATTERN.fullmatch(fields["Candidate commit"]):
        raise EvidenceError(f"{path}: Candidate commit must be a full lowercase SHA")
    if not RUN_URL_PATTERN.fullmatch(fields["Candidate workflow run"]):
        raise EvidenceError(
            f"{path}: Candidate workflow run must be a canonical run URL"
        )
    if not SHA256_PATTERN.fullmatch(fields["Artifact SHA-256"]):
        raise EvidenceError(f"{path}: Artifact SHA-256 must be a lowercase digest")
    if fields["Package type"] != "portable archive":
        raise EvidenceError(f"{path}: Package type must be portable archive for v0.6")
    if fields["Fixture artifact"] != FIXTURE_ARTIFACT:
        raise EvidenceError(
            f"{path}: Fixture artifact must be {FIXTURE_ARTIFACT} for v0.6"
        )
    if not SHA256_PATTERN.fullmatch(fields["Fixture manifest SHA-256"]):
        raise EvidenceError(
            f"{path}: Fixture manifest SHA-256 must be a lowercase digest"
        )

    expected_names = {
        f"viewr-{version}-{target}.zip" for target in PLATFORM_TARGETS[platform]
    }
    if fields["Artifact filename"] not in expected_names:
        names = ", ".join(sorted(expected_names))
        raise EvidenceError(f"{path}: Artifact filename must be one of: {names}")
    try:
        dt.date.fromisoformat(fields["Run date"])
    except ValueError as error:
        raise EvidenceError(f"{path}: Run date must use YYYY-MM-DD") from error


def parse_record(path: Path, matrix_ids: Sequence[str] | None = None) -> Record:
    """Parse and validate one Markdown evidence record."""
    platform = path.stem.lower()
    if platform not in PLATFORM_TARGETS:
        raise EvidenceError(
            f"{path}: filename must be windows.md, macos.md, or linux.md"
        )
    identifiers = tuple(matrix_ids) if matrix_ids is not None else load_matrix_ids()
    expected_title = f"# Product quality evidence: {platform}"
    lines = path.read_text(encoding="utf-8").splitlines()
    if not lines or lines[0] != expected_title:
        raise EvidenceError(f"{path}: first line must be {expected_title!r}")

    fields: dict[str, str] = {}
    results: dict[str, Result] = {}
    for line in lines:
        cells = _table_cells(line)
        if cells is None:
            continue
        if len(cells) == 2 and cells[0] not in {"Field", "---"}:
            if cells[0] in fields:
                raise EvidenceError(f"{path}: duplicate metadata field {cells[0]}")
            fields[cells[0]] = cells[1]
        elif len(cells) == 3 and cells[0] not in {"Check", "---"}:
            if cells[0] not in identifiers:
                raise EvidenceError(f"{path}: unexpected matrix result {cells[0]}")
            if cells[0] in results:
                raise EvidenceError(f"{path}: duplicate matrix result {cells[0]}")
            outcome, observation = cells[1], cells[2]
            if outcome not in RESULTS:
                raise EvidenceError(
                    f"{path}: {cells[0]} has unsupported result {outcome!r}"
                )
            _require_value(path, f"{cells[0]} observation", observation)
            if outcome == "Approved exception" and not ISSUE_URL_PATTERN.search(
                observation
            ):
                raise EvidenceError(
                    f"{path}: {cells[0]} approved exception must link its GitHub issue"
                )
            results[cells[0]] = Result(outcome, observation)

    _validate_fields(path, platform, fields)
    missing_results = [
        identifier for identifier in identifiers if identifier not in results
    ]
    if missing_results:
        raise EvidenceError(
            f"{path}: missing matrix results: {', '.join(missing_results)}"
        )
    return Record(path, platform, fields, results)


def validate_gate(directory: Path) -> tuple[Record, ...]:
    """Validate all platform records as one release-gate evidence set."""
    identifiers = load_matrix_ids()
    records = tuple(
        parse_record(directory / f"{platform}.md", identifiers)
        for platform in PLATFORM_TARGETS
    )
    provenance_fields = (
        "Version",
        "Candidate commit",
        "Candidate workflow run",
        "Fixture artifact",
        "Fixture manifest SHA-256",
    )
    baseline = records[0]
    for record in records[1:]:
        for field in provenance_fields:
            if record.fields[field] != baseline.fields[field]:
                raise EvidenceError(
                    f"{directory}: every platform must share the same {field.lower()}"
                )
    failures = [
        f"{record.platform}:{identifier}"
        for record in records
        for identifier, result in record.results.items()
        if result.outcome == "Fail"
    ]
    if failures:
        raise EvidenceError(
            f"{directory}: failing results block the gate: {', '.join(failures)}"
        )
    return records


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _artifact_target(record: Record) -> str:
    name = record.fields["Artifact filename"]
    prefix = f"viewr-{record.fields['Version']}-"
    return name.removeprefix(prefix).removesuffix(".zip")


def _fixture_files(fixture_root: Path, expected: frozenset[str]) -> dict[str, Path]:
    if fixture_root.is_symlink() or not fixture_root.is_dir():
        raise EvidenceError(
            f"candidate fixture artifact is missing or not a directory: {fixture_root}"
        )
    actual: dict[str, Path] = {}
    for path in fixture_root.rglob("*"):
        if path.is_symlink():
            raise EvidenceError(f"candidate fixture is linked: {path}")
        if path.is_dir():
            continue
        if not path.is_file():
            raise EvidenceError(f"candidate fixture is not a regular file: {path}")
        try:
            resolved = path.resolve(strict=True)
            relative = resolved.relative_to(
                fixture_root.resolve(strict=True)
            ).as_posix()
        except (OSError, ValueError) as error:
            raise EvidenceError(
                f"candidate fixture escapes its artifact: {path}"
            ) from error
        if path.stat().st_size == 0:
            raise EvidenceError(f"candidate fixture is empty: {path}")
        actual[relative] = path
    actual_paths = frozenset(actual)
    if actual_paths != expected:
        missing = sorted(expected.difference(actual_paths))
        unexpected = sorted(actual_paths.difference(expected))
        raise EvidenceError(
            "candidate fixture set mismatch; "
            f"missing={missing}, unexpected={unexpected}"
        )
    return actual


def write_fixture_checksums(fixture_root: Path) -> str:
    """Write a canonical checksum manifest for a newly generated fixture set."""
    checksum_path = fixture_root / FIXTURE_CHECKSUMS
    if checksum_path.exists() or checksum_path.is_symlink():
        raise EvidenceError(f"refusing to replace fixture checksums: {checksum_path}")
    files = _fixture_files(fixture_root, FIXTURE_CONTENT_PATHS)
    lines = [f"{_sha256_file(files[path])}  {path}\n" for path in sorted(files)]
    try:
        with checksum_path.open("x", encoding="ascii", newline="") as output:
            output.writelines(lines)
    except FileExistsError as error:
        raise EvidenceError(
            f"refusing to replace fixture checksums: {checksum_path}"
        ) from error
    return _sha256_file(checksum_path)


def verify_fixtures(artifact_root: Path, expected_manifest_digest: str) -> None:
    """Require and hash the complete synthetic fixture artifact."""
    fixture_root = artifact_root / FIXTURE_ARTIFACT
    files = _fixture_files(fixture_root, FIXTURE_PATHS)
    checksum_path = files[FIXTURE_CHECKSUMS]
    if not hmac.compare_digest(_sha256_file(checksum_path), expected_manifest_digest):
        raise EvidenceError(
            "candidate fixture checksum manifest does not match its recorded SHA-256"
        )
    try:
        lines = checksum_path.read_text(encoding="ascii").splitlines()
    except UnicodeDecodeError as error:
        raise EvidenceError("candidate fixture checksums must be ASCII") from error
    recorded: dict[str, str] = {}
    for line in lines:
        match = re.fullmatch(r"([0-9a-f]{64})  ([^\\\r\n]+)", line)
        if match is None or match.group(2) in recorded:
            raise EvidenceError("candidate fixture checksum manifest is invalid")
        recorded[match.group(2)] = match.group(1)
    if frozenset(recorded) != FIXTURE_CONTENT_PATHS:
        raise EvidenceError(
            "candidate fixture checksum manifest has the wrong file set"
        )
    for path in sorted(FIXTURE_CONTENT_PATHS):
        if not hmac.compare_digest(_sha256_file(files[path]), recorded[path]):
            raise EvidenceError(f"candidate fixture checksum mismatch: {path}")


def verify_artifacts(records: Sequence[Record], artifact_root: Path) -> None:
    """Bind every platform record to downloaded archive bytes and sidecars."""
    try:
        root = artifact_root.resolve(strict=True)
    except OSError as error:
        raise EvidenceError(
            f"artifact directory is unavailable: {artifact_root}"
        ) from error
    if not root.is_dir():
        raise EvidenceError(f"artifact path is not a directory: {artifact_root}")

    for record in records:
        name = record.fields["Artifact filename"]
        target = _artifact_target(record)
        archive = root / f"viewr-{target}" / name
        sidecar = archive.with_suffix(archive.suffix + ".sha256")
        for path, label in ((archive, "archive"), (sidecar, "checksum sidecar")):
            if path.is_symlink() or not path.is_file():
                raise EvidenceError(
                    f"{record.path}: downloaded {label} is missing or not a regular file: {path}"
                )
            try:
                path.resolve(strict=True).relative_to(root)
            except (OSError, ValueError) as error:
                raise EvidenceError(
                    f"{record.path}: downloaded {label} escapes the artifact directory"
                ) from error

        declared_digest = record.fields["Artifact SHA-256"]
        try:
            sidecar_text = sidecar.read_text(encoding="ascii")
        except UnicodeDecodeError as error:
            raise EvidenceError(
                f"{record.path}: checksum sidecar must be ASCII"
            ) from error
        expected_sidecar = f"{declared_digest}  {name}\n"
        if sidecar_text != expected_sidecar:
            raise EvidenceError(
                f"{record.path}: checksum sidecar does not match the recorded artifact"
            )
        if not hmac.compare_digest(_sha256_file(archive), declared_digest):
            raise EvidenceError(
                f"{record.path}: downloaded archive does not match Artifact SHA-256"
            )
    verify_fixtures(root, records[0].fields["Fixture manifest SHA-256"])


def _load_run_metadata(run_id: int) -> dict[str, object]:
    fields = "conclusion,event,headBranch,headSha,status,url,workflowName,databaseId"
    try:
        completed = subprocess.run(
            [
                "gh",
                "run",
                "view",
                str(run_id),
                "--repo",
                REPOSITORY,
                "--json",
                fields,
            ],
            check=False,
            capture_output=True,
            text=True,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise EvidenceError(
            f"could not inspect candidate workflow run {run_id}"
        ) from error
    if completed.returncode != 0:
        detail = completed.stderr.strip() or "GitHub CLI returned no detail"
        raise EvidenceError(
            f"could not inspect candidate workflow run {run_id}: {detail}"
        )
    try:
        metadata = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise EvidenceError(
            f"candidate workflow run {run_id} returned invalid JSON"
        ) from error
    if not isinstance(metadata, dict):
        raise EvidenceError(
            f"candidate workflow run {run_id} returned invalid metadata"
        )
    return metadata


def verify_run(records: Sequence[Record], metadata: Mapping[str, object]) -> None:
    """Confirm that recorded provenance names one successful main candidate run."""
    baseline = records[0]
    run_url = baseline.fields["Candidate workflow run"]
    match = RUN_URL_PATTERN.fullmatch(run_url)
    if match is None:
        raise EvidenceError("candidate workflow run URL is invalid")
    expected = {
        "databaseId": int(match.group(1)),
        "workflowName": WORKFLOW_NAME,
        "event": "workflow_dispatch",
        "headBranch": "main",
        "headSha": baseline.fields["Candidate commit"],
        "status": "completed",
        "conclusion": "success",
        "url": run_url,
    }
    for field, value in expected.items():
        if metadata.get(field) != value:
            raise EvidenceError(
                f"candidate workflow run {field} must be {value!r}, got {metadata.get(field)!r}"
            )


def validate_candidate_gate(
    directory: Path,
    artifact_root: Path,
    run_metadata: Mapping[str, object] | None = None,
) -> tuple[Record, ...]:
    """Validate results, remote workflow provenance, and downloaded artifact bytes."""
    records = validate_gate(directory)
    run_url = records[0].fields["Candidate workflow run"]
    match = RUN_URL_PATTERN.fullmatch(run_url)
    if match is None:
        raise EvidenceError("candidate workflow run URL is invalid")
    metadata = (
        run_metadata
        if run_metadata is not None
        else _load_run_metadata(int(match.group(1)))
    )
    verify_run(records, metadata)
    verify_artifacts(records, artifact_root)
    return records


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Validate viewr product-quality evidence records."
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    check = subparsers.add_parser("check", help="validate one or more complete records")
    check.add_argument("records", nargs="+", type=Path)
    gate = subparsers.add_parser("gate", help="validate the three-platform gate")
    gate.add_argument("directory", type=Path)
    gate.add_argument(
        "--artifacts",
        required=True,
        type=Path,
        help="directory produced by gh run download for the candidate run",
    )
    fixtures = subparsers.add_parser(
        "fixture-manifest", help="checksum a newly generated fixture directory"
    )
    fixtures.add_argument("directory", type=Path)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Run the evidence validator."""
    args = _parser().parse_args(argv)
    try:
        if args.command == "fixture-manifest":
            digest = write_fixture_checksums(args.directory)
            print(f"product-quality fixture manifest created: {digest}")
            return 0
        if args.command == "check":
            records = tuple(parse_record(path) for path in args.records)
        else:
            records = validate_candidate_gate(args.directory, args.artifacts)
    except (EvidenceError, OSError) as error:
        print(f"product-quality evidence failed: {error}", file=sys.stderr)
        return 1

    platforms = ", ".join(record.platform for record in records)
    print(f"product-quality evidence passed: {platforms}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
