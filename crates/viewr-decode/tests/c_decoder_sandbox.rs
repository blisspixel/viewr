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

struct WorkerDecode {
    pixels: Vec<u8>,
    color_profile: viewr_protocol::WorkerColorProfile,
}

#[cfg(feature = "heic")]
fn reference_heic_decoding_options() -> libheif_rs::DecodingOptions {
    let mut options = libheif_rs::DecodingOptions::new().expect("allocate HEIC decode options");
    options.set_strict_decoding(true);
    options.set_convert_hdr_to_8bit(true);
    let mut color_conversion = options.color_conversion_options();
    color_conversion.preferred_chroma_upsampling_algorithm =
        libheif_rs::ChromaUpsamplingAlgorithm::Bilinear;
    color_conversion.only_use_preferred_chroma_algorithm = true;
    options.set_color_conversion_options(color_conversion);
    options
}

fn decode_in_worker(format: &str, encoded: &[u8], expected_size: (u32, u32)) -> WorkerDecode {
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
    let viewr_protocol::WorkerResponse::PixelStream {
        width,
        height,
        color_profile,
    } = response
    else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("worker rejected a valid {format} image: {response:?}");
    };
    assert_eq!((width, height), expected_size);
    assert_ne!(
        color_profile,
        viewr_protocol::WorkerColorProfile::Unknown,
        "{format} worker silently discarded color-space evidence"
    );

    let mut pixels = vec![0_u8; viewr_protocol::checked_rgba_len(width, height).unwrap()];
    stdout.read_exact(&mut pixels).expect("read decoded pixels");
    assert!(pixels.iter().any(|&channel| channel != 0));
    viewr_protocol::write_ack(&mut stdin)
        .and_then(|()| stdin.flush())
        .expect("acknowledge decoded pixels");
    drop(stdin);

    assert!(child.wait().expect("wait for decode worker").success());
    WorkerDecode {
        pixels,
        color_profile,
    }
}

#[cfg(feature = "avif")]
#[test]
fn avif_decodes_under_the_production_worker_policy() {
    let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(3, 2, |x, y| {
        image::Rgba([(x * 70) as u8, (y * 90) as u8, 40, 255])
    }));
    let mut encoded = libavif_image::save(&image)
        .expect("encode AVIF fixture")
        .as_slice()
        .to_vec();
    let nclx = encoded
        .windows(4)
        .position(|window| window == b"nclx")
        .expect("AVIF fixture contains a coded color profile");
    encoded[nclx + 4..nclx + 11].copy_from_slice(&[0, 1, 0, 13, 0, 1, 0x80]);
    let reference = libavif_image::read(&encoded)
        .expect("decode AVIF reference pixels")
        .into_rgba8()
        .into_raw();

    let decoded = decode_in_worker("avif", encoded.as_slice(), (3, 2));
    assert_eq!(decoded.pixels, reference);
    assert_eq!(
        decoded.color_profile,
        viewr_protocol::WorkerColorProfile::Cicp(viewr_protocol::CicpColor {
            color_primaries: 1,
            transfer_characteristics: 13,
            matrix_coefficients: 1,
            full_range: true,
        })
    );
}

#[cfg(feature = "heic")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HeicNclxFixture {
    None,
    Container,
    BitstreamOnly,
}

#[cfg(feature = "heic")]
type HeicFixture = (Vec<u8>, Vec<u8>, (u32, u32), Option<(u16, u16)>);

