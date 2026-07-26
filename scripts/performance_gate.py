"""Black-box GUI performance regression gate for viewr.

The gate creates a deterministic temporary PNG corpus, runs the release viewer's
explicit local probe under a virtual display on Linux, and enforces conservative
process-level budgets. It uses only the Python standard library and deletes the
corpus on exit.
"""

from __future__ import annotations

import argparse
import binascii
import json
import math
import os
from dataclasses import dataclass
from pathlib import Path
import shutil
import statistics
import struct
import subprocess
import sys
import tempfile
from typing import Any, Sequence
import zlib


REPORT_KEYS = frozenset(
    {
        "window_ready_us",
        "first_pixel_us",
        "max_navigation_us",
        "idle_redraws",
        "peak_resident_bytes",
        "playlist_entries",
        "decoded_cache_entries",
        "decoded_cache_bytes",
        "thumbnail_texture_entries",
    }
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

    window_ready_us: int
    first_pixel_us: int
    max_navigation_us: int
    idle_redraws: int
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
            type(payload[key]) is not int or payload[key] < 0 for key in REPORT_KEYS
        ):
            raise PerformanceGateError(
                "probe report values must be non-negative integers"
            )
        return ProbeReport(**payload)
    raise PerformanceGateError("probe produced no machine-readable report")


def _command(binary: Path, image: Path, use_xvfb: bool) -> list[str]:
    command = [str(binary), "performance-probe", str(image)]
    if not use_xvfb:
        return command
    xvfb = shutil.which("xvfb-run")
    if xvfb is None:
        raise PerformanceGateError("xvfb-run is required for the Linux GUI probe")
    return [xvfb, "-a", *command]


def run_probe(binary: Path, image: Path, use_xvfb: bool) -> ProbeReport:
    """Run one isolated probe process with a hard wall-clock timeout."""

    environment = os.environ.copy()
    environment.pop("RUST_LOG", None)
    environment.pop("VIEWR_LOG", None)
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


def _median_report(reports: list[ProbeReport]) -> ProbeReport:
    """Use medians for noisy timings and maxima for resource/capacity values."""

    return ProbeReport(
        window_ready_us=int(
            statistics.median(report.window_ready_us for report in reports)
        ),
        first_pixel_us=int(
            statistics.median(report.first_pixel_us for report in reports)
        ),
        max_navigation_us=max(report.max_navigation_us for report in reports),
        idle_redraws=max(report.idle_redraws for report in reports),
        peak_resident_bytes=max(report.peak_resident_bytes for report in reports),
        playlist_entries=max(report.playlist_entries for report in reports),
        decoded_cache_entries=max(report.decoded_cache_entries for report in reports),
        decoded_cache_bytes=max(report.decoded_cache_bytes for report in reports),
        thumbnail_texture_entries=max(
            report.thumbnail_texture_entries for report in reports
        ),
    )


def evaluate(
    small: ProbeReport,
    large: ProbeReport,
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

    over(small.window_ready_us / 1000, budgets.window_ready_ms, "window ready", "ms")
    over(small.first_pixel_us / 1000, budgets.first_pixel_ms, "first pixel", "ms")
    over(
        small.max_navigation_us / 1000,
        budgets.navigation_ms,
        "sampled navigation",
        "ms",
    )
    for label, report in (("small", small), ("large", large)):
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
    parser.add_argument("--peak-resident-mib", type=_positive_finite_float, default=768)
    parser.add_argument("--folder-growth-mib", type=_positive_finite_float, default=96)
    parser.add_argument(
        "--xvfb",
        action=argparse.BooleanOptionalAction,
        default=sys.platform.startswith("linux"),
        help="run the GUI through xvfb-run (default on Linux)",
    )
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = _arguments(argv)
    binary = args.binary.resolve()
    if not binary.is_file():
        raise PerformanceGateError(f"viewr binary does not exist: {binary}")
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

    # The application intentionally scrubs stale `viewr-*` temp names at launch.
    # This harness owns and cleans its directory, so use a disjoint prefix.
    with tempfile.TemporaryDirectory(prefix="performance-gate-") as temp:
        root = Path(temp)
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
            run_probe(binary, small_image, args.xvfb) for _ in range(args.runs)
        ]
        large_reports = [
            run_probe(binary, large_image, args.xvfb) for _ in range(args.runs)
        ]
        small = _median_report(small_reports)
        large = _median_report(large_reports)
        cache_stress = run_probe(binary, cache_image, args.xvfb)
        small_rss_floor_bytes = min(
            report.peak_resident_bytes for report in small_reports
        )

    print(
        "performance: "
        f"window={small.window_ready_us / 1000:.2f} ms, "
        f"first-pixel={small.first_pixel_us / 1000:.2f} ms, "
        f"navigation-max={small.max_navigation_us / 1000:.2f} ms, "
        f"idle-redraws={max(small.idle_redraws, large.idle_redraws)}, "
        f"small-rss={small.peak_resident_bytes / (1024 * 1024):.2f} MiB, "
        f"large-rss={large.peak_resident_bytes / (1024 * 1024):.2f} MiB, "
        f"large-folder={large.playlist_entries} images, "
        f"cache-stress={cache_stress.decoded_cache_entries} entries/"
        f"{cache_stress.decoded_cache_bytes / (1024 * 1024):.2f} MiB"
    )
    failures = evaluate(
        small,
        large,
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
