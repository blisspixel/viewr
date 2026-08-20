"""Tests for product-quality evidence validation."""

from __future__ import annotations

import contextlib
import hashlib
import io
import re
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts import product_quality_evidence as evidence


COMMIT = "a" * 40
DIGESTS = {
    "windows": "1" * 64,
    "macos": "2" * 64,
    "linux": "3" * 64,
}
TARGETS = {
    "windows": "x86_64-pc-windows-msvc",
    "macos": "aarch64-apple-darwin",
    "linux": "x86_64-unknown-linux-gnu",
}
RUN_URL = "https://github.com/blisspixel/viewr/actions/runs/123456"


class ProductQualityEvidenceTests(unittest.TestCase):
    """Product-quality records fail closed without blocking honest failures."""

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.identifiers = evidence.load_matrix_ids()

    def write_record(
        self,
        platform: str,
        *,
        commit: str = COMMIT,
        run_url: str = RUN_URL,
        omitted_result: str | None = None,
        result_override: tuple[str, str, str] | None = None,
        field_override: tuple[str, str] | None = None,
        digest: str | None = None,
        fixture_digest: str | None = None,
    ) -> Path:
        fields = {
            "Version": "0.6.0",
            "Candidate commit": commit,
            "Candidate workflow run": run_url,
            "Fixture artifact": "product-quality-fixtures",
            "Fixture manifest SHA-256": fixture_digest or ("4" * 64),
            "Artifact filename": f"viewr-0.6.0-{TARGETS[platform]}.zip",
            "Artifact SHA-256": digest or DIGESTS[platform],
            "Package type": "portable archive",
            "Operating system": f"Synthetic {platform} test host",
            "Display scale": "100%, 150%, and 200%",
            "Graphics adapter": "Synthetic adapter",
            "Run date": "2026-08-20",
        }
        if field_override is not None:
            fields[field_override[0]] = field_override[1]
        results = {
            identifier: ("Pass", f"Observed expected behavior for {identifier}.")
            for identifier in self.identifiers
            if identifier != omitted_result
        }
        if result_override is not None:
            identifier, outcome, observation = result_override
            results[identifier] = (outcome, observation)

        lines = [
            f"# Product quality evidence: {platform}",
            "",
            "| Field | Value |",
            "| --- | --- |",
        ]
        lines.extend(f"| {field} | {value} |" for field, value in fields.items())
        lines.extend(("", "| Check | Result | Observation |", "| --- | --- | --- |"))
        lines.extend(
            f"| {identifier} | {outcome} | {observation} |"
            for identifier, (outcome, observation) in results.items()
        )
        path = self.directory / f"{platform}.md"
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return path

    def write_gate(self) -> None:
        for platform in TARGETS:
            self.write_record(platform)

    def write_candidate_gate(self) -> Path:
        artifact_root = self.directory / "artifacts"
        fixture_root = artifact_root / evidence.FIXTURE_ARTIFACT
        (fixture_root / evidence.FIXTURE_CHECKSUMS).unlink(missing_ok=True)
        for relative in evidence.FIXTURE_CONTENT_PATHS:
            fixture = fixture_root / relative
            fixture.parent.mkdir(parents=True, exist_ok=True)
            fixture.write_bytes(f"synthetic {relative}\n".encode())
        fixture_digest = evidence.write_fixture_checksums(fixture_root)
        for platform, target in TARGETS.items():
            payload = f"synthetic candidate archive for {platform}\n".encode()
            digest = hashlib.sha256(payload).hexdigest()
            artifact_directory = artifact_root / f"viewr-{target}"
            artifact_directory.mkdir(parents=True, exist_ok=True)
            name = f"viewr-0.6.0-{target}.zip"
            archive = artifact_directory / name
            archive.write_bytes(payload)
            archive.with_suffix(".zip.sha256").write_text(
                f"{digest}  {name}\n", encoding="ascii"
            )
            self.write_record(platform, digest=digest, fixture_digest=fixture_digest)
        return artifact_root

    @staticmethod
    def run_metadata(**overrides: object) -> dict[str, object]:
        metadata: dict[str, object] = {
            "databaseId": 123456,
            "workflowName": "Release artifacts",
            "event": "workflow_dispatch",
            "headBranch": "main",
            "headSha": COMMIT,
            "status": "completed",
            "conclusion": "success",
            "url": RUN_URL,
        }
        metadata.update(overrides)
        return metadata

    def test_matrix_has_stable_unique_identifiers(self) -> None:
        self.assertEqual(len(self.identifiers), 26)
        self.assertEqual(self.identifiers[0], "PQ-FT-01")
        self.assertEqual(self.identifiers[-1], "PQ-VS-04")

    def test_fixture_contract_matches_the_rust_generator(self) -> None:
        generator = (
            evidence.REPOSITORY_ROOT
            / "crates/viewr/examples/gen_product_quality_fixtures.rs"
        ).read_text(encoding="utf-8")
        fixture_block = generator.split("const FIXTURE_PATHS", 1)[1].split("];", 1)[0]
        generated_paths = set(
            re.findall(
                r'"((?:browse|editing|failure|sequences|visual)/[^"]+)"',
                fixture_block,
            )
        )
        self.assertEqual(
            generated_paths,
            evidence.FIXTURE_CONTENT_PATHS.difference({"fixture-manifest.txt"}),
        )

    def test_parse_complete_record(self) -> None:
        record = evidence.parse_record(self.write_record("windows"))
        self.assertEqual(record.platform, "windows")
        self.assertEqual(record.fields["Candidate commit"], COMMIT)
        self.assertEqual(len(record.results), len(self.identifiers))

    def test_missing_result_is_rejected(self) -> None:
        path = self.write_record("windows", omitted_result="PQ-FT-02")
        with self.assertRaisesRegex(evidence.EvidenceError, "missing matrix results"):
            evidence.parse_record(path)

    def test_duplicate_and_unexpected_rows_are_rejected(self) -> None:
        path = self.write_record("windows")
        original = path.read_text(encoding="utf-8")
        cases = (
            ("| Version | 0.6.0 |\n", "duplicate metadata field"),
            ("| Unknown field | value |\n", "unexpected metadata fields"),
            (
                "| PQ-FT-01 | Pass | Duplicate observation. |\n",
                "duplicate matrix result",
            ),
            ("| PQ-XX-99 | Pass | Unknown check. |\n", "unexpected matrix result"),
        )
        for extra_row, message in cases:
            with self.subTest(message=message):
                path.write_text(original + extra_row, encoding="utf-8")
                with self.assertRaisesRegex(evidence.EvidenceError, message):
                    evidence.parse_record(path)

    def test_placeholder_observation_is_rejected(self) -> None:
        path = self.write_record(
            "windows",
            result_override=("PQ-FT-01", "Pass", "TBD"),
        )
        with self.assertRaisesRegex(
            evidence.EvidenceError, "must contain a recorded value"
        ):
            evidence.parse_record(path)

    def test_approved_exception_requires_issue(self) -> None:
        path = self.write_record(
            "windows",
            result_override=("PQ-FT-01", "Approved exception", "Reviewed locally."),
        )
        with self.assertRaisesRegex(
            evidence.EvidenceError, "must link its GitHub issue"
        ):
            evidence.parse_record(path)

        path = self.write_record(
            "windows",
            result_override=(
                "PQ-FT-01",
                "Approved exception",
                "Reviewed in https://github.com/blisspixel/viewr/issues/88.",
            ),
        )
        self.assertEqual(
            evidence.parse_record(path).results["PQ-FT-01"].outcome,
            "Approved exception",
        )

    def test_invalid_provenance_is_rejected(self) -> None:
        cases = (
            ("Version", "v0.6.0", "stable semantic version"),
            ("Candidate commit", "abc123", "full lowercase SHA"),
            ("Candidate workflow run", "https://example.com/1", "canonical run URL"),
            ("Fixture artifact", "fixtures", "Fixture artifact"),
            ("Fixture manifest SHA-256", "ABC", "Fixture manifest SHA-256"),
            ("Artifact SHA-256", "ABC", "lowercase digest"),
            ("Package type", "installer", "portable archive"),
            ("Artifact filename", "viewr.zip", "Artifact filename"),
            ("Run date", "August 20", "YYYY-MM-DD"),
        )
        for field, value, message in cases:
            with self.subTest(field=field):
                path = self.write_record("windows", field_override=(field, value))
                with self.assertRaisesRegex(evidence.EvidenceError, message):
                    evidence.parse_record(path)

    def test_record_shape_is_rejected(self) -> None:
        wrong_name = self.write_record("windows").with_name("win.md")
        wrong_name.write_text("# Product quality evidence: win\n", encoding="utf-8")
        with self.assertRaisesRegex(evidence.EvidenceError, "filename must be"):
            evidence.parse_record(wrong_name)

        path = self.write_record("windows")
        path.write_text(
            path.read_text(encoding="utf-8").replace(
                "# Product quality evidence: windows", "# Windows result", 1
            ),
            encoding="utf-8",
        )
        with self.assertRaisesRegex(evidence.EvidenceError, "first line must be"):
            evidence.parse_record(path)

    def test_table_parser_preserves_escaped_pipes_and_trailing_slashes(self) -> None:
        self.assertEqual(
            evidence._table_cells(r"| Check | observed \| value |"),
            ["Check", "observed | value"],
        )
        self.assertEqual(
            evidence._table_cells("| Check | trailing\\|"), ["Check", "trailing\\"]
        )
        self.assertIsNone(evidence._table_cells("not a table"))

    def test_gate_requires_shared_provenance(self) -> None:
        self.write_gate()
        records = evidence.validate_gate(self.directory)
        self.assertEqual([record.platform for record in records], list(TARGETS))

        self.write_record("linux", commit="b" * 40)
        with self.assertRaisesRegex(evidence.EvidenceError, "same candidate commit"):
            evidence.validate_gate(self.directory)

    def test_gate_rejects_fail_but_check_preserves_it(self) -> None:
        self.write_gate()
        path = self.write_record(
            "windows",
            result_override=("PQ-RC-03", "Fail", "Worker loss stayed busy."),
        )
        self.assertEqual(
            evidence.parse_record(path).results["PQ-RC-03"].outcome, "Fail"
        )
        with self.assertRaisesRegex(evidence.EvidenceError, "failing results block"):
            evidence.validate_gate(self.directory)

    def test_candidate_gate_verifies_run_and_downloaded_bytes(self) -> None:
        artifact_root = self.write_candidate_gate()
        records = evidence.validate_candidate_gate(
            self.directory, artifact_root, self.run_metadata()
        )
        self.assertEqual([record.platform for record in records], list(TARGETS))

    def test_candidate_gate_rejects_run_mismatch(self) -> None:
        artifact_root = self.write_candidate_gate()
        cases = (
            ("databaseId", 999, "databaseId"),
            ("workflowName", "CI", "workflowName"),
            ("event", "push", "event"),
            ("headBranch", "feature", "headBranch"),
            ("headSha", "b" * 40, "headSha"),
            ("status", "in_progress", "status"),
            ("conclusion", "failure", "conclusion"),
            ("url", RUN_URL + "/attempts/2", "url"),
        )
        for field, value, message in cases:
            with self.subTest(field=field):
                with self.assertRaisesRegex(evidence.EvidenceError, message):
                    evidence.validate_candidate_gate(
                        self.directory,
                        artifact_root,
                        self.run_metadata(**{field: value}),
                    )

    def test_candidate_gate_rejects_unbound_artifacts(self) -> None:
        artifact_root = self.write_candidate_gate()
        target = TARGETS["windows"]
        archive = artifact_root / f"viewr-{target}" / f"viewr-0.6.0-{target}.zip"
        archive.write_bytes(b"different candidate bytes")
        with self.assertRaisesRegex(evidence.EvidenceError, "does not match Artifact"):
            evidence.validate_candidate_gate(
                self.directory, artifact_root, self.run_metadata()
            )

        artifact_root = self.write_candidate_gate()
        archive.with_suffix(".zip.sha256").write_text(
            f"{'f' * 64}  {archive.name}\n", encoding="ascii"
        )
        with self.assertRaisesRegex(evidence.EvidenceError, "sidecar does not match"):
            evidence.validate_candidate_gate(
                self.directory, artifact_root, self.run_metadata()
            )

        artifact_root = self.write_candidate_gate()
        archive.with_suffix(".zip.sha256").unlink()
        with self.assertRaisesRegex(evidence.EvidenceError, "is missing"):
            evidence.validate_candidate_gate(
                self.directory, artifact_root, self.run_metadata()
            )

        artifact_root = self.write_candidate_gate()
        (artifact_root / evidence.FIXTURE_ARTIFACT / "visual/small.png").unlink()
        with self.assertRaisesRegex(evidence.EvidenceError, "fixture set mismatch"):
            evidence.validate_candidate_gate(
                self.directory, artifact_root, self.run_metadata()
            )

        artifact_root = self.write_candidate_gate()
        (artifact_root / evidence.FIXTURE_ARTIFACT / "visual/small.png").write_bytes(
            b"altered fixture bytes"
        )
        with self.assertRaisesRegex(evidence.EvidenceError, "checksum mismatch"):
            evidence.validate_candidate_gate(
                self.directory, artifact_root, self.run_metadata()
            )

        artifact_root = self.write_candidate_gate()
        checksum_path = (
            artifact_root / evidence.FIXTURE_ARTIFACT / evidence.FIXTURE_CHECKSUMS
        )
        checksum_path.write_text(
            checksum_path.read_text(encoding="ascii") + "\n", encoding="ascii"
        )
        with self.assertRaisesRegex(evidence.EvidenceError, "recorded SHA-256"):
            evidence.validate_candidate_gate(
                self.directory, artifact_root, self.run_metadata()
            )

    def test_run_metadata_loader_fails_closed(self) -> None:
        success = mock.Mock(
            returncode=0,
            stdout=evidence.json.dumps(self.run_metadata()),
            stderr="",
        )
        with mock.patch.object(evidence.subprocess, "run", return_value=success):
            self.assertEqual(evidence._load_run_metadata(123456), self.run_metadata())

        failure = mock.Mock(returncode=1, stdout="", stderr="not found")
        with mock.patch.object(evidence.subprocess, "run", return_value=failure):
            with self.assertRaisesRegex(evidence.EvidenceError, "not found"):
                evidence._load_run_metadata(123456)

        invalid = mock.Mock(returncode=0, stdout="not json", stderr="")
        with mock.patch.object(evidence.subprocess, "run", return_value=invalid):
            with self.assertRaisesRegex(evidence.EvidenceError, "invalid JSON"):
                evidence._load_run_metadata(123456)

    def test_fixture_manifest_command_is_canonical_and_refuses_overwrite(self) -> None:
        fixture_root = self.directory / "standalone-fixtures"
        for relative in evidence.FIXTURE_CONTENT_PATHS:
            fixture = fixture_root / relative
            fixture.parent.mkdir(parents=True, exist_ok=True)
            fixture.write_bytes(f"fixture {relative}\n".encode())
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(evidence.main(["fixture-manifest", str(fixture_root)]), 0)
        self.assertRegex(output.getvalue(), r"[0-9a-f]{64}")
        manifest = fixture_root / evidence.FIXTURE_CHECKSUMS
        self.assertEqual(
            len(manifest.read_text(encoding="ascii").splitlines()),
            len(evidence.FIXTURE_CONTENT_PATHS),
        )

        error = io.StringIO()
        with contextlib.redirect_stderr(error):
            self.assertEqual(evidence.main(["fixture-manifest", str(fixture_root)]), 1)
        self.assertIn("refusing to replace", error.getvalue())

    def test_main_reports_success_and_failure(self) -> None:
        path = self.write_record("windows")
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(evidence.main(["check", str(path)]), 0)
        self.assertIn("evidence passed: windows", output.getvalue())

        error = io.StringIO()
        with contextlib.redirect_stderr(error):
            self.assertEqual(
                evidence.main(["check", str(path.with_name("missing.md"))]), 1
            )
        self.assertIn("evidence failed", error.getvalue())

        artifact_root = self.write_candidate_gate()
        output = io.StringIO()
        with (
            mock.patch.object(
                evidence, "_load_run_metadata", return_value=self.run_metadata()
            ),
            contextlib.redirect_stdout(output),
        ):
            self.assertEqual(
                evidence.main(
                    [
                        "gate",
                        str(self.directory),
                        "--artifacts",
                        str(artifact_root),
                    ]
                ),
                0,
            )
        self.assertIn("evidence passed: windows, macos, linux", output.getvalue())


if __name__ == "__main__":
    unittest.main()
