"""Contract tests for the dependency-free GUI performance gate."""

from __future__ import annotations

from contextlib import redirect_stderr, redirect_stdout
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock
import zlib

from scripts.performance_gate import (
    _command,
    _idle_diagnostics,
    _median_report,
    Budgets,
    PerformanceGateError,
    ProbeReport,
    create_linked_corpus,
    deterministic_png,
    evaluate,
    evaluate_cache_stress,
    main,
    parse_report,
    run_probe,
)


def report(**overrides: int | bool) -> ProbeReport:
    values = {
        "window_ready_us": 100_000,
        "first_pixel_us": 200_000,
        "max_navigation_us": 10_000,
        "idle_redraws": 0,
        "idle_non_redraw_events": 0,
        "idle_event_repaint_requests": 0,
        "idle_scheduled_egui_repaints": 0,
        "idle_window_focused": False,
        "idle_pointer_inside": False,
        "peak_resident_bytes": 200 * 1024 * 1024,
        "playlist_entries": 16,
        "decoded_cache_entries": 5,
        "decoded_cache_bytes": 256 * 1024 * 1024,
        "thumbnail_texture_entries": 9,
    }
    values.update(overrides)
    return ProbeReport(**values)


class PerformanceGateTests(unittest.TestCase):
    def test_png_is_valid_and_has_exact_dimensions(self) -> None:
        encoded = deterministic_png(7, 5)
        self.assertTrue(encoded.startswith(b"\x89PNG\r\n\x1a\n"))
        self.assertEqual(int.from_bytes(encoded[16:20], "big"), 7)
        self.assertEqual(int.from_bytes(encoded[20:24], "big"), 5)
        idat = encoded.index(b"IDAT")
        size = int.from_bytes(encoded[idat - 4 : idat], "big")
        raw = zlib.decompress(encoded[idat + 4 : idat + 4 + size])
        self.assertEqual(len(raw), 5 * (1 + 7 * 3))
        with self.assertRaises(ValueError):
            deterministic_png(0, 5)

    def test_linked_corpus_has_deterministic_exact_size(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "source.png"
            source.write_bytes(deterministic_png(1, 1))
            first = create_linked_corpus(root / "corpus", source, 7)
            self.assertEqual(first.name, "image-00000.png")
            self.assertEqual(len(list((root / "corpus").iterdir())), 7)
            with self.assertRaises(ValueError):
                create_linked_corpus(root / "empty", source, 0)

    def test_linked_corpus_bounds_links_per_source_file(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "source.png"
            source.write_bytes(b"source")
            with mock.patch(
                "scripts.performance_gate.MAX_LINKED_TARGETS_PER_SOURCE", 2
            ):
                create_linked_corpus(root / "corpus", source, 5)
            self.assertEqual(len(list((root / "corpus").iterdir())), 5)
            self.assertTrue((root / ".corpus-source-00001.bin").is_file())
            self.assertTrue((root / ".corpus-source-00002.bin").is_file())

    def test_linked_corpus_copies_when_hard_links_are_unavailable(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "source.png"
            source.write_bytes(b"source")
            with mock.patch("scripts.performance_gate.os.link", side_effect=OSError):
                first = create_linked_corpus(root / "corpus", source, 2)
            self.assertEqual(first.read_bytes(), b"source")

    def test_report_parser_requires_exact_integer_and_boolean_schema(self) -> None:
        payload = report().__dict__
        self.assertEqual(parse_report(json.dumps(payload)), report())
        with self.assertRaises(PerformanceGateError):
            parse_report(json.dumps({**payload, "extra": 1}))
        with self.assertRaises(PerformanceGateError):
            parse_report(json.dumps({**payload, "first_pixel_us": -1}))
        with self.assertRaises(PerformanceGateError):
            parse_report(json.dumps({**payload, "first_pixel_us": True}))
        with self.assertRaises(PerformanceGateError):
            parse_report(json.dumps({**payload, "idle_window_focused": 1}))
        self.assertEqual(parse_report("{not json}\n" + json.dumps(payload)), report())
        with self.assertRaises(PerformanceGateError):
            parse_report("not json")

    def test_probe_command_requires_xvfb_when_requested(self) -> None:
        binary = Path("viewr")
        image = Path("image.png")
        self.assertEqual(
            _command(binary, image, False),
            ["viewr", "performance-probe", "image.png"],
        )
        with mock.patch(
            "scripts.performance_gate.shutil.which", return_value="xvfb-run"
        ):
            self.assertEqual(
                _command(binary, image, True),
                ["xvfb-run", "-a", "viewr", "performance-probe", "image.png"],
            )
        with mock.patch("scripts.performance_gate.shutil.which", return_value=None):
            with self.assertRaisesRegex(PerformanceGateError, "xvfb-run is required"):
                _command(binary, image, True)

    def test_run_probe_validates_process_result_and_scrubs_log_settings(self) -> None:
        payload = json.dumps(report().__dict__)
        completed = subprocess.CompletedProcess([], 0, payload, "")
        with mock.patch.dict(os.environ, {"RUST_LOG": "debug", "VIEWR_LOG": "trace"}):
            with mock.patch(
                "scripts.performance_gate.subprocess.run", return_value=completed
            ) as execute:
                self.assertEqual(
                    run_probe(Path("viewr"), Path("image.png"), False), report()
                )
        environment = execute.call_args.kwargs["env"]
        self.assertNotIn("RUST_LOG", environment)
        self.assertNotIn("VIEWR_LOG", environment)
        self.assertEqual(execute.call_args.kwargs["timeout"], 90)

        failed = subprocess.CompletedProcess([], 2, "", "probe failed")
        with mock.patch("scripts.performance_gate.subprocess.run", return_value=failed):
            with self.assertRaisesRegex(PerformanceGateError, "probe failed"):
                run_probe(Path("viewr"), Path("image.png"), False)
        with mock.patch(
            "scripts.performance_gate.subprocess.run",
            side_effect=subprocess.TimeoutExpired("viewr", 90),
        ):
            with self.assertRaisesRegex(PerformanceGateError, "could not execute"):
                run_probe(Path("viewr"), Path("image.png"), False)

    def test_report_aggregation_uses_timing_medians_and_resource_maxima(self) -> None:
        reports = [
            report(
                window_ready_us=300,
                first_pixel_us=900,
                idle_non_redraw_events=1,
                idle_window_focused=False,
                peak_resident_bytes=1,
                decoded_cache_bytes=1,
            ),
            report(
                window_ready_us=100,
                first_pixel_us=500,
                idle_non_redraw_events=3,
                idle_pointer_inside=True,
                peak_resident_bytes=3,
                decoded_cache_bytes=3,
            ),
            report(
                window_ready_us=200,
                first_pixel_us=700,
                peak_resident_bytes=2,
                decoded_cache_bytes=2,
            ),
        ]
        combined = _median_report(reports)
        self.assertEqual(combined.window_ready_us, 200)
        self.assertEqual(combined.first_pixel_us, 700)
        self.assertEqual(combined.peak_resident_bytes, 3)
        self.assertEqual(combined.decoded_cache_bytes, 3)
        self.assertEqual(combined.idle_non_redraw_events, 3)
        self.assertFalse(combined.idle_window_focused)
        self.assertTrue(combined.idle_pointer_inside)

    def test_idle_diagnostics_preserve_run_order_and_fixed_fields(self) -> None:
        rendered = _idle_diagnostics(
            [
                report(idle_redraws=1, idle_window_focused=True),
                report(
                    idle_redraws=3,
                    idle_non_redraw_events=2,
                    idle_event_repaint_requests=1,
                    idle_scheduled_egui_repaints=4,
                    idle_pointer_inside=True,
                ),
            ],
            [report(idle_redraws=2)],
            report(idle_redraws=1),
        )
        payload = json.loads(rendered)
        self.assertEqual(
            [item["delivered_redraws"] for item in payload["small"]], [1, 3]
        )
        self.assertEqual(payload["small"][1]["non_redraw_events"], 2)
        self.assertEqual(payload["small"][1]["event_repaint_requests"], 1)
        self.assertEqual(payload["small"][1]["scheduled_egui_repaints"], 4)
        self.assertTrue(payload["small"][0]["window_focused"])
        self.assertTrue(payload["small"][1]["pointer_inside"])
        self.assertNotIn("path", rendered.casefold())

    def test_evaluate_reports_every_timing_memory_and_cache_violation(self) -> None:
        budgets = Budgets(3000, 5000, 500, 2, 768, 96)
        small = report(
            window_ready_us=3_000_001,
            first_pixel_us=5_000_001,
            max_navigation_us=500_001,
            idle_redraws=3,
            decoded_cache_entries=6,
            decoded_cache_bytes=256 * 1024 * 1024 + 1,
            thumbnail_texture_entries=10,
        )
        large = report(
            peak_resident_bytes=800 * 1024 * 1024,
            playlist_entries=4000,
            idle_redraws=3,
            decoded_cache_entries=6,
            decoded_cache_bytes=256 * 1024 * 1024 + 1,
            thumbnail_texture_entries=10,
        )
        failures = evaluate(small, large, budgets, 16, 50_000)
        self.assertEqual(len(failures), 14)
        self.assertTrue(any("window ready" in failure for failure in failures))
        self.assertTrue(any("first pixel" in failure for failure in failures))
        self.assertTrue(
            any("folder-size resident growth" in failure for failure in failures)
        )
        self.assertTrue(any("large probe scanned" in failure for failure in failures))
        self.assertTrue(any("idle redraws" in failure for failure in failures))

    def test_evaluate_accepts_reports_on_the_exact_limits(self) -> None:
        budgets = Budgets(3000, 5000, 500, 2, 768, 96)
        small = report(
            window_ready_us=3_000_000,
            first_pixel_us=5_000_000,
            max_navigation_us=500_000,
            idle_redraws=2,
        )
        large = report(
            peak_resident_bytes=(200 + 96) * 1024 * 1024,
            playlist_entries=50_000,
        )
        self.assertEqual(evaluate(small, large, budgets, 16, 50_000), [])

    def test_cache_stress_gate_exercises_byte_eviction_and_accounting(self) -> None:
        image_bytes = 4096 * 4096 * 4
        accepted = report(
            playlist_entries=8,
            decoded_cache_entries=4,
            decoded_cache_bytes=4 * image_bytes,
        )
        self.assertEqual(evaluate_cache_stress(accepted, 8, image_bytes), [])

        broken_eviction = report(
            playlist_entries=8,
            decoded_cache_entries=5,
            decoded_cache_bytes=5 * image_bytes,
        )
        failures = evaluate_cache_stress(broken_eviction, 8, image_bytes)
        self.assertTrue(any("expected 4" in failure for failure in failures))
        self.assertTrue(any("limit is 268435456" in failure for failure in failures))

        under_retained = report(
            playlist_entries=8,
            decoded_cache_entries=1,
            decoded_cache_bytes=image_bytes,
        )
        self.assertTrue(
            any(
                "under-retention" in failure
                for failure in evaluate_cache_stress(under_retained, 8, image_bytes)
            )
        )

        broken_accounting = report(
            playlist_entries=8,
            decoded_cache_entries=4,
            decoded_cache_bytes=0,
        )
        self.assertTrue(
            any(
                "accounting" in failure
                for failure in evaluate_cache_stress(broken_accounting, 8, image_bytes)
            )
        )
        with self.assertRaisesRegex(PerformanceGateError, "too small"):
            evaluate_cache_stress(accepted, 8, 1)

    def test_growth_uses_the_lowest_observed_small_folder_baseline(self) -> None:
        budgets = Budgets(3000, 5000, 500, 2, 768, 96)
        small = report(peak_resident_bytes=300 * 1024 * 1024)
        large = report(
            peak_resident_bytes=350 * 1024 * 1024,
            playlist_entries=50_000,
        )
        self.assertEqual(evaluate(small, large, budgets, 16, 50_000), [])
        failures = evaluate(
            small,
            large,
            budgets,
            16,
            50_000,
            small_rss_floor_bytes=200 * 1024 * 1024,
        )
        self.assertTrue(any("folder-size resident growth" in item for item in failures))

    def test_main_validates_arguments_and_runs_the_complete_gate(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            binary = Path(temp) / "viewr"
            binary.write_bytes(b"binary")
            base = [
                "--binary",
                str(binary),
                "--no-xvfb",
                "--runs",
                "1",
                "--small-count",
                "5",
                "--large-count",
                "6",
            ]
            probe_results = [
                report(playlist_entries=5),
                report(playlist_entries=6),
                report(
                    playlist_entries=8,
                    decoded_cache_entries=4,
                    decoded_cache_bytes=4 * 4096 * 4096 * 4,
                ),
            ]
            output = io.StringIO()
            with mock.patch(
                "scripts.performance_gate.run_probe", side_effect=probe_results
            ) as probe:
                with redirect_stdout(output):
                    self.assertEqual(main(base), 0)
            self.assertEqual(probe.call_count, 3)
            self.assertIn("performance gate: OK", output.getvalue())
            self.assertNotIn("idle diagnostics:", output.getvalue())

            diagnostic_output = io.StringIO()
            with mock.patch(
                "scripts.performance_gate.run_probe", side_effect=probe_results
            ):
                with redirect_stdout(diagnostic_output):
                    self.assertEqual(main([*base, "--idle-diagnostics"]), 0)
            self.assertIn("idle diagnostics:", diagnostic_output.getvalue())

            failing_results = [
                report(first_pixel_us=5_000_001, playlist_entries=5),
                report(playlist_entries=6),
                report(
                    playlist_entries=8,
                    decoded_cache_entries=4,
                    decoded_cache_bytes=4 * 4096 * 4096 * 4,
                ),
            ]
            errors = io.StringIO()
            with mock.patch(
                "scripts.performance_gate.run_probe", side_effect=failing_results
            ):
                with redirect_stdout(io.StringIO()), redirect_stderr(errors):
                    self.assertEqual(main(base), 1)
            self.assertIn("first pixel", errors.getvalue())
            self.assertIn("idle diagnostics:", errors.getvalue())

    def test_main_rejects_missing_binary_even_runs_and_invalid_counts(self) -> None:
        with self.assertRaisesRegex(PerformanceGateError, "does not exist"):
            main(["--binary", "missing-viewr", "--no-xvfb"])
        with tempfile.TemporaryDirectory() as temp:
            binary = Path(temp) / "viewr"
            binary.write_bytes(b"binary")
            with self.assertRaisesRegex(PerformanceGateError, "positive odd"):
                main(["--binary", str(binary), "--runs", "2", "--no-xvfb"])
            with self.assertRaisesRegex(PerformanceGateError, "no greater than"):
                main(["--binary", str(binary), "--runs", "11", "--no-xvfb"])
            with self.assertRaisesRegex(PerformanceGateError, "5 <= small < large"):
                main(
                    [
                        "--binary",
                        str(binary),
                        "--small-count",
                        "4",
                        "--no-xvfb",
                    ]
                )
            with self.assertRaisesRegex(PerformanceGateError, "must not exceed"):
                main(
                    [
                        "--binary",
                        str(binary),
                        "--large-count",
                        "100001",
                        "--no-xvfb",
                    ]
                )

    def test_main_rejects_nonfinite_and_negative_budgets(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            binary = Path(temp) / "viewr"
            binary.write_bytes(b"binary")
            for option, value in (
                ("--window-ready-ms", "nan"),
                ("--first-pixel-ms", "inf"),
                ("--navigation-ms", "0"),
                ("--peak-resident-mib", "-1"),
                ("--folder-growth-mib", "-inf"),
                ("--idle-redraws", "-1"),
            ):
                with self.subTest(option=option, value=value):
                    with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                        main(
                            [
                                "--binary",
                                str(binary),
                                option,
                                value,
                                "--no-xvfb",
                            ]
                        )


if __name__ == "__main__":
    unittest.main()
