//! Largest embedded baseline JPEG inside a DNG or Sony ARW.
//!
//! The walk accepts only classic TIFF (not `BigTIFF`). A preview IFD must be
//! marked as a reduced-resolution image and use old or new JPEG compression.
//! An unreduced IFD is the sensor image and is ignored, including Sony sensor
//! data stored with compression 7. The returned pixels are the preview after
//! its own JPEG orientation is applied.

use std::collections::HashSet;
use std::io::Cursor;

use image::ImageDecoder;
use viewr_protocol::{MAX_DECODE_DIMENSION, WorkerColorProfile};

use super::DecodedOutput;

const MAX_IFDS: usize = 32;
const MAX_ENTRIES: u16 = 64;
const MAX_VALUE_COUNT: u32 = 16;
const TIFF_MAGIC: u16 = 42;
const TYPE_SHORT: u16 = 3;
const TYPE_LONG: u16 = 4;
const TYPE_IFD: u16 = 13;
const TAG_NEW_SUBFILE_TYPE: u16 = 254;
const TAG_COMPRESSION: u16 = 259;
const TAG_STRIP_OFFSETS: u16 = 273;
const TAG_STRIP_BYTE_COUNTS: u16 = 279;
const TAG_SUB_IFD: u16 = 330;
const TAG_JPEG_INTERCHANGE_FORMAT: u16 = 513;
const TAG_JPEG_INTERCHANGE_FORMAT_LENGTH: u16 = 514;
const COMPRESSION_OLD_JPEG: u32 = 6;
const COMPRESSION_JPEG: u32 = 7;
const NEW_SUBFILE_REDUCED: u32 = 1;
const SOF0: u8 = 0xC0;

struct Candidate {
    offset: usize,
    len: usize,
    area: u64,
}

struct Ifd {
    next: Option<usize>,
    sub_ifds: Vec<usize>,
    preview: Option<Candidate>,
}

/// Decode the largest embedded baseline JPEG preview.
///
/// A frame past the pixel budget, or a frame the JPEG decoder rejects, is
/// skipped. A smaller baseline preview in the same file can still be shown.
/// `None` means no such preview could be shown. Callers keep the camera-RAW
/// refusal in that case.
pub(crate) fn decode_embedded_preview(bytes: &[u8]) -> Option<DecodedOutput> {
    for candidate in baseline_previews(bytes) {
        let Some(end) = candidate.offset.checked_add(candidate.len) else {
            continue;
        };
        let Some(jpeg) = bytes.get(candidate.offset..end) else {
            continue;
        };
        let Some((width, height, rgba)) = decode_baseline_jpeg(jpeg) else {
            continue;
        };
        if viewr_protocol::checked_rgba_len(width, height).ok() != Some(rgba.len()) {
            continue;
        }
        return Some(DecodedOutput {
            width,
            height,
            rgba,
            color_profile: WorkerColorProfile::Unknown,
        });
    }
    None
}

fn baseline_previews(bytes: &[u8]) -> Vec<Candidate> {
    let Some(le) = tiff_little_endian(bytes) else {
        return Vec::new();
    };
    let Some(first) = read_u32(bytes, 4, le).and_then(|offset| usize::try_from(offset).ok()) else {
        return Vec::new();
    };
    let mut queue = vec![first];
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    while let Some(offset) = queue.pop() {
        if !seen.insert(offset) || seen.len() > MAX_IFDS {
            continue;
        }
        let Some(ifd) = parse_ifd(bytes, offset, le) else {
            continue;
        };
        if let Some(next) = ifd.next {
            queue.push(next);
        }
        queue.extend(ifd.sub_ifds);
        if let Some(preview) = ifd.preview {
            found.push(preview);
        }
    }
    found.sort_by_key(|preview| std::cmp::Reverse(preview.area));
    found
}

