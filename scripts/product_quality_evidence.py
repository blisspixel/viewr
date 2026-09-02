"""Validate artifact-bound v0.6 product-quality evidence records."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import hmac
import json
import math
import re
import shutil
import stat
import statistics
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping, Sequence

if __package__:
    from scripts import release_artifact
else:
    import release_artifact


REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
MATRIX_PATH = REPOSITORY_ROOT / "docs" / "PRODUCT-QUALITY.md"
REPOSITORY = "blisspixel/viewr"
WORKFLOW_NAME = "Release artifacts"
EVIDENCE_VERSION = "0.6.1"
EVIDENCE_DIRECTORY = f"v{EVIDENCE_VERSION}"
PLATFORM_TARGETS = {
    "windows": ("x86_64-pc-windows-msvc",),
    "macos": ("aarch64-apple-darwin",),
    "linux": ("x86_64-unknown-linux-gnu",),
}
PERFORMANCE_SESSIONS = {
    "windows": ("windows-100", "windows-150", "windows-200"),
    "macos": ("macos-retina", "macos-external"),
    "linux": ("linux-wayland", "linux-x11", "linux-mesa-software"),
}
PERFORMANCE_HOST_PLATFORMS = {
    "windows": "Windows",
    "macos": "Darwin",
    "linux": "Linux",
}
PERFORMANCE_BUDGETS = {
    "window_ready_ms": 3000,
    "first_pixel_ms": 5000,
    "navigation_ms": 500,
    "idle_redraws": 2,
    "peak_resident_mib": 768,
    "folder_growth_mib": 96,
}
PERFORMANCE_REPORT_KEYS = frozenset(
    {
        "schema",
        "status",
        "executable_sha256",
        "session_label",
        "host_platform",
        "session_evidence",
        "renderer_controls",
        "budgets",
        "summary",
        "runs",
        "failures",
    }
)
PERFORMANCE_SUMMARY_KEYS = frozenset(
    {
        "window_ready_ms",
        "first_pixel_ms",
        "navigation_max_ms",
        "idle_redraws",
        "small_rss_mib",
        "small_rss_floor_mib",
        "large_rss_mib",
        "folder_growth_mib",
        "large_folder_images",
        "cache_stress_entries",
        "cache_stress_bytes",
        "cache_stress_mib",
    }
)
PROBE_REPORT_KEYS = frozenset(
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
ADAPTER_BACKENDS = frozenset({"vulkan", "metal", "dx12", "gl"})
ADAPTER_DEVICE_TYPES = frozenset(
    {"other", "integrated-gpu", "discrete-gpu", "virtual-gpu", "cpu"}
)
PLATFORM_ADAPTER_BACKENDS = {
    "Windows": frozenset({"dx12", "vulkan", "gl"}),
    "Darwin": frozenset({"metal"}),
    "Linux": frozenset({"vulkan", "gl"}),
}
SOFTWARE_ADAPTER_PATTERN = re.compile(
    r"\b(?:llvmpipe|softpipe|swrast|software rasterizer)\b", re.IGNORECASE
)
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
        "mosaic/01-wide.png",
        "mosaic/02-tall.png",
        "mosaic/03-square.png",
        "mosaic/04-wide.png",
        "mosaic/05-tall.png",
        "mosaic/06-panoramic.png",
        "mosaic/07-tall.png",
        "mosaic/08-wide.png",
        "mosaic/09-square.png",
        "mosaic/10-wide.png",
        "mosaic/11-tall.png",
        "mosaic/12-wide.png",
        "mosaic/13-square.png",
        "mosaic/14-portrait.png",
        "mosaic/15-landscape.png",
        "mosaic/16-panoramic.png",
        "mosaic/17-tall.png",
        "mosaic/18-wide.png",
        "mosaic/19-square.png",
        "mosaic/20-portrait.png",
        "mosaic/21-landscape.png",
        "mosaic/22-wide.png",
        "mosaic/23-tall.png",
        "mosaic/24-wide.png",
        "mosaic/25-square.png",
        "mosaic/26-tall.png",
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
APPLICATION_TARGETS = frozenset(
    {
        "x86_64-pc-windows-msvc",
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "x86_64-unknown-linux-gnu",
    }
)
EXPECTED_REMOTE_ARTIFACTS = frozenset(
    {FIXTURE_ARTIFACT, *(f"viewr-{target}" for target in APPLICATION_TARGETS)}
)

# The rehearsal artifacts are currently 7.4 to 10.7 MiB for applications and
# 42 KiB for fixtures. These ceilings leave ample release growth while bounding
# bytes accepted from a compromised or incorrectly selected workflow run.
MAX_APPLICATION_ARTIFACT_BYTES = 64 * 1024 * 1024
MAX_FIXTURE_ARTIFACT_BYTES = 16 * 1024 * 1024
MAX_ARTIFACT_SET_BYTES = 272 * 1024 * 1024
MAX_DOWNLOADED_FILE_BYTES = MAX_APPLICATION_ARTIFACT_BYTES
MAX_SIDECAR_BYTES = 256
MAX_RECORD_BYTES = 1024 * 1024
MAX_PERFORMANCE_REPORT_BYTES = 4 * 1024 * 1024
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
COMMIT_PATTERN = re.compile(r"[0-9a-f]{40}")
SHA256_PATTERN = re.compile(r"[0-9a-f]{64}")
RUN_URL_PATTERN = re.compile(
    r"https://github\.com/blisspixel/viewr/actions/runs/([1-9][0-9]*)"
)
ISSUE_URL_PATTERN = re.compile(
    r"https://github\.com/blisspixel/viewr/issues/[1-9][0-9]*"
    r"(?=$|[\s)\].,;:])"
)
PLACEHOLDER_PATTERN = re.compile(
    r"(?:\bTBD\b|\bTODO\b|\bplaceholder\b|\bnot yet recorded\b)", re.IGNORECASE
)
GENERIC_OBSERVATION_PATTERN = re.compile(
    r"\s*(?:"
    r"(?:pass(?:ed)?|ok(?:ay)?|verified|all good|tested successfully)"
    r"(?:\s+(?:on|for)\s+[^.!]+)?|"
    r"(?:everything\s+)?works?(?: as expected)?|"
    r"(?:observed|verified)?\s*(?:the\s+)?expected behavior"
    r"(?:\s+for\s+PQ-[A-Z]{2}-[0-9]{2})?|"
    r"no (?:issues?|problems?)(?:\s+were)?(?: found| observed)?|"
    r"(?:looks?|behaves?) good"
    r")[.!]?\s*",
    re.IGNORECASE,
)
GENERIC_METADATA_PATTERN = re.compile(
    r"\s*(?:unknown|n/?a|none|default|adapter|graphics adapter|"
    r"synthetic(?:\s+.*)?|test(?:\s+.*)?)\s*",
    re.IGNORECASE,
)
EXCEPTION_SEVERITY_PATTERN = re.compile(
    r"^(low|medium|high|critical) severity:\s+",
    re.IGNORECASE,
)
NUMBER_PATTERN = r"(?:0|[1-9][0-9]*(?:,[0-9]{3})*)(?:\.[0-9]+)?"
INTEGER_PATTERN = r"(?:0|[1-9][0-9]*(?:,[0-9]{3})*)"
PERFORMANCE_PATTERNS = {
    "window ready": re.compile(
        rf"\b(?:window|window[- ]ready)\s*[:=]\s*({NUMBER_PATTERN})\s*ms\b",
        re.IGNORECASE,
    ),
    "first pixel": re.compile(
        rf"\bfirst[- ]pixel\s*[:=]\s*({NUMBER_PATTERN})\s*ms\b",
        re.IGNORECASE,
    ),
    "navigation": re.compile(
        rf"\bnavigation(?:-max)?\s*[:=]\s*({NUMBER_PATTERN})\s*ms\b",
        re.IGNORECASE,
    ),
    "idle redraws": re.compile(
        rf"\bidle[- ]redraws?\s*[:=]\s*({INTEGER_PATTERN})\b",
        re.IGNORECASE,
    ),
    "small RSS": re.compile(
        rf"\bsmall[- ]rss\s*[:=]\s*({NUMBER_PATTERN})\s*MiB\b",
        re.IGNORECASE,
    ),
    "large RSS": re.compile(
        rf"\blarge[- ]rss\s*[:=]\s*({NUMBER_PATTERN})\s*MiB\b",
        re.IGNORECASE,
    ),
    "folder growth": re.compile(
        rf"\bfolder[- ]growth\s*[:=]\s*({NUMBER_PATTERN})\s*MiB\b",
        re.IGNORECASE,
    ),
    "file count": re.compile(
        rf"\b(?:file[- ]count|large[- ]folder)\s*[:=]\s*({INTEGER_PATTERN})"
        r"(?:\s*(?:files|images))?\b",
        re.IGNORECASE,
    ),
    "cache count": re.compile(
        rf"\b(?:cache[- ]count|cache[- ]stress)\s*[:=]\s*({INTEGER_PATTERN})"
        r"\s*(?:entries|items)\b",
        re.IGNORECASE,
    ),
    "cache MiB": re.compile(
        rf"(?:\bcache[- ]mib\s*[:=]\s*({NUMBER_PATTERN})\s*MiB\b|"
        rf"\bcache[- ]stress\s*[:=]\s*{INTEGER_PATTERN}\s*"
        rf"(?:entries|items)\s*/\s*({NUMBER_PATTERN})\s*MiB\b)",
        re.IGNORECASE,
    ),
}
MAIN_SHA256_PATTERN = re.compile(r"(?i:\bviewr[- ]sha256\s*[:=]\s*)([0-9a-f]{64})\b")
DECODER_SHA256_PATTERN = re.compile(
    r"(?i:\bviewr[- ]decode[- ]sha256\s*[:=]\s*)([0-9a-f]{64})\b"
)
AUTOMATED_PREREQUISITE_TOKENS = {
    "PQ-RC-03": (
        "crop_preview_disconnect_copy_and_recovery_priority_are_truthful",
        "dropped_and_panicking_workers_have_stable_terminal_failures",
        "dropped_worker_and_panicking_worker_are_observable_terminal_failures",
    ),
    "PQ-RC-04": ("recovery_blocks_only_actions_that_need_the_unsettled_owner",),
    "PQ-RC-05": (
        "a_missing_library_names_the_package_for_each_supported_distribution",
        "doctor_reports_a_graphics_runtime_that_cannot_present",
        "software-Mesa",
    ),
}
MANUAL_OBSERVATION_TERMS = {
    "PQ-FT-01": ("launch", "Open File", "Open Folder", "local-only"),
    "PQ-FT-02": ("natural order", "window size", "last good"),
    "PQ-FT-03": ("dropped file", "dropped folder"),
    "PQ-FT-04": (
        "Open Folder",
        "sandbox_profiles",
        "selected_file_scan_outcomes_cover_success_and_limits",
    ),
    "PQ-FT-05": ("version", "platform", "license", "privacy", "Escape", "Close"),
    "PQ-FT-06": ("malformed", "unsupported", "Retry", "menus"),
    "PQ-FT-07": ("language", "Spanish", "French", "German", "restart"),
    "PQ-PW-01": ("keyboard", "Home", "End", "Page Up", "Page Down"),
    "PQ-PW-02": ("TIFF", "ICO", "page", "blocked"),
    "PQ-PW-03": ("GIF", "WebP", "APNG", "paused", "frame"),
    "PQ-PW-04": ("fit", "pan", "actual size", "viewport center", "pointer"),
    "PQ-PW-05": ("Tools", "Folder Previews", "Image Information", "overlap"),
    "PQ-PW-06": ("F5", "unsaved", "last good"),
    "PQ-PW-07": ("Save As", "Trash", "Undo"),
    "PQ-PW-08": (
        "full-image",
        "12",
        "corner markers",
        "justified",
        "aspect ratio",
        "fullscreen",
        "Escape",
    ),
    "PQ-PW-09": ("Delete", "fully presented", "serialized", "loading", "Undo"),
    "PQ-AD-01": ("doctor", "worker protocol", "windowing", "graphics"),
    "PQ-AD-02": ("v0.6.1", "immutable", "updater", "security"),
    "PQ-AD-03": ("Update modal", "network", "browser"),
    "PQ-AD-04": ("trust warning", "security controls"),
    "PQ-RC-01": ("malformed", "previous image", "Retry"),
    "PQ-RC-02": ("deleted", "last good", "selected path"),
    "PQ-VS-01": ("empty", "opening", "error", "stable geometry"),
    "PQ-VS-02": ("Light", "Dark", "Console", "pixels unchanged"),
    "PQ-VS-03": ("focus rings", "panels", "display profile"),
}
AUTOMATED_ANCHOR_TOKENS = {
    "PQ-FT-04": (
        "sandbox_profiles",
        "selected_file_scan_outcomes_cover_success_and_limits",
    ),
    "PQ-PW-08": (
        "twelve_landscape_photos_fill_the_screen_in_justified_rows",
        "source_aspects_define_tiles_without_equal_cell_letterboxing",
        "collage_accepts_twelve_photos_and_tiny_views_stay_safe",
        "collage_tile_enlarges_a_complete_small_image_without_changing_its_aspect",
        "no_eviction_insert_rejects_pressure_without_displacing_existing_images",
        "mosaic_loading_announcement_is_stable_until_the_terminal_count",
    ),
    "PQ-VS-03": (
        "profile_refresh_follows_monitor_identity_changes",
        "returning_to_a_prior_monitor_is_a_new_identity",
    ),
}


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


def _require_observation(path: Path, identifier: str, observation: str) -> None:
    _require_value(path, f"{identifier} observation", observation)
    if GENERIC_OBSERVATION_PATTERN.fullmatch(observation):
        raise EvidenceError(
            f"{path}: {identifier} observation must describe concrete evidence"
        )
    terms = MANUAL_OBSERVATION_TERMS.get(identifier)
    if terms is not None:
        missing = [
            term for term in terms if term.casefold() not in observation.casefold()
        ]
        if missing:
            raise EvidenceError(
                f"{path}: {identifier} observation must name concrete results: "
                f"{', '.join(missing)}"
            )
        if (
            re.search(
                r"\b(?:host|session|laptop|desktop|display)\b",
                observation,
                re.IGNORECASE,
            )
            is None
        ):
            raise EvidenceError(
                f"{path}: {identifier} observation must name its host or session"
            )


def _validate_exception(path: Path, identifier: str, observation: str) -> None:
    if not ISSUE_URL_PATTERN.search(observation):
        raise EvidenceError(
            f"{path}: {identifier} approved exception must link its GitHub issue"
        )
    severity_match = EXCEPTION_SEVERITY_PATTERN.search(observation)
    if severity_match is None:
        raise EvidenceError(
            f"{path}: {identifier} approved exception must start with "
            "Low severity: or Medium severity:"
        )
    severity = severity_match.group(1).casefold()
    if severity not in {"low", "medium"}:
        raise EvidenceError(
            f"{path}: {identifier} {severity.title()} severity cannot be approved"
        )


def _require_platform_metadata(
    path: Path, platform: str, fields: Mapping[str, str], results: Mapping[str, Result]
) -> None:
    for field in ("Operating system", "Display scale", "Graphics adapter"):
        if GENERIC_METADATA_PATTERN.fullmatch(fields[field]):
            raise EvidenceError(f"{path}: {field} must identify the tested environment")

    operating_system = fields["Operating system"]
    combined = " ".join(
        (
            operating_system,
            fields["Display scale"],
            fields["Graphics adapter"],
            results["PQ-VS-03"].observation,
        )
    )
    if platform == "windows":
        if (
            re.search(r"\bWindows\s+(?:10|11)\b", operating_system, re.IGNORECASE)
            is None
        ):
            raise EvidenceError(
                f"{path}: Operating system must name the tested Windows 10 or 11 release"
            )
        missing_scales = [
            scale
            for scale in ("100", "150", "200")
            if re.search(rf"(?<![0-9]){scale}\s*%", combined) is None
        ]
        if missing_scales:
            raise EvidenceError(
                f"{path}: Windows evidence must cover display scales "
                f"{', '.join(scale + '%' for scale in missing_scales)}"
            )
        if (
            re.search(r"\b(?:move|moved|moving)\b", combined, re.IGNORECASE) is None
            or re.search(r"\b(?:display|monitor)s?\b", combined, re.IGNORECASE) is None
        ):
            raise EvidenceError(
                f"{path}: Windows evidence must cover a move between displays"
            )
    elif platform == "macos":
        if (
            re.search(
                r"\bmacOS\s+[0-9]+(?:\.[0-9]+){1,2}\b",
                operating_system,
                re.IGNORECASE,
            )
            is None
        ):
            raise EvidenceError(
                f"{path}: Operating system must name the tested macOS version"
            )
        requirements = {
            "Apple Silicon or arm64 host": (
                r"\b(?:Apple Silicon|arm64|aarch64|Apple M[1-9][0-9]*)\b"
            ),
            "Retina display": r"\bRetina\b",
            "external display": r"\bexternal\s+(?:display|monitor)\b",
        }
        for label, pattern in requirements.items():
            if re.search(pattern, combined, re.IGNORECASE) is None:
                raise EvidenceError(f"{path}: macOS evidence must cover a {label}")
        if re.search(r"\b(?:move|moved|moving)\b", combined, re.IGNORECASE) is None:
            raise EvidenceError(
                f"{path}: macOS evidence must cover a live display move"
            )
    else:
        if (
            re.search(r"\bLinux\b", operating_system, re.IGNORECASE) is None
            or re.search(r"\b[0-9]+\.[0-9]+(?:\.[0-9]+)?\b", operating_system) is None
        ):
            raise EvidenceError(
                f"{path}: Operating system must name the tested Linux version"
            )
        requirements = {
            "native Wayland session": r"\bWayland\b",
            "X11 or Xwayland session": r"\b(?:X11|Xwayland)\b",
            "Mesa renderer": r"\bMesa\b",
            "software rendering session": r"\b(?:software|llvmpipe|softpipe)\b",
        }
        for label, pattern in requirements.items():
            if re.search(pattern, combined, re.IGNORECASE) is None:
                raise EvidenceError(f"{path}: Linux evidence must cover a {label}")
        if re.search(r"\b[1-9][0-9]{1,2}\s*%", combined) is None:
            raise EvidenceError(
                f"{path}: Linux evidence must record the tested display scale"
            )


def _validate_automated_prerequisites(
    path: Path, fields: Mapping[str, str], results: Mapping[str, Result]
) -> None:
    run_url = fields["Candidate workflow run"]
    for identifier, tokens in AUTOMATED_PREREQUISITE_TOKENS.items():
        observation = results[identifier].observation
        if run_url not in observation:
            raise EvidenceError(
                f"{path}: {identifier} must cite the candidate workflow run"
            )
        missing = [token for token in tokens if token not in observation]
        if missing:
            raise EvidenceError(
                f"{path}: {identifier} must name automated evidence: "
                f"{', '.join(missing)}"
            )
    for identifier, tokens in AUTOMATED_ANCHOR_TOKENS.items():
        observation = results[identifier].observation
        if run_url not in observation:
            raise EvidenceError(
                f"{path}: {identifier} must cite the candidate workflow run"
            )
        missing = [token for token in tokens if token not in observation]
        if missing:
            raise EvidenceError(
                f"{path}: {identifier} must name automated evidence: "
                f"{', '.join(missing)}"
            )


def _performance_value(match: re.Match[str]) -> float:
    value = next(group for group in match.groups() if group is not None)
    return float(value.replace(",", ""))


def _validate_performance_observation(
    path: Path, platform: str, results: Mapping[str, Result]
) -> dict[str, float]:
    result = results["PQ-VS-04"]
    observation = result.observation
    if result.outcome == "Fail":
        return {}
    for session in PERFORMANCE_SESSIONS[platform]:
        report_path = f"performance/{session}.json"
        if report_path not in observation:
            raise EvidenceError(f"{path}: PQ-VS-04 observation must cite {report_path}")
    values: dict[str, float] = {}
    for label, pattern in PERFORMANCE_PATTERNS.items():
        match = pattern.search(observation)
        if match is None:
            raise EvidenceError(
                f"{path}: PQ-VS-04 observation must record numeric {label}"
            )
        values[label] = _performance_value(match)

    for label, value in values.items():
        if label in {"idle redraws", "folder growth"}:
            continue
        if value <= 0:
            raise EvidenceError(f"{path}: PQ-VS-04 {label} must be greater than zero")
    if values["file count"] < 50_000:
        raise EvidenceError(
            f"{path}: PQ-VS-04 file count must exercise at least 50,000 files"
        )
    if MAIN_SHA256_PATTERN.search(observation) is None:
        raise EvidenceError(
            f"{path}: PQ-VS-04 observation must record the viewr SHA-256"
        )
    if DECODER_SHA256_PATTERN.search(observation) is None:
        raise EvidenceError(
            f"{path}: PQ-VS-04 observation must record the viewr-decode SHA-256"
        )

    if result.outcome != "Pass":
        return values
    limits = {
        "window ready": 3_000,
        "first pixel": 5_000,
        "navigation": 500,
        "idle redraws": 2,
        "large RSS": 768,
    }
    for label, limit in limits.items():
        if values[label] > limit:
            raise EvidenceError(f"{path}: PQ-VS-04 {label} exceeds the {limit:g} limit")
    if values["cache count"] != 4 or values["cache MiB"] != 256:
        raise EvidenceError(
            f"{path}: PQ-VS-04 cache stress must retain exactly 4 entries and 256 MiB"
        )
    if values["folder growth"] > 96:
        raise EvidenceError(f"{path}: PQ-VS-04 folder growth exceeds the 96 MiB limit")
    return values


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
    if version != EVIDENCE_VERSION:
        raise EvidenceError(f"{path}: Version must be {EVIDENCE_VERSION}")
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


def _is_link_like(path: Path) -> bool:
    """Detect symbolic links and Windows reparse points such as junctions."""
    if path.is_symlink():
        return True
    attributes = getattr(path.lstat(), "st_file_attributes", 0)
    reparse_attribute = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)
    return bool(attributes & reparse_attribute)


def _require_directory(
    path: Path, label: str, *, root: Path | None = None, direct_child: bool = False
) -> Path:
    try:
        linked = _is_link_like(path)
    except OSError as error:
        raise EvidenceError(f"{label} is unavailable: {path}") from error
    if linked or not path.is_dir():
        raise EvidenceError(f"{label} is missing, linked, or not a directory: {path}")
    try:
        resolved = path.resolve(strict=True)
        if root is not None:
            resolved.relative_to(root)
            if direct_child and resolved.parent != root:
                raise ValueError
    except (OSError, ValueError) as error:
        raise EvidenceError(
            f"{label} escapes its expected directory: {path}"
        ) from error
    return resolved


def _require_regular_file(
    path: Path,
    label: str,
    *,
    root: Path,
    direct_child: bool = False,
    max_bytes: int | None = None,
) -> Path:
    try:
        linked = _is_link_like(path)
    except OSError as error:
        raise EvidenceError(f"{label} is unavailable: {path}") from error
    if linked or not path.is_file():
        raise EvidenceError(
            f"{label} is missing, linked, or not a regular file: {path}"
        )
    try:
        resolved = path.resolve(strict=True)
        resolved.relative_to(root)
        if direct_child and resolved.parent != root:
            raise ValueError
        size = resolved.stat().st_size
    except (OSError, ValueError) as error:
        raise EvidenceError(
            f"{label} escapes its expected directory: {path}"
        ) from error
    if max_bytes is not None and size > max_bytes:
        raise EvidenceError(
            f"{label} exceeds its {max_bytes}-byte safety limit: {path}"
        )
    return resolved


def _read_bounded_bytes(path: Path, max_bytes: int, label: str) -> bytes:
    try:
        with path.open("rb") as source:
            payload = source.read(max_bytes + 1)
    except OSError as error:
        raise EvidenceError(f"could not read {label}: {path}") from error
    if len(payload) > max_bytes:
        raise EvidenceError(
            f"{label} exceeds its {max_bytes}-byte safety limit: {path}"
        )
    return payload


def parse_record(path: Path, matrix_ids: Sequence[str] | None = None) -> Record:
    """Parse and validate one Markdown evidence record."""
    if path.parent.name != EVIDENCE_DIRECTORY:
        raise EvidenceError(
            f"{path}: evidence records must be stored under {EVIDENCE_DIRECTORY}"
        )
    platform = path.stem.lower()
    if platform not in PLATFORM_TARGETS:
        raise EvidenceError(
            f"{path}: filename must be windows.md, macos.md, or linux.md"
        )
    record_root = _require_directory(path.parent, "evidence record directory")
    record_path = _require_regular_file(
        path,
        "evidence record",
        root=record_root,
        direct_child=True,
        max_bytes=MAX_RECORD_BYTES,
    )
    identifiers = tuple(matrix_ids) if matrix_ids is not None else load_matrix_ids()
    expected_title = f"# Product quality evidence: {platform}"
    try:
        lines = (
            _read_bounded_bytes(record_path, MAX_RECORD_BYTES, "evidence record")
            .decode("utf-8")
            .splitlines()
        )
    except UnicodeDecodeError as error:
        raise EvidenceError(f"{path}: evidence record must be UTF-8") from error
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
            _require_observation(path, cells[0], observation)
            if outcome == "Approved exception":
                _validate_exception(path, cells[0], observation)
            results[cells[0]] = Result(outcome, observation)

    _validate_fields(path, platform, fields)
    missing_results = [
        identifier for identifier in identifiers if identifier not in results
    ]
    if missing_results:
        raise EvidenceError(
            f"{path}: missing matrix results: {', '.join(missing_results)}"
        )
    _require_platform_metadata(path, platform, fields, results)
    _validate_automated_prerequisites(path, fields, results)
    _validate_performance_observation(path, platform, results)
    return Record(path, platform, fields, results)


def validate_gate(directory: Path) -> tuple[Record, ...]:
    """Validate all platform records as one release-gate evidence set."""
    if directory.name != EVIDENCE_DIRECTORY:
        raise EvidenceError(
            f"{directory}: evidence directory must be named {EVIDENCE_DIRECTORY}"
        )
    evidence_root = _require_directory(directory, "evidence directory")
    identifiers = load_matrix_ids()
    records = tuple(
        parse_record(evidence_root / f"{platform}.md", identifiers)
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
    for identifier in (*AUTOMATED_PREREQUISITE_TOKENS, "PQ-VS-04"):
        outcomes = {record.results[identifier].outcome for record in records}
        if outcomes != {"Pass"}:
            raise EvidenceError(
                f"{directory}: {identifier} hard prerequisite must pass"
            )
        if identifier == "PQ-VS-04":
            continue
        observations = {record.results[identifier].observation for record in records}
        if len(observations) != 1:
            raise EvidenceError(
                f"{directory}: every platform must share {identifier} automated evidence"
            )
    return records


def _sha256_file(path: Path, max_bytes: int | None = None) -> str:
    digest = hashlib.sha256()
    total = 0
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            total += len(chunk)
            if max_bytes is not None and total > max_bytes:
                raise EvidenceError(
                    f"file exceeds its {max_bytes}-byte safety limit: {path}"
                )
            digest.update(chunk)
    return digest.hexdigest()


def _require_exact_json_keys(
    value: object, expected: frozenset[str], label: str
) -> dict[str, object]:
    if not isinstance(value, dict) or set(value) != expected:
        raise EvidenceError(f"{label} has an invalid field set")
    return value


def _require_probe_report(value: object, label: str) -> dict[str, object]:
    report = _require_exact_json_keys(value, PROBE_REPORT_KEYS, label)
    validated: dict[str, object] = {}
    boolean_fields = {"idle_window_focused", "idle_pointer_inside"}
    string_fields = {
        "adapter_backend",
        "adapter_name",
        "adapter_device_type",
        "adapter_driver",
    }
    for field, raw in report.items():
        if field in boolean_fields:
            if not isinstance(raw, bool):
                raise EvidenceError(f"{label}.{field} must be boolean")
        elif field in string_fields:
            if not isinstance(raw, str):
                raise EvidenceError(f"{label}.{field} must be a string")
        elif isinstance(raw, bool) or not isinstance(raw, int) or raw < 0:
            raise EvidenceError(f"{label}.{field} must be a non-negative integer")
        validated[field] = raw
    if validated["adapter_backend"] not in ADAPTER_BACKENDS:
        raise EvidenceError(f"{label}.adapter_backend is unsupported")
    if validated["adapter_device_type"] not in ADAPTER_DEVICE_TYPES:
        raise EvidenceError(f"{label}.adapter_device_type is unsupported")
    for field in ("adapter_name", "adapter_driver"):
        text = str(validated[field])
        if len(text) > 256 or any(not character.isprintable() for character in text):
            raise EvidenceError(f"{label}.{field} must be bounded printable text")
        if field == "adapter_name" and not text.strip():
            raise EvidenceError(f"{label}.adapter_name must be nonempty")
    return validated


def _expected_performance_summary(
    small: Sequence[Mapping[str, object]],
    large: Sequence[Mapping[str, object]],
    cache_stress: Mapping[str, object],
) -> dict[str, int | float]:
    retained_reports = (*small, *large, cache_stress)
    small_peak = max(int(report["peak_resident_bytes"]) for report in small)
    small_floor = min(int(report["peak_resident_bytes"]) for report in small)
    large_peak = max(int(report["peak_resident_bytes"]) for report in large)
    return {
        "window_ready_ms": round(
            max(
                int(
                    statistics.median(
                        int(report["window_ready_us"]) for report in small
                    )
                ),
                int(
                    statistics.median(
                        int(report["window_ready_us"]) for report in large
                    )
                ),
            )
            / 1000,
            2,
        ),
        "first_pixel_ms": round(
            max(
                int(
                    statistics.median(int(report["first_pixel_us"]) for report in small)
                ),
                int(
                    statistics.median(int(report["first_pixel_us"]) for report in large)
                ),
            )
            / 1000,
            2,
        ),
        "navigation_max_ms": round(
            max(int(report["max_navigation_us"]) for report in retained_reports) / 1000,
            2,
        ),
        "idle_redraws": max(int(report["idle_redraws"]) for report in retained_reports),
        "small_rss_mib": round(small_peak / (1024 * 1024), 2),
        "small_rss_floor_mib": round(small_floor / (1024 * 1024), 2),
        "large_rss_mib": round(large_peak / (1024 * 1024), 2),
        "folder_growth_mib": round(max(0, large_peak - small_floor) / (1024 * 1024), 2),
        "large_folder_images": max(int(report["playlist_entries"]) for report in large),
        "cache_stress_entries": int(cache_stress["decoded_cache_entries"]),
        "cache_stress_bytes": int(cache_stress["decoded_cache_bytes"]),
        "cache_stress_mib": round(
            int(cache_stress["decoded_cache_bytes"]) / (1024 * 1024), 2
        ),
    }


def _validate_session_evidence(
    path: Path, session: str, platform_name: str, value: object
) -> dict[str, object]:
    if platform_name == "Windows":
        evidence = _require_exact_json_keys(
            value, frozenset({"display_scale_percent"}), f"{path}: session_evidence"
        )
        scale = evidence["display_scale_percent"]
        if isinstance(scale, bool) or not isinstance(scale, int):
            raise EvidenceError(f"{path}: Windows display scale must be an integer")
        expected_scale = int(session.removeprefix("windows-"))
        if scale != expected_scale:
            raise EvidenceError(
                f"{path}: {session} must measure {expected_scale}% display scale"
            )
        return evidence

    if platform_name == "Darwin":
        evidence = _require_exact_json_keys(
            value,
            frozenset(
                {
                    "display_identity_sha256",
                    "display_builtin",
                    "display_retina",
                    "display_scale_percent",
                }
            ),
            f"{path}: session_evidence",
        )
        identity = evidence["display_identity_sha256"]
        if not isinstance(identity, str) or SHA256_PATTERN.fullmatch(identity) is None:
            raise EvidenceError(f"{path}: macOS main display identity is invalid")
        if not isinstance(evidence["display_builtin"], bool) or not isinstance(
            evidence["display_retina"], bool
        ):
            raise EvidenceError(f"{path}: macOS display flags must be boolean")
        scale = evidence["display_scale_percent"]
        if isinstance(scale, bool) or not isinstance(scale, int) or scale <= 0:
            raise EvidenceError(f"{path}: macOS display scale must be positive")
        if session == "macos-retina" and (
            not evidence["display_builtin"]
            or not evidence["display_retina"]
            or scale < 200
        ):
            raise EvidenceError(
                f"{path}: macos-retina must measure the built-in Retina display"
            )
        if session == "macos-external" and evidence["display_builtin"]:
            raise EvidenceError(
                f"{path}: macos-external must measure an external main display"
            )
        return evidence

    linux_fields = {"linux_session"}
    if session == "linux-mesa-software":
        linux_fields.update(
            {
                "opengl_renderer",
                "opengl_vendor",
                "opengl_mesa",
                "opengl_software",
            }
        )
    evidence = _require_exact_json_keys(
        value, frozenset(linux_fields), f"{path}: session_evidence"
    )
    text_fields = ["linux_session"]
    if session == "linux-mesa-software":
        text_fields.extend(("opengl_renderer", "opengl_vendor"))
    for field in text_fields:
        if not isinstance(evidence[field], str) or not str(evidence[field]).strip():
            raise EvidenceError(f"{path}: Linux {field} must be a nonempty string")
    if session == "linux-mesa-software" and (
        not isinstance(evidence["opengl_mesa"], bool)
        or not isinstance(evidence["opengl_software"], bool)
    ):
        raise EvidenceError(f"{path}: Linux OpenGL flags must be boolean")
    measured_session = evidence["linux_session"]
    if session == "linux-wayland" and measured_session != "wayland":
        raise EvidenceError(f"{path}: linux-wayland must measure a Wayland session")
    if session == "linux-x11" and measured_session not in {"x11", "xwayland"}:
        raise EvidenceError(f"{path}: linux-x11 must measure X11 or Xwayland")
    if session == "linux-mesa-software" and (
        not evidence["opengl_mesa"] or not evidence["opengl_software"]
    ):
        raise EvidenceError(
            f"{path}: Mesa software session must measure a Mesa software renderer"
        )
    return evidence


def _validate_performance_report(
    path: Path,
    session: str,
    platform_name: str,
    executable_sha256: Mapping[str, str],
    seen_run_signatures: set[str],
    seen_display_identities: set[str],
) -> dict[str, int | float]:
    if _is_link_like(path) or not path.is_file():
        raise EvidenceError(f"performance report is missing or linked: {path}")
    try:
        payload = json.loads(
            _read_bounded_bytes(
                path, MAX_PERFORMANCE_REPORT_BYTES, "performance report"
            ).decode("utf-8")
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise EvidenceError(f"performance report is invalid JSON: {path}") from error
    report = _require_exact_json_keys(payload, PERFORMANCE_REPORT_KEYS, str(path))
    if report["schema"] != 3:
        raise EvidenceError(f"{path}: unsupported performance report schema")
    if report["status"] != "pass" or report["failures"] != []:
        raise EvidenceError(f"{path}: performance report must pass without failures")
    if report["session_label"] != session:
        raise EvidenceError(f"{path}: session label must be {session}")
    if report["host_platform"] != platform_name:
        raise EvidenceError(f"{path}: host platform must be {platform_name}")
    executables = _require_exact_json_keys(
        report["executable_sha256"],
        frozenset({"viewr", "viewr-decode"}),
        f"{path}: executable_sha256",
    )
    if executables != executable_sha256:
        raise EvidenceError(
            f"{path}: executable SHA-256 values do not match the archive"
        )
    if report["budgets"] != PERFORMANCE_BUDGETS:
        raise EvidenceError(
            f"{path}: performance budgets do not match the release gate"
        )

    renderer = _require_exact_json_keys(
        report["renderer_controls"],
        frozenset({"wgpu_backend", "libgl_always_software"}),
        f"{path}: renderer_controls",
    )
    if not all(isinstance(value, str) for value in renderer.values()):
        raise EvidenceError(f"{path}: renderer controls must be strings")
    if session == "linux-mesa-software" and (
        str(renderer["wgpu_backend"]).casefold() != "gl"
        or renderer["libgl_always_software"] != "1"
    ):
        raise EvidenceError(
            f"{path}: Mesa software session must record WGPU_BACKEND=gl and "
            "LIBGL_ALWAYS_SOFTWARE=1"
        )

    session_evidence = _validate_session_evidence(
        path, session, platform_name, report["session_evidence"]
    )
    if platform_name == "Darwin":
        identity = str(session_evidence["display_identity_sha256"])
        if identity in seen_display_identities:
            raise EvidenceError(
                f"{path}: macOS sessions must measure distinct displays"
            )
        seen_display_identities.add(identity)

    runs = _require_exact_json_keys(
        report["runs"], frozenset({"small", "large", "cache_stress"}), f"{path}: runs"
    )
    if (
        not isinstance(runs["small"], list)
        or not isinstance(runs["large"], list)
        or len(runs["small"]) != 3
        or len(runs["large"]) != 3
    ):
        raise EvidenceError(f"{path}: performance report must retain three timed runs")
    small = [
        _require_probe_report(value, f"{path}: small[{index}]")
        for index, value in enumerate(runs["small"])
    ]
    large = [
        _require_probe_report(value, f"{path}: large[{index}]")
        for index, value in enumerate(runs["large"])
    ]
    cache_stress = _require_probe_report(runs["cache_stress"], f"{path}: cache_stress")
    labeled_reports = [
        *((f"small[{index}]", report) for index, report in enumerate(small)),
        *((f"large[{index}]", report) for index, report in enumerate(large)),
        ("cache_stress", cache_stress),
    ]
    adapter_identities = {
        (
            str(one["adapter_backend"]),
            str(one["adapter_name"]),
            str(one["adapter_device_type"]),
            str(one["adapter_driver"]),
        )
        for _, one in labeled_reports
    }
    if len(adapter_identities) != 1:
        raise EvidenceError(f"{path}: probe runs selected different GPU adapters")
    adapter_backend, adapter_name, adapter_device_type, adapter_driver = next(
        iter(adapter_identities)
    )
    if adapter_backend not in PLATFORM_ADAPTER_BACKENDS[platform_name]:
        raise EvidenceError(
            f"{path}: {adapter_backend} is not a valid {platform_name} adapter backend"
        )
    software_adapter = (
        SOFTWARE_ADAPTER_PATTERN.search(f"{adapter_name} {adapter_driver}") is not None
    )
    if session == "linux-mesa-software":
        if (
            adapter_backend != "gl"
            or adapter_device_type not in {"cpu", "other"}
            or not software_adapter
        ):
            raise EvidenceError(
                f"{path}: Mesa software session must use viewr's actual GL software adapter"
            )
    elif adapter_device_type == "cpu" or software_adapter:
        raise EvidenceError(
            f"{path}: representative hardware session used a software adapter"
        )
    for label, one in labeled_reports:
        for field in (
            "window_ready_us",
            "first_pixel_us",
            "max_navigation_us",
            "peak_resident_bytes",
            "playlist_entries",
        ):
            if int(one[field]) <= 0:
                raise EvidenceError(
                    f"{path}: {label}.{field} must be greater than zero"
                )
    if any(int(one["playlist_entries"]) != 16 for one in small):
        raise EvidenceError(f"{path}: every small run must scan 16 images")
    if any(int(one["playlist_entries"]) != 50_000 for one in large):
        raise EvidenceError(f"{path}: every large run must scan 50,000 images")
    if int(cache_stress["playlist_entries"]) != 8:
        raise EvidenceError(f"{path}: cache stress must scan 8 images")

    decoded_limit = 256 * 1024 * 1024
    for one in (*small, *large):
        if (
            int(one["idle_redraws"]) > 2
            or int(one["decoded_cache_entries"]) > 5
            or int(one["decoded_cache_bytes"]) > decoded_limit
            or int(one["thumbnail_texture_entries"]) > 9
        ):
            raise EvidenceError(
                f"{path}: a retained probe run exceeds a cache or idle limit"
            )
    if int(cache_stress["idle_redraws"]) > 2:
        raise EvidenceError(
            f"{path}: a retained probe run exceeds a cache or idle limit"
        )
    if (
        int(cache_stress["decoded_cache_entries"]) != 4
        or int(cache_stress["decoded_cache_bytes"]) != decoded_limit
        or int(cache_stress["thumbnail_texture_entries"]) > 9
    ):
        raise EvidenceError(f"{path}: cache-stress proof is incomplete")

    expected_summary = _expected_performance_summary(small, large, cache_stress)
    summary = _require_exact_json_keys(
        report["summary"], PERFORMANCE_SUMMARY_KEYS, f"{path}: summary"
    )
    for field, value in summary.items():
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise EvidenceError(f"{path}: summary {field} must be numeric")
        if not math.isfinite(float(value)) or float(value) < 0:
            raise EvidenceError(
                f"{path}: summary {field} must be finite and non-negative"
            )
    if summary != expected_summary:
        raise EvidenceError(f"{path}: performance summary does not match retained runs")
    run_signature = hashlib.sha256(
        json.dumps(runs, separators=(",", ":"), sort_keys=True).encode("utf-8")
    ).hexdigest()
    if run_signature in seen_run_signatures:
        raise EvidenceError(
            f"{path}: required sessions must not reuse copied raw run evidence"
        )
    seen_run_signatures.add(run_signature)
    if (
        float(summary["window_ready_ms"]) > 3000
        or float(summary["first_pixel_ms"]) > 5000
        or float(summary["navigation_max_ms"]) > 500
        or int(summary["idle_redraws"]) > 2
        or float(summary["large_rss_mib"]) > 768
        or float(summary["folder_growth_mib"]) > 96
    ):
        raise EvidenceError(f"{path}: performance summary exceeds a release budget")
    return expected_summary


def _artifact_target(record: Record) -> str:
    name = record.fields["Artifact filename"]
    prefix = f"viewr-{record.fields['Version']}-"
    return name.removeprefix(prefix).removesuffix(".zip")


def _fixture_files(
    fixture_root: Path,
    expected: frozenset[str],
    *,
    artifact_root: Path | None = None,
) -> dict[str, Path]:
    root = _require_directory(
        fixture_root,
        "candidate fixture artifact",
        root=artifact_root,
        direct_child=artifact_root is not None,
    )
    actual: dict[str, Path] = {}
    total_size = 0
    for path in root.rglob("*"):
        if _is_link_like(path):
            raise EvidenceError(
                f"candidate fixture is linked or reparse-backed: {path}"
            )
        if path.is_dir():
            _require_directory(path, "candidate fixture directory", root=root)
            continue
        resolved = _require_regular_file(
            path,
            "candidate fixture",
            root=root,
            max_bytes=MAX_FIXTURE_ARTIFACT_BYTES,
        )
        relative = resolved.relative_to(root).as_posix()
        size = resolved.stat().st_size
        if size == 0:
            raise EvidenceError(f"candidate fixture is empty: {path}")
        total_size += size
        actual[relative] = resolved
    if total_size > MAX_FIXTURE_ARTIFACT_BYTES:
        raise EvidenceError("candidate fixture artifact exceeds its size limit")
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
    files = _fixture_files(fixture_root, FIXTURE_PATHS, artifact_root=artifact_root)
    checksum_path = files[FIXTURE_CHECKSUMS]
    if not hmac.compare_digest(
        _sha256_file(checksum_path, MAX_FIXTURE_ARTIFACT_BYTES),
        expected_manifest_digest,
    ):
        raise EvidenceError(
            "candidate fixture checksum manifest does not match its recorded SHA-256"
        )
    try:
        lines = (
            _read_bounded_bytes(
                checksum_path, MAX_FIXTURE_ARTIFACT_BYTES, "fixture checksum manifest"
            )
            .decode("ascii")
            .splitlines()
        )
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
        if not hmac.compare_digest(
            _sha256_file(files[path], MAX_FIXTURE_ARTIFACT_BYTES), recorded[path]
        ):
            raise EvidenceError(f"candidate fixture checksum mismatch: {path}")


def verify_artifacts(
    records: Sequence[Record], artifact_root: Path
) -> dict[str, dict[str, str]]:
    """Bind every platform record to downloaded archive bytes and sidecars."""
    root = _require_directory(artifact_root, "artifact directory")
    expected_directories = set(EXPECTED_REMOTE_ARTIFACTS)
    actual_directories: set[str] = set()
    for entry in root.iterdir():
        directory = _require_directory(
            entry,
            "candidate artifact download entry",
            root=root,
            direct_child=True,
        )
        actual_directories.add(directory.name)
    if actual_directories != expected_directories:
        missing = sorted(expected_directories - actual_directories)
        unexpected = sorted(actual_directories - expected_directories)
        raise EvidenceError(
            f"candidate artifact download set mismatch; missing={missing}, "
            f"unexpected={unexpected}"
        )

    artifact_paths: dict[str, tuple[Path, Path]] = {}
    downloaded_sizes = 0
    for target in sorted(APPLICATION_TARGETS):
        artifact_directory = _require_directory(
            root / f"viewr-{target}",
            "candidate application artifact directory",
            root=root,
            direct_child=True,
        )
        archive_name = f"viewr-{EVIDENCE_VERSION}-{target}.zip"
        expected_members = {archive_name, f"{archive_name}.sha256"}
        actual_members: set[str] = set()
        for entry in artifact_directory.iterdir():
            member = _require_regular_file(
                entry,
                "candidate application artifact member",
                root=artifact_directory,
                direct_child=True,
            )
            actual_members.add(member.name)
        if actual_members != expected_members:
            missing = sorted(expected_members - actual_members)
            unexpected = sorted(actual_members - expected_members)
            raise EvidenceError(
                f"candidate application artifact set mismatch for {target}; "
                f"missing={missing}, unexpected={unexpected}"
            )
        archive = _require_regular_file(
            artifact_directory / archive_name,
            "candidate application archive",
            root=artifact_directory,
            direct_child=True,
            max_bytes=MAX_DOWNLOADED_FILE_BYTES,
        )
        sidecar = _require_regular_file(
            artifact_directory / f"{archive_name}.sha256",
            "candidate checksum sidecar",
            root=artifact_directory,
            direct_child=True,
            max_bytes=MAX_SIDECAR_BYTES,
        )
        downloaded_sizes += archive.stat().st_size + sidecar.stat().st_size
        artifact_paths[target] = (archive, sidecar)

    fixture_files = _fixture_files(
        root / FIXTURE_ARTIFACT, FIXTURE_PATHS, artifact_root=root
    )
    downloaded_sizes += sum(path.stat().st_size for path in fixture_files.values())
    if downloaded_sizes > MAX_ARTIFACT_SET_BYTES:
        raise EvidenceError(
            "candidate artifact download exceeds its aggregate safety limit"
        )

    downloaded_digests: dict[str, str] = {}
    manifests: dict[str, Mapping[str, object]] = {}
    declared_digests = {
        _artifact_target(record): record.fields["Artifact SHA-256"]
        for record in records
    }
    for target in sorted(APPLICATION_TARGETS):
        archive, sidecar = artifact_paths[target]
        archive_digest = _sha256_file(archive, MAX_DOWNLOADED_FILE_BYTES)
        sidecar_bytes = _read_bounded_bytes(
            sidecar, MAX_SIDECAR_BYTES, "candidate checksum sidecar"
        )
        try:
            sidecar_text = sidecar_bytes.decode("ascii")
        except UnicodeDecodeError as error:
            raise EvidenceError(
                f"candidate checksum sidecar must be ASCII for {target}"
            ) from error
        expected_sidecar = f"{archive_digest}  {archive.name}\n"
        if (
            sidecar_bytes != expected_sidecar.encode("ascii")
            or sidecar_text != expected_sidecar
        ):
            raise EvidenceError(
                f"candidate checksum sidecar does not match archive bytes for {target}"
            )
        if target in declared_digests and not hmac.compare_digest(
            archive_digest, declared_digests[target]
        ):
            raise EvidenceError(
                f"downloaded archive does not match Artifact SHA-256 for {target}"
            )
        with tempfile.TemporaryDirectory(
            prefix="product-quality-archive-snapshot-"
        ) as snapshot_directory:
            snapshot_root = Path(snapshot_directory)
            snapshot_archive = snapshot_root / archive.name
            snapshot_sidecar = snapshot_root / sidecar.name
            try:
                shutil.copyfile(archive, snapshot_archive)
                snapshot_sidecar.write_bytes(sidecar_bytes)
            except OSError as error:
                raise EvidenceError(
                    f"could not snapshot candidate archive for {target}"
                ) from error
            snapshot_digest = _sha256_file(snapshot_archive, MAX_DOWNLOADED_FILE_BYTES)
            snapshot_sidecar_bytes = _read_bounded_bytes(
                snapshot_sidecar,
                MAX_SIDECAR_BYTES,
                "candidate checksum sidecar snapshot",
            )
            if (
                not hmac.compare_digest(archive_digest, snapshot_digest)
                or sidecar_bytes != snapshot_sidecar_bytes
            ):
                raise EvidenceError(
                    f"candidate archive or sidecar changed while snapshotting for {target}"
                )
            try:
                manifest = release_artifact.verify_release_artifact(snapshot_archive)
            except (OSError, release_artifact.ReleaseError) as error:
                raise EvidenceError(
                    f"canonical archive verification failed for {target}: {error}"
                ) from error
            snapshot_digest_after = _sha256_file(
                snapshot_archive, MAX_DOWNLOADED_FILE_BYTES
            )
            snapshot_sidecar_bytes_after = _read_bounded_bytes(
                snapshot_sidecar,
                MAX_SIDECAR_BYTES,
                "candidate checksum sidecar snapshot",
            )
        archive_digest_after = _sha256_file(archive, MAX_DOWNLOADED_FILE_BYTES)
        sidecar_bytes_after = _read_bounded_bytes(
            sidecar, MAX_SIDECAR_BYTES, "candidate checksum sidecar"
        )
        if (
            not hmac.compare_digest(archive_digest, archive_digest_after)
            or sidecar_bytes != sidecar_bytes_after
            or not hmac.compare_digest(snapshot_digest, snapshot_digest_after)
            or snapshot_sidecar_bytes != snapshot_sidecar_bytes_after
        ):
            raise EvidenceError(
                f"candidate archive or sidecar changed during canonical verification for {target}"
            )
        if (
            manifest.get("version") != EVIDENCE_VERSION
            or manifest.get("target") != target
        ):
            raise EvidenceError(
                f"archive manifest version or target is incorrect for {target}"
            )
        downloaded_digests[target] = archive_digest
        manifests[target] = manifest

    executable_digests: dict[str, dict[str, str]] = {}
    for record in records:
        target = _artifact_target(record)
        declared_digest = record.fields["Artifact SHA-256"]
        if not hmac.compare_digest(downloaded_digests[target], declared_digest):
            raise EvidenceError(
                f"{record.path}: downloaded archive does not match Artifact SHA-256"
            )
        manifest = manifests[target]
        suffix = ".exe" if record.platform == "windows" else ""
        executable_paths = {
            "viewr": f"bin/viewr{suffix}",
            "viewr-decode": f"bin/viewr-decode{suffix}",
        }
        files = manifest.get("files")
        if not isinstance(files, list):
            raise EvidenceError(f"{record.path}: archive manifest files are invalid")
        platform_digests: dict[str, str] = {}
        for executable_name, executable_path in executable_paths.items():
            entries = [
                entry
                for entry in files
                if isinstance(entry, dict) and entry.get("path") == executable_path
            ]
            if len(entries) != 1 or not isinstance(entries[0].get("sha256"), str):
                raise EvidenceError(
                    f"{record.path}: archive manifest does not identify {executable_path}"
                )
            digest = entries[0]["sha256"]
            if SHA256_PATTERN.fullmatch(digest) is None:
                raise EvidenceError(
                    f"{record.path}: archive {executable_name} SHA-256 is invalid"
                )
            platform_digests[executable_name] = digest
        executable_digests[record.platform] = platform_digests

    verify_fixtures(root, records[0].fields["Fixture manifest SHA-256"])
    return executable_digests


def verify_performance_reports(
    directory: Path,
    records: Sequence[Record],
    executable_digests: Mapping[str, Mapping[str, str]],
) -> None:
    """Validate every required session report and its archive binary binding."""

    evidence_root = _require_directory(directory, "evidence directory")
    report_root = _require_directory(
        evidence_root / "performance",
        "performance report directory",
        root=evidence_root,
        direct_child=True,
    )
    expected_names = {
        f"{session}.json"
        for sessions in PERFORMANCE_SESSIONS.values()
        for session in sessions
    }
    actual_names: set[str] = set()
    for entry in report_root.iterdir():
        report = _require_regular_file(
            entry,
            "performance report",
            root=report_root,
            direct_child=True,
            max_bytes=MAX_PERFORMANCE_REPORT_BYTES,
        )
        actual_names.add(report.name)
    if actual_names != expected_names:
        missing = sorted(expected_names - actual_names)
        unexpected = sorted(actual_names - expected_names)
        raise EvidenceError(
            f"performance report set mismatch; missing={missing}, unexpected={unexpected}"
        )

    summaries: dict[str, list[dict[str, int | float]]] = {}
    seen_run_signatures: set[str] = set()
    for record in records:
        platform_summaries: list[dict[str, int | float]] = []
        platform_digests = executable_digests[record.platform]
        observation = record.results["PQ-VS-04"].observation
        observed_digests = {
            "viewr": MAIN_SHA256_PATTERN.search(observation),
            "viewr-decode": DECODER_SHA256_PATTERN.search(observation),
        }
        for name, match in observed_digests.items():
            if match is None or match.group(1) != platform_digests[name]:
                raise EvidenceError(
                    f"{record.path}: PQ-VS-04 {name} SHA-256 does not match the archive"
                )
        seen_display_identities: set[str] = set()
        for session in PERFORMANCE_SESSIONS[record.platform]:
            platform_summaries.append(
                _validate_performance_report(
                    report_root / f"{session}.json",
                    session,
                    PERFORMANCE_HOST_PLATFORMS[record.platform],
                    platform_digests,
                    seen_run_signatures,
                    seen_display_identities,
                )
            )
        summaries[record.platform] = platform_summaries

    for record in records:
        rollup = {
            "window ready": max(
                float(summary["window_ready_ms"])
                for summary in summaries[record.platform]
            ),
            "first pixel": max(
                float(summary["first_pixel_ms"])
                for summary in summaries[record.platform]
            ),
            "navigation": max(
                float(summary["navigation_max_ms"])
                for summary in summaries[record.platform]
            ),
            "idle redraws": max(
                int(summary["idle_redraws"]) for summary in summaries[record.platform]
            ),
            "small RSS": max(
                float(summary["small_rss_mib"])
                for summary in summaries[record.platform]
            ),
            "large RSS": max(
                float(summary["large_rss_mib"])
                for summary in summaries[record.platform]
            ),
            "folder growth": max(
                float(summary["folder_growth_mib"])
                for summary in summaries[record.platform]
            ),
            "file count": min(
                int(summary["large_folder_images"])
                for summary in summaries[record.platform]
            ),
            "cache count": min(
                int(summary["cache_stress_entries"])
                for summary in summaries[record.platform]
            ),
            "cache MiB": min(
                float(summary["cache_stress_mib"])
                for summary in summaries[record.platform]
            ),
        }
        recorded = _validate_performance_observation(
            record.path, record.platform, record.results
        )
        if recorded != rollup:
            raise EvidenceError(
                f"{record.path}: PQ-VS-04 rollup does not match session reports"
            )


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
    executable_digests = verify_artifacts(records, artifact_root)
    verify_performance_reports(directory, records, executable_digests)
    return records


def _verify_remote_artifact_metadata(run_id: int) -> None:
    """Reject incomplete, expired, duplicate, or oversized remote artifact sets."""
    endpoint = f"repos/{REPOSITORY}/actions/runs/{run_id}/artifacts?per_page=100"
    try:
        completed = subprocess.run(
            ["gh", "api", "--method", "GET", endpoint],
            check=False,
            capture_output=True,
            text=True,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise EvidenceError(
            f"could not inspect candidate workflow artifacts for run {run_id}"
        ) from error
    if completed.returncode != 0:
        detail = completed.stderr.strip() or "GitHub CLI returned no detail"
        raise EvidenceError(
            f"could not inspect candidate workflow artifacts for run {run_id}: {detail}"
        )
    try:
        payload = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise EvidenceError(
            f"candidate workflow artifacts for run {run_id} returned invalid JSON"
        ) from error
    if not isinstance(payload, dict):
        raise EvidenceError("candidate workflow artifact metadata is invalid")
    artifacts = payload.get("artifacts")
    total_count = payload.get("total_count")
    if (
        isinstance(total_count, bool)
        or not isinstance(total_count, int)
        or total_count != len(EXPECTED_REMOTE_ARTIFACTS)
        or not isinstance(artifacts, list)
        or len(artifacts) != len(EXPECTED_REMOTE_ARTIFACTS)
    ):
        raise EvidenceError("candidate workflow artifact count is invalid")

    names: set[str] = set()
    aggregate_size = 0
    for artifact in artifacts:
        if not isinstance(artifact, dict):
            raise EvidenceError("candidate workflow artifact metadata is invalid")
        name = artifact.get("name")
        size = artifact.get("size_in_bytes")
        expired = artifact.get("expired")
        if not isinstance(name, str) or name in names:
            raise EvidenceError("candidate workflow artifact names must be unique")
        if name not in EXPECTED_REMOTE_ARTIFACTS:
            raise EvidenceError(f"unexpected candidate workflow artifact: {name}")
        if expired is not False:
            raise EvidenceError(f"candidate workflow artifact is expired: {name}")
        if isinstance(size, bool) or not isinstance(size, int) or size < 0:
            raise EvidenceError(f"candidate workflow artifact size is invalid: {name}")
        limit = (
            MAX_FIXTURE_ARTIFACT_BYTES
            if name == FIXTURE_ARTIFACT
            else MAX_APPLICATION_ARTIFACT_BYTES
        )
        if size > limit:
            raise EvidenceError(
                f"candidate workflow artifact exceeds its size limit: {name}"
            )
        names.add(name)
        aggregate_size += size
    if names != EXPECTED_REMOTE_ARTIFACTS:
        missing = sorted(EXPECTED_REMOTE_ARTIFACTS - names)
        raise EvidenceError(
            f"candidate workflow artifact names are incomplete: {missing}"
        )
    if aggregate_size > MAX_ARTIFACT_SET_BYTES:
        raise EvidenceError("candidate workflow artifact set exceeds its size limit")


def _download_run_artifacts(run_id: int, destination: Path) -> None:
    root = _require_directory(destination, "fresh artifact download destination")
    if any(root.iterdir()):
        raise EvidenceError("fresh artifact download destination must be empty")
    _verify_remote_artifact_metadata(run_id)
    try:
        completed = subprocess.run(
            [
                "gh",
                "run",
                "download",
                str(run_id),
                "--repo",
                REPOSITORY,
                "--dir",
                str(root),
            ],
            check=False,
            capture_output=True,
            text=True,
            timeout=300,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise EvidenceError(
            f"could not download candidate workflow artifacts for run {run_id}"
        ) from error
    if completed.returncode != 0:
        detail = completed.stderr.strip() or "GitHub CLI returned no detail"
        raise EvidenceError(
            f"could not download candidate workflow artifacts for run {run_id}: {detail}"
        )


def validate_remote_candidate_gate(directory: Path) -> tuple[Record, ...]:
    """Download the recorded run freshly, then validate all remote-bound evidence."""

    records = validate_gate(directory)
    run_url = records[0].fields["Candidate workflow run"]
    match = RUN_URL_PATTERN.fullmatch(run_url)
    if match is None:
        raise EvidenceError("candidate workflow run URL is invalid")
    run_id = int(match.group(1))
    metadata = _load_run_metadata(run_id)
    verify_run(records, metadata)
    with tempfile.TemporaryDirectory(prefix="product-quality-artifacts-") as temporary:
        artifact_root = Path(temporary)
        _download_run_artifacts(run_id, artifact_root)
        executable_digests = verify_artifacts(records, artifact_root)
        verify_performance_reports(directory, records, executable_digests)
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
            records = validate_remote_candidate_gate(args.directory)
    except (EvidenceError, OSError) as error:
        print(f"product-quality evidence failed: {error}", file=sys.stderr)
        return 1

    platforms = ", ".join(record.platform for record in records)
    print(f"product-quality evidence passed: {platforms}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
