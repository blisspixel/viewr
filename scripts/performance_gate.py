"""Black-box GUI performance regression gate for viewr.

The gate creates a deterministic temporary PNG corpus, runs the release viewer's
explicit local probe under a virtual display on Linux, and enforces conservative
process-level budgets. It uses only the Python standard library and deletes the
corpus on exit.
"""

from __future__ import annotations

import argparse
import binascii
import ctypes
import hashlib
import hmac
import json
import math
import os
import platform
import re
from dataclasses import asdict, dataclass
from pathlib import Path
import shutil
import stat
import statistics
import struct
import subprocess
import sys
import tempfile
from typing import Any, Sequence
import zlib


REPORT_KEYS = frozenset(
    {
        "adapter_backend",
        "adapter_name",
        "adapter_device_type",
        "adapter_driver",
        "window_ready_us",
        "first_pixel_us",
        "max_navigation_us",
        "idle_redraws",
        "idle_non_redraw_events",
        "idle_event_repaint_requests",
        "idle_scheduled_egui_repaints",
        "idle_window_focused",
        "idle_pointer_inside",
        "peak_resident_bytes",
        "playlist_entries",
        "decoded_cache_entries",
        "decoded_cache_bytes",
        "thumbnail_texture_entries",
    }
)
BOOLEAN_REPORT_KEYS = frozenset({"idle_window_focused", "idle_pointer_inside"})
STRING_REPORT_KEYS = frozenset(
    {"adapter_backend", "adapter_name", "adapter_device_type", "adapter_driver"}
)
INTEGER_REPORT_KEYS = REPORT_KEYS - BOOLEAN_REPORT_KEYS - STRING_REPORT_KEYS
ADAPTER_BACKENDS = frozenset({"vulkan", "metal", "dx12", "gl"})
ADAPTER_DEVICE_TYPES = frozenset(
    {"other", "integrated-gpu", "discrete-gpu", "virtual-gpu", "cpu"}
)
MAX_LINKED_TARGETS_PER_SOURCE = 512
MAX_PROBE_RUNS = 9
MAX_CORPUS_COUNT = 100_000
DECODED_CACHE_LIMIT_BYTES = 256 * 1024 * 1024
CACHE_STRESS_WIDTH = 4096
CACHE_STRESS_HEIGHT = 4096
CACHE_STRESS_COUNT = 8


class PerformanceGateError(RuntimeError):
    """A probe could not run or violated its performance contract."""


def _positive_finite_float(value: str) -> float:
    """Parse a strictly positive, finite command-line budget."""

    parsed = float(value)
    if not math.isfinite(parsed) or parsed <= 0:
        raise argparse.ArgumentTypeError("must be a positive finite number")
    return parsed


def _nonnegative_int(value: str) -> int:
    """Parse a non-negative integer command-line budget."""

    parsed = int(value)
    if parsed < 0:
        raise argparse.ArgumentTypeError("must be a non-negative integer")
    return parsed


@dataclass(frozen=True)
class ProbeReport:
    """Validated machine-readable output from one viewr probe process."""

    adapter_backend: str
    adapter_name: str
    adapter_device_type: str
    adapter_driver: str
    window_ready_us: int
    first_pixel_us: int
    max_navigation_us: int
    idle_redraws: int
    idle_non_redraw_events: int
    idle_event_repaint_requests: int
    idle_scheduled_egui_repaints: int
    idle_window_focused: bool
    idle_pointer_inside: bool
    peak_resident_bytes: int
    playlist_entries: int
    decoded_cache_entries: int
    decoded_cache_bytes: int
    thumbnail_texture_entries: int


@dataclass(frozen=True)
class Budgets:
    """Absolute and folder-scaling limits enforced by the gate."""

    window_ready_ms: float
    first_pixel_ms: float
    navigation_ms: float
    idle_redraws: int
    peak_resident_mib: float
    folder_growth_mib: float


def _png_chunk(kind: bytes, payload: bytes) -> bytes:
    checksum = binascii.crc32(kind)
    checksum = binascii.crc32(payload, checksum) & 0xFFFFFFFF
    return (
        struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", checksum)
    )


def deterministic_png(width: int, height: int) -> bytes:
    """Create a valid opaque RGB PNG without image-library dependencies."""

    if width <= 0 or height <= 0:
        raise ValueError("PNG dimensions must be positive")
    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    row = b"\x00" + b"\x35\x71\xa8" * width
    compressor = zlib.compressobj(level=6)
    compressed_rows = [compressor.compress(row) for _ in range(height)]
    compressed_rows.append(compressor.flush())
    pixels = b"".join(compressed_rows)
    return b"".join(
        (
            b"\x89PNG\r\n\x1a\n",
            _png_chunk(b"IHDR", header),
            _png_chunk(b"IDAT", pixels),
            _png_chunk(b"IEND", b""),
        )
    )


