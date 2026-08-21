"""Contract tests for the software OpenGL presentation probe."""

from __future__ import annotations

import io
import json
import subprocess
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock

from scripts.software_gl_probe import (
    SoftwareGlProbeError,
    main,
    presented,
    probe_command,
    run,
)


def report_json(**overrides: int | bool | str) -> str:
    values = {
        "adapter_backend": "gl",
        "adapter_name": "llvmpipe (LLVM 20.1.8)",
        "adapter_device_type": "cpu",
        "adapter_driver": "llvmpipe",
        "window_ready_us": 1000,
        "first_pixel_us": 4321,
        "max_navigation_us": 2000,
        "idle_redraws": 0,
        "idle_non_redraw_events": 0,
        "idle_event_repaint_requests": 0,
        "idle_scheduled_egui_repaints": 0,
        "idle_window_focused": False,
        "idle_pointer_inside": False,
        "peak_resident_bytes": 1024,
        "playlist_entries": 16,
        "decoded_cache_entries": 0,
        "decoded_cache_bytes": 0,
        "thumbnail_texture_entries": 0,
    }
    values.update(overrides)
    return json.dumps(values)


class SoftwareGlProbeTests(unittest.TestCase):
    """The probe must fail loudly when no surface is presented."""

    def test_probe_always_runs_through_a_virtual_display(self) -> None:
        with mock.patch(
            "scripts.software_gl_probe.shutil.which", return_value="/usr/bin/xvfb-run"
        ):
            self.assertEqual(
                probe_command(Path("viewr"), Path("image.png")),
                [
                    "/usr/bin/xvfb-run",
                    "-a",
                    "viewr",
                    "performance-probe",
                    "image.png",
                ],
            )
        with mock.patch("scripts.software_gl_probe.shutil.which", return_value=None):
            with self.assertRaisesRegex(SoftwareGlProbeError, "xvfb-run is required"):
                probe_command(Path("viewr"), Path("image.png"))

    def test_a_report_counts_only_when_a_frame_reached_the_display(self) -> None:
        self.assertTrue(presented(report_json()))
        # A probe that exits without ever presenting reports no first pixel.
        self.assertFalse(presented(report_json(first_pixel_us=0)))
        with self.assertRaisesRegex(SoftwareGlProbeError, "invalid"):
            presented("<not json>")
        with self.assertRaisesRegex(SoftwareGlProbeError, "unexpected schema"):
            presented('{"window_ready_us": 10}')
        with self.assertRaisesRegex(SoftwareGlProbeError, "software adapter"):
            presented(report_json(adapter_backend="vulkan"))
        with self.assertRaisesRegex(SoftwareGlProbeError, "software adapter"):
            presented(
                report_json(
                    adapter_name="AMD Radeon 780M",
                    adapter_device_type="integrated-gpu",
                    adapter_driver="Mesa",
                )
            )
        self.assertTrue(
            presented(
                report_json(
                    adapter_name="softpipe",
                    adapter_device_type="other",
                    adapter_driver="",
                )
            )
        )

    def test_a_failed_surface_is_reported_with_the_process_error(self) -> None:
        failed = subprocess.CompletedProcess(
            [], 1, "", "viewr: cannot present images on this display"
        )
        with mock.patch(
            "scripts.software_gl_probe.shutil.which", return_value="/usr/bin/xvfb-run"
        ):
            with mock.patch(
                "scripts.software_gl_probe.subprocess.run", return_value=failed
            ):
                with self.assertRaisesRegex(SoftwareGlProbeError, "did not present"):
                    run(Path("viewr"))

            # A clean exit that never presented is still a failure.
            silent = subprocess.CompletedProcess(
                [], 0, report_json(first_pixel_us=0), ""
            )
            with mock.patch(
                "scripts.software_gl_probe.subprocess.run", return_value=silent
            ):
                with self.assertRaisesRegex(SoftwareGlProbeError, "without presenting"):
                    run(Path("viewr"))

            presented_run = subprocess.CompletedProcess(
                [], 0, report_json(first_pixel_us=9000), ""
            )
            with mock.patch(
                "scripts.software_gl_probe.subprocess.run", return_value=presented_run
            ):
                self.assertEqual(run(Path("viewr")), 0)

    def test_main_reports_the_outcome_on_the_expected_stream(self) -> None:
        output, errors = io.StringIO(), io.StringIO()
        with mock.patch("scripts.software_gl_probe.run", return_value=0):
            with redirect_stdout(output):
                self.assertEqual(main(["--binary", "viewr"]), 0)
        self.assertIn("presented through the OpenGL backend", output.getvalue())

        with mock.patch(
            "scripts.software_gl_probe.run",
            side_effect=SoftwareGlProbeError("no adapter"),
        ):
            with redirect_stderr(errors):
                self.assertEqual(main(["--binary", "viewr"]), 1)
        self.assertIn("no adapter", errors.getvalue())


if __name__ == "__main__":
    unittest.main()
