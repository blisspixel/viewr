//! Generate the deterministic, synthetic fixture set used by the v0.6 manual gate.
//!
//! Usage: `cargo run -p viewr --example gen_product_quality_fixtures -- <out-dir>`

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use image::codecs::gif::{GifDecoder, GifEncoder, Repeat};
use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::codecs::png::PngDecoder;
use image::codecs::webp::{WebPDecoder, WebPEncoder};
use image::{AnimationDecoder, Delay, ExtendedColorType, Frame, Rgba, RgbaImage};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 200;
const FIXTURE_PATHS: &[(&str, &str)] = &[
    (
        "browse/1-red.png",
        "PQ-FT-02, PQ-FT-03, and natural folder navigation",
    ),
    ("browse/2-green.png", "natural folder navigation"),
    ("browse/10-blue.png", "natural folder navigation"),
    ("editing/replacement.png", "PQ-PW-06 external replacement"),
    ("editing/source.png", "PQ-PW-06, PQ-PW-07, and PQ-RC-02"),
    ("failure/malformed.png", "PQ-FT-06 malformed input"),
    ("failure/unsupported.txt", "PQ-FT-06 unsupported input"),
    ("mosaic/01-wide.png", "PQ-PW-08 first full-image group"),
    ("mosaic/02-tall.png", "PQ-PW-08 first full-image group"),
    ("mosaic/03-square.png", "PQ-PW-08 first full-image group"),
    ("mosaic/04-wide.png", "PQ-PW-08 first full-image group"),
    ("mosaic/05-tall.png", "PQ-PW-08 first full-image group"),
    ("mosaic/06-panoramic.png", "PQ-PW-08 first full-image group"),
    ("mosaic/07-tall.png", "PQ-PW-08 first full-image group"),
    ("mosaic/08-wide.png", "PQ-PW-08 first full-image group"),
    ("mosaic/09-square.png", "PQ-PW-08 second full-image group"),
    ("mosaic/10-wide.png", "PQ-PW-08 second full-image group"),
    ("sequences/two-frame.gif", "PQ-PW-03 animated GIF"),
    ("sequences/two-frame.png", "PQ-PW-03 APNG"),
    ("sequences/two-frame.webp", "PQ-PW-03 animated WebP"),
    ("sequences/two-page.tiff", "PQ-PW-02 multi-page TIFF"),
    ("sequences/two-size.ico", "PQ-PW-02 multi-size ICO"),
    ("visual/large.png", "PQ-PW-04 and PQ-VS-04 large source"),
    ("visual/small.png", "PQ-PW-04 actual-size source"),
];

fn synthetic(width: u32, height: u32, accent: [u8; 3]) -> RgbaImage {
    RgbaImage::from_fn(width, height, |x, y| {
        let horizontal = u8::try_from((x * 255) / width.max(1)).unwrap_or(255);
        let vertical = u8::try_from((y * 255) / height.max(1)).unwrap_or(255);
        let stripe = if (x / 24 + y / 24).is_multiple_of(2) {
            36
        } else {
            0
        };
        Rgba([
            accent[0]
                .saturating_add(horizontal / 3)
                .saturating_add(stripe),
            accent[1].saturating_add(vertical / 3),
            accent[2].saturating_add((horizontal / 6) + (vertical / 6)),
            255,
        ])
    })
}

fn mosaic_fixture(width: u32, height: u32, accent: [u8; 3]) -> RgbaImage {
    let mut image = synthetic(width, height, accent);
    let marker = (width.min(height) / 8).clamp(8, 28);
    for y in 0..height {
        for x in 0..width {
            let color = if x < marker && y < marker {
                Some([255, 32, 32, 255])
            } else if x >= width - marker && y < marker {
                Some([32, 255, 32, 255])
            } else if x < marker && y >= height - marker {
                Some([32, 96, 255, 255])
            } else if x >= width - marker && y >= height - marker {
                Some([255, 224, 32, 255])
            } else {
                None
            };
            if let Some(color) = color {
                image.put_pixel(x, y, Rgba(color));
            }
        }
    }
    image
}

