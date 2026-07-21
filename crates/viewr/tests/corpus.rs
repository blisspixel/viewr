//! Golden decode and edit feature tests. These generate a small corpus across
//! every core-encodable format and size at test time (nothing is committed),
//! then confirm that every file opens to the expected dimensions and that the
//! crop and save-as pipeline round-trips.
//!
//! Temp dirs use [`viewr::ephemeral::TempWorkspace`] so debris is always removed,
//! including on panic.

use std::fs;
use std::path::Path;

use image::{DynamicImage, Rgb, RgbImage};
use viewr::decode::DecodedImage;
use viewr::edit::{self, Rect};
use viewr::ephemeral::TempWorkspace;

const FORMATS: &[&str] = &["png", "jpg", "bmp", "tiff", "gif", "qoi", "tga", "ppm"];
const SIZES: &[(u32, u32)] = &[(16, 16), (640, 426), (1920, 1080)];

fn gradient(width: u32, height: u32) -> RgbImage {
    let mut img = RgbImage::new(width, height);
    for (x, y, pixel) in img.enumerate_pixels_mut() {
        *pixel = Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8]);
    }
    img
}

fn save_one(img: &RgbImage, path: &Path) {
    if path.extension().is_some_and(|e| e == "gif") {
        DynamicImage::ImageRgb8(img.clone())
            .into_rgba8()
            .save(path)
            .unwrap();
    } else {
        img.save(path).unwrap();
    }
}

#[test]
fn every_format_and_size_decodes_to_expected_dimensions() {
    let ws = TempWorkspace::new("corpus_decode").unwrap();
    for &(w, h) in SIZES {
        let img = gradient(w, h);
        for fmt in FORMATS {
            let path = ws.path().join(format!("g_{w}x{h}.{fmt}"));
            save_one(&img, &path);

            let decoded = DecodedImage::load(&path)
                .unwrap_or_else(|e| panic!("failed to open {fmt} {w}x{h}: {e}"));
            assert_eq!(decoded.width, w, "width for {fmt} {w}x{h}");
            assert_eq!(decoded.height, h, "height for {fmt} {w}x{h}");
            assert_eq!(
                decoded.rgba.len(),
                (w * h * 4) as usize,
                "buffer length for {fmt} {w}x{h}"
            );
        }
    }
}

#[test]
fn opening_a_non_image_is_a_clean_error_not_a_panic() {
    let ws = TempWorkspace::new("corpus_notimage").unwrap();
    let path = ws.path().join("notes.txt");
    fs::write(&path, b"this is not an image").unwrap();
    assert!(DecodedImage::load(&path).is_err());
}

/// Hostile / truncated inputs must fail as `Error::Decode`, never panic.
#[test]
fn adversarial_bytes_do_not_panic() {
    let ws = TempWorkspace::new("corpus_hostile").unwrap();
    let cases: &[(&str, &[u8])] = &[
        ("empty.png", b""),
        ("trunc.jpg", b"\xff\xd8\xff"),
        ("trunc.png", b"\x89PNG\r\n\x1a\n"),
        ("garbage.webp", b"RIFF\x00\x00\x00\x00WEBP"),
        ("bad.svg", b"<svg"),
        ("zeros.jxl", &[0u8; 64]),
        ("huge_header.jpg", &[0xff, 0xd8, 0xff, 0xe0, 0xff, 0xff]),
        (
            "polyglot.gif",
            b"GIF89a\x01\x00\x01\x00\x00\x00\x00\x21\xff",
        ),
        ("not_tiff.tif", b"II*\x00not-a-tiff"),
        // Unclosed root: not a complete SVG document.
        ("trunc_svg.svg", b"<svg xmlns='http://www.w3.org/2000/svg'"),
    ];
    for (name, bytes) in cases {
        let path = ws.path().join(name);
        fs::write(&path, bytes).unwrap();
        let result = std::panic::catch_unwind(|| DecodedImage::load(&path));
        assert!(result.is_ok(), "decode panicked on hostile sample {name}");
        assert!(
            result.unwrap().is_err(),
            "hostile sample {name} should not decode as a valid image"
        );
    }
}

#[test]
fn crop_then_save_as_roundtrips_across_formats() {
    let ws = TempWorkspace::new("corpus_cropsave").unwrap();
    let source = ws.path().join("source.png");
    save_one(&gradient(200, 120), &source);

    let original = DecodedImage::load(&source).unwrap();
    let cropped = edit::crop(
        &original,
        Rect {
            x: 40,
            y: 20,
            width: 100,
            height: 60,
        },
    );
    assert_eq!((cropped.width, cropped.height), (100, 60));

    // Save the crop into several formats, reload each, confirm dimensions hold.
    for fmt in ["png", "jpg", "bmp", "tiff"] {
        let out = ws.path().join(format!("crop.{fmt}"));
        edit::save(&cropped, &out).unwrap_or_else(|e| panic!("save {fmt}: {e}"));
        let reloaded = DecodedImage::load(&out).unwrap();
        assert_eq!((reloaded.width, reloaded.height), (100, 60), "format {fmt}");
    }
}

#[test]
fn lossless_save_preserves_pixels_exactly() {
    let ws = TempWorkspace::new("corpus_lossless").unwrap();
    let original = DecodedImage {
        rgba: gradient(64, 48)
            .into_raw()
            .chunks(3)
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        width: 64,
        height: 48,
    };
    let out = ws.path().join("roundtrip.png");
    edit::save(&original, &out).unwrap();
    let reloaded = DecodedImage::load(&out).unwrap();
    assert_eq!(
        reloaded.rgba, original.rgba,
        "PNG round-trip must be lossless"
    );
}

#[test]
fn svg_decodes_to_expected_dimensions() {
    let ws = TempWorkspace::new("corpus_svg").unwrap();
    let svg_content = r#"<svg width="200" height="150" xmlns="http://www.w3.org/2000/svg">
        <rect width="200" height="150" fill="red" />
    </svg>"#;
    let path = ws.path().join("test.svg");
    fs::write(&path, svg_content).unwrap();

    let decoded = DecodedImage::load(&path).expect("failed to open SVG");
    assert_eq!(decoded.width, 200, "SVG width");
    assert_eq!(decoded.height, 150, "SVG height");
    assert_eq!(decoded.rgba.len(), (200 * 150 * 4), "SVG buffer size");
}
