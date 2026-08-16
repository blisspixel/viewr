#!/usr/bin/env python3
"""Prove viewr can present through the OpenGL backend on a software X session.

A host with no Vulkan driver still has to show a photo. wgpu reaches Mesa's
OpenGL through EGL, and only when the display connection is handed to the wgpu
instance, so a broken hand-off leaves every window surface unpresentable while
every unit test stays green. This runs the real GUI probe under `xvfb-run` with
the OpenGL backend selected and fails when no surface is created.

Timing is deliberately not asserted here; `performance_gate.py` owns budgets.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Sequence

from scripts.performance_gate import deterministic_png

PROBE_TIMEOUT_SECONDS = 180


class SoftwareGlProbeError(RuntimeError):
    """Raised when the OpenGL backend cannot present on this host."""


def _arguments(argv: Sequence[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary",
        type=Path,
        required=True,
        help="path to the built viewr executable",
    )
    return parser.parse_args(argv)


def probe_command(binary: Path, image: Path) -> list[str]:
    """Build the isolated probe command, always through a virtual display."""

    xvfb = shutil.which("xvfb-run")
    if xvfb is None:
        raise SoftwareGlProbeError("xvfb-run is required for the software GL probe")
    return [xvfb, "-a", str(binary), "performance-probe", str(image)]


def presented(report: str) -> bool:
    """Whether the probe reported a presented first frame."""

    try:
        measurements = json.loads(report)
    except json.JSONDecodeError as error:
        raise SoftwareGlProbeError(f"probe report was not JSON: {error}") from error
    first_pixel = measurements.get("first_pixel_us")
    if not isinstance(first_pixel, int):
        raise SoftwareGlProbeError("probe report has no first_pixel_us measurement")
    return first_pixel > 0


def run(binary: Path) -> int:
    """Run one probe on a generated image and report whether it presented."""

    with tempfile.TemporaryDirectory(prefix="viewr-software-gl-") as workspace:
        image = Path(workspace) / "probe.png"
        image.write_bytes(deterministic_png(256, 192))
        command = probe_command(binary, image)
        try:
            completed = subprocess.run(
                command,
                capture_output=True,
                text=True,
                timeout=PROBE_TIMEOUT_SECONDS,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise SoftwareGlProbeError(
                f"could not execute the probe: {error}"
            ) from error

    if completed.returncode != 0:
        raise SoftwareGlProbeError(
            "the OpenGL backend did not present on this software X session "
            f"(exit {completed.returncode}): {completed.stderr.strip()}"
        )
    if not presented(completed.stdout):
        raise SoftwareGlProbeError(
            "the probe exited cleanly without presenting a frame"
        )
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    arguments = _arguments(argv)
    try:
        run(arguments.binary)
    except SoftwareGlProbeError as error:
        print(f"software GL probe: {error}", file=sys.stderr)
        return 1
    print("software GL probe: presented through the OpenGL backend")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
