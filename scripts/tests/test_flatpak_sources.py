"""Contracts for the generated offline Flatpak Cargo source map."""

from __future__ import annotations

import json
from pathlib import Path
import unittest


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
SOURCE_MAP = REPOSITORY_ROOT / "packaging" / "flatpak" / "cargo-sources.json"


class FlatpakCargoSourceTests(unittest.TestCase):
    """Keep Cargo's replacement source pointed at Flatpak's archive location."""

    def test_vendored_source_directory_matches_archive_destinations(self) -> None:
        sources = json.loads(SOURCE_MAP.read_text(encoding="utf-8"))
        archives = [source for source in sources if source["type"] == "archive"]
        self.assertGreater(len(archives), 0)
        self.assertTrue(
            all(source["dest"].startswith("cargo/vendor/") for source in archives)
        )

        config = sources[-1]
        self.assertEqual(config["type"], "inline")
        self.assertEqual(config["dest"], "cargo")
        self.assertEqual(config["dest-filename"], "config.toml")
        self.assertIn('directory = "cargo/vendor"', config["contents"])
        self.assertNotIn('directory = "vendor"', config["contents"])


if __name__ == "__main__":
    unittest.main()
