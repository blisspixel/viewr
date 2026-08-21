"""Tests for product-quality evidence validation."""

from __future__ import annotations

import contextlib
import hashlib
import io
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock
import zipfile

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
MAIN_SHA256 = "f" * 64
DECODER_SHA256 = "e" * 64
PLATFORM_METADATA = {
    "windows": {
        "Operating system": "Windows 11 24H2 build 26100.4946",
        "Display scale": (
            "Internal and external displays exercised at 100%, 150%, and 200% "
            "with a live mixed-DPI move"
        ),
        "Graphics adapter": "NVIDIA GeForce RTX 4060 Laptop GPU 32.0.15.7688",
    },
    "macos": {
        "Operating system": "macOS 15.6.1 build 24G90",
        "Display scale": (
            "Built-in Retina display and external display exercised during a live move"
        ),
        "Graphics adapter": "Apple M3 Pro 18-core GPU",
    },
    "linux": {
        "Operating system": "Ubuntu 26.04 LTS, Linux 6.17.0",
        "Display scale": "Native Wayland and X11 sessions at 100% scale",
        "Graphics adapter": (
            "AMD Radeon 780M with Mesa 26.1 plus Mesa llvmpipe software rendering"
        ),
    },
}
PERFORMANCE_CORE = (
    "Candidate performance: window=117.82 ms, first-pixel=282.89 ms, "
    "navigation-max=186.65 ms, "
    "idle-redraws=0, small-rss=312.25 MiB, large-rss=330.08 MiB, "
    "folder-growth=17.83 MiB, large-folder=50,000 images, "
    "cache-stress=4 entries/256 MiB, "
    f"viewr-sha256={MAIN_SHA256}, viewr-decode-sha256={DECODER_SHA256}. Reports: "
)


def performance_observation(
    platform: str,
    main_sha256: str = MAIN_SHA256,
    decoder_sha256: str = DECODER_SHA256,
) -> str:
    reports = ", ".join(
        f"performance/{session}.json"
        for session in evidence.PERFORMANCE_SESSIONS[platform]
    )
    return (
        PERFORMANCE_CORE.replace(MAIN_SHA256, main_sha256).replace(
            DECODER_SHA256, decoder_sha256
        )
        + reports
        + "."
    )


PERFORMANCE_OBSERVATION = performance_observation("windows")