#[cfg(feature = "heic")]
fn reference_heic_decode(
    encoded: &[u8],
    nclx_fixture: HeicNclxFixture,
    width: u32,
    height: u32,
) -> (Vec<u8>, Option<(u16, u16)>) {
    use libheif_rs::{
        ColorPrimaries, ColorSpace, HeifContext, LibHeif, RgbChroma, TransferCharacteristics,
    };

    let lib_heif = LibHeif::new();
    let reference_context = HeifContext::read_from_bytes(encoded).expect("read HEIC reference");
    let reference_handle = reference_context
        .primary_image_handle()
        .expect("select HEIC reference image");
    if nclx_fixture == HeicNclxFixture::Container {
        let source_profile = reference_handle
            .color_profile_nclx()
            .expect("read source Display P3 NCLX profile");
        assert_eq!(
            source_profile.color_primaries(),
            ColorPrimaries::SMPTE_EG_432_1
        );
        assert_eq!(
            source_profile.transfer_characteristics(),
            TransferCharacteristics::IEC_61966_2_1
        );
    } else {
        assert!(
            reference_handle.color_profile_nclx().is_none(),
            "fixture must not contain an NCLX colr property"
        );
    }
    #[cfg(feature = "heic-latest-ci")]
    let mut decoding_options = reference_heic_decoding_options();
    #[cfg(not(feature = "heic-latest-ci"))]
    let decoding_options = reference_heic_decoding_options();
    #[cfg(feature = "heic-latest-ci")]
    match nclx_fixture {
        HeicNclxFixture::Container => {
            decoding_options.set_output_image_nclx_profile(reference_handle.color_profile_nclx());
        }
        HeicNclxFixture::None | HeicNclxFixture::BitstreamOnly => {}
    }
    let reference_image = lib_heif
        .decode(
            &reference_handle,
            ColorSpace::Rgb(RgbChroma::Rgba),
            Some(decoding_options),
        )
        .expect("decode HEIC reference pixels");
    let output_encoding = reference_image.color_profile_nclx().map(|profile| {
        (
            profile.color_primaries() as u16,
            profile.transfer_characteristics() as u16,
        )
    });
    let plane = reference_image
        .planes()
        .interleaved
        .expect("HEIC reference RGBA plane");
    let row_bytes = width as usize * 4;
    let mut reference = Vec::with_capacity(row_bytes * height as usize);
    for row in plane.data.chunks_exact(plane.stride).take(height as usize) {
        reference.extend_from_slice(&row[..row_bytes]);
    }
    (reference, output_encoding)
}