fn save_png(path: &Path, image: &RgbaImage) -> anyhow::Result<()> {
    image
        .save(path)
        .with_context(|| format!("write synthetic PNG {}", path.display()))
}

fn write_gif(path: &Path, frames: &[RgbaImage]) -> anyhow::Result<()> {
    let mut encoder = GifEncoder::new(File::create(path)?);
    encoder.set_repeat(Repeat::Infinite)?;
    encoder.encode_frames(
        frames
            .iter()
            .cloned()
            .map(|image| Frame::from_parts(image, 0, 0, Delay::from_numer_denom_ms(500, 1))),
    )?;
    Ok(())
}

fn write_apng(path: &Path, frames: &[RgbaImage]) -> anyhow::Result<()> {
    let mut encoder = png::Encoder::new(BufWriter::new(File::create(path)?), WIDTH, HEIGHT);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_animated(u32::try_from(frames.len())?, 0)?;
    encoder.set_frame_delay(1, 2)?;
    encoder.validate_sequence(true);
    let mut writer = encoder.write_header()?;
    for frame in frames {
        writer.write_image_data(frame.as_raw())?;
    }
    writer.finish()?;
    Ok(())
}

fn push_u24(bytes: &mut Vec<u8>, value: u32) {
    assert!(value <= 0x00ff_ffff);
    let encoded = value.to_le_bytes();
    bytes.extend_from_slice(&encoded[..3]);
}

fn push_webp_chunk(bytes: &mut Vec<u8>, kind: [u8; 4], payload: &[u8]) {
    bytes.extend_from_slice(&kind);
    bytes.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(payload);
    if !payload.len().is_multiple_of(2) {
        bytes.push(0);
    }
}

fn lossless_webp_image_chunk(frame: &RgbaImage) -> anyhow::Result<Vec<u8>> {
    let mut still = Vec::new();
    WebPEncoder::new_lossless(&mut still).encode(
        frame.as_raw(),
        frame.width(),
        frame.height(),
        ExtendedColorType::Rgba8,
    )?;
    if still.len() < 20 || &still[..4] != b"RIFF" || &still[8..12] != b"WEBP" {
        bail!("lossless WebP encoder returned an invalid container");
    }
    let size = usize::try_from(u32::from_le_bytes(still[16..20].try_into()?))?;
    let padded_size = size + (size % 2);
    let end = 20_usize
        .checked_add(padded_size)
        .context("WebP frame length overflow")?;
    if &still[12..16] != b"VP8L" || end != still.len() {
        bail!("lossless WebP encoder returned an unexpected chunk set");
    }
    Ok(still[12..end].to_vec())
}

fn write_animated_webp(path: &Path, frames: &[RgbaImage]) -> anyhow::Result<()> {
    let mut chunks = Vec::new();
    let mut extended = vec![0x02, 0, 0, 0];
    push_u24(&mut extended, WIDTH - 1);
    push_u24(&mut extended, HEIGHT - 1);
    push_webp_chunk(&mut chunks, *b"VP8X", &extended);

    let mut animation = Vec::new();
    animation.extend_from_slice(&0_u32.to_le_bytes());
    animation.extend_from_slice(&0_u16.to_le_bytes());
    push_webp_chunk(&mut chunks, *b"ANIM", &animation);

    for frame in frames {
        let mut payload = Vec::new();
        push_u24(&mut payload, 0);
        push_u24(&mut payload, 0);
        push_u24(&mut payload, WIDTH - 1);
        push_u24(&mut payload, HEIGHT - 1);
        push_u24(&mut payload, 500);
        payload.push(0);
        payload.extend_from_slice(&lossless_webp_image_chunk(frame)?);
        push_webp_chunk(&mut chunks, *b"ANMF", &payload);
    }

    let riff_size = u32::try_from(
        4_usize
            .checked_add(chunks.len())
            .context("WebP too large")?,
    )?;
    let mut file = BufWriter::new(File::create(path)?);
    file.write_all(b"RIFF")?;
    file.write_all(&riff_size.to_le_bytes())?;
    file.write_all(b"WEBP")?;
    file.write_all(&chunks)?;
    file.flush()?;
    Ok(())
}

