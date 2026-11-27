"""Tests for the Linux release glibc floor measurement."""

from __future__ import annotations

import unittest

from scripts.linux_glibc_floor import (
    format_glibc_version,
    highest_glibc_version,
    parse_glibc_version,
)


class LinuxGlibcFloorTests(unittest.TestCase):
    def test_highest_symbol_version_wins(self) -> None:
        text = "\n".join(
            [
                "0000000000000000 DF *UND* 0000000000000000 GLIBC_2.34  atan2",
                "0000000000000000 DF *UND* 0000000000000000 GLIBC_2.39  posix_spawn",
                "0000000000000000 DF *UND* 0000000000000000 GLIBC_2.17  clock_gettime",
            ]
        )

        self.assertEqual(highest_glibc_version(text), (2, 39))
        self.assertEqual(format_glibc_version((2, 39)), "2.39")

    def test_a_newer_minor_is_above_the_floor(self) -> None:
        text = "DF *UND* GLIBC_2.43 atan2f"

        self.assertGreater(highest_glibc_version(text), parse_glibc_version("2.39"))

    def test_missing_symbols_are_not_a_floor(self) -> None:
        self.assertIsNone(highest_glibc_version("no dynamic symbols"))

    def test_parse_rejects_a_word(self) -> None:
        with self.assertRaises(ValueError):
            parse_glibc_version("glibc")


if __name__ == "__main__":
    unittest.main()