#[cfg(feature = "heic")]
fn heic_fixture(icc_profile: Option<&[u8]>, nclx_fixture: HeicNclxFixture) -> HeicFixture {
    use libheif_rs::{
        Channel, ColorPrimaries, ColorProfileNCLX, ColorProfileRaw, ColorSpace, CompressionFormat,
        EncoderQuality, EncodingOptions, HeifContext, Image, LibHeif, RgbChroma,
        color_profile_types,
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
    if let Some(profile) = icc_profile {
        image
            .set_color_profile_raw(&ColorProfileRaw::new(
                color_profile_types::PROF,
                profile.to_vec(),
            ))
            .expect("attach HEIC ICC profile");
    }
    if nclx_fixture != HeicNclxFixture::None {
        let mut nclx = ColorProfileNCLX::new().expect("allocate Display P3 NCLX profile");
        nclx.set_color_primaries(ColorPrimaries::SMPTE_EG_432_1);
        image
            .set_color_profile_nclx(&nclx)
            .expect("attach HEIC NCLX profile");
    }

    let lib_heif = LibHeif::new();
    let mut context = HeifContext::new().expect("create HEIC context");
    let mut hevc_encoder = lib_heif
        .encoder_for_format(CompressionFormat::Hevc)
        .expect("load HEVC encoder");
    hevc_encoder
        .set_quality(EncoderQuality::LossLess)
        .expect("configure HEVC encoder");
    let mut encoding_options = EncodingOptions::default();
    encoding_options.set_save_two_colr_boxes_when_icc_and_nclx_available(
        nclx_fixture == HeicNclxFixture::Container,
    );
    encoding_options.set_mac_os_compatibility_workaround_no_nclx_profile(
        nclx_fixture == HeicNclxFixture::BitstreamOnly,
    );
    context
        .encode_image(&image, &mut hevc_encoder, Some(encoding_options))
        .expect("encode HEIC fixture");
    let mut heic_bytes = context.write_to_bytes().expect("serialize HEIC fixture");
    if nclx_fixture == HeicNclxFixture::Container {
        let nclx = heic_bytes
            .windows(4)
            .position(|window| window == b"nclx")
            .expect("HEIC fixture contains an NCLX profile");
        heic_bytes[nclx + 4..nclx + 11].copy_from_slice(&[0, 12, 0, 13, 0, 1, 0x80]);
    }
    let (reference, output_encoding) =
        reference_heic_decode(&heic_bytes, nclx_fixture, width, height);

    (heic_bytes, reference, (width, height), output_encoding)
}

#[cfg(feature = "heic")]
#[test]
fn heic_decodes_under_the_production_worker_policy() {
    let (encoded, reference, size, _) = heic_fixture(None, HeicNclxFixture::None);
    let decoded = decode_in_worker("heic", &encoded, size);
    assert_eq!(decoded.pixels, reference);
    assert!(
        matches!(decoded.color_profile, viewr_protocol::WorkerColorProfile::Cicp(cicp) if cicp.is_srgb()),
        "libheif must describe its converted RGBA output as sRGB"
    );
}

#[cfg(feature = "heic")]
#[test]
fn heic_icc_profile_takes_precedence_over_synthesized_nclx() {
    let display_p3 = moxcms::ColorProfile::new_display_p3()
        .encode()
        .expect("encode Display P3 fixture profile");
    let (encoded, reference, size, _) = heic_fixture(Some(&display_p3), HeicNclxFixture::None);
    let decoded = decode_in_worker("heic", &encoded, size);

    assert_eq!(decoded.pixels, reference);
    assert_eq!(
        decoded.color_profile,
        viewr_protocol::WorkerColorProfile::Icc(display_p3)
    );
}

#[cfg(feature = "heic-latest-ci")]
#[test]
fn heic_latest_preserves_source_profile_contract() {
    let display_p3 = moxcms::ColorProfile::new_display_p3()
        .encode()
        .expect("encode Display P3 fixture profile");
    let (encoded, reference, size, _) = heic_fixture(Some(&display_p3), HeicNclxFixture::Container);
    let decoded = decode_in_worker("heic", &encoded, size);

    assert_eq!(decoded.pixels, reference);
    assert_eq!(
        decoded.color_profile,
        viewr_protocol::WorkerColorProfile::Icc(display_p3),
        "newer libheif must not silently invalidate the retained source ICC"
    );
}

#[cfg(feature = "heic-latest-ci")]
#[test]
fn heic_latest_reports_bitstream_only_nclx() {
    let (encoded, reference, size, expected_encoding) =
        heic_fixture(None, HeicNclxFixture::BitstreamOnly);
    let decoded = decode_in_worker("heic", &encoded, size);

    assert_eq!(decoded.pixels, reference);
    let viewr_protocol::WorkerColorProfile::Cicp(cicp) = decoded.color_profile else {
        panic!("bitstream-only color signaling must remain typed CICP evidence");
    };
    assert_eq!(
        (cicp.color_primaries, cicp.transfer_characteristics),
        expected_encoding.expect("reference decoder must describe its output encoding"),
        "the worker must report the decoder's actual output encoding"
    );
}

#[cfg(feature = "heic-latest-ci")]
#[test]
fn heic_latest_preserves_icc_under_the_passthrough_contract() {
    let display_p3 = moxcms::ColorProfile::new_display_p3()
        .encode()
        .expect("encode Display P3 fixture profile");
    let (encoded, reference, size, _) =
        heic_fixture(Some(&display_p3), HeicNclxFixture::BitstreamOnly);
    let decoded = decode_in_worker("heic", &encoded, size);

    assert_eq!(decoded.pixels, reference);
    assert_eq!(
        decoded.color_profile,
        viewr_protocol::WorkerColorProfile::Icc(display_p3),
        "bitstream passthrough must preserve an ICC that still describes the output pixels"
    );
}
