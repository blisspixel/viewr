//! Linux runtime proofs for feature-gated C decoders in the isolated worker.

#![cfg(all(target_os = "linux", any(feature = "avif", feature = "heic")))]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn worker_command(format: &str) -> Command {
    let worker = env!("CARGO_BIN_EXE_viewr-decode");
    let Some(trace_directory) = std::env::var_os("VIEWR_TEST_STRACE_DIRECTORY") else {
        return Command::new(worker);
    };

    let mut command = Command::new("strace");
    command
        .args(["-f", "-qq", "-o"])
        .arg(PathBuf::from(trace_directory).join(format!("{format}.trace")))
        .arg("--")
        .arg(worker);
    command
}

fn decode_in_worker(format: &str, encoded: &[u8], expected_size: (u32, u32)) {
    let mut child = worker_command(format)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn feature-gated decode worker");
    let mut stdin = child.stdin.take().expect("worker stdin");
    let mut stdout = child.stdout.take().expect("worker stdout");

    viewr_protocol::write_decode_request(&mut stdin, format, encoded)
        .and_then(|()| stdin.flush())
        .expect("send encoded image");
    let response = viewr_protocol::read_worker_response(&mut stdout)
        .inspect_err(|_| {
            let _ = child.kill();
            let _ = child.wait();
        })
        .expect("read worker response");
    let viewr_protocol::WorkerResponse::PixelStream { width, height } = response else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("worker rejected a valid {format} image: {response:?}");
    };
    assert_eq!((width, height), expected_size);

    let mut pixels = vec![0_u8; viewr_protocol::checked_rgba_len(width, height).unwrap()];
    stdout.read_exact(&mut pixels).expect("read decoded pixels");
    assert!(pixels.iter().any(|&channel| channel != 0));
    viewr_protocol::write_ack(&mut stdin)
        .and_then(|()| stdin.flush())
        .expect("acknowledge decoded pixels");
    drop(stdin);

    assert!(child.wait().expect("wait for decode worker").success());
}

#[cfg(feature = "avif")]
#[test]
fn avif_decodes_under_the_production_worker_policy() {
    let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(3, 2, |x, y| {
        image::Rgba([(x * 70) as u8, (y * 90) as u8, 40, 255])
    }));
    let encoded = libavif_image::save(&image).expect("encode AVIF fixture");

    decode_in_worker("avif", encoded.as_slice(), (3, 2));
}

#[cfg(feature = "heic")]
#[test]
fn heic_decodes_under_the_production_worker_policy() {
    use libheif_rs::{
        Channel, ColorSpace, CompressionFormat, EncoderQuality, EncodingOptions, HeifContext,
        Image, LibHeif, RgbChroma,
    };

    let (width, height) = (3, 2);
    let mut image = Image::new(width, height, ColorSpace::Rgb(RgbChroma::Rgb))
        .expect("create HEIC fixture image");
    image
        .create_plane(Channel::Interleaved, width, height, 24)
        .expect("create HEIC RGB plane");
    let plane = image
        .planes_mut()
        .interleaved
        .expect("HEIC interleaved plane");
    for y in 0..height as usize {
        for x in 0..width as usize {
            let offset = y * plane.stride + x * 3;
            plane.data[offset..offset + 3].copy_from_slice(&[(x * 70) as u8, (y * 90) as u8, 40]);
        }
    }

    let lib_heif = LibHeif::new();
    let mut context = HeifContext::new().expect("create HEIC context");
    let mut hevc_encoder = lib_heif
        .encoder_for_format(CompressionFormat::Hevc)
        .expect("load HEVC encoder");
    hevc_encoder
        .set_quality(EncoderQuality::LossLess)
        .expect("configure HEVC encoder");
    context
        .encode_image(&image, &mut hevc_encoder, Some(EncodingOptions::default()))
        .expect("encode HEIC fixture");
    let heic_bytes = context.write_to_bytes().expect("serialize HEIC fixture");

    decode_in_worker("heic", &heic_bytes, (width, height));
}
