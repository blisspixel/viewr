"""Tests for semantic third-party license inventory verification."""

from __future__ import annotations

from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

from scripts.verify_license_inventory import InventoryError, verify_inventories


def _inventory(sections: str, summary: str = "MIT License: 2 packages") -> str:
    return f"""<!doctype html>
<html><body><main>
<h2>License summary</h2><ul><li>{summary}</li></ul>
<h2>License texts and packages</h2>
{sections}
</main></body></html>
"""


def _section(
    license_id: str,
    name: str,
    package: str,
    text: str,
    href: str = "https://crates.io/example",
) -> str:
    return f"""<section>
<h3 id=\"{license_id}\">{name}</h3>
<div class=\"packages\"><a href=\"{href}\">{package}</a></div>
<pre>{text}</pre>
</section>"""


class LicenseInventoryTests(unittest.TestCase):
    def _verify(self, expected: str, actual: str) -> None:
        with TemporaryDirectory() as directory:
            root = Path(directory)
            committed = root / "committed.html"
            generated = root / "generated.html"
            committed.write_bytes(expected.encode("utf-8"))
            generated.write_bytes(actual.encode("utf-8"))
            verify_inventories(committed, generated)

    def test_ignores_repository_links_line_endings_and_section_order(self) -> None:
        alpha = _section("MIT", "MIT License", "alpha 1.0.0", "MIT text\r\nline 2")
        beta = _section("MIT", "MIT License", "beta 2.0.0", "Other MIT text")
        generated_alpha = _section(
            "MIT",
            "MIT License",
            "alpha 1.0.0",
            "MIT text\nline 2",
            "https://example.invalid/moved",
        )
        generated_beta = _section(
            "MIT",
            "MIT License",
            "beta 2.0.0",
            "Other MIT text",
            "https://example.invalid/also-moved",
        )
        self._verify(
            _inventory(alpha + beta), _inventory(generated_beta + generated_alpha)
        )

    def test_rejects_changed_package_or_version(self) -> None:
        expected = _inventory(_section("MIT", "MIT License", "alpha 1.0.0", "MIT"))
        actual = _inventory(_section("MIT", "MIT License", "alpha 1.0.1", "MIT"))
        with self.assertRaisesRegex(InventoryError, "inventory drift"):
            self._verify(expected, actual)

    def test_rejects_changed_license_text(self) -> None:
        expected = _inventory(_section("MIT", "MIT License", "alpha 1.0.0", "MIT"))
        actual = _inventory(
            _section("MIT", "MIT License", "alpha 1.0.0", "changed text")
        )
        with self.assertRaisesRegex(InventoryError, "inventory drift"):
            self._verify(expected, actual)

    def test_rejects_missing_license_text(self) -> None:
        malformed = _inventory(
            '<section><h3 id="MIT">MIT License</h3>'
            '<div class="packages"><a>alpha 1.0.0</a></div></section>'
        )
        with self.assertRaisesRegex(InventoryError, "no license text"):
            self._verify(malformed, malformed)

    def test_rejects_duplicate_license_sections(self) -> None:
        section = _section("MIT", "MIT License", "alpha 1.0.0", "MIT")
        with self.assertRaisesRegex(InventoryError, "multiplicity differs"):
            self._verify(_inventory(section), _inventory(section + section))


if __name__ == "__main__":
    unittest.main()