fn write_tiff(path: &Path) -> anyhow::Result<()> {
    let mut encoder = tiff::encoder::TiffEncoder::new(File::create(path)?)?;
    for (width, height, color) in [
        (WIDTH, HEIGHT, [210, 30, 30]),
        (WIDTH / 2, HEIGHT / 2, [30, 60, 210]),
    ] {
        let mut pixels = vec![0_u8; usize::try_from(width * height * 3)?];
        for pixel in pixels.chunks_exact_mut(3) {
            pixel.copy_from_slice(&color);
        }
        encoder.write_image::<tiff::encoder::colortype::RGB8>(width, height, &pixels)?;
    }
    Ok(())
}

fn write_ico(path: &Path) -> anyhow::Result<()> {
    let frames = [(16, [210, 30, 30, 255]), (64, [30, 60, 210, 255])]
        .into_iter()
        .map(|(size, color)| {
            let pixels = RgbaImage::from_pixel(size, size, Rgba(color));
            IcoFrame::as_png(pixels.as_raw(), size, size, ExtendedColorType::Rgba8)
        })
        .collect::<Result<Vec<_>, _>>()?;
    IcoEncoder::new(File::create(path)?).encode_images(&frames)?;
    Ok(())
}

fn require_two_frames(path: &Path) -> anyhow::Result<()> {
    let file = BufReader::new(File::open(path)?);
    let frames = match path.extension().and_then(|value| value.to_str()) {
        Some("gif") => GifDecoder::new(file)?.into_frames().collect_frames()?,
        Some("png") => PngDecoder::new(file)?
            .apng()?
            .into_frames()
            .collect_frames()?,
        Some("webp") => WebPDecoder::new(file)?.into_frames().collect_frames()?,
        extension => bail!("unsupported animation validation extension: {extension:?}"),
    };
    if frames.len() != 2 {
        bail!(
            "{} decoded {} frames instead of 2",
            path.display(),
            frames.len()
        );
    }
    Ok(())
}

fn require_two_tiff_pages(path: &Path) -> anyhow::Result<()> {
    let mut decoder = tiff::decoder::Decoder::new(BufReader::new(File::open(path)?))?;
    let mut pages = 1;
    decoder.dimensions()?;
    while decoder.more_images() {
        decoder.next_image()?;
        decoder.dimensions()?;
        pages += 1;
    }
    if pages != 2 {
        bail!("{} decoded {pages} pages instead of 2", path.display());
    }
    Ok(())
}

fn require_two_ico_frames(path: &Path) -> anyhow::Result<()> {
    let mut header = [0_u8; 6];
    File::open(path)?.read_exact(&mut header)?;
    let reserved = u16::from_le_bytes(header[0..2].try_into()?);
    let kind = u16::from_le_bytes(header[2..4].try_into()?);
    let frames = u16::from_le_bytes(header[4..6].try_into()?);
    if (reserved, kind, frames) != (0, 1, 2) {
        bail!("{} has an invalid two-frame ICO header", path.display());
    }
    Ok(())
}

fn write_manifest(root: &Path) -> anyhow::Result<()> {
    let mut manifest = String::from(
        "viewr product-quality fixture set 1\n\
         All files are deterministic synthetic test data. No personal metadata is present.\n\n",
    );
    for (path, purpose) in FIXTURE_PATHS {
        manifest.push_str(path);
        manifest.push_str(" | ");
        manifest.push_str(purpose);
        manifest.push('\n');
    }
    std::fs::write(root.join("fixture-manifest.txt"), manifest)?;
    Ok(())
}

