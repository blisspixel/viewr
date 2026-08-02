"""Tests for semantic third-party license inventory verification."""

from __future__ import annotations

from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

from scripts.verify_license_inventory import InventoryError, verify_inventories


def _inventory(sections: str, summary: str = "MIT (MIT License): 2 packages") -> str:
    return f"""viewr third-party licenses

## License summary

- {summary}

## License texts and packages

{sections}
"""


def _section(
    license_id: str,
    name: str,
    package: str,
    text: str,
) -> str:
    return f"""### {license_id}
name: {name}
used_by:
  {package}
text:
<<<
{text}
>>>

"""


class LicenseInventoryTests(unittest.TestCase):
    def _verify(self, expected: str, actual: str) -> None:
        with TemporaryDirectory() as directory:
            root = Path(directory)
            committed = root / "committed.txt"
            generated = root / "generated.txt"
            committed.write_bytes(expected.encode("utf-8"))
            generated.write_bytes(actual.encode("utf-8"))
            verify_inventories(committed, generated)

    def test_ignores_line_endings_and_section_order(self) -> None:
        alpha = _section("MIT", "MIT License", "alpha 1.0.0", "MIT text\r\nline 2")
        beta = _section("MIT", "MIT License", "beta 2.0.0", "Other MIT text")
        generated_alpha = _section(
            "MIT",
            "MIT License",
            "alpha 1.0.0",
            "MIT text\nline 2",
        )
        generated_beta = _section(
            "MIT",
            "MIT License",
            "beta 2.0.0",
            "Other MIT text",
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
            "### MIT\nname: MIT License\nused_by:\n  alpha 1.0.0\ntext:\n<<<\n>>>\n"
        )
        with self.assertRaisesRegex(InventoryError, "no license text"):
            self._verify(malformed, malformed)

    def test_rejects_duplicate_license_sections(self) -> None:
        section = _section("MIT", "MIT License", "alpha 1.0.0", "MIT")
        with self.assertRaisesRegex(InventoryError, "multiplicity differs"):
            self._verify(_inventory(section), _inventory(section + section))


if __name__ == "__main__":
    unittest.main()
