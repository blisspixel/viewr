//! Regenerate the small, successful pure-Rust decoder seeds under `fuzz/`.
//!
//! Usage: `cargo run --example gen_fuzz_seeds -- [out_dir]`.

use std::io::Cursor;
use std::path::Path;

use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use viewr::fs::CORE_EXTENSIONS;

const ENCODED_FORMATS: &[(&str, ImageFormat)] = &[
    ("jpg", ImageFormat::Jpeg),
    ("png", ImageFormat::Png),
    ("gif", ImageFormat::Gif),
    ("webp", ImageFormat::WebP),
    ("bmp", ImageFormat::Bmp),
    ("tiff", ImageFormat::Tiff),
    ("ico", ImageFormat::Ico),
    ("qoi", ImageFormat::Qoi),
    ("tga", ImageFormat::Tga),
    ("pnm", ImageFormat::Pnm),
    ("hdr", ImageFormat::Hdr),
    ("exr", ImageFormat::OpenExr),
    ("ff", ImageFormat::Farbfeld),
];

// A 2-by-2 lossless codestream produced by libjxl from `synthetic_image`'s
// RGB pixels. Embedding the stable payload keeps corpus regeneration independent
// of an optional external JPEG XL encoder.
const JXL_PAYLOAD: &[u8] = &[
    0xff, 0x0a, 0x08, 0x10, 0x10, 0x09, 0x08, 0x04, 0x01, 0x00, 0x74, 0x00, 0x4b, 0x12, 0xc5, 0x82,
    0x85, 0x24, 0x96, 0x81, 0x8d, 0x03, 0x00, 0x00, 0x03, 0xd3, 0x9e, 0x0f, 0x80, 0xbd, 0x01, 0x15,
    0x59, 0x60, 0x7d, 0xbb, 0xf9, 0xe9, 0xaf, 0x93, 0xf8,
];

fn selector(extension: &str) -> anyhow::Result<u8> {
    let index = CORE_EXTENSIONS
        .iter()
        .position(|candidate| *candidate == extension)
        .ok_or_else(|| anyhow::anyhow!("{extension} is not a declared core extension"))?;
    u8::try_from(index).map_err(Into::into)
}

fn write_seed(directory: &Path, extension: &str, payload: &[u8]) -> anyhow::Result<()> {
    let mut seed = Vec::with_capacity(payload.len() + 1);
    seed.push(selector(extension)?);
    seed.extend_from_slice(payload);
    std::fs::write(directory.join(format!("valid-{extension}")), seed)?;
    Ok(())
}

fn synthetic_image() -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(2, 2, |x, y| {
        Rgb([
            u8::try_from(x * 127).unwrap_or(255),
            u8::try_from(y * 127).unwrap_or(255),
            63,
        ])
    }))
}

fn synthetic_float_image() -> DynamicImage {
    DynamicImage::ImageRgb32F(image::Rgb32FImage::from_fn(2, 2, |x, y| {
        Rgb([x as f32 * 0.5, y as f32 * 0.5, 0.25])
    }))
}

fn synthetic_rgba_image() -> DynamicImage {
    DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        2,
        2,
        image::Rgba([127, 63, 31, 255]),
    ))
}

fn synthetic_u16_image() -> DynamicImage {
    DynamicImage::ImageRgba16(image::ImageBuffer::from_pixel(
        2,
        2,
        image::Rgba([u16::MAX, 0, 0x3fff, u16::MAX]),
    ))
}

fn dxt1_dds() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(136);
    bytes.extend_from_slice(b"DDS ");
    for value in [
        124_u32,
        0x0008_1007,
        4,
        4,
        8,
        0,
        0, // Header and dimensions.
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&[0_u8; 44]); // Reserved header fields.
    bytes.extend_from_slice(&32_u32.to_le_bytes()); // Pixel format size.
    bytes.extend_from_slice(&4_u32.to_le_bytes()); // DDPF_FOURCC.
    bytes.extend_from_slice(b"DXT1");
    bytes.extend_from_slice(&[0_u8; 20]); // RGB bit count and masks.
    bytes.extend_from_slice(&0x1000_u32.to_le_bytes()); // DDSCAPS_TEXTURE.
    bytes.extend_from_slice(&[0_u8; 16]); // Remaining caps and reserved field.
    bytes.extend_from_slice(&[0x00, 0xf8, 0xe0, 0x07, 0, 0, 0, 0]); // One red DXT1 block.
    debug_assert_eq!(bytes.len(), 136);
    bytes
}

fn main() -> anyhow::Result<()> {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "fuzz/corpus/decode_memory".to_owned());
    let directory = Path::new(&output);
    std::fs::create_dir_all(directory)?;

    for &(extension, format) in ENCODED_FORMATS {
        let image = match format {
            ImageFormat::Hdr | ImageFormat::OpenExr => synthetic_float_image(),
            ImageFormat::Farbfeld => synthetic_u16_image(),
            ImageFormat::Ico => synthetic_rgba_image(),
            _ => synthetic_image(),
        };
        let mut encoded = Cursor::new(Vec::new());
        image.write_to(&mut encoded, format)?;
        write_seed(directory, extension, encoded.get_ref())?;
    }

    write_seed(
        directory,
        "svg",
        br##"<svg width="2" height="2" xmlns="http://www.w3.org/2000/svg"><rect width="2" height="2" fill="#3f7f00"/></svg>"##,
    )?;
    write_seed(directory, "dds", &dxt1_dds())?;
    write_seed(directory, "jxl", JXL_PAYLOAD)?;
    println!("wrote {} deterministic seeds", ENCODED_FORMATS.len() + 3);
    Ok(())
}