def create_linked_corpus(directory: Path, source: Path, count: int) -> Path:
    """Populate `directory` with bounded-link shards and return the first path."""

    directory.mkdir(parents=True, exist_ok=False)
    first: Path | None = None
    for index in range(count):
        shard_index = index // MAX_LINKED_TARGETS_PER_SOURCE
        if shard_index == 0:
            shard = source
        else:
            shard = source.with_name(f".{directory.name}-source-{shard_index:05}.bin")
            if index % MAX_LINKED_TARGETS_PER_SOURCE == 0:
                shutil.copyfile(source, shard)
        target = directory / f"image-{index:05}.png"
        try:
            os.link(shard, target)
        except OSError:
            shutil.copyfile(shard, target)
        first = first or target
    if first is None:
        raise ValueError("corpus count must be positive")
    return first


def parse_report(stdout: str) -> ProbeReport:
    """Parse the final exact-shape JSON object emitted by the GUI probe."""

    for line in reversed(stdout.splitlines()):
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            payload: Any = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(payload, dict) or frozenset(payload) != REPORT_KEYS:
            raise PerformanceGateError("probe report has an unexpected schema")
        if any(
            type(payload[key]) is not int or payload[key] < 0
            for key in INTEGER_REPORT_KEYS
        ) or any(type(payload[key]) is not bool for key in BOOLEAN_REPORT_KEYS):
            raise PerformanceGateError(
                "probe report values must use non-negative integers and exact booleans"
            )
        if any(type(payload[key]) is not str for key in STRING_REPORT_KEYS):
            raise PerformanceGateError("probe adapter identity must use exact strings")
        if payload["adapter_backend"] not in ADAPTER_BACKENDS:
            raise PerformanceGateError("probe adapter backend is unsupported")
        if payload["adapter_device_type"] not in ADAPTER_DEVICE_TYPES:
            raise PerformanceGateError("probe adapter device type is unsupported")
        for key in ("adapter_name", "adapter_driver"):
            value = payload[key]
            if len(value) > 256 or any(
                not character.isprintable() for character in value
            ):
                raise PerformanceGateError(
                    f"probe {key.replace('_', ' ')} is not bounded printable text"
                )
            if key == "adapter_name" and not value.strip():
                raise PerformanceGateError("probe adapter name must be nonempty")
        return ProbeReport(**payload)
    raise PerformanceGateError("probe produced no machine-readable report")


def _command(binary: Path, image: Path, use_xvfb: bool) -> list[str]:
    command = [str(binary), "performance-probe", str(image)]
    if not use_xvfb:
        return command
    xvfb = _trusted_path_tool("xvfb-run", "the Linux GUI probe")
    return [str(xvfb), "-a", *command]