fn generate(root: &Path) -> anyhow::Result<()> {
    if root.exists() {
        bail!(
            "refusing to replace existing fixture directory: {}",
            root.display()
        );
    }
    for directory in [
        "browse",
        "editing",
        "failure",
        "mosaic",
        "sequences",
        "visual",
    ] {
        std::fs::create_dir_all(root.join(directory))?;
    }

    let red = synthetic(WIDTH, HEIGHT, [150, 5, 5]);
    let green = synthetic(WIDTH, HEIGHT, [5, 130, 5]);
    let blue = synthetic(WIDTH, HEIGHT, [5, 20, 150]);
    save_png(&root.join("browse/1-red.png"), &red)?;
    save_png(&root.join("browse/2-green.png"), &green)?;
    save_png(&root.join("browse/10-blue.png"), &blue)?;
    save_png(&root.join("editing/source.png"), &red)?;
    save_png(&root.join("editing/replacement.png"), &blue)?;
    for (name, width, height, accent) in [
        ("01-wide.png", 480, 180, [90, 15, 20]),
        ("02-tall.png", 180, 480, [15, 80, 20]),
        ("03-square.png", 300, 300, [15, 35, 100]),
        ("04-wide.png", 480, 300, [85, 45, 10]),
        ("05-tall.png", 240, 400, [65, 15, 85]),
        ("06-panoramic.png", 640, 160, [10, 70, 75]),
        ("07-tall.png", 160, 640, [75, 70, 10]),
        ("08-wide.png", 400, 260, [85, 25, 55]),
        ("09-square.png", 260, 260, [20, 75, 55]),
        ("10-wide.png", 520, 240, [45, 35, 95]),
    ] {
        save_png(
            &root.join("mosaic").join(name),
            &mosaic_fixture(width, height, accent),
        )?;
    }
    save_png(
        &root.join("visual/small.png"),
        &synthetic(64, 64, [60, 30, 80]),
    )?;
    save_png(
        &root.join("visual/large.png"),
        &synthetic(2048, 1536, [15, 40, 70]),
    )?;

    std::fs::write(
        root.join("failure/unsupported.txt"),
        b"synthetic non-image\n",
    )?;
    std::fs::write(
        root.join("failure/malformed.png"),
        b"\x89PNG\r\n\x1a\nsynthetic-truncated-image",
    )?;

    let animation_frames = [red.clone(), blue.clone()];
    write_gif(&root.join("sequences/two-frame.gif"), &animation_frames)?;
    write_apng(&root.join("sequences/two-frame.png"), &animation_frames)?;
    write_animated_webp(&root.join("sequences/two-frame.webp"), &animation_frames)?;
    write_tiff(&root.join("sequences/two-page.tiff"))?;
    write_ico(&root.join("sequences/two-size.ico"))?;
    write_manifest(root)?;

    for path in [
        root.join("sequences/two-frame.gif"),
        root.join("sequences/two-frame.png"),
        root.join("sequences/two-frame.webp"),
    ] {
        require_two_frames(&path)?;
    }
    require_two_tiff_pages(&root.join("sequences/two-page.tiff"))?;
    require_two_ico_frames(&root.join("sequences/two-size.ico"))?;
    for (path, _) in FIXTURE_PATHS {
        if !root.join(path).is_file() {
            bail!("fixture was not created: {path}");
        }
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .context("usage: gen_product_quality_fixtures <out-dir>")?;
    generate(&output)?;
    println!(
        "generated {} synthetic fixtures in {}",
        FIXTURE_PATHS.len(),
        output.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewr::ephemeral::TempWorkspace;

    #[test]
    fn fixture_set_is_complete_deterministic_and_refuses_overwrite() {
        let workspace = TempWorkspace::new("product_quality_fixtures").unwrap();
        let first = workspace.path().join("first");
        let second = workspace.path().join("second");
        generate(&first).unwrap();
        generate(&second).unwrap();

        for relative in FIXTURE_PATHS
            .iter()
            .map(|(path, _)| *path)
            .chain(["fixture-manifest.txt"])
        {
            assert_eq!(
                std::fs::read(first.join(relative)).unwrap(),
                std::fs::read(second.join(relative)).unwrap(),
                "fixture differs across identical generations: {relative}"
            );
        }
        assert!(
            generate(&first)
                .unwrap_err()
                .to_string()
                .contains("refusing")
        );
    }
}