class ProductQualityEvidenceTests(unittest.TestCase):
    """Product-quality records fail closed without blocking honest failures."""

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.directory = self.root / evidence.EVIDENCE_DIRECTORY
        self.directory.mkdir()
        self.identifiers = evidence.load_matrix_ids()
        verifier = mock.patch.object(
            evidence.release_artifact,
            "verify_release_artifact",
            side_effect=self.verify_test_archive,
        )
        verifier.start()
        self.addCleanup(verifier.stop)

    @staticmethod
    def verify_test_archive(path: Path) -> dict[str, object]:
        target = next(
            target
            for target in (*TARGETS.values(), "x86_64-apple-darwin")
            if path.name == f"viewr-0.6.0-{target}.zip"
        )
        prefix = path.name.removesuffix(".zip")
        binary_name = "viewr.exe" if "windows" in target else "viewr"
        worker_name = "viewr-decode.exe" if "windows" in target else "viewr-decode"
        with zipfile.ZipFile(path) as archive:
            binary = archive.read(f"{prefix}/bin/{binary_name}")
            worker = archive.read(f"{prefix}/bin/{worker_name}")
        return {
            "version": "0.6.0",
            "target": target,
            "files": [
                {
                    "path": f"bin/{binary_name}",
                    "sha256": hashlib.sha256(binary).hexdigest(),
                },
                {
                    "path": f"bin/{worker_name}",
                    "sha256": hashlib.sha256(worker).hexdigest(),
                },
            ],
        }

    @staticmethod
    def substantive_observation(platform: str, identifier: str) -> str:
        if identifier == "PQ-VS-04":
            return performance_observation(platform)
        if identifier in evidence.AUTOMATED_PREREQUISITE_TOKENS:
            named_evidence = ", ".join(
                evidence.AUTOMATED_PREREQUISITE_TOKENS[identifier]
            )
            return (
                f"Candidate workflow {RUN_URL} passed {named_evidence} and recorded "
                f"the controlled recovery outcome for {identifier} on commit {COMMIT}."
            )
        terms = ", ".join(evidence.MANUAL_OBSERVATION_TERMS[identifier])
        anchor_tokens = ", ".join(evidence.AUTOMATED_ANCHOR_TOKENS.get(identifier, ()))
        anchor = (
            f" Candidate workflow {RUN_URL} passed {anchor_tokens}."
            if anchor_tokens
            else ""
        )
        return (
            f"On the named {platform} host or session, the candidate archive exercised "
            f"{identifier} with the retained synthetic fixture and verified {terms}. "
            "The visible result and recovery state matched the required row." + anchor
        )

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
        main_digest: str = MAIN_SHA256,
        decoder_digest: str = DECODER_SHA256,
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
            **PLATFORM_METADATA[platform],
            "Run date": "2026-08-20",
        }
        if field_override is not None:
            fields[field_override[0]] = field_override[1]
        results = {
            identifier: ("Pass", self.substantive_observation(platform, identifier))
            for identifier in self.identifiers
            if identifier != omitted_result
        }
        if "PQ-VS-04" in results:
            outcome, observation = results["PQ-VS-04"]
            results["PQ-VS-04"] = (
                outcome,
                observation.replace(MAIN_SHA256, main_digest).replace(
                    DECODER_SHA256, decoder_digest
                ),
            )
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

    @staticmethod
    def probe_report(
        playlist_entries: int,
        peak_resident_mib: float,
        *,
        adapter_backend: str = "dx12",
        adapter_name: str = "AMD Radeon 780M",
        adapter_device_type: str = "integrated-gpu",
        adapter_driver: str = "AMD",
    ) -> dict[str, object]:
        return {
            "adapter_backend": adapter_backend,
            "adapter_name": adapter_name,
            "adapter_device_type": adapter_device_type,
            "adapter_driver": adapter_driver,
            "window_ready_us": 117_820,
            "first_pixel_us": 282_890,
            "max_navigation_us": 186_650,
            "idle_redraws": 0,
            "idle_non_redraw_events": 0,
            "idle_event_repaint_requests": 0,
            "idle_scheduled_egui_repaints": 0,
            "idle_window_focused": True,
            "idle_pointer_inside": False,
            "peak_resident_bytes": int(peak_resident_mib * 1024 * 1024),
            "playlist_entries": playlist_entries,
            "decoded_cache_entries": 4,
            "decoded_cache_bytes": 256 * 1024 * 1024,
            "thumbnail_texture_entries": 9,
        }

    def write_performance_report(
        self,
        session: str,
        platform: str,
        main_digest: str,
        decoder_digest: str,
    ) -> None:
        software = session == "linux-mesa-software"
        if software:
            adapter = {
                "adapter_backend": "gl",
                "adapter_name": "llvmpipe (LLVM 20.1.8)",
                "adapter_device_type": "cpu",
                "adapter_driver": "llvmpipe",
            }
        elif platform == "macos":
            adapter = {
                "adapter_backend": "metal",
                "adapter_name": "Apple M4",
                "adapter_device_type": "integrated-gpu",
                "adapter_driver": "Metal",
            }
        elif platform == "linux":
            adapter = {
                "adapter_backend": "vulkan",
                "adapter_name": "AMD Radeon 780M",
                "adapter_device_type": "integrated-gpu",
                "adapter_driver": "Mesa",
            }
        else:
            adapter = {}
        small = [self.probe_report(16, 312.25, **adapter) for _ in range(3)]
        session_nonce = sum(ord(character) for character in session)
        small[0]["idle_non_redraw_events"] = session_nonce
        large = [self.probe_report(50_000, 330.08, **adapter) for _ in range(3)]
        cache_stress = self.probe_report(8, 330.08, **adapter)
        summary = evidence._expected_performance_summary(small, large, cache_stress)
        renderer = {"wgpu_backend": "", "libgl_always_software": ""}
        if session == "linux-mesa-software":
            renderer = {"wgpu_backend": "gl", "libgl_always_software": "1"}
        if platform == "windows":
            session_evidence: dict[str, object] = {
                "display_scale_percent": int(session.removeprefix("windows-"))
            }
        elif platform == "macos":
            built_in = session == "macos-retina"
            session_evidence = {
                "display_identity_sha256": hashlib.sha256(session.encode()).hexdigest(),
                "display_builtin": built_in,
                "display_retina": built_in,
                "display_scale_percent": 200 if built_in else 100,
            }
        else:
            session_evidence = {
                "linux_session": ("wayland" if session == "linux-wayland" else "x11")
            }
            if software:
                session_evidence.update(
                    {
                        "opengl_renderer": "llvmpipe",
                        "opengl_vendor": "Mesa/X.org",
                        "opengl_mesa": True,
                        "opengl_software": True,
                    }
                )
        report = {
            "schema": 3,
            "status": "pass",
            "executable_sha256": {
                "viewr": main_digest,
                "viewr-decode": decoder_digest,
            },
            "session_label": session,
            "host_platform": evidence.PERFORMANCE_HOST_PLATFORMS[platform],
            "session_evidence": session_evidence,
            "renderer_controls": renderer,
            "budgets": evidence.PERFORMANCE_BUDGETS,
            "summary": summary,
            "runs": {
                "small": small,
                "large": large,
                "cache_stress": cache_stress,
            },
            "failures": [],
        }
        report_root = self.directory / "performance"
        report_root.mkdir(exist_ok=True)
        (report_root / f"{session}.json").write_text(
            evidence.json.dumps(report, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )

    def test_performance_summary_uses_every_retained_navigation_and_idle(self) -> None:
        small = [self.probe_report(16, 312.25) for _ in range(3)]
        large = [self.probe_report(50_000, 330.08) for _ in range(3)]
        cache_stress = self.probe_report(8, 330.08)
        large[0]["window_ready_us"] = 710_000
        large[1]["window_ready_us"] = 720_000
        large[2]["window_ready_us"] = 730_000
        large[0]["first_pixel_us"] = 810_000
        large[1]["first_pixel_us"] = 820_000
        large[2]["first_pixel_us"] = 830_000
        large[1]["max_navigation_us"] = 610_000
        cache_stress["idle_redraws"] = 2

        summary = evidence._expected_performance_summary(small, large, cache_stress)

        self.assertEqual(summary["window_ready_ms"], 720.0)
        self.assertEqual(summary["first_pixel_ms"], 820.0)
        self.assertEqual(summary["navigation_max_ms"], 610.0)
        self.assertEqual(summary["idle_redraws"], 2)

        large[1]["max_navigation_us"] = 400_000
        cache_stress["max_navigation_us"] = 730_000
        summary = evidence._expected_performance_summary(small, large, cache_stress)
        self.assertEqual(summary["navigation_max_ms"], 730.0)

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
            binary = f"synthetic candidate binary for {platform}\n".encode()
            worker = f"synthetic candidate worker for {platform}\n".encode()
            main_digest = hashlib.sha256(binary).hexdigest()
            decoder_digest = hashlib.sha256(worker).hexdigest()
            artifact_directory = artifact_root / f"viewr-{target}"
            artifact_directory.mkdir(parents=True, exist_ok=True)
            name = f"viewr-0.6.0-{target}.zip"
            archive = artifact_directory / name
            prefix = archive.name.removesuffix(".zip")
            binary_name = "viewr.exe" if platform == "windows" else "viewr"
            with zipfile.ZipFile(archive, "w") as package:
                package.writestr(f"{prefix}/bin/{binary_name}", binary)
                worker_name = (
                    "viewr-decode.exe" if platform == "windows" else "viewr-decode"
                )
                package.writestr(f"{prefix}/bin/{worker_name}", worker)
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            archive.with_suffix(".zip.sha256").write_bytes(
                f"{digest}  {name}\n".encode("ascii")
            )
            self.write_record(
                platform,
                digest=digest,
                fixture_digest=fixture_digest,
                main_digest=main_digest,
                decoder_digest=decoder_digest,
            )
            for session in evidence.PERFORMANCE_SESSIONS[platform]:
                self.write_performance_report(
                    session, platform, main_digest, decoder_digest
                )

        intel_target = "x86_64-apple-darwin"
        intel_directory = artifact_root / f"viewr-{intel_target}"
        intel_directory.mkdir(parents=True, exist_ok=True)
        intel_archive = intel_directory / f"viewr-0.6.0-{intel_target}.zip"
        intel_prefix = intel_archive.name.removesuffix(".zip")
        with zipfile.ZipFile(intel_archive, "w") as package:
            package.writestr(f"{intel_prefix}/bin/viewr", b"synthetic Intel binary")
            package.writestr(
                f"{intel_prefix}/bin/viewr-decode", b"synthetic Intel worker"
            )
        intel_digest = hashlib.sha256(intel_archive.read_bytes()).hexdigest()
        intel_archive.with_suffix(".zip.sha256").write_bytes(
            f"{intel_digest}  {intel_archive.name}\n".encode("ascii")
        )
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

    @staticmethod
    def artifact_metadata(**size_overrides: int) -> dict[str, object]:
        artifacts = []
        for name in sorted(evidence.EXPECTED_REMOTE_ARTIFACTS):
            default_size = 42_490 if name == evidence.FIXTURE_ARTIFACT else 12_000_000
            artifacts.append(
                {
                    "name": name,
                    "size_in_bytes": size_overrides.get(name, default_size),
                    "expired": False,
                }
            )
        return {"total_count": len(artifacts), "artifacts": artifacts}

    def test_matrix_has_stable_unique_identifiers(self) -> None:
        self.assertEqual(len(self.identifiers), 26)
        self.assertEqual(self.identifiers[0], "PQ-FT-01")
        self.assertEqual(self.identifiers[-1], "PQ-VS-04")

    def test_direct_entrypoint_help_uses_the_repository_release_verifier(self) -> None:
        completed = subprocess.run(
            [
                sys.executable,
                "-B",
                str(evidence.REPOSITORY_ROOT / "scripts/product_quality_evidence.py"),
                "--help",
            ],
            cwd=evidence.REPOSITORY_ROOT,
            check=False,
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn("Validate viewr product-quality evidence", completed.stdout)

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

    def test_generic_observation_is_rejected(self) -> None:
        for observation in (
            "Pass.",
            "Works as expected",
            "Everything works as expected.",
            "Observed expected behavior for PQ-FT-01.",
            "No issues found.",
            "No issues were observed.",
            "Passed on Windows.",
            "Looks good.",
            "Tested successfully.",
        ):
            with self.subTest(observation=observation):
                path = self.write_record(
                    "windows",
                    result_override=("PQ-FT-01", "Pass", observation),
                )
                with self.assertRaisesRegex(
                    evidence.EvidenceError, "must describe concrete evidence"
                ):
                    evidence.parse_record(path)

        path = self.write_record(
            "windows",
            result_override=(
                "PQ-FT-01",
                "Pass",
                (
                    "The windows candidate archive exercised PQ-FT-01 and the named "
                    "control, visible result, and recovery state were recorded."
                ),
            ),
        )
        with self.assertRaisesRegex(
            evidence.EvidenceError, "must name concrete results"
        ):
            evidence.parse_record(path)

        valid = self.substantive_observation("windows", "PQ-FT-01")
        path = self.write_record(
            "windows",
            result_override=(
                "PQ-FT-01",
                "Pass",
                valid.replace("host or session", "environment"),
            ),
        )
        with self.assertRaisesRegex(evidence.EvidenceError, "host or session"):
            evidence.parse_record(path)

    def test_approved_exception_requires_issue_and_acceptable_severity(self) -> None:
        concrete = self.substantive_observation("windows", "PQ-FT-01")
        path = self.write_record(
            "windows",
            result_override=(
                "PQ-FT-01",
                "Approved exception",
                "Low severity: " + concrete,
            ),
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
                (
                    "Low severity: " + concrete + " A malformed issue address "
                    "https://github.com/blisspixel/viewr/issues/88extra does not own it."
                ),
            ),
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
                (
                    concrete
                    + " Reviewed in https://github.com/blisspixel/viewr/issues/88."
                ),
            ),
        )
        with self.assertRaisesRegex(
            evidence.EvidenceError, "must start with Low severity"
        ):
            evidence.parse_record(path)

        for severity in ("Critical", "High"):
            with self.subTest(severity=severity):
                path = self.write_record(
                    "windows",
                    result_override=(
                        "PQ-FT-01",
                        "Approved exception",
                        (
                            f"{severity} severity: {concrete} "
                            "https://github.com/blisspixel/viewr/issues/88 owns it."
                        ),
                    ),
                )
                with self.assertRaisesRegex(
                    evidence.EvidenceError, f"{severity} severity cannot be approved"
                ):
                    evidence.parse_record(path)

        for observation in (
            (
                "Low severity: "
                + concrete
                + " https://github.com/blisspixel/viewr/issues/88 owns the follow-up."
            ),
            (
                "Medium severity: "
                + concrete
                + " https://github.com/blisspixel/viewr/issues/89 owns the follow-up."
            ),
        ):
            with self.subTest(observation=observation):
                path = self.write_record(
                    "windows",
                    result_override=(
                        "PQ-FT-01",
                        "Approved exception",
                        observation,
                    ),
                )
                self.assertEqual(
                    evidence.parse_record(path).results["PQ-FT-01"].outcome,
                    "Approved exception",
                )

    def test_exact_evidence_version_and_directory_are_required(self) -> None:
        path = self.write_record("windows", field_override=("Version", "0.5.0"))
        with self.assertRaisesRegex(evidence.EvidenceError, "Version must be 0.6.0"):
            evidence.parse_record(path)

        valid = self.write_record("windows")
        wrong_directory = self.root / "product-quality"
        wrong_directory.mkdir()
        misplaced = wrong_directory / valid.name
        misplaced.write_bytes(valid.read_bytes())
        with self.assertRaisesRegex(
            evidence.EvidenceError, "must be stored under v0.6.0"
        ):
            evidence.parse_record(misplaced)

        with self.assertRaisesRegex(
            evidence.EvidenceError, "evidence directory must be named v0.6.0"
        ):
            evidence.validate_gate(wrong_directory)

    def test_platform_metadata_proves_representative_coverage(self) -> None:
        cases = (
            (
                "windows",
                "Operating system",
                "Windows Server 2025",
                "Windows 10 or 11",
            ),
            (
                "windows",
                "Display scale",
                "Internal and external displays at 100% and 150%",
                "200%",
            ),
            ("macos", "Operating system", "macOS", "tested macOS version"),
            (
                "macos",
                "Display scale",
                "Built-in Retina display only",
                "external display",
            ),
            (
                "macos",
                "Display scale",
                "External display only",
                "Retina display",
            ),
            (
                "macos",
                "Graphics adapter",
                "Intel Iris Plus Graphics 655",
                "Apple Silicon or arm64",
            ),
            (
                "linux",
                "Operating system",
                "Ubuntu Linux",
                "tested Linux version",
            ),
            (
                "linux",
                "Display scale",
                "Native Wayland session at 100% scale",
                "X11 or Xwayland",
            ),
            (
                "linux",
                "Display scale",
                "Native X11 session at 100% scale",
                "native Wayland",
            ),
            (
                "linux",
                "Graphics adapter",
                "AMD Radeon 780M with proprietary driver and software renderer",
                "Mesa renderer",
            ),
            (
                "linux",
                "Graphics adapter",
                "AMD Radeon 780M with Mesa 26.1 hardware rendering",
                "software rendering",
            ),
        )
        for platform, field, value, message in cases:
            with self.subTest(platform=platform, field=field, message=message):
                path = self.write_record(platform, field_override=(field, value))
                with self.assertRaisesRegex(evidence.EvidenceError, message):
                    evidence.parse_record(path)

        path = self.write_record(
            "windows", field_override=("Graphics adapter", "Synthetic adapter")
        )
        with self.assertRaisesRegex(
            evidence.EvidenceError, "must identify the tested environment"
        ):
            evidence.parse_record(path)

    def test_platform_coverage_can_span_metadata_and_observations(self) -> None:
        path = self.write_record(
            "macos",
            field_override=("Display scale", "Built-in Retina display"),
            result_override=(
                "PQ-VS-03",
                "Pass",
                (
                    self.substantive_observation("macos", "PQ-VS-03") + " "
                    "Moved the candidate window from the built-in Retina panel to an "
                    "external display; focus and profile state refreshed."
                ),
            ),
        )
        self.assertEqual(evidence.parse_record(path).platform, "macos")

        path = self.write_record(
            "linux",
            field_override=("Display scale", "Native Wayland at 100% scale"),
            result_override=(
                "PQ-VS-03",
                "Pass",
                (
                    self.substantive_observation("linux", "PQ-VS-03") + " "
                    "Repeated the scale and focus checks in an Xwayland session using "
                    "the same candidate archive."
                ),
            ),
        )
        self.assertEqual(evidence.parse_record(path).platform, "linux")

    def test_platform_coverage_requires_display_transitions_and_scale(self) -> None:
        cases = (
            (
                "windows",
                "Display scale",
                "Displays at 100%, 150%, and 200%",
                "move between displays",
            ),
            (
                "macos",
                "Display scale",
                "Built-in Retina display and external display",
                "live display move",
            ),
            (
                "linux",
                "Display scale",
                "Native Wayland and X11 sessions",
                "tested display scale",
            ),
        )
        for platform, field, value, message in cases:
            with self.subTest(platform=platform):
                path = self.write_record(platform, field_override=(field, value))
                with self.assertRaisesRegex(evidence.EvidenceError, message):
                    evidence.parse_record(path)

    def test_automated_prerequisites_require_run_and_named_evidence(self) -> None:
        valid = self.substantive_observation("windows", "PQ-RC-03")
        cases = (
            (valid.replace(RUN_URL, "the candidate run"), "candidate workflow run"),
            (
                valid.replace(
                    "crop_preview_disconnect_copy_and_recovery_priority_are_truthful",
                    "crop recovery test",
                ),
                "must name automated evidence",
            ),
        )
        for observation, message in cases:
            with self.subTest(message=message):
                path = self.write_record(
                    "windows",
                    result_override=("PQ-RC-03", "Pass", observation),
                )
                with self.assertRaisesRegex(evidence.EvidenceError, message):
                    evidence.parse_record(path)

        for identifier in evidence.AUTOMATED_ANCHOR_TOKENS:
            valid = self.substantive_observation("windows", identifier)
            token = evidence.AUTOMATED_ANCHOR_TOKENS[identifier][0]
            with self.subTest(identifier=identifier):
                path = self.write_record(
                    "windows",
                    result_override=(
                        identifier,
                        "Pass",
                        valid.replace(token, "unnamed automated check"),
                    ),
                )
                with self.assertRaisesRegex(evidence.EvidenceError, "must name"):
                    evidence.parse_record(path)

    def test_performance_observation_requires_every_numeric_measurement(self) -> None:
        cases = (
            (
                PERFORMANCE_OBSERVATION.replace("window=117.82 ms, ", ""),
                "numeric window ready",
            ),
            (
                PERFORMANCE_OBSERVATION.replace("first-pixel=282.89 ms, ", ""),
                "numeric first pixel",
            ),
            (
                PERFORMANCE_OBSERVATION.replace(
                    "navigation-max=186.65 ms, ", "navigation=fast, "
                ),
                "numeric navigation",
            ),
            (
                PERFORMANCE_OBSERVATION.replace("idle-redraws=0, ", ""),
                "numeric idle redraws",
            ),
            (
                PERFORMANCE_OBSERVATION.replace("small-rss=312.25 MiB, ", ""),
                "numeric small RSS",
            ),
            (
                PERFORMANCE_OBSERVATION.replace("large-rss=330.08 MiB, ", ""),
                "numeric large RSS",
            ),
            (
                PERFORMANCE_OBSERVATION.replace("folder-growth=17.83 MiB, ", ""),
                "numeric folder growth",
            ),
            (
                PERFORMANCE_OBSERVATION.replace("large-folder=50,000 images, ", ""),
                "numeric file count",
            ),
            (
                PERFORMANCE_OBSERVATION.replace(
                    "cache-stress=4 entries/256 MiB, ", "cache-mib=256 MiB, "
                ),
                "numeric cache count",
            ),
            (
                PERFORMANCE_OBSERVATION.replace(
                    "cache-stress=4 entries/256 MiB", "cache-count=4 entries"
                ),
                "numeric cache MiB",
            ),
            (
                PERFORMANCE_OBSERVATION.replace(f"viewr-sha256={MAIN_SHA256}, ", ""),
                "viewr SHA-256",
            ),
            (
                PERFORMANCE_OBSERVATION.replace(
                    f"viewr-decode-sha256={DECODER_SHA256}. ", ""
                ),
                "viewr-decode SHA-256",
            ),
            (
                PERFORMANCE_OBSERVATION.replace(MAIN_SHA256, MAIN_SHA256.upper()),
                "viewr SHA-256",
            ),
        )
        for observation, message in cases:
            with self.subTest(message=message):
                path = self.write_record(
                    "windows",
                    result_override=("PQ-VS-04", "Pass", observation),
                )
                with self.assertRaisesRegex(evidence.EvidenceError, message):
                    evidence.parse_record(path)

        path = self.write_record(
            "windows",
            result_override=(
                "PQ-VS-04",
                "Pass",
                PERFORMANCE_OBSERVATION.replace("50,000 images", "49,999 images"),
            ),
        )
        with self.assertRaisesRegex(evidence.EvidenceError, "at least 50,000 files"):
            evidence.parse_record(path)

    def test_performance_observation_accepts_probe_output_names(self) -> None:
        observation = (
            "Candidate performance: window=117.82 ms, first-pixel=282.89 ms, "
            "navigation-max=186.65 ms, "
            "idle-redraws=0, small-rss=312.25 MiB, large-rss=330.08 MiB, "
            "folder-growth=17.83 MiB, large-folder=50,000 images, "
            "cache-stress=4 entries/256 MiB, "
            f"viewr-sha256={MAIN_SHA256}, "
            f"viewr-decode-sha256={DECODER_SHA256}. Reports: "
            "performance/linux-wayland.json, performance/linux-x11.json, "
            "performance/linux-mesa-software.json."
        )
        path = self.write_record(
            "linux", result_override=("PQ-VS-04", "Pass", observation)
        )
        self.assertEqual(
            evidence.parse_record(path).results["PQ-VS-04"].observation,
            observation,
        )

    def test_passing_performance_observation_must_meet_every_budget(self) -> None:
        cases = (
            ("window=117.82", "window=3000.01", "window ready exceeds"),
            ("first-pixel=282.89", "first-pixel=5000.01", "first pixel exceeds"),
            (
                "navigation-max=186.65",
                "navigation-max=500.01",
                "navigation exceeds",
            ),
            ("idle-redraws=0", "idle-redraws=3", "idle redraws exceeds"),
            ("large-rss=330.08", "large-rss=768.01", "large RSS exceeds"),
            (
                "folder-growth=17.83",
                "folder-growth=96.01",
                "folder growth exceeds",
            ),
            (
                "cache-stress=4 entries/256 MiB",
                "cache-stress=3 entries/192 MiB",
                "cache stress must retain exactly",
            ),
        )
        for old, new, message in cases:
            with self.subTest(message=message):
                path = self.write_record(
                    "windows",
                    result_override=(
                        "PQ-VS-04",
                        "Pass",
                        PERFORMANCE_OBSERVATION.replace(old, new),
                    ),
                )
                with self.assertRaisesRegex(evidence.EvidenceError, message):
                    evidence.parse_record(path)

    def test_macos_record_requires_the_apple_silicon_archive(self) -> None:
        path = self.write_record(
            "macos",
            field_override=(
                "Artifact filename",
                "viewr-0.6.0-x86_64-apple-darwin.zip",
            ),
        )
        with self.assertRaisesRegex(evidence.EvidenceError, "aarch64-apple-darwin"):
            evidence.parse_record(path)

    def test_invalid_provenance_is_rejected(self) -> None:
        cases = (
            ("Version", "v0.6.0", "Version must be 0.6.0"),
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
        observation = self.substantive_observation("windows", "PQ-RC-03")
        path = self.write_record(
            "windows",
            result_override=(
                "PQ-RC-03",
                "Fail",
                observation + " Worker loss stayed busy instead of reaching recovery.",
            ),
        )
        self.assertEqual(
            evidence.parse_record(path).results["PQ-RC-03"].outcome, "Fail"
        )
        with self.assertRaisesRegex(evidence.EvidenceError, "failing results block"):
            evidence.validate_gate(self.directory)

    def test_gate_requires_shared_passing_automated_prerequisites(self) -> None:
        self.write_gate()
        observation = self.substantive_observation("windows", "PQ-RC-03")
        self.write_record(
            "windows",
            result_override=(
                "PQ-RC-03",
                "Approved exception",
                (
                    "Low severity: tracked at "
                    "https://github.com/blisspixel/viewr/issues/42. " + observation
                ),
            ),
        )
        with self.assertRaisesRegex(
            evidence.EvidenceError, "hard prerequisite must pass"
        ):
            evidence.validate_gate(self.directory)

        self.write_record(
            "windows",
            result_override=(
                "PQ-RC-03",
                "Pass",
                observation + " Windows record added different common evidence.",
            ),
        )
        with self.assertRaisesRegex(
            evidence.EvidenceError, "must share PQ-RC-03 automated evidence"
        ):
            evidence.validate_gate(self.directory)

    def test_candidate_gate_verifies_run_and_downloaded_bytes(self) -> None:
        artifact_root = self.write_candidate_gate()
        records = evidence.validate_candidate_gate(
            self.directory, artifact_root, self.run_metadata()
        )
        self.assertEqual([record.platform for record in records], list(TARGETS))

    def test_candidate_gate_runs_canonical_archive_verification(self) -> None:
        artifact_root = self.write_candidate_gate()
        with mock.patch.object(
            evidence.release_artifact,
            "verify_release_artifact",
            side_effect=evidence.release_artifact.ReleaseError("manifest mismatch"),
        ):
            with self.assertRaisesRegex(
                evidence.EvidenceError, "canonical archive verification failed"
            ):
                evidence.validate_candidate_gate(
                    self.directory, artifact_root, self.run_metadata()
                )

    def test_candidate_gate_rejects_archive_or_sidecar_changes_during_verification(
        self,
    ) -> None:
        for changed_part in ("archive", "sidecar"):
            with self.subTest(changed_part=changed_part):
                artifact_root = self.write_candidate_gate()
                target = TARGETS["windows"]
                archive = (
                    artifact_root / f"viewr-{target}" / f"viewr-0.6.0-{target}.zip"
                )
                original_verify = self.verify_test_archive

                def mutate_during_verification(path: Path) -> dict[str, object]:
                    manifest = original_verify(path)
                    if path.name == archive.name:
                        if changed_part == "archive":
                            path.write_bytes(b"replacement archive bytes")
                        else:
                            path.with_suffix(".zip.sha256").write_bytes(
                                f"{'0' * 64}  {path.name}\n".encode("ascii")
                            )
                    return manifest

                with mock.patch.object(
                    evidence.release_artifact,
                    "verify_release_artifact",
                    side_effect=mutate_during_verification,
                ):
                    with self.assertRaisesRegex(
                        evidence.EvidenceError, "changed during canonical verification"
                    ):
                        evidence.validate_candidate_gate(
                            self.directory, artifact_root, self.run_metadata()
                        )

    def test_candidate_gate_applies_bounded_reads_and_aggregate_budget(self) -> None:
        cases = (
            ("MAX_DOWNLOADED_FILE_BYTES", 1, "safety limit"),
            ("MAX_SIDECAR_BYTES", 1, "safety limit"),
            ("MAX_FIXTURE_ARTIFACT_BYTES", 1, "safety limit"),
            ("MAX_ARTIFACT_SET_BYTES", 1, "aggregate safety limit"),
        )
        for constant, limit, message in cases:
            with self.subTest(constant=constant):
                artifact_root = self.write_candidate_gate()
                with mock.patch.object(evidence, constant, limit):
                    with self.assertRaisesRegex(evidence.EvidenceError, message):
                        evidence.validate_candidate_gate(
                            self.directory, artifact_root, self.run_metadata()
                        )

        artifact_root = self.write_candidate_gate()
        with mock.patch.object(evidence, "MAX_PERFORMANCE_REPORT_BYTES", 1):
            with self.assertRaisesRegex(evidence.EvidenceError, "safety limit"):
                evidence.validate_candidate_gate(
                    self.directory, artifact_root, self.run_metadata()
                )

        path = self.write_record("windows")
        with mock.patch.object(evidence, "MAX_RECORD_BYTES", 1):
            with self.assertRaisesRegex(evidence.EvidenceError, "safety limit"):
                evidence.parse_record(path)

    def test_record_paths_reject_link_like_files_and_directories(self) -> None:
        path = self.write_record("windows")
        real_is_link_like = evidence._is_link_like
        for linked_path in (path, path.parent):
            with self.subTest(linked_path=linked_path):
                with mock.patch.object(
                    evidence,
                    "_is_link_like",
                    side_effect=lambda candidate, linked=linked_path: (
                        candidate == linked or real_is_link_like(candidate)
                    ),
                ):
                    with self.assertRaisesRegex(evidence.EvidenceError, "linked"):
                        evidence.parse_record(path)

    @unittest.skipUnless(sys.platform == "win32", "Windows junction test")
    def test_candidate_gate_rejects_application_fixture_and_report_junctions(
        self,
    ) -> None:
        for relative in (
            Path("artifacts") / f"viewr-{TARGETS['windows']}",
            Path("artifacts") / evidence.FIXTURE_ARTIFACT,
            Path("performance"),
        ):
            with self.subTest(relative=relative):
                artifact_root = self.write_candidate_gate()
                linked = self.directory / relative
                outside = self.root / ("outside-" + linked.name)
                linked.rename(outside)
                completed = subprocess.run(
                    ["cmd", "/c", "mklink", "/J", str(linked), str(outside)],
                    check=False,
                    capture_output=True,
                    text=True,
                )
                self.assertEqual(completed.returncode, 0, completed.stderr)
                try:
                    with self.assertRaisesRegex(
                        evidence.EvidenceError, "linked|reparse"
                    ):
                        evidence.validate_candidate_gate(
                            self.directory, artifact_root, self.run_metadata()
                        )
                finally:
                    linked.rmdir()
                    shutil.rmtree(outside)

    def test_candidate_gate_rejects_unbound_or_failed_performance_reports(self) -> None:
        def update_all_runs(report: dict[str, object], **fields: object) -> None:
            runs = report["runs"]
            for one in [*runs["small"], *runs["large"], runs["cache_stress"]]:
                one.update(fields)

        def set_cache_stress_idle_over_budget(report: dict[str, object]) -> None:
            report["runs"]["cache_stress"]["idle_redraws"] = 3
            report["summary"]["idle_redraws"] = 3

        def set_large_window_over_budget(report: dict[str, object]) -> None:
            for one in report["runs"]["large"]:
                one["window_ready_us"] = 3_001_000
            report["summary"]["window_ready_ms"] = 3001.0

        def set_large_first_pixel_over_budget(report: dict[str, object]) -> None:
            for one in report["runs"]["large"]:
                one["first_pixel_us"] = 5_001_000
            report["summary"]["first_pixel_ms"] = 5001.0

        def set_large_navigation_over_budget(report: dict[str, object]) -> None:
            report["runs"]["large"][0]["max_navigation_us"] = 600_000
            report["summary"]["navigation_max_ms"] = 600.0

        def set_cache_navigation_over_budget(report: dict[str, object]) -> None:
            report["runs"]["cache_stress"]["max_navigation_us"] = 600_000
            report["summary"]["navigation_max_ms"] = 600.0

        cases = (
            (
                "windows-100",
                lambda report: report["executable_sha256"].__setitem__(
                    "viewr-decode", "0" * 64
                ),
                "executable SHA-256 values do not match the archive",
            ),
            (
                "windows-100",
                lambda report: (
                    report.__setitem__("status", "fail"),
                    report.__setitem__("failures", ["first pixel exceeded"]),
                ),
                "must pass without failures",
            ),
            (
                "windows-100",
                lambda report: report["runs"]["small"][0].__setitem__(
                    "decoded_cache_entries", 6
                ),
                "exceeds a cache or idle limit",
            ),
            (
                "windows-100",
                lambda report: report["summary"].__setitem__(
                    "first_pixel_ms", report["summary"]["first_pixel_ms"] + 1
                ),
                "summary does not match retained runs",
            ),
            (
                "windows-100",
                set_large_window_over_budget,
                "performance summary exceeds a release budget",
            ),
            (
                "windows-100",
                set_large_first_pixel_over_budget,
                "performance summary exceeds a release budget",
            ),
            (
                "windows-100",
                set_large_navigation_over_budget,
                "performance summary exceeds a release budget",
            ),
            (
                "windows-100",
                set_cache_navigation_over_budget,
                "performance summary exceeds a release budget",
            ),
            (
                "windows-100",
                set_cache_stress_idle_over_budget,
                "exceeds a cache or idle limit",
            ),
            (
                "linux-mesa-software",
                lambda report: report.__setitem__(
                    "renderer_controls",
                    {"wgpu_backend": "", "libgl_always_software": ""},
                ),
                "WGPU_BACKEND=gl",
            ),
            (
                "windows-150",
                lambda report: report.__setitem__(
                    "session_evidence", {"display_scale_percent": 100}
                ),
                "must measure 150%",
            ),
            (
                "linux-mesa-software",
                lambda report: report["session_evidence"].__setitem__(
                    "opengl_software", False
                ),
                "must measure a Mesa software renderer",
            ),
            (
                "linux-wayland",
                lambda report: report["session_evidence"].__setitem__(
                    "linux_session", "x11"
                ),
                "must measure a Wayland session",
            ),
            (
                "linux-x11",
                lambda report: update_all_runs(
                    report,
                    adapter_backend="gl",
                    adapter_name="llvmpipe",
                    adapter_device_type="cpu",
                    adapter_driver="llvmpipe",
                ),
                "representative hardware session used a software adapter",
            ),
            (
                "linux-mesa-software",
                lambda report: update_all_runs(
                    report,
                    adapter_backend="gl",
                    adapter_name="AMD Radeon 780M",
                    adapter_device_type="integrated-gpu",
                    adapter_driver="Mesa",
                ),
                "must use viewr's actual GL software adapter",
            ),
            (
                "windows-100",
                lambda report: report["runs"]["small"][0].__setitem__(
                    "adapter_name", "Different GPU"
                ),
                "selected different GPU adapters",
            ),
            (
                "macos-retina",
                lambda report: update_all_runs(report, adapter_backend="dx12"),
                "not a valid Darwin adapter backend",
            ),
            (
                "macos-retina",
                lambda report: update_all_runs(report, adapter_backend="gl"),
                "not a valid Darwin adapter backend",
            ),
            (
                "macos-retina",
                lambda report: report["session_evidence"].__setitem__(
                    "display_builtin", False
                ),
                "built-in Retina display",
            ),
        )
        for session, mutate, message in cases:
            with self.subTest(session=session, message=message):
                artifact_root = self.write_candidate_gate()
                path = self.directory / "performance" / f"{session}.json"
                report = evidence.json.loads(path.read_text(encoding="utf-8"))
                mutate(report)
                path.write_text(
                    evidence.json.dumps(report, indent=2, sort_keys=True) + "\n",
                    encoding="utf-8",
                )
                with self.assertRaisesRegex(evidence.EvidenceError, message):
                    evidence.validate_candidate_gate(
                        self.directory, artifact_root, self.run_metadata()
                    )

    def test_candidate_gate_does_not_apply_large_folder_rss_budget_to_cache_stress(
        self,
    ) -> None:
        artifact_root = self.write_candidate_gate()
        path = self.directory / "performance" / "windows-100.json"
        report = evidence.json.loads(path.read_text(encoding="utf-8"))
        report["runs"]["cache_stress"]["peak_resident_bytes"] = 1024 * 1024 * 1024
        path.write_text(
            evidence.json.dumps(report, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )

        records = evidence.validate_candidate_gate(
            self.directory, artifact_root, self.run_metadata()
        )

        self.assertEqual(len(records), 3)

    def test_candidate_gate_rejects_reused_session_runs_and_display_identity(
        self,
    ) -> None:
        artifact_root = self.write_candidate_gate()
        source = self.directory / "performance" / "windows-100.json"
        destination = self.directory / "performance" / "windows-150.json"
        copied = evidence.json.loads(source.read_text(encoding="utf-8"))
        copied["session_label"] = "windows-150"
        copied["session_evidence"] = {"display_scale_percent": 150}
        destination.write_text(
            evidence.json.dumps(copied, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        with self.assertRaisesRegex(evidence.EvidenceError, "must not reuse copied"):
            evidence.validate_candidate_gate(
                self.directory, artifact_root, self.run_metadata()
            )

        artifact_root = self.write_candidate_gate()
        linux = self.directory / "performance" / "linux-wayland.json"
        windows = self.directory / "performance" / "windows-100.json"
        linux_report = evidence.json.loads(linux.read_text(encoding="utf-8"))
        windows_report = evidence.json.loads(windows.read_text(encoding="utf-8"))
        windows_report["runs"] = linux_report["runs"]
        windows_report["summary"] = linux_report["summary"]
        windows.write_text(
            evidence.json.dumps(windows_report, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        with self.assertRaisesRegex(evidence.EvidenceError, "must not reuse copied"):
            evidence.validate_candidate_gate(
                self.directory, artifact_root, self.run_metadata()
            )

        artifact_root = self.write_candidate_gate()
        retina = self.directory / "performance" / "macos-retina.json"
        external = self.directory / "performance" / "macos-external.json"
        retina_report = evidence.json.loads(retina.read_text(encoding="utf-8"))
        external_report = evidence.json.loads(external.read_text(encoding="utf-8"))
        external_report["session_evidence"]["display_identity_sha256"] = retina_report[
            "session_evidence"
        ]["display_identity_sha256"]
        external.write_text(
            evidence.json.dumps(external_report, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        with self.assertRaisesRegex(evidence.EvidenceError, "distinct displays"):
            evidence.validate_candidate_gate(
                self.directory, artifact_root, self.run_metadata()
            )

    def test_candidate_gate_accepts_strongly_identified_softpipe_adapter(self) -> None:
        artifact_root = self.write_candidate_gate()
        path = self.directory / "performance" / "linux-mesa-software.json"
        report = evidence.json.loads(path.read_text(encoding="utf-8"))
        report["session_evidence"]["opengl_renderer"] = "softpipe"
        for one in [
            *report["runs"]["small"],
            *report["runs"]["large"],
            report["runs"]["cache_stress"],
        ]:
            one.update(
                adapter_name="softpipe",
                adapter_device_type="other",
                adapter_driver="",
            )
        path.write_text(
            evidence.json.dumps(report, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )

        records = evidence.validate_candidate_gate(
            self.directory, artifact_root, self.run_metadata()
        )
        self.assertEqual([record.platform for record in records], list(TARGETS))

    def test_candidate_gate_requires_exact_reports_and_matching_rollup(self) -> None:
        artifact_root = self.write_candidate_gate()
        (self.directory / "performance" / "windows-100.json").unlink()
        with self.assertRaisesRegex(evidence.EvidenceError, "report set mismatch"):
            evidence.validate_candidate_gate(
                self.directory, artifact_root, self.run_metadata()
            )

        artifact_root = self.write_candidate_gate()
        record = self.directory / "windows.md"
        record.write_text(
            record.read_text(encoding="utf-8").replace(
                "window=117.82 ms", "window=118.82 ms"
            ),
            encoding="utf-8",
        )
        with self.assertRaisesRegex(
            evidence.EvidenceError, "rollup does not match session reports"
        ):
            evidence.validate_candidate_gate(
                self.directory, artifact_root, self.run_metadata()
            )

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
        changed_digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        archive.with_suffix(".zip.sha256").write_bytes(
            f"{changed_digest}  {archive.name}\n".encode("ascii")
        )
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
        sidecar_path = archive.with_suffix(".zip.sha256")
        sidecar_path.write_bytes(sidecar_path.read_bytes().replace(b"\n", b"\r\n"))
        with self.assertRaisesRegex(evidence.EvidenceError, "sidecar does not match"):
            evidence.validate_candidate_gate(
                self.directory, artifact_root, self.run_metadata()
            )

        artifact_root = self.write_candidate_gate()
        archive.with_suffix(".zip.sha256").unlink()
        with self.assertRaisesRegex(evidence.EvidenceError, "artifact set mismatch"):
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

        artifact_root = self.write_candidate_gate()
        target_directory = artifact_root / f"viewr-{TARGETS['windows']}"
        (target_directory / "unexpected.txt").write_text("extra", encoding="utf-8")
        with self.assertRaisesRegex(evidence.EvidenceError, "artifact set mismatch"):
            evidence.validate_candidate_gate(
                self.directory, artifact_root, self.run_metadata()
            )
        (target_directory / "unexpected.txt").unlink()

        artifact_root = self.write_candidate_gate()
        intel_target = "x86_64-apple-darwin"
        intel_sidecar = (
            artifact_root
            / f"viewr-{intel_target}"
            / f"viewr-0.6.0-{intel_target}.zip.sha256"
        )
        intel_sidecar.write_text(
            f"{'0' * 64}  viewr-0.6.0-{intel_target}.zip\n", encoding="ascii"
        )
        with self.assertRaisesRegex(evidence.EvidenceError, "sidecar does not match"):
            evidence.validate_candidate_gate(
                self.directory, artifact_root, self.run_metadata()
            )

        artifact_root = self.write_candidate_gate()
        intel_sidecar = (
            artifact_root
            / f"viewr-{intel_target}"
            / f"viewr-0.6.0-{intel_target}.zip.sha256"
        )
        intel_sidecar.unlink()
        with self.assertRaisesRegex(evidence.EvidenceError, "artifact set mismatch"):
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

        self.write_candidate_gate()
        output = io.StringIO()
        with (
            mock.patch.object(
                evidence,
                "validate_remote_candidate_gate",
                return_value=evidence.validate_gate(self.directory),
            ) as validate_remote,
            contextlib.redirect_stdout(output),
        ):
            self.assertEqual(evidence.main(["gate", str(self.directory)]), 0)
        validate_remote.assert_called_once_with(self.directory)
        self.assertIn("evidence passed: windows, macos, linux", output.getvalue())

    def test_remote_artifact_download_uses_recorded_run_and_empty_directory(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary)
            metadata = mock.Mock(
                returncode=0,
                stdout=evidence.json.dumps(self.artifact_metadata()),
                stderr="",
            )
            completed = mock.Mock(returncode=0, stdout="", stderr="")
            with mock.patch.object(
                evidence.subprocess, "run", side_effect=(metadata, completed)
            ) as execute:
                evidence._download_run_artifacts(123456, destination)
            self.assertEqual(
                [call.args[0] for call in execute.call_args_list],
                [
                    [
                        "gh",
                        "api",
                        "--method",
                        "GET",
                        "repos/blisspixel/viewr/actions/runs/123456/artifacts?per_page=100",
                    ],
                    [
                        "gh",
                        "run",
                        "download",
                        "123456",
                        "--repo",
                        "blisspixel/viewr",
                        "--dir",
                        str(destination.resolve()),
                    ],
                ],
            )

            (destination / "occupied").write_text("x", encoding="utf-8")
            with self.assertRaisesRegex(evidence.EvidenceError, "must be empty"):
                evidence._download_run_artifacts(123456, destination)

    def test_remote_artifact_metadata_rejects_names_expiry_and_size_budgets(
        self,
    ) -> None:
        valid = self.artifact_metadata()
        cases: list[tuple[dict[str, object], str]] = []

        unexpected = evidence.json.loads(evidence.json.dumps(valid))
        unexpected["artifacts"][0]["name"] = "unexpected"
        cases.append((unexpected, "unexpected candidate workflow artifact"))

        duplicate = evidence.json.loads(evidence.json.dumps(valid))
        duplicate["artifacts"][1]["name"] = duplicate["artifacts"][0]["name"]
        cases.append((duplicate, "names must be unique"))

        expired = evidence.json.loads(evidence.json.dumps(valid))
        expired["artifacts"][0]["expired"] = True
        cases.append((expired, "is expired"))

        oversized = evidence.json.loads(evidence.json.dumps(valid))
        application = next(
            artifact
            for artifact in oversized["artifacts"]
            if artifact["name"] != evidence.FIXTURE_ARTIFACT
        )
        application["size_in_bytes"] = evidence.MAX_APPLICATION_ARTIFACT_BYTES + 1
        cases.append((oversized, "exceeds its size limit"))

        oversized_fixture = evidence.json.loads(evidence.json.dumps(valid))
        fixture = next(
            artifact
            for artifact in oversized_fixture["artifacts"]
            if artifact["name"] == evidence.FIXTURE_ARTIFACT
        )
        fixture["size_in_bytes"] = evidence.MAX_FIXTURE_ARTIFACT_BYTES + 1
        cases.append((oversized_fixture, "exceeds its size limit"))

        for payload, message in cases:
            with self.subTest(message=message):
                completed = mock.Mock(
                    returncode=0,
                    stdout=evidence.json.dumps(payload),
                    stderr="",
                )
                with mock.patch.object(
                    evidence.subprocess, "run", return_value=completed
                ) as execute:
                    with self.assertRaisesRegex(evidence.EvidenceError, message):
                        evidence._verify_remote_artifact_metadata(123456)
                execute.assert_called_once()

        with mock.patch.object(evidence, "MAX_ARTIFACT_SET_BYTES", 1):
            completed = mock.Mock(
                returncode=0,
                stdout=evidence.json.dumps(valid),
                stderr="",
            )
            with mock.patch.object(evidence.subprocess, "run", return_value=completed):
                with self.assertRaisesRegex(evidence.EvidenceError, "set exceeds"):
                    evidence._verify_remote_artifact_metadata(123456)

    def test_remote_gate_validates_only_a_fresh_recorded_run_download(self) -> None:
        artifact_root = self.write_candidate_gate()

        def copy_download(run_id: int, destination: Path) -> None:
            self.assertEqual(run_id, 123456)
            shutil.copytree(artifact_root, destination, dirs_exist_ok=True)

        with (
            mock.patch.object(
                evidence, "_load_run_metadata", return_value=self.run_metadata()
            ) as load_metadata,
            mock.patch.object(
                evidence, "_download_run_artifacts", side_effect=copy_download
            ) as download,
        ):
            records = evidence.validate_remote_candidate_gate(self.directory)
        self.assertEqual([record.platform for record in records], list(TARGETS))
        load_metadata.assert_called_once_with(123456)
        download.assert_called_once()


if __name__ == "__main__":
    unittest.main()
