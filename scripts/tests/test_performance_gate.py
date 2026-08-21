"""Contract tests for the dependency-free GUI performance gate."""

from __future__ import annotations

from contextlib import redirect_stderr, redirect_stdout
import io
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest
from unittest import mock
import zlib

from scripts.performance_gate import (
    _command,
    _copy_executables,
    _evidence_report,
    _executable_paths,
    _idle_diagnostics,
    _linux_graphics_evidence,
    _macos_display_evidence,
    _median_report,
    _require_one_adapter,
    _session_evidence,
    _trusted_executable,
    _trusted_path_tool,
    _windows_scale_percent,
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


def report(**overrides: int | bool | str) -> ProbeReport:
    values = {
        "adapter_backend": "dx12",
        "adapter_name": "Test GPU",
        "adapter_device_type": "discrete-gpu",
        "adapter_driver": "Test Driver",
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
        with self.assertRaises(PerformanceGateError):
            parse_report(json.dumps({**payload, "adapter_backend": "unknown"}))
        with self.assertRaises(PerformanceGateError):
            parse_report(json.dumps({**payload, "adapter_name": ""}))
        with self.assertRaises(PerformanceGateError):
            parse_report(json.dumps({**payload, "adapter_driver": "bad\nvalue"}))
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
        trusted_xvfb = Path.cwd() / "trusted" / "xvfb-run"
        with mock.patch(
            "scripts.performance_gate._trusted_path_tool", return_value=trusted_xvfb
        ) as locate:
            self.assertEqual(
                _command(binary, image, True),
                [
                    str(trusted_xvfb),
                    "-a",
                    "viewr",
                    "performance-probe",
                    "image.png",
                ],
            )
        locate.assert_called_once_with("xvfb-run", "the Linux GUI probe")
        with mock.patch(
            "scripts.performance_gate._trusted_path_tool",
            side_effect=PerformanceGateError("xvfb-run is required"),
        ):
            with self.assertRaisesRegex(PerformanceGateError, "xvfb-run is required"):
                _command(binary, image, True)

    def test_run_probe_validates_process_result_and_scrubs_log_settings(self) -> None:
        payload = json.dumps(report().__dict__)
        completed = subprocess.CompletedProcess([], 0, payload, "")
        with mock.patch.dict(
            os.environ,
            {
                "RUST_LOG": "debug",
                "VIEWR_LOG": "trace",
                "VIEWR_DECODE_BIN": "untrusted-worker",
            },
        ):
            with mock.patch(
                "scripts.performance_gate.subprocess.run", return_value=completed
            ) as execute:
                self.assertEqual(
                    run_probe(Path("viewr"), Path("image.png"), False), report()
                )
        environment = execute.call_args.kwargs["env"]
        self.assertNotIn("RUST_LOG", environment)
        self.assertNotIn("VIEWR_LOG", environment)
        self.assertNotIn("VIEWR_DECODE_BIN", environment)
        self.assertEqual(execute.call_args.kwargs["timeout"], 90)

        failed = subprocess.CompletedProcess([], 2, "", "probe failed")
        with mock.patch("scripts.performance_gate.subprocess.run", return_value=failed):
            with self.assertRaisesRegex(PerformanceGateError, "probe failed"):
                run_probe(Path("viewr"), Path("image.png"), False)

    def test_executable_binding_requires_the_colocated_decoder_worker(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            binary = Path(temp) / "viewr"
            binary.write_bytes(b"main")
            with self.assertRaisesRegex(PerformanceGateError, "decoder worker"):
                _executable_paths(binary)
            worker = binary.with_name("viewr-decode")
            worker.write_bytes(b"worker")
            self.assertEqual(
                _executable_paths(binary),
                {"viewr": binary, "viewr-decode": worker},
            )

    def test_executable_pair_is_copied_with_permissions_and_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "source"
            source.mkdir()
            binary = source / "viewr"
            worker = source / "viewr-decode"
            binary.write_bytes(b"main")
            worker.write_bytes(b"worker")
            if os.name != "nt":
                binary.chmod(0o751)
                worker.chmod(0o711)
            timestamp_ns = 1_700_000_000_000_000_000
            os.utime(binary, ns=(timestamp_ns, timestamp_ns))
            os.utime(worker, ns=(timestamp_ns, timestamp_ns))

            copies = _copy_executables(_executable_paths(binary), root / "executables")

            self.assertEqual(copies["viewr"].read_bytes(), b"main")
            self.assertEqual(copies["viewr-decode"].read_bytes(), b"worker")
            self.assertEqual(copies["viewr"].parent, copies["viewr-decode"].parent)
            self.assertNotEqual(copies["viewr"].parent, binary.parent)
            self.assertEqual(copies["viewr"].stat().st_mtime_ns, timestamp_ns)
            self.assertEqual(copies["viewr-decode"].stat().st_mtime_ns, timestamp_ns)
            if os.name != "nt":
                self.assertEqual(stat.S_IMODE(copies["viewr"].stat().st_mode), 0o751)
                self.assertEqual(
                    stat.S_IMODE(copies["viewr-decode"].stat().st_mode), 0o711
                )
                self.assertEqual(
                    stat.S_IMODE(copies["viewr"].parent.stat().st_mode), 0o700
                )

    def test_trusted_external_tool_requires_a_safe_absolute_path(self) -> None:
        resolved = Path.cwd() / "trusted" / "bin" / "glxinfo"
        with (
            mock.patch(
                "scripts.performance_gate.shutil.which", return_value="tools/glxinfo"
            ) as locate,
            mock.patch(
                "scripts.performance_gate._trusted_executable", return_value=resolved
            ) as validate,
        ):
            self.assertEqual(
                _trusted_path_tool("glxinfo", "Linux renderer evidence"), resolved
            )
        locate.assert_called_once_with("glxinfo")
        validate.assert_called_once_with(Path("tools/glxinfo"), "glxinfo")
        with mock.patch("scripts.performance_gate.shutil.which", return_value=None):
            with self.assertRaisesRegex(PerformanceGateError, "glxinfo is required"):
                _trusted_path_tool("glxinfo", "Linux renderer evidence")

        safe_metadata = os.stat_result(
            (stat.S_IFREG | 0o755, 0, 0, 0, 0, 0, 0, 0, 0, 0)
        )
        with (
            mock.patch("pathlib.Path.lstat", return_value=safe_metadata),
            mock.patch("scripts.performance_gate.os.access", return_value=True),
        ):
            self.assertTrue(
                _trusted_executable(Path("safe-tool"), "tool").is_absolute()
            )

        link_metadata = os.stat_result(
            (stat.S_IFLNK | 0o777, 0, 0, 0, 0, 0, 0, 0, 0, 0)
        )
        with mock.patch("pathlib.Path.lstat", return_value=link_metadata):
            with self.assertRaisesRegex(PerformanceGateError, "link or reparse"):
                _trusted_executable(Path("unsafe-tool"), "tool")

        writable_metadata = os.stat_result(
            (stat.S_IFREG | 0o775, 0, 0, 0, 0, 0, 0, 0, 0, 0)
        )
        with mock.patch("pathlib.Path.lstat", return_value=writable_metadata):
            with self.assertRaisesRegex(PerformanceGateError, "group- or world"):
                _trusted_executable(Path("unsafe-tool"), "tool")

        with mock.patch(
            "pathlib.Path.lstat", side_effect=[safe_metadata, writable_metadata]
        ):
            with self.assertRaisesRegex(PerformanceGateError, "group- or world"):
                _trusted_executable(Path("unsafe-parent/tool"), "tool")

    def test_session_evidence_measures_linux_renderer_and_macos_main_display(
        self,
    ) -> None:
        glxinfo = subprocess.CompletedProcess(
            [],
            0,
            "OpenGL vendor string: Mesa/X.org\n"
            "OpenGL renderer string: llvmpipe (LLVM 20.1.8, 256 bits)\n",
            "",
        )
        with (
            mock.patch.dict(
                os.environ,
                {
                    "XDG_SESSION_TYPE": "wayland",
                    "WAYLAND_DISPLAY": "wayland-0",
                    "DISPLAY": ":0",
                },
                clear=True,
            ),
            mock.patch(
                "scripts.performance_gate.subprocess.run", return_value=glxinfo
            ) as execute,
            mock.patch(
                "scripts.performance_gate._trusted_path_tool",
                return_value=Path("/usr/bin/glxinfo"),
            ),
        ):
            linux = _linux_graphics_evidence(require_opengl=True)
        self.assertEqual(linux["linux_session"], "wayland")
        self.assertEqual(linux["opengl_renderer"], "llvmpipe (LLVM 20.1.8, 256 bits)")
        self.assertTrue(linux["opengl_mesa"])
        self.assertTrue(linux["opengl_software"])
        self.assertEqual(
            execute.call_args.args[0], [str(Path("/usr/bin/glxinfo")), "-B"]
        )

        with (
            mock.patch.dict(
                os.environ,
                {
                    "XDG_SESSION_TYPE": "wayland",
                    "WAYLAND_DISPLAY": "wayland-0",
                },
                clear=True,
            ),
            mock.patch("scripts.performance_gate._trusted_path_tool") as locate,
        ):
            self.assertEqual(_linux_graphics_evidence(), {"linux_session": "wayland"})
        locate.assert_not_called()
        with mock.patch.dict(
            os.environ,
            {
                "XDG_SESSION_TYPE": "wayland",
                "WAYLAND_DISPLAY": "wayland-0",
            },
            clear=True,
        ):
            with self.assertRaisesRegex(PerformanceGateError, "requires DISPLAY"):
                _linux_graphics_evidence(require_opengl=True)

        profiler_payload = {
            "SPDisplaysDataType": [
                {
                    "spdisplays_ndrvs": [
                        {
                            "_name": "Built-in Display",
                            "spdisplays_main": "spdisplays_yes",
                            "spdisplays_display_type": "spdisplays_built-in_retinaLCD",
                            "spdisplays_pixelresolution": (
                                "spdisplays_3456x2234Retina"
                            ),
                            "spdisplays_resolution": "1728 x 1117 Retina",
                            "spdisplays_vendor-id": "610",
                        }
                    ]
                }
            ]
        }
        profiler = subprocess.CompletedProcess([], 0, json.dumps(profiler_payload), "")
        with (
            mock.patch(
                "scripts.performance_gate.subprocess.run", return_value=profiler
            ) as execute,
            mock.patch(
                "scripts.performance_gate._trusted_executable",
                return_value=Path("/usr/sbin/system_profiler"),
            ) as validate,
        ):
            macos = _macos_display_evidence()
        self.assertTrue(macos["display_builtin"])
        self.assertTrue(macos["display_retina"])
        self.assertEqual(macos["display_scale_percent"], 200)
        self.assertRegex(str(macos["display_identity_sha256"]), r"^[0-9a-f]{64}$")
        validate.assert_called_once_with(
            Path("/usr/sbin/system_profiler"), "system_profiler"
        )
        self.assertEqual(
            execute.call_args.args[0],
            [
                str(Path("/usr/sbin/system_profiler")),
                "SPDisplaysDataType",
                "-json",
            ],
        )

        with mock.patch(
            "scripts.performance_gate._windows_scale_percent", return_value=150
        ):
            self.assertEqual(
                _session_evidence("Windows"), {"display_scale_percent": 150}
            )

        with mock.patch(
            "scripts.performance_gate.ctypes.windll.shcore.GetScaleFactorForDevice",
            return_value=150,
        ) as get_scale:
            self.assertEqual(_windows_scale_percent(), 150)
        get_scale.assert_called_once_with(0)
        with mock.patch(
            "scripts.performance_gate.ctypes.windll.shcore.GetScaleFactorForDevice",
            return_value=0,
        ):
            with self.assertRaisesRegex(PerformanceGateError, "could not measure"):
                _windows_scale_percent()
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
        self.assertEqual(combined.adapter_name, "Test GPU")

        with self.assertRaisesRegex(
            PerformanceGateError, "selected different GPU adapters"
        ):
            _median_report([report(), report(adapter_name="Different GPU")])
        self.assertEqual(
            _require_one_adapter([report(), report()]),
            ("dx12", "Test GPU", "discrete-gpu", "Test Driver"),
        )

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

    def test_evidence_report_is_path_free_numeric_and_binary_bound(self) -> None:
        with (
            tempfile.TemporaryDirectory() as temp,
            mock.patch.dict(
                os.environ,
                {
                    "WGPU_BACKEND": "GL",
                    "LIBGL_ALWAYS_SOFTWARE": "private/path",
                },
            ),
        ):
            binary = Path(temp) / "private-folder" / "viewr"
            binary.parent.mkdir()
            binary.write_bytes(b"candidate binary")
            small_reports = [report(), report(window_ready_us=120_000)]
            large_reports = [report(playlist_entries=50_000)]
            cache_stress = report(
                playlist_entries=8,
                decoded_cache_entries=4,
                decoded_cache_bytes=4 * 4096 * 4096 * 4,
            )
            rendered = _evidence_report(
                {
                    "viewr": "f" * 64,
                    "viewr-decode": "e" * 64,
                },
                "windows-200",
                "Windows",
                {"display_scale_percent": 200},
                Budgets(3000, 5000, 500, 2, 768, 96),
                small_reports,
                large_reports,
                cache_stress,
                _median_report(small_reports),
                _median_report(large_reports),
                190 * 1024 * 1024,
                [],
            )

        self.assertEqual(rendered["schema"], 3)
        self.assertEqual(rendered["status"], "pass")
        self.assertEqual(rendered["session_label"], "windows-200")
        self.assertEqual(rendered["host_platform"], "Windows")
        self.assertEqual(
            rendered["executable_sha256"],
            {"viewr": "f" * 64, "viewr-decode": "e" * 64},
        )
        self.assertEqual(rendered["session_evidence"], {"display_scale_percent": 200})
        self.assertEqual(rendered["summary"]["large_folder_images"], 50_000)
        self.assertEqual(rendered["summary"]["cache_stress_mib"], 256.0)
        self.assertEqual(rendered["summary"]["small_rss_floor_mib"], 190.0)
        self.assertEqual(rendered["summary"]["folder_growth_mib"], 10.0)
        self.assertEqual(
            rendered["renderer_controls"],
            {"wgpu_backend": "gl", "libgl_always_software": ""},
        )
        self.assertNotIn("private-folder", json.dumps(rendered))
        self.assertNotIn("private/path", json.dumps(rendered))

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
            binary.with_name("viewr-decode").write_bytes(b"worker")
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
            with (
                mock.patch(
                    "scripts.performance_gate.run_probe", side_effect=probe_results
                ),
                mock.patch(
                    "scripts.performance_gate._session_evidence",
                    return_value={"display_scale_percent": 100},
                ),
            ):
                with redirect_stdout(diagnostic_output):
                    self.assertEqual(main([*base, "--idle-diagnostics"]), 0)
            self.assertIn("idle diagnostics:", diagnostic_output.getvalue())

            report_file = Path(temp) / "evidence" / "performance.json"
            with (
                mock.patch(
                    "scripts.performance_gate.run_probe", side_effect=probe_results
                ),
                mock.patch(
                    "scripts.performance_gate._session_evidence",
                    return_value={"display_scale_percent": 100},
                ),
            ):
                with redirect_stdout(io.StringIO()):
                    self.assertEqual(
                        main(
                            [
                                *base,
                                "--report-file",
                                str(report_file),
                                "--session-label",
                                "performance",
                            ]
                        ),
                        0,
                    )
            evidence = json.loads(report_file.read_text(encoding="utf-8"))
            self.assertEqual(evidence["status"], "pass")
            self.assertEqual(evidence["summary"]["large_folder_images"], 6)
            self.assertEqual(evidence["failures"], [])
            with mock.patch(
                "scripts.performance_gate.run_probe", side_effect=probe_results
            ) as probe:
                with self.assertRaisesRegex(
                    PerformanceGateError, "report destination already exists"
                ):
                    main([*base, "--report-file", str(report_file)])
            probe.assert_not_called()

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
            with (
                mock.patch(
                    "scripts.performance_gate.run_probe", side_effect=failing_results
                ),
                mock.patch(
                    "scripts.performance_gate._session_evidence",
                    return_value={"display_scale_percent": 100},
                ),
            ):
                failure_report = Path(temp) / "performance-failure.json"
                with redirect_stdout(io.StringIO()), redirect_stderr(errors):
                    self.assertEqual(
                        main(
                            [
                                *base,
                                "--report-file",
                                str(failure_report),
                                "--session-label",
                                "performance-failure",
                            ]
                        ),
                        1,
                    )
            self.assertIn("first pixel", errors.getvalue())
            self.assertIn("idle diagnostics:", errors.getvalue())
            failure_evidence = json.loads(failure_report.read_text(encoding="utf-8"))
            self.assertEqual(failure_evidence["status"], "fail")
            self.assertTrue(
                any(
                    "first pixel" in failure for failure in failure_evidence["failures"]
                )
            )

    def test_main_runs_only_the_private_executable_copies(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            source_directory = Path(temp)
            binary = source_directory / "viewr"
            worker = source_directory / "viewr-decode"
            binary.write_bytes(b"candidate-main")
            worker.write_bytes(b"candidate-worker")
            if os.name != "nt":
                binary.chmod(0o751)
                worker.chmod(0o711)
            results = iter(
                [
                    report(playlist_entries=5),
                    report(playlist_entries=6),
                    report(
                        playlist_entries=8,
                        decoded_cache_entries=4,
                        decoded_cache_bytes=4 * 4096 * 4096 * 4,
                    ),
                ]
            )
            observed: list[Path] = []

            def copied_probe(
                tested_binary: Path, _image: Path, _use_xvfb: bool
            ) -> ProbeReport:
                self.assertNotEqual(tested_binary, binary)
                self.assertNotEqual(tested_binary.parent, binary.parent)
                self.assertEqual(tested_binary.read_bytes(), b"candidate-main")
                self.assertEqual(
                    tested_binary.with_name("viewr-decode").read_bytes(),
                    b"candidate-worker",
                )
                if not observed:
                    binary.write_bytes(b"changed-source-main")
                    worker.write_bytes(b"changed-source-worker")
                observed.append(tested_binary)
                return next(results)

            with (
                mock.patch(
                    "scripts.performance_gate.run_probe", side_effect=copied_probe
                ),
                redirect_stdout(io.StringIO()),
            ):
                self.assertEqual(
                    main(
                        [
                            "--binary",
                            str(binary),
                            "--runs",
                            "1",
                            "--small-count",
                            "5",
                            "--large-count",
                            "6",
                            "--no-xvfb",
                        ]
                    ),
                    0,
                )
            self.assertEqual(len(observed), 3)
            self.assertEqual(len(set(observed)), 1)
            self.assertFalse(observed[0].exists())

    def test_main_rejects_missing_binary_even_runs_and_invalid_counts(self) -> None:
        with self.assertRaisesRegex(PerformanceGateError, "does not exist"):
            main(["--binary", "missing-viewr", "--no-xvfb"])
        with tempfile.TemporaryDirectory() as temp:
            binary = Path(temp) / "viewr"
            binary.write_bytes(b"binary")
            binary.with_name("viewr-decode").write_bytes(b"worker")
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

            report = Path(temp) / "windows-100.json"
            with self.assertRaisesRegex(PerformanceGateError, "requires --report-file"):
                main(
                    [
                        "--binary",
                        str(binary),
                        "--session-label",
                        "windows-100",
                        "--no-xvfb",
                    ]
                )
            with self.assertRaisesRegex(PerformanceGateError, "must match"):
                main(
                    [
                        "--binary",
                        str(binary),
                        "--report-file",
                        str(report),
                        "--session-label",
                        "windows-200",
                        "--no-xvfb",
                    ]
                )
            with self.assertRaisesRegex(PerformanceGateError, "2 to 64 lowercase"):
                main(
                    [
                        "--binary",
                        str(binary),
                        "--report-file",
                        str(report),
                        "--session-label",
                        "Windows Private Path",
                        "--no-xvfb",
                    ]
                )

    def test_main_rejects_a_binary_changed_during_the_run(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            binary = Path(temp) / "viewr"
            binary.write_bytes(b"binary")
            binary.with_name("viewr-decode").write_bytes(b"worker")
            probe_results = [
                report(playlist_entries=5),
                report(playlist_entries=6),
                report(
                    playlist_entries=8,
                    decoded_cache_entries=4,
                    decoded_cache_bytes=4 * 4096 * 4096 * 4,
                ),
            ]
            with (
                mock.patch(
                    "scripts.performance_gate.run_probe", side_effect=probe_results
                ),
                mock.patch(
                    "scripts.performance_gate._executable_digests",
                    side_effect=[
                        {"viewr": "a" * 64, "viewr-decode": "c" * 64},
                        {"viewr": "a" * 64, "viewr-decode": "d" * 64},
                    ],
                ),
                self.assertRaisesRegex(PerformanceGateError, "changed during"),
            ):
                main(
                    [
                        "--binary",
                        str(binary),
                        "--runs",
                        "1",
                        "--small-count",
                        "5",
                        "--large-count",
                        "6",
                        "--no-xvfb",
                    ]
                )

    def test_main_rejects_session_evidence_changed_during_the_run(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            binary = Path(temp) / "viewr"
            binary.write_bytes(b"binary")
            binary.with_name("viewr-decode").write_bytes(b"worker")
            probe_results = [
                report(playlist_entries=5),
                report(playlist_entries=6),
                report(
                    playlist_entries=8,
                    decoded_cache_entries=4,
                    decoded_cache_bytes=4 * 4096 * 4096 * 4,
                ),
            ]
            report_file = Path(temp) / "session-change.json"
            with (
                mock.patch(
                    "scripts.performance_gate.run_probe", side_effect=probe_results
                ),
                mock.patch(
                    "scripts.performance_gate._session_evidence",
                    side_effect=[
                        {"display_scale_percent": 100},
                        {"display_scale_percent": 150},
                    ],
                ),
                self.assertRaisesRegex(PerformanceGateError, "changed during"),
            ):
                main(
                    [
                        "--binary",
                        str(binary),
                        "--runs",
                        "1",
                        "--small-count",
                        "5",
                        "--large-count",
                        "6",
                        "--no-xvfb",
                        "--report-file",
                        str(report_file),
                        "--session-label",
                        "session-change",
                    ]
                )
            self.assertFalse(report_file.exists())

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
