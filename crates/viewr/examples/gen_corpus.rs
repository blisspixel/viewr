//! Generate a test image corpus across formats and sizes, including a very large
//! image, for manual testing and benchmarking.
//!
//! Usage: `cargo run --example gen_corpus -- [out_dir]` (defaults to `corpus`).

// u/v (texture coords) and r/g/b (channels) are idiomatic here, and the gradient
// values are non-negative and clamped before the u8 cast, so sign loss cannot
// occur. These allows are scoped to this pixel-generation tool.
#![allow(clippy::cast_sign_loss, clippy::many_single_char_names)]

use std::path::Path;

use image::{DynamicImage, Rgb, RgbImage};

/// Draw a deterministic gradient with a few bands so the result looks like a
/// real photo rather than flat color, and so decoders have detail to chew on.
/// RGB8 (no alpha) because JPEG and several other encoders require it; formats
/// that want alpha are converted at save time.
fn synthesize(width: u32, height: u32) -> RgbImage {
    let mut img = RgbImage::new(width, height);
    let (fw, fh) = (width.max(1) as f32, height.max(1) as f32);
    for (x, y, pixel) in img.enumerate_pixels_mut() {
        let u = x as f32 / fw;
        let v = y as f32 / fh;
        let r = (u * 255.0) as u8;
        let g = (v * 255.0) as u8;
        let b = (((u + v) * 0.5 + 0.25 * (u * 12.0).sin()) * 255.0).clamp(0.0, 255.0) as u8;
        *pixel = Rgb([r, g, b]);
    }
    img
}

/// Save `img` as `path`, converting to RGBA first for formats whose encoder
/// wants it.
fn save(img: &RgbImage, path: &Path) -> anyhow::Result<()> {
    let is_rgba_format = path.extension().is_some_and(|e| e == "gif");
    if is_rgba_format {
        DynamicImage::ImageRgb8(img.clone())
            .into_rgba8()
            .save(path)?;
    } else {
        img.save(path)?;
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "corpus".to_string());
    let dir = Path::new(&out);
    std::fs::create_dir_all(dir)?;

    // Encoders the pure-Rust `image` core supports for writing.
    let formats = ["png", "jpg", "bmp", "tiff", "gif", "qoi", "tga", "ppm"];
    let sizes = [(16u32, 16u32), (256, 171), (1920, 1080), (4000, 3000)];

    for (w, h) in sizes {
        let img = synthesize(w, h);
        for fmt in formats {
            save(&img, &dir.join(format!("grad_{w}x{h}.{fmt}")))?;
        }
    }

    // One very large image to exercise big-file decode, upload, and clamping.
    save(&synthesize(8000, 6000), &dir.join("grad_8000x6000.png"))?;

    let count = formats.len() * sizes.len() + 1;
    println!("wrote {count} images to {}", dir.display());
    Ok(())
}