fn tiff_little_endian(bytes: &[u8]) -> Option<bool> {
    let order = bytes.get(..2)?;
    let le = match order {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    if read_u16(bytes, 2, le)? != TIFF_MAGIC {
        return None;
    }
    Some(le)
}

fn parse_ifd(bytes: &[u8], offset: usize, le: bool) -> Option<Ifd> {
    let count = read_u16(bytes, offset, le)?;
    if count > MAX_ENTRIES {
        return None;
    }
    let table = offset.checked_add(2)?;
    let entry_bytes = usize::from(count).checked_mul(12)?;
    let next_at = table.checked_add(entry_bytes)?;
    bytes.get(table..next_at.checked_add(4)?)?;
    let mut reduced = false;
    let mut compression = None;
    let mut jpeg_at = None;
    let mut jpeg_len = None;
    let mut strip_at = None;
    let mut strip_len = None;
    let mut strip_count = 0u32;
    let mut sub_ifds = Vec::new();
    for index in 0..count {
        let entry = table + usize::from(index) * 12;
        match read_u16(bytes, entry, le)? {
            TAG_NEW_SUBFILE_TYPE => {
                reduced =
                    scalar(bytes, entry, le).is_some_and(|value| value & NEW_SUBFILE_REDUCED != 0);
            }
            TAG_COMPRESSION => compression = scalar(bytes, entry, le),
            TAG_JPEG_INTERCHANGE_FORMAT => jpeg_at = scalar(bytes, entry, le),
            TAG_JPEG_INTERCHANGE_FORMAT_LENGTH => jpeg_len = scalar(bytes, entry, le),
            TAG_STRIP_OFFSETS => {
                if let Some(values) = entry_values(bytes, entry, le) {
                    strip_count = u32::try_from(values.len()).unwrap_or(0);
                    strip_at = values.first().copied();
                }
            }
            TAG_STRIP_BYTE_COUNTS => {
                strip_len =
                    entry_values(bytes, entry, le).and_then(|values| values.first().copied());
            }
            TAG_SUB_IFD => {
                if let Some(values) = entry_values(bytes, entry, le) {
                    sub_ifds.extend(
                        values
                            .into_iter()
                            .map(usize::try_from)
                            .filter_map(Result::ok)
                            .filter(|value| *value != 0),
                    );
                }
            }
            _ => {}
        }
    }
    let next = read_u32(bytes, next_at, le)
        .filter(|value| *value != 0)
        .and_then(|value| usize::try_from(value).ok());
    let preview = preview_candidate(
        bytes,
        &PreviewTags {
            reduced,
            compression,
            jpeg_at,
            jpeg_len,
            strip_at,
            strip_len,
            strip_count,
        },
    );
    Some(Ifd {
        next,
        sub_ifds,
        preview,
    })
}

struct PreviewTags {
    reduced: bool,
    compression: Option<u32>,
    jpeg_at: Option<u32>,
    jpeg_len: Option<u32>,
    strip_at: Option<u32>,
    strip_len: Option<u32>,
    strip_count: u32,
}

fn preview_candidate(bytes: &[u8], tags: &PreviewTags) -> Option<Candidate> {
    if !tags.reduced
        || !matches!(
            tags.compression,
            Some(COMPRESSION_OLD_JPEG | COMPRESSION_JPEG)
        )
    {
        return None;
    }
    let (offset, len) = if let (Some(offset), Some(len)) = (tags.jpeg_at, tags.jpeg_len) {
        (offset, len)
    } else if tags.strip_count == 1 {
        (tags.strip_at?, tags.strip_len?)
    } else {
        return None;
    };
    let offset = usize::try_from(offset).ok()?;
    let len = usize::try_from(len).ok()?;
    let jpeg = bytes.get(offset..offset.checked_add(len)?)?;
    let (width, height) = baseline_size(jpeg)?;
    let area = u64::from(width).checked_mul(u64::from(height))?;
    Some(Candidate { offset, len, area })
}

fn entry_values(bytes: &[u8], entry: usize, le: bool) -> Option<Vec<u32>> {
    let kind = read_u16(bytes, entry + 2, le)?;
    let count = read_u32(bytes, entry + 4, le)?;
    if count == 0 || count > MAX_VALUE_COUNT {
        return None;
    }
    let size = match kind {
        TYPE_SHORT => 2usize,
        TYPE_LONG | TYPE_IFD => 4,
        _ => return None,
    };
    let total = size.checked_mul(usize::try_from(count).ok()?)?;
    let inline = bytes.get(entry + 8..entry + 12)?;
    let raw = if total <= 4 {
        inline.get(..total)?
    } else {
        let start = usize::try_from(read_u32(bytes, entry + 8, le)?).ok()?;
        bytes.get(start..start.checked_add(total)?)?
    };
    let mut values = Vec::with_capacity(usize::try_from(count).ok()?);
    for index in 0..usize::try_from(count).ok()? {
        let at = index * size;
        let value = if size == 2 {
            u32::from(read_u16(raw, at, le)?)
        } else {
            read_u32(raw, at, le)?
        };
        values.push(value);
    }
    Some(values)
}

fn scalar(bytes: &[u8], entry: usize, le: bool) -> Option<u32> {
    let mut values = entry_values(bytes, entry, le)?;
    if values.len() == 1 {
        values.pop()
    } else {
        None
    }
}

fn baseline_size(jpeg: &[u8]) -> Option<(u32, u32)> {
    if jpeg.first().copied() != Some(0xFF) || jpeg.get(1).copied() != Some(0xD8) {
        return None;
    }
    if !tail_has_eoi(jpeg) {
        return None;
    }
    let mut index = 2usize;
    while index + 1 < jpeg.len() {
        if jpeg.get(index).copied() != Some(0xFF) {
            return None;
        }
        while jpeg.get(index).copied() == Some(0xFF) {
            index += 1;
        }
        let marker = *jpeg.get(index)?;
        index += 1;
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        if marker == 0xD9 {
            return None;
        }
        let length = usize::from(u16::from_be_bytes(
            jpeg.get(index..index + 2)?.try_into().ok()?,
        ));
        if length < 2 || index.checked_add(length)? > jpeg.len() {
            return None;
        }
        if is_sof(marker) {
            if marker != SOF0 || length < 7 {
                return None;
            }
            let height = u32::from(u16::from_be_bytes(
                jpeg.get(index + 3..index + 5)?.try_into().ok()?,
            ));
            let width = u32::from(u16::from_be_bytes(
                jpeg.get(index + 5..index + 7)?.try_into().ok()?,
            ));
            // Edge checks alone are not enough. A 65535 by 4096 baseline frame
            // is inside the strict edge limit and would still make the JPEG
            // decoder allocate past the pixel budget. Reject it here, before
            // that decoder runs.
            if viewr_protocol::checked_rgba_len(width, height).is_err() {
                return None;
            }
            return Some((width, height));
        }
        index = index.checked_add(length)?;
    }
    None
}

fn is_sof(marker: u8) -> bool {
    (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC
}

fn tail_has_eoi(jpeg: &[u8]) -> bool {
    let start = jpeg.len().saturating_sub(4096);
    jpeg.get(start..)
        .is_some_and(|tail| tail.windows(2).any(|pair| pair == [0xFF, 0xD9]))
}

fn decode_baseline_jpeg(jpeg: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_DIMENSION);
    limits.max_image_height = Some(MAX_DECODE_DIMENSION);
    limits.max_alloc = Some(viewr_protocol::MAX_RGBA_BYTES);
    let mut reader = image::ImageReader::with_format(Cursor::new(jpeg), image::ImageFormat::Jpeg);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().ok()?;
    let (width, height) = decoder.dimensions();
    if viewr_protocol::checked_rgba_len(width, height).is_err()
        || decoder.total_bytes() > viewr_protocol::MAX_RGBA_BYTES
    {
        return None;
    }
    let orientation = ImageDecoder::orientation(&mut decoder).ok()?;
    let mut image = image::DynamicImage::from_decoder(decoder).ok()?;
    image.apply_orientation(orientation);
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    Some((width, height, rgba.into_raw()))
}

fn read_u16(bytes: &[u8], offset: usize, le: bool) -> Option<u16> {
    let raw: [u8; 2] = bytes.get(offset..offset + 2)?.try_into().ok()?;
    Some(if le {
        u16::from_le_bytes(raw)
    } else {
        u16::from_be_bytes(raw)
    })
}

fn read_u32(bytes: &[u8], offset: usize, le: bool) -> Option<u32> {
    let raw: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
    Some(if le {
        u32::from_le_bytes(raw)
    } else {
        u32::from_be_bytes(raw)
    })
}

#[cfg(test)]
mod tests {
    use super::decode_embedded_preview;
    use image::ExtendedColorType;
    use image::codecs::jpeg::JpegEncoder;
    use viewr_protocol::FormatRefusal;

    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let pixel = [9, 8, 7];
        let mut pixels = vec![0u8; usize::try_from(width * height * 3).unwrap()];
        for chunk in pixels.as_chunks_mut::<3>().0 {
            chunk.copy_from_slice(&pixel);
        }
        let mut encoded = Vec::new();
        JpegEncoder::new_with_quality(&mut encoded, 90)
            .encode(&pixels, width, height, ExtendedColorType::Rgb8)
            .expect("encode baseline jpeg");
        encoded
    }

    fn endian_u16(le: bool, value: u16) -> [u8; 2] {
        if le {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        }
    }

    fn endian_u32(le: bool, value: u32) -> [u8; 4] {
        if le {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        }
    }

    fn long_entry(le: bool, tag: u16, value: u32) -> [u8; 12] {
        let mut entry = [0u8; 12];
        entry[0..2].copy_from_slice(&endian_u16(le, tag));
        entry[2..4].copy_from_slice(&endian_u16(le, 4));
        entry[4..8].copy_from_slice(&endian_u32(le, 1));
        entry[8..12].copy_from_slice(&endian_u32(le, value));
        entry
    }

    fn short_entry(le: bool, tag: u16, value: u16) -> [u8; 12] {
        let mut entry = long_entry(le, tag, 0);
        entry[2..4].copy_from_slice(&endian_u16(le, 3));
        entry[8..10].copy_from_slice(&endian_u16(le, value));
        entry[10] = 0;
        entry[11] = 0;
        entry
    }

    fn ifd(le: bool, entries: &[[u8; 12]], next: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(endian_u16(le, u16::try_from(entries.len()).unwrap()));
        for entry in entries {
            bytes.extend_from_slice(entry);
        }
        bytes.extend(endian_u32(le, next));
        bytes
    }

    fn preview_file(le: bool, small: &[u8], large: &[u8]) -> Vec<u8> {
        let ifd1_at = 8 + 2 + 12 + 4;
        let sub_at = ifd1_at + 2 + 12 * 4 + 4;
        let small_at = sub_at + 2 + 12 * 4 + 4;
        let large_at = small_at + small.len();
        let ifd0 = ifd(
            le,
            &[long_entry(le, 330, u32::try_from(sub_at).unwrap())],
            u32::try_from(ifd1_at).unwrap(),
        );
        let preview = |offset: usize, bytes: &[u8]| {
            [
                long_entry(le, 254, 1),
                short_entry(le, 259, 7),
                long_entry(le, 513, u32::try_from(offset).unwrap()),
                long_entry(le, 514, u32::try_from(bytes.len()).unwrap()),
            ]
        };
        let ifd1 = ifd(le, &preview(small_at, small), 0);
        let sub = ifd(le, &preview(large_at, large), 0);
        let mut file = Vec::new();
        file.extend(if le { b"II" } else { b"MM" });
        file.extend(endian_u16(le, 42));
        file.extend(endian_u32(le, 8));
        file.extend(ifd0);
        file.extend(ifd1);
        file.extend(sub);
        assert_eq!(file.len(), small_at);
        file.extend(small);
        file.extend(large);
        file
    }

    #[test]
    fn largest_baseline_preview_wins_in_either_byte_order() {
        let small = jpeg(1, 1);
        let large = jpeg(2, 2);
        for le in [true, false] {
            let file = preview_file(le, &small, &large);
            let decoded = decode_embedded_preview(&file).expect("preview");
            assert_eq!((decoded.width, decoded.height), (2, 2), "le={le}");
            let arw = super::super::decode_input("arw", &file).expect("arw preview");
            assert_eq!((arw.width, arw.height), (2, 2), "le={le}");
            for format in ["cr2", "nef", "rw2", "orf", "raf"] {
                let Err(message) = super::super::decode_input(format, &file) else {
                    panic!("{format} must stay deferred");
                };
                assert_eq!(
                    FormatRefusal::parse(&message),
                    Some(FormatRefusal::Raw),
                    "{format}"
                );
            }
        }
    }

    #[test]
    fn lossless_sensor_jpeg_and_a_looping_ifd_do_not_decode() {
        let mut sensor = vec![
            0xFF, 0xD8, 0xFF, 0xC3, 0x00, 0x0B, 0x08, 0x01, 0x00, 0x01, 0x00,
        ];
        sensor.extend_from_slice(&[0x01, 0x11, 0x00, 0xFF, 0xD9]);
        let file = preview_file(true, &sensor, &sensor);
        assert!(decode_embedded_preview(&file).is_none());

        // A zero-count IFD whose next pointer is itself must stop.
        let mut looped = Vec::new();
        looped.extend_from_slice(b"II");
        looped.extend(42u16.to_le_bytes());
        looped.extend(8u32.to_le_bytes());
        looped.extend(0u16.to_le_bytes());
        looped.extend(8u32.to_le_bytes());
        assert!(decode_embedded_preview(&looped).is_none());
        assert!(decode_embedded_preview(b"MM\x00\x2b\x00\x00\x00\x08").is_none());
        assert!(decode_embedded_preview(&[0, 1, 2, 3]).is_none());
    }

    fn counted_entry(le: bool, tag: u16, kind: u16, count: u32, value: u32) -> [u8; 12] {
        let mut entry = long_entry(le, tag, value);
        entry[2..4].copy_from_slice(&endian_u16(le, kind));
        entry[4..8].copy_from_slice(&endian_u32(le, count));
        entry
    }

    fn classic_header(le: bool) -> Vec<u8> {
        let mut file = Vec::new();
        file.extend(if le { b"II" } else { b"MM" });
        file.extend(endian_u16(le, 42));
        file.extend(endian_u32(le, 8));
        file
    }

    fn interchange_entries(
        le: bool,
        reduced: u32,
        compression: u16,
        at: usize,
        len: usize,
    ) -> [[u8; 12]; 4] {
        [
            long_entry(le, 254, reduced),
            short_entry(le, 259, compression),
            long_entry(le, 513, u32::try_from(at).unwrap()),
            long_entry(le, 514, u32::try_from(len).unwrap()),
        ]
    }

    #[test]
    fn strip_old_jpeg_and_oriented_preview_decode() {
        let pixels = jpeg(2, 1);
        let mut oriented = pixels.clone();
        let exif = [
            0xFF, 0xE1, 0x00, 0x22, b'E', b'x', b'i', b'f', 0x00, 0x00, b'I', b'I', 0x2A, 0x00,
            0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x12, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00,
            0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        oriented.splice(2..2, exif);
        let oriented_at = 8 + 2 + 12 * 4 + 4;
        let oriented_ifd = ifd(
            true,
            &interchange_entries(true, 1, 7, oriented_at, oriented.len()),
            0,
        );
        let mut oriented_file = classic_header(true);
        oriented_file.extend(oriented_ifd);
        assert_eq!(oriented_file.len(), oriented_at);
        oriented_file.extend(oriented);
        let decoded = decode_embedded_preview(&oriented_file).expect("oriented preview");
        assert_eq!((decoded.width, decoded.height), (1, 2));

        let strip_at = oriented_at;
        let mut strip_file = classic_header(true);
        strip_file.extend(ifd(
            true,
            &[
                long_entry(true, 254, 1),
                short_entry(true, 259, 6),
                long_entry(true, 273, u32::try_from(strip_at).unwrap()),
                long_entry(true, 279, u32::try_from(pixels.len()).unwrap()),
            ],
            0,
        ));
        assert_eq!(strip_file.len(), strip_at);
        strip_file.extend(&pixels);
        let stripped = decode_embedded_preview(&strip_file).expect("old-jpeg strip");
        assert_eq!((stripped.width, stripped.height), (2, 1));
    }

    #[test]
    fn subfile_links_survive_an_oversized_tag_and_an_ifd_type() {
        let preview = jpeg(1, 1);
        for le in [true, false] {
            let ifd0_end = 8 + 2 + 12 + 4;
            let preview_end = ifd0_end + 2 + 12 * 4 + 4;
            let jpeg_at = preview_end;
            let mut file = classic_header(le);
            file.extend(ifd(
                le,
                &[counted_entry(le, 330, 4, 17, 0)],
                u32::try_from(ifd0_end).unwrap(),
            ));
            file.extend(ifd(
                le,
                &interchange_entries(le, 1, 7, jpeg_at, preview.len()),
                0,
            ));
            assert_eq!(file.len(), jpeg_at);
            file.extend(&preview);
            let decoded = decode_embedded_preview(&file).expect("next ifd");
            assert_eq!((decoded.width, decoded.height), (1, 1), "le={le}");

            let mut linked = classic_header(le);
            let sub_at = 8 + 2 + 12 + 4;
            let sub_jpeg_at = sub_at + 2 + 12 * 4 + 4;
            linked.extend(ifd(
                le,
                &[counted_entry(
                    le,
                    330,
                    13,
                    1,
                    u32::try_from(sub_at).unwrap(),
                )],
                0,
            ));
            linked.extend(ifd(
                le,
                &interchange_entries(le, 1, 7, sub_jpeg_at, preview.len()),
                0,
            ));
            assert_eq!(linked.len(), sub_jpeg_at);
            linked.extend(&preview);
            let child = decode_embedded_preview(&linked).expect("ifd type subfile");
            assert_eq!((child.width, child.height), (1, 1), "le={le}");
        }

        let full = jpeg(1, 1);
        let full_at = 8 + 2 + 12 * 4 + 4;
        let mut unreduced = classic_header(true);
        unreduced.extend(ifd(
            true,
            &interchange_entries(true, 0, 7, full_at, full.len()),
            0,
        ));
        unreduced.extend(&full);
        assert!(decode_embedded_preview(&unreduced).is_none());

        let mut cut = jpeg(1, 1);
        assert_eq!(cut.split_off(cut.len() - 2), vec![0xFF, 0xD9]);
        let cut_at = full_at;
        let mut damaged = classic_header(true);
        damaged.extend(ifd(
            true,
            &interchange_entries(true, 1, 7, cut_at, cut.len()),
            0,
        ));
        damaged.extend(cut);
        assert!(decode_embedded_preview(&damaged).is_none());
    }

    #[test]
    fn an_oversized_baseline_frame_does_not_hide_a_smaller_preview() {
        let mut huge = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08];
        huge.extend(65_535u16.to_be_bytes());
        huge.extend(4_096u16.to_be_bytes());
        huge.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9]);
        let small = jpeg(1, 1);
        let file = preview_file(true, &small, &huge);
        let decoded = decode_embedded_preview(&file).expect("smaller preview");
        assert_eq!((decoded.width, decoded.height), (1, 1));
        assert!(decode_embedded_preview(&preview_file(true, &huge, &huge)).is_none());
    }

    #[test]
    fn an_undecodable_larger_frame_does_not_hide_a_smaller_preview() {
        let mut broken = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08];
        broken.extend(8u16.to_be_bytes());
        broken.extend(8u16.to_be_bytes());
        broken.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9]);
        let small = jpeg(1, 1);
        let decoded =
            decode_embedded_preview(&preview_file(true, &small, &broken)).expect("smaller preview");
        assert_eq!((decoded.width, decoded.height), (1, 1));
    }

    #[test]
    fn an_unreduced_sensor_image_does_not_hide_the_arw_preview() {
        let preview = jpeg(2, 2);
        let sensor = jpeg(4, 4);
        let thumb = jpeg(1, 1);
        let thumb_ifd = 8 + 2 + 12 * 5 + 4;
        let sensor_ifd = thumb_ifd + 2 + 12 * 4 + 4;
        let thumb_at = sensor_ifd + 2 + 12 * 4 + 4;
        let preview_at = thumb_at + thumb.len();
        let sensor_at = preview_at + preview.len();
        let mut file = classic_header(true);
        file.extend(ifd(
            true,
            &[
                long_entry(true, 254, 1),
                short_entry(true, 259, 6),
                long_entry(true, 513, u32::try_from(preview_at).unwrap()),
                long_entry(true, 514, u32::try_from(preview.len()).unwrap()),
                long_entry(true, 330, u32::try_from(sensor_ifd).unwrap()),
            ],
            u32::try_from(thumb_ifd).unwrap(),
        ));
        file.extend(ifd(
            true,
            &interchange_entries(true, 1, 6, thumb_at, thumb.len()),
            0,
        ));
        file.extend(ifd(
            true,
            &[
                long_entry(true, 254, 0),
                short_entry(true, 259, 32_767),
                long_entry(true, 273, u32::try_from(sensor_at).unwrap()),
                long_entry(true, 279, u32::try_from(sensor.len()).unwrap()),
            ],
            0,
        ));
        assert_eq!(file.len(), thumb_at);
        file.extend(&thumb);
        file.extend(&preview);
        file.extend(&sensor);
        let decoded = super::super::decode_input("arw", &file).expect("arw preview");
        assert_eq!((decoded.width, decoded.height), (2, 2));
        assert_eq!(
            decoded.color_profile,
            viewr_protocol::WorkerColorProfile::Unknown
        );

        // The a7R V shape: a reduced compression-7 preview is larger than the
        // compression-6 preview, and the unreduced compression-7 sensor is not.
        let large = jpeg(3, 2);
        let medium = jpeg(2, 1);
        let hidden = jpeg(8, 8);
        let medium_ifd = 8 + 2 + 12 * 4 + 4;
        let sensor2 = medium_ifd + 2 + 12 * 5 + 4;
        let large_at = sensor2 + 2 + 12 * 4 + 4;
        let medium_at = large_at + large.len();
        let hidden_at = medium_at + medium.len();
        let mut sony = classic_header(true);
        sony.extend(ifd(
            true,
            &interchange_entries(true, 1, 7, large_at, large.len()),
            u32::try_from(medium_ifd).unwrap(),
        ));
        sony.extend(ifd(
            true,
            &[
                long_entry(true, 254, 1),
                short_entry(true, 259, 6),
                long_entry(true, 513, u32::try_from(medium_at).unwrap()),
                long_entry(true, 514, u32::try_from(medium.len()).unwrap()),
                long_entry(true, 330, u32::try_from(sensor2).unwrap()),
            ],
            0,
        ));
        sony.extend(ifd(
            true,
            &[
                long_entry(true, 254, 0),
                short_entry(true, 259, 7),
                long_entry(true, 273, u32::try_from(hidden_at).unwrap()),
                long_entry(true, 279, u32::try_from(hidden.len()).unwrap()),
            ],
            0,
        ));
        assert_eq!(sony.len(), large_at);
        sony.extend(&large);
        sony.extend(&medium);
        sony.extend(&hidden);
        let chosen = decode_embedded_preview(&sony).expect("larger reduced preview");
        assert_eq!((chosen.width, chosen.height), (3, 2));
        let routed = super::super::decode_input("arw", &sony).expect("routed arw");
        assert_eq!((routed.width, routed.height), (3, 2));
    }
}