def run_probe(binary: Path, image: Path, use_xvfb: bool) -> ProbeReport:
    """Run one isolated probe process with a hard wall-clock timeout."""

    environment = os.environ.copy()
    environment.pop("RUST_LOG", None)
    environment.pop("VIEWR_LOG", None)
    environment.pop("VIEWR_DECODE_BIN", None)
    try:
        completed = subprocess.run(
            _command(binary, image, use_xvfb),
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            env=environment,
            timeout=90,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise PerformanceGateError(f"could not execute GUI probe: {error}") from error
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip() or "no diagnostic"
        raise PerformanceGateError(
            f"GUI probe exited with {completed.returncode}: {detail}"
        )
    return parse_report(completed.stdout)


def _require_one_adapter(reports: Sequence[ProbeReport]) -> tuple[str, str, str, str]:
    """Return the one actual adapter identity shared by every probe process."""

    adapter_identities = {
        (
            report.adapter_backend,
            report.adapter_name,
            report.adapter_device_type,
            report.adapter_driver,
        )
        for report in reports
    }
    if len(adapter_identities) != 1:
        raise PerformanceGateError("probe runs selected different GPU adapters")
    return next(iter(adapter_identities))


def _median_report(reports: list[ProbeReport]) -> ProbeReport:
    """Use medians for noisy timings and maxima for resource/capacity values."""

    adapter_backend, adapter_name, adapter_device_type, adapter_driver = (
        _require_one_adapter(reports)
    )
    return ProbeReport(
        adapter_backend=adapter_backend,
        adapter_name=adapter_name,
        adapter_device_type=adapter_device_type,
        adapter_driver=adapter_driver,
        window_ready_us=int(
            statistics.median(report.window_ready_us for report in reports)
        ),
        first_pixel_us=int(
            statistics.median(report.first_pixel_us for report in reports)
        ),
        max_navigation_us=max(report.max_navigation_us for report in reports),
        idle_redraws=max(report.idle_redraws for report in reports),
        idle_non_redraw_events=max(report.idle_non_redraw_events for report in reports),
        idle_event_repaint_requests=max(
            report.idle_event_repaint_requests for report in reports
        ),
        idle_scheduled_egui_repaints=max(
            report.idle_scheduled_egui_repaints for report in reports
        ),
        idle_window_focused=any(report.idle_window_focused for report in reports),
        idle_pointer_inside=any(report.idle_pointer_inside for report in reports),
        peak_resident_bytes=max(report.peak_resident_bytes for report in reports),
        playlist_entries=max(report.playlist_entries for report in reports),
        decoded_cache_entries=max(report.decoded_cache_entries for report in reports),
        decoded_cache_bytes=max(report.decoded_cache_bytes for report in reports),
        thumbnail_texture_entries=max(
            report.thumbnail_texture_entries for report in reports
        ),
    )


def _idle_diagnostics(
    small: list[ProbeReport], large: list[ProbeReport], cache_stress: ProbeReport
) -> str:
    """Return fixed, path-free per-run idle evidence without losing run order."""

    def one(report: ProbeReport) -> dict[str, int | bool]:
        return {
            "delivered_redraws": report.idle_redraws,
            "non_redraw_events": report.idle_non_redraw_events,
            "event_repaint_requests": report.idle_event_repaint_requests,
            "scheduled_egui_repaints": report.idle_scheduled_egui_repaints,
            "window_focused": report.idle_window_focused,
            "pointer_inside": report.idle_pointer_inside,
        }

    payload = {
        "small": [one(report) for report in small],
        "large": [one(report) for report in large],
        "cache_stress": one(cache_stress),
    }
    return json.dumps(payload, separators=(",", ":"), sort_keys=True)


def evaluate(
    small: ProbeReport,
    large: ProbeReport,
    cache_stress: ProbeReport,
    budgets: Budgets,
    small_count: int,
    large_count: int,
    small_rss_floor_bytes: int | None = None,
) -> list[str]:
    """Return every budget violation instead of hiding later failures."""

    failures: list[str] = []

    def over(actual: float, limit: float, label: str, unit: str) -> None:
        if actual > limit:
            failures.append(f"{label}: {actual:.2f} {unit} exceeds {limit:.2f} {unit}")

    over(
        max(small.window_ready_us, large.window_ready_us) / 1000,
        budgets.window_ready_ms,
        "window ready",
        "ms",
    )
    over(
        max(small.first_pixel_us, large.first_pixel_us) / 1000,
        budgets.first_pixel_ms,
        "first pixel",
        "ms",
    )
    over(
        max(
            small.max_navigation_us,
            large.max_navigation_us,
            cache_stress.max_navigation_us,
        )
        / 1000,
        budgets.navigation_ms,
        "sampled navigation",
        "ms",
    )
    for label, report in (
        ("small", small),
        ("large", large),
        ("cache-stress", cache_stress),
    ):
        if report.idle_redraws > budgets.idle_redraws:
            failures.append(
                f"{label} idle redraws: {report.idle_redraws} exceeds "
                f"{budgets.idle_redraws}"
            )
    over(
        large.peak_resident_bytes / (1024 * 1024),
        budgets.peak_resident_mib,
        "large-folder peak resident set",
        "MiB",
    )
    small_rss_floor_bytes = (
        small.peak_resident_bytes
        if small_rss_floor_bytes is None
        else small_rss_floor_bytes
    )
    growth_bytes = max(0, large.peak_resident_bytes - small_rss_floor_bytes)
    over(
        growth_bytes / (1024 * 1024),
        budgets.folder_growth_mib,
        "folder-size resident growth",
        "MiB",
    )
    if small.playlist_entries != small_count:
        failures.append(
            f"small probe scanned {small.playlist_entries} entries; expected {small_count}"
        )
    if large.playlist_entries != large_count:
        failures.append(
            f"large probe scanned {large.playlist_entries} entries; expected {large_count}"
        )
    for label, report in (("small", small), ("large", large)):
        if report.decoded_cache_entries > 5:
            failures.append(
                f"{label} decoded cache retained {report.decoded_cache_entries}; limit is 5"
            )
        if report.decoded_cache_bytes > DECODED_CACHE_LIMIT_BYTES:
            failures.append(
                f"{label} decoded cache retained {report.decoded_cache_bytes} bytes; "
                f"limit is {DECODED_CACHE_LIMIT_BYTES}"
            )
        if report.thumbnail_texture_entries > 9:
            failures.append(
                f"{label} thumbnail cache retained "
                f"{report.thumbnail_texture_entries}; limit is 9"
            )
    return failures


def evaluate_cache_stress(
    report: ProbeReport,
    corpus_count: int,
    decoded_image_bytes: int,
) -> list[str]:
    """Validate a corpus where five decoded neighbors exceed the byte budget."""

    if corpus_count < 6 or decoded_image_bytes <= 0:
        raise PerformanceGateError("cache-stress corpus parameters are invalid")
    if decoded_image_bytes * 5 <= DECODED_CACHE_LIMIT_BYTES:
        raise PerformanceGateError(
            "cache-stress images are too small to exercise byte-based eviction"
        )

    failures: list[str] = []
    if report.playlist_entries != corpus_count:
        failures.append(
            f"cache-stress probe scanned {report.playlist_entries} entries; "
            f"expected {corpus_count}"
        )
    maximum_entries = DECODED_CACHE_LIMIT_BYTES // decoded_image_bytes
    if report.decoded_cache_entries != maximum_entries:
        failures.append(
            "cache-stress decoded cache retained "
            f"{report.decoded_cache_entries} entries; expected {maximum_entries} "
            "to prove byte-budget eviction without under-retention"
        )
    expected_bytes = report.decoded_cache_entries * decoded_image_bytes
    if report.decoded_cache_bytes != expected_bytes:
        failures.append(
            "cache-stress decoded cache accounting reported "
            f"{report.decoded_cache_bytes} bytes; expected {expected_bytes}"
        )
    if report.decoded_cache_bytes > DECODED_CACHE_LIMIT_BYTES:
        failures.append(
            "cache-stress decoded cache retained "
            f"{report.decoded_cache_bytes} bytes; limit is {DECODED_CACHE_LIMIT_BYTES}"
        )
    if report.thumbnail_texture_entries > 9:
        failures.append(
            "cache-stress thumbnail cache retained "
            f"{report.thumbnail_texture_entries}; limit is 9"
        )
    return failures


def _arguments(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--small-count", type=int, default=16)
    parser.add_argument("--large-count", type=int, default=50_000)
    parser.add_argument("--window-ready-ms", type=_positive_finite_float, default=3000)
    parser.add_argument("--first-pixel-ms", type=_positive_finite_float, default=5000)
    parser.add_argument("--navigation-ms", type=_positive_finite_float, default=500)
    parser.add_argument("--idle-redraws", type=_nonnegative_int, default=2)
    parser.add_argument(
        "--idle-diagnostics",
        action="store_true",
        help="print fixed per-run idle attribution even when the gate passes",
    )
    parser.add_argument("--peak-resident-mib", type=_positive_finite_float, default=768)
    parser.add_argument("--folder-growth-mib", type=_positive_finite_float, default=96)
    parser.add_argument(
        "--xvfb",
        action=argparse.BooleanOptionalAction,
        default=sys.platform.startswith("linux"),
        help="run the GUI through xvfb-run (default on Linux)",
    )
    parser.add_argument(
        "--report-file",
        type=Path,
        help=(
            "write a path-free JSON evidence report bound to the tested binary; "
            "the destination must not already exist"
        ),
    )
    parser.add_argument(
        "--session-label",
        help=(
            "path-free lowercase label stored in --report-file; required when a "
            "report is requested"
        ),
    )
    return parser.parse_args(argv)


def _binary_sha256(binary: Path) -> str:
    digest = hashlib.sha256()
    with binary.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _is_link_or_reparse_point(metadata: os.stat_result) -> bool:
    if stat.S_ISLNK(metadata.st_mode):
        return True
    attributes = getattr(metadata, "st_file_attributes", 0) or 0
    reparse_attribute = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)
    return bool(attributes & reparse_attribute)


def _require_regular_nonlink(path: Path, description: str) -> os.stat_result:
    try:
        metadata = path.lstat()
    except (FileNotFoundError, OSError) as error:
        raise PerformanceGateError(f"{description} does not exist: {path}") from error
    if _is_link_or_reparse_point(metadata) or not stat.S_ISREG(metadata.st_mode):
        raise PerformanceGateError(
            f"{description} must be a regular non-link file: {path}"
        )
    return metadata


def _executable_paths(binary: Path) -> dict[str, Path]:
    binary = binary.absolute()
    _require_regular_nonlink(binary, "viewr binary")
    worker_name = (
        "viewr-decode.exe" if binary.suffix.casefold() == ".exe" else "viewr-decode"
    )
    worker = binary.with_name(worker_name)
    _require_regular_nonlink(worker, "decoder worker beside viewr")
    return {"viewr": binary, "viewr-decode": worker}


def _executable_digests(executables: dict[str, Path]) -> dict[str, str]:
    return {name: _binary_sha256(path) for name, path in executables.items()}


def _copy_executables(
    executables: dict[str, Path], destination: Path
) -> dict[str, Path]:
    """Copy the executable pair into one private harness-owned directory."""

    destination.mkdir(mode=0o700, parents=False, exist_ok=False)
    if os.name != "nt":
        destination.chmod(0o700)
    copies: dict[str, Path] = {}
    for name, source in executables.items():
        source_metadata = _require_regular_nonlink(source, name)
        copied = destination / source.name
        try:
            shutil.copy2(source, copied, follow_symlinks=False)
        except OSError as error:
            raise PerformanceGateError(
                f"could not copy {name} into the private harness directory"
            ) from error
        copied_metadata = _require_regular_nonlink(copied, f"copied {name}")
        if stat.S_IMODE(copied_metadata.st_mode) != stat.S_IMODE(
            source_metadata.st_mode
        ):
            raise PerformanceGateError(
                f"copied {name} did not preserve its execute permissions"
            )
        copies[name] = copied
    return copies


def _trusted_executable(candidate: Path, description: str) -> Path:
    """Return an absolute executable whose path chain is not mutable by others."""

    candidate = candidate.absolute()
    current = candidate
    while True:
        try:
            metadata = current.lstat()
        except (FileNotFoundError, OSError) as error:
            raise PerformanceGateError(
                f"trusted {description} path does not exist: {current}"
            ) from error
        if _is_link_or_reparse_point(metadata):
            raise PerformanceGateError(
                f"trusted {description} path contains a link or reparse point: {current}"
            )
        if metadata.st_mode & (stat.S_IWGRP | stat.S_IWOTH):
            raise PerformanceGateError(
                f"trusted {description} path is group- or world-writable: {current}"
            )
        parent = current.parent
        if parent == current:
            break
        current = parent

    metadata = _require_regular_nonlink(candidate, description)
    if not os.access(candidate, os.X_OK):
        raise PerformanceGateError(
            f"trusted {description} is not executable: {candidate}"
        )
    return candidate


def _trusted_path_tool(name: str, description: str) -> Path:
    located = shutil.which(name)
    if located is None:
        raise PerformanceGateError(f"{name} is required for {description}")
    return _trusted_executable(Path(located), name)


def _windows_scale_percent() -> int:
    try:
        scale = int(ctypes.windll.shcore.GetScaleFactorForDevice(0))
    except (AttributeError, OSError) as error:
        raise PerformanceGateError(
            "could not measure the primary Windows display scale"
        ) from error
    if scale not in {
        100,
        120,
        125,
        140,
        150,
        160,
        175,
        180,
        200,
        225,
        250,
        300,
        350,
        400,
        450,
        500,
    }:
        raise PerformanceGateError(
            "could not measure the primary Windows display scale"
        )
    return scale


def _macos_display_evidence() -> dict[str, object]:
    system_profiler = _trusted_executable(
        Path("/usr/sbin/system_profiler"), "system_profiler"
    )
    try:
        completed = subprocess.run(
            [str(system_profiler), "SPDisplaysDataType", "-json"],
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise PerformanceGateError(
            "could not inspect the main macOS display"
        ) from error
    if completed.returncode != 0:
        raise PerformanceGateError("could not inspect the main macOS display")
    try:
        payload = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise PerformanceGateError(
            "macOS display inspection returned invalid JSON"
        ) from error
    adapters = payload.get("SPDisplaysDataType") if isinstance(payload, dict) else None
    displays = [
        display
        for adapter in adapters or []
        if isinstance(adapter, dict)
        for display in adapter.get("spdisplays_ndrvs", [])
        if isinstance(display, dict)
    ]
    main_displays = [
        display
        for display in displays
        if display.get("spdisplays_main") == "spdisplays_yes"
    ]
    if len(main_displays) != 1:
        raise PerformanceGateError(
            "macOS display inspection did not identify one main display"
        )
    display = main_displays[0]
    description = " ".join(
        str(display.get(field, ""))
        for field in (
            "_name",
            "spdisplays_display_type",
            "spdisplays_connection_type",
            "spdisplays_pixelresolution",
            "spdisplays_pixels",
            "spdisplays_resolution",
            "spdisplays_vendor-id",
            "spdisplays_device-id",
            "spdisplays_display-serial-number",
            "_spdisplays_display-vendor-id",
            "_spdisplays_display-product-id",
            "_spdisplays_display-serial-number",
            "_spdisplays_displayID",
        )
    ).strip()
    if not description:
        raise PerformanceGateError("macOS main display identity is unavailable")
    folded = description.casefold()
    pixels = re.search(
        r"([0-9]+)\s*x\s*([0-9]+)",
        str(
            display.get(
                "spdisplays_pixelresolution", display.get("spdisplays_pixels", "")
            )
        ),
    )
    logical = re.search(
        r"([0-9]+)\s*x\s*([0-9]+)", str(display.get("spdisplays_resolution", ""))
    )
    scale_percent = 0
    if pixels is not None and logical is not None and int(logical.group(1)) > 0:
        scale_percent = round(int(pixels.group(1)) / int(logical.group(1)) * 100)
    return {
        "display_identity_sha256": hashlib.sha256(
            description.encode("utf-8")
        ).hexdigest(),
        "display_builtin": "built-in" in folded or "spdisplays_internal" in folded,
        "display_retina": "retina" in folded or scale_percent >= 200,
        "display_scale_percent": scale_percent,
    }


def _linux_graphics_evidence(*, require_opengl: bool = False) -> dict[str, object]:
    session_type = os.environ.get("XDG_SESSION_TYPE", "").casefold()
    has_wayland = bool(os.environ.get("WAYLAND_DISPLAY"))
    has_x11 = bool(os.environ.get("DISPLAY"))
    forced_backend = os.environ.get("WINIT_UNIX_BACKEND", "").casefold()
    if session_type == "wayland" and has_wayland and forced_backend != "x11":
        measured_session = "wayland"
    elif session_type == "x11" and has_x11:
        measured_session = "x11"
    elif session_type == "wayland" and has_x11 and forced_backend == "x11":
        measured_session = "xwayland"
    else:
        raise PerformanceGateError(
            "could not prove a native Wayland, X11, or Xwayland session"
        )
    evidence: dict[str, object] = {"linux_session": measured_session}
    if not require_opengl:
        return evidence
    if not has_x11:
        raise PerformanceGateError(
            "the Mesa software session requires DISPLAY for glxinfo evidence"
        )
    glxinfo = _trusted_path_tool("glxinfo", "Linux renderer evidence")
    try:
        completed = subprocess.run(
            [str(glxinfo), "-B"],
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=15,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise PerformanceGateError(
            "glxinfo -B is required to record the Linux renderer"
        ) from error
    if completed.returncode != 0:
        raise PerformanceGateError("glxinfo -B could not inspect the Linux renderer")
    renderer_match = re.search(
        r"^OpenGL renderer string:\s*(\S[^\r\n]*)$", completed.stdout, re.MULTILINE
    )
    vendor_match = re.search(
        r"^OpenGL vendor string:\s*(\S[^\r\n]*)$", completed.stdout, re.MULTILINE
    )
    if renderer_match is None or vendor_match is None:
        raise PerformanceGateError("glxinfo -B did not identify the Linux renderer")
    renderer = renderer_match.group(1).strip()
    vendor = vendor_match.group(1).strip()
    software = (
        re.search(r"\b(?:llvmpipe|softpipe|software rasterizer)\b", renderer, re.I)
        is not None
    )
    mesa = "mesa" in completed.stdout.casefold()
    evidence.update(
        {
            "opengl_renderer": renderer,
            "opengl_vendor": vendor,
            "opengl_mesa": mesa,
            "opengl_software": software,
        }
    )
    return evidence


def _session_evidence(
    host_platform: str, session_label: str | None = None
) -> dict[str, object]:
    if host_platform == "Windows":
        return {"display_scale_percent": _windows_scale_percent()}
    if host_platform == "Darwin":
        return _macos_display_evidence()
    if host_platform == "Linux":
        return _linux_graphics_evidence(
            require_opengl=session_label == "linux-mesa-software"
        )
    raise PerformanceGateError(
        f"unsupported performance evidence platform: {host_platform}"
    )


def _renderer_controls() -> dict[str, str]:
    backend = os.environ.get("WGPU_BACKEND", "").casefold()
    if backend not in {"dx12", "gl", "metal", "vulkan"}:
        backend = ""
    software = "1" if os.environ.get("LIBGL_ALWAYS_SOFTWARE") == "1" else ""
    return {
        "wgpu_backend": backend,
        "libgl_always_software": software,
    }


def _evidence_report(
    executable_sha256: dict[str, str],
    session_label: str,
    host_platform: str,
    session_evidence: dict[str, object],
    budgets: Budgets,
    small_reports: list[ProbeReport],
    large_reports: list[ProbeReport],
    cache_stress: ProbeReport,
    small: ProbeReport,
    large: ProbeReport,
    small_rss_floor_bytes: int,
    failures: list[str],
) -> dict[str, object]:
    """Return path-free, byte-bound evidence for one complete gate execution."""

    retained_reports = (*small_reports, *large_reports, cache_stress)
    return {
        "schema": 3,
        "status": "fail" if failures else "pass",
        "executable_sha256": executable_sha256,
        "session_label": session_label,
        "host_platform": host_platform,
        "session_evidence": session_evidence,
        "renderer_controls": _renderer_controls(),
        "budgets": asdict(budgets),
        "summary": {
            "window_ready_ms": round(
                max(small.window_ready_us, large.window_ready_us) / 1000, 2
            ),
            "first_pixel_ms": round(
                max(small.first_pixel_us, large.first_pixel_us) / 1000, 2
            ),
            "navigation_max_ms": round(
                max(report.max_navigation_us for report in retained_reports) / 1000,
                2,
            ),
            "idle_redraws": max(report.idle_redraws for report in retained_reports),
            "small_rss_mib": round(small.peak_resident_bytes / (1024 * 1024), 2),
            "small_rss_floor_mib": round(small_rss_floor_bytes / (1024 * 1024), 2),
            "large_rss_mib": round(large.peak_resident_bytes / (1024 * 1024), 2),
            "folder_growth_mib": round(
                max(0, large.peak_resident_bytes - small_rss_floor_bytes)
                / (1024 * 1024),
                2,
            ),
            "large_folder_images": large.playlist_entries,
            "cache_stress_entries": cache_stress.decoded_cache_entries,
            "cache_stress_bytes": cache_stress.decoded_cache_bytes,
            "cache_stress_mib": round(
                cache_stress.decoded_cache_bytes / (1024 * 1024), 2
            ),
        },
        "runs": {
            "small": [asdict(report) for report in small_reports],
            "large": [asdict(report) for report in large_reports],
            "cache_stress": asdict(cache_stress),
        },
        "failures": failures,
    }


def _write_evidence_report(destination: Path, report: dict[str, object]) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    try:
        with destination.open("x", encoding="utf-8", newline="\n") as output:
            json.dump(report, output, indent=2, sort_keys=True)
            output.write("\n")
    except FileExistsError as error:
        raise PerformanceGateError(
            f"report destination already exists: {destination}"
        ) from error


def main(argv: Sequence[str] | None = None) -> int:
    args = _arguments(argv)
    binary = args.binary.absolute()
    executables = _executable_paths(binary)
    if args.report_file is not None and args.report_file.exists():
        raise PerformanceGateError(
            f"report destination already exists: {args.report_file}"
        )
    if args.report_file is None and args.session_label is not None:
        raise PerformanceGateError("--session-label requires --report-file")
    if args.report_file is not None:
        if args.report_file.suffix.casefold() != ".json":
            raise PerformanceGateError("--report-file must use the .json extension")
        if (
            args.session_label is None
            or re.fullmatch(r"[a-z0-9][a-z0-9-]{1,63}", args.session_label) is None
        ):
            raise PerformanceGateError(
                "--session-label must be 2 to 64 lowercase letters, digits, or hyphens"
            )
        if args.report_file.stem != args.session_label:
            raise PerformanceGateError("--report-file stem must match --session-label")
    if args.runs < 1 or args.runs > MAX_PROBE_RUNS or args.runs % 2 == 0:
        raise PerformanceGateError(
            f"--runs must be a positive odd number no greater than {MAX_PROBE_RUNS}"
        )
    if args.small_count < 5 or args.large_count <= args.small_count:
        raise PerformanceGateError("corpus counts must satisfy 5 <= small < large")
    if args.large_count > MAX_CORPUS_COUNT:
        raise PerformanceGateError(f"--large-count must not exceed {MAX_CORPUS_COUNT}")
    budgets = Budgets(
        window_ready_ms=args.window_ready_ms,
        first_pixel_ms=args.first_pixel_ms,
        navigation_ms=args.navigation_ms,
        idle_redraws=args.idle_redraws,
        peak_resident_mib=args.peak_resident_mib,
        folder_growth_mib=args.folder_growth_mib,
    )
    host_platform: str | None = None
    session_evidence: dict[str, object] | None = None
    if args.report_file is not None:
        host_platform = platform.system()
        session_evidence = _session_evidence(host_platform, args.session_label)

    # This harness owns and cleans its directory. Keep a disjoint prefix so
    # performance-probe artifacts remain unambiguous during a failed run.
    with tempfile.TemporaryDirectory(prefix="performance-gate-") as temp:
        root = Path(temp)
        if os.name != "nt":
            root.chmod(0o700)
        harness_executables = _copy_executables(executables, root / "executables")
        executable_sha256 = _executable_digests(harness_executables)
        harness_binary = harness_executables["viewr"]
        source = root / "source.png"
        source.write_bytes(deterministic_png(1920, 1080))
        small_image = create_linked_corpus(root / "small", source, args.small_count)
        large_image = create_linked_corpus(root / "large", source, args.large_count)
        cache_source = root / "cache-source.png"
        cache_source.write_bytes(
            deterministic_png(CACHE_STRESS_WIDTH, CACHE_STRESS_HEIGHT)
        )
        cache_image = create_linked_corpus(
            root / "cache-stress", cache_source, CACHE_STRESS_COUNT
        )
        small_reports = [
            run_probe(harness_binary, small_image, args.xvfb) for _ in range(args.runs)
        ]
        large_reports = [
            run_probe(harness_binary, large_image, args.xvfb) for _ in range(args.runs)
        ]
        small = _median_report(small_reports)
        large = _median_report(large_reports)
        cache_stress = run_probe(harness_binary, cache_image, args.xvfb)
        _require_one_adapter([*small_reports, *large_reports, cache_stress])
        small_rss_floor_bytes = min(
            report.peak_resident_bytes for report in small_reports
        )
        final_sha256 = _executable_digests(harness_executables)
        if any(
            not hmac.compare_digest(executable_sha256[name], final_sha256[name])
            for name in executable_sha256
        ):
            raise PerformanceGateError(
                "a tested executable changed during the performance run"
            )
    if host_platform is not None and session_evidence is not None:
        if _session_evidence(host_platform, args.session_label) != session_evidence:
            raise PerformanceGateError(
                "measured display, session, or renderer changed during the performance run"
            )

    print(
        "performance: "
        f"window={max(small.window_ready_us, large.window_ready_us) / 1000:.2f} ms, "
        "first-pixel="
        f"{max(small.first_pixel_us, large.first_pixel_us) / 1000:.2f} ms, "
        "navigation-max="
        f"{max(small.max_navigation_us, large.max_navigation_us, cache_stress.max_navigation_us) / 1000:.2f} ms, "
        "idle-redraws="
        f"{max(small.idle_redraws, large.idle_redraws, cache_stress.idle_redraws)}, "
        f"small-rss={small.peak_resident_bytes / (1024 * 1024):.2f} MiB, "
        f"large-rss={large.peak_resident_bytes / (1024 * 1024):.2f} MiB, "
        f"large-folder={large.playlist_entries} images, "
        f"cache-stress={cache_stress.decoded_cache_entries} entries/"
        f"{cache_stress.decoded_cache_bytes / (1024 * 1024):.2f} MiB"
    )
    failures = evaluate(
        small,
        large,
        cache_stress,
        budgets,
        args.small_count,
        args.large_count,
        small_rss_floor_bytes,
    )
    failures.extend(
        evaluate_cache_stress(
            cache_stress,
            CACHE_STRESS_COUNT,
            CACHE_STRESS_WIDTH * CACHE_STRESS_HEIGHT * 4,
        )
    )
    if args.report_file is not None:
        assert args.session_label is not None
        assert host_platform is not None
        assert session_evidence is not None
        _write_evidence_report(
            args.report_file,
            _evidence_report(
                executable_sha256,
                args.session_label,
                host_platform,
                session_evidence,
                budgets,
                small_reports,
                large_reports,
                cache_stress,
                small,
                large,
                small_rss_floor_bytes,
                failures,
            ),
        )
    if args.idle_diagnostics or failures:
        destination = sys.stderr if failures else sys.stdout
        print(
            f"idle diagnostics: {_idle_diagnostics(small_reports, large_reports, cache_stress)}",
            file=destination,
        )
    if failures:
        for failure in failures:
            print(f"performance gate failed: {failure}", file=sys.stderr)
        return 1
    print("performance gate: OK")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except PerformanceGateError as error:
        print(f"performance gate failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
