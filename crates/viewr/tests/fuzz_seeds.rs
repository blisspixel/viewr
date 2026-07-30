//! Stable validation for every successful decoder seed consumed by cargo-fuzz.

use std::collections::BTreeSet;

use viewr::decode::DecodedImage;
use viewr::fs::CORE_EXTENSIONS;

const DISTINCT_CORE_DECODERS: &[&str] = &[
    "jpg", "png", "gif", "webp", "bmp", "tiff", "ico", "qoi", "tga", "pnm", "hdr", "exr", "ff",
    "dds", "jxl", "svg",
];

fn decoder_seed_directory() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/decode_memory")
}

#[test]
fn malformed_fuzz_seeds_select_every_declared_core_extension() {
    let mut covered = BTreeSet::new();

    for entry in std::fs::read_dir(decoder_seed_directory()).expect("read decoder seed directory") {
        let entry = entry.expect("read decoder seed entry");
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("malformed-") && !name.starts_with("truncated-") {
            continue;
        }
        let bytes = std::fs::read(entry.path()).expect("read decoder seed");
        let (&selector, _) = bytes.split_first().expect("seed has selector");
        covered.insert(CORE_EXTENSIONS[usize::from(selector) % CORE_EXTENSIONS.len()]);
    }

    assert_eq!(
        covered,
        CORE_EXTENSIONS.iter().copied().collect(),
        "add a malformed seed whenever a core extension is added"
    );
}

#[test]
fn successful_fuzz_seeds_reach_every_distinct_core_decoder() {
    let mut covered = BTreeSet::new();

    for entry in std::fs::read_dir(decoder_seed_directory()).expect("read decoder seed directory") {
        let entry = entry.expect("read decoder seed entry");
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("valid-") {
            continue;
        }
        let bytes = std::fs::read(entry.path()).expect("read decoder seed");
        let (&selector, payload) = bytes.split_first().expect("seed has selector");
        let extension = CORE_EXTENSIONS[usize::from(selector) % CORE_EXTENSIONS.len()];
        assert_eq!(name, format!("valid-{extension}"));

        let decoded = DecodedImage::load_from_memory_with_extension(payload, extension)
            .unwrap_or_else(|error| panic!("{name} did not reach a successful decode: {error}"));
        assert!(decoded.width > 0 && decoded.height > 0, "{name}");
        covered.insert(extension);
    }

    assert_eq!(
        covered,
        DISTINCT_CORE_DECODERS.iter().copied().collect(),
        "add a successful seed whenever a distinct core decoder is added"
    );
}

#[test]
fn unused_jxl_lf_level_regression_is_rejected_without_panicking() {
    let bytes = std::fs::read(decoder_seed_directory().join("regression-jxl-unused-lf-level"))
        .expect("read JXL regression seed");
    let (&selector, payload) = bytes.split_first().expect("seed has selector");
    let extension = CORE_EXTENSIONS[usize::from(selector) % CORE_EXTENSIONS.len()];
    assert_eq!(extension, "jxl");

    for result in [
        DecodedImage::load_from_memory_with_extension(payload, extension),
        DecodedImage::load_from_memory(payload),
    ] {
        assert!(result.is_err(), "malformed JXL input was accepted");
    }
}
