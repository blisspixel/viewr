from __future__ import annotations

from pathlib import Path
import tomllib
import unittest


PROJECT_ROOT = Path(__file__).resolve().parents[2]


class VendorPatchTests(unittest.TestCase):
    def test_vendor_crates_are_workspace_test_members(self) -> None:
        manifest = tomllib.loads(
            (PROJECT_ROOT / "Cargo.toml").read_text(encoding="utf-8")
        )
        members = set(manifest["workspace"]["members"])

        self.assertIn("vendor/jxl-color", members)
        self.assertIn("vendor/jxl-render", members)

    def test_avx2_entry_points_require_the_exact_cpu_features(self) -> None:
        for relative_path in (
            "vendor/jxl-color/src/xyb.rs",
            "vendor/jxl-color/src/ycbcr.rs",
        ):
            source = (PROJECT_ROOT / relative_path).read_text(encoding="utf-8")

            self.assertIn("crate::avx2_fma_dispatch(", source)
            self.assertIn('is_x86_feature_detected!("avx2")', source)
            self.assertIn('is_x86_feature_detected!("fma")', source)
            self.assertNotIn('is_x86_feature_detected!("avx")', source)
            self.assertIn('#[target_feature(enable = "avx2")]', source)
            self.assertIn('#[target_feature(enable = "fma")]', source)

    def test_protocol_round_trip_seeds_select_the_intended_harness_arms(self) -> None:
        corpus = PROJECT_ROOT / "fuzz" / "corpus" / "protocol_frames"
        request = (corpus / "valid-request").read_bytes()
        error = (corpus / "valid-error").read_bytes()

        self.assertEqual(request[0] % 9, 1)
        self.assertEqual(error[0] % 9, 3)


if __name__ == "__main__":
    unittest.main()
