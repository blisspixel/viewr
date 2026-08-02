//! Best-effort, local-only facts for the Image Information panel.

use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crc32fast::Hasher;
use flate2::read::ZlibDecoder;
use little_exif::exif_tag::ExifTag;
use little_exif::filetype::FileExtension;
use little_exif::ifd::ExifTagGroup;
use little_exif::metadata::Metadata;
use little_exif::rational::uR64;

pub(crate) const MAX_EXIF_BYTES: u64 = 2 * 1024 * 1024;
const MAX_EXIF_ALLOCATION_BYTES: usize = 2 * 1024 * 1024;
const MAX_CONTAINER_SCAN_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CONTAINER_CHUNKS: usize = 4096;
const MAX_EXIF_TAGS: usize = 4096;
const MAX_IFD_DIRECTORIES: usize = 64;
const MAX_TEXT_BYTES: usize = 1024;
const PNG_RAW_PROFILE_KEYWORD: &[u8] = b"Raw profile type exif";
const MAX_PNG_RAW_PROFILE_BYTES: u64 = (MAX_EXIF_BYTES + 7) * 2 + 32;
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// Useful file and camera facts. Missing or malformed metadata leaves a field
/// empty and never prevents the image itself from opening.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent presence flags intentionally avoid retaining sensitive EXIF values"
)]
pub struct ImageDetails {
    /// Encoded file size on disk.
    pub file_bytes: Option<u64>,
    /// Human-readable container/codec name.
    pub format: Option<String>,
    /// Combined camera make and model.
    pub camera: Option<String>,
    /// Lens model.
    pub lens: Option<String>,
    /// EXIF original capture timestamp.
    pub captured_at: Option<String>,
    /// Exposure time, such as `1/125 s`.
    pub exposure: Option<String>,
    /// Aperture, such as `f/2.8`.
    pub aperture: Option<String>,
    /// ISO sensitivity.
    pub iso: Option<String>,
    /// Focal length, such as `50 mm`.
    pub focal_length: Option<String>,
    /// Number of supported EXIF tags inspected within the fixed parser budget.
    pub exif_tag_count: usize,
    /// Whether any location-related EXIF tags are present.
    pub has_location: bool,
    /// Whether owner, artist, or copyright EXIF fields are present.
    pub has_owner_or_author: bool,
    /// Whether camera, lens, or image-specific identifiers are present.
    pub has_device_identifier: bool,
    /// Whether a description or comment is present.
    pub has_description_or_comment: bool,
    /// Whether an editing or encoding software field is present.
    pub has_software_history: bool,
    /// Whether EXIF carries an embedded thumbnail.
    pub has_embedded_thumbnail: bool,
    /// Whether opaque maker-specific data is present.
    pub has_maker_specific_data: bool,
}

impl ImageDetails {
    /// Read file-system and supported EXIF facts without failing the open path.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let Ok(source) = crate::fs::ImageSource::open(path) else {
            return Self {
                format: extension_format_label(path).map(str::to_owned),
                ..Self::default()
            };
        };
        Self::load_from_source(path, &source)
    }

    /// Inspect facts through the retained handle that supplied the accepted pixels.
    #[must_use]
    pub(crate) fn load_from_source(path: &Path, source: &crate::fs::ImageSource) -> Self {
        Self::load_from_source_while(path, source, || true)
    }

    /// Inspect retained facts while cooperatively stopping superseded work.
    #[must_use]
    pub(crate) fn load_from_source_while(
        path: &Path,
        source: &crate::fs::ImageSource,
        mut keep_going: impl FnMut() -> bool,
    ) -> Self {
        if !source.version_is_current_while(&mut keep_going) {
            return Self::default();
        }
        let mut details = Self {
            file_bytes: source
                .clone_for_decode()
                .ok()
                .and_then(|file| file.metadata().ok())
                .map(|metadata| metadata.len()),
            format: detected_format_from_source(path, source),
            ..Self::default()
        };
        let metadata = load_bounded_metadata_from_source_while(source, &mut keep_going);
        if !source.version_is_current_while(&mut keep_going) {
            return Self::default();
        }
        let Some(metadata) = metadata else {
            return details;
        };

        inspect_metadata(&mut details, &metadata);
        if source.version_is_current_while(&mut keep_going) {
            details
        } else {
            Self::default()
        }
    }
}

fn inspect_metadata(details: &mut ImageDetails, metadata: &Metadata) {
    let mut make = None;
    let mut model = None;
    for tag in metadata.into_iter().take(MAX_EXIF_TAGS) {
        details.exif_tag_count = details.exif_tag_count.saturating_add(1);
        details.has_location |= tag.get_group() == ExifTagGroup::GPS;
        match tag {
            ExifTag::Make(value) => make = clean_text(value),
            ExifTag::Model(value) => model = clean_text(value),
            ExifTag::LensModel(value) => details.lens = clean_text(value),
            ExifTag::DateTimeOriginal(value) => {
                details.captured_at = clean_text(value).map(normalize_exif_date);
            }
            ExifTag::ExposureTime(values) => {
                details.exposure = values.first().and_then(format_exposure);
            }
            ExifTag::FNumber(values) => {
                details.aperture = values
                    .first()
                    .and_then(rational_value)
                    .map(|value| format!("f/{value:.1}"));
            }
            ExifTag::ISO(values) => {
                details.iso = values.first().map(|value| format!("ISO {value}"));
            }
            ExifTag::FocalLength(values) => {
                details.focal_length = values
                    .first()
                    .and_then(rational_value)
                    .map(|value| format!("{value:.1} mm"));
            }
            ExifTag::OwnerName(value) | ExifTag::Artist(value) | ExifTag::Copyright(value) => {
                details.has_owner_or_author |= clean_text(value).is_some();
            }
            ExifTag::SerialNumber(value)
            | ExifTag::LensSerialNumber(value)
            | ExifTag::ImageUniqueID(value) => {
                details.has_device_identifier |= clean_text(value).is_some();
            }
            ExifTag::ImageDescription(value) => {
                details.has_description_or_comment |= clean_text(value).is_some();
            }
            ExifTag::UserComment(value) => {
                details.has_description_or_comment |= !value.is_empty();
            }
            ExifTag::Software(value) => {
                details.has_software_history |= clean_text(value).is_some();
            }
            ExifTag::ThumbnailOffset(offsets, data) => {
                details.has_embedded_thumbnail |=
                    !data.is_empty() || offsets.iter().any(|offset| *offset != 0);
            }
            ExifTag::ThumbnailLength(lengths) => {
                details.has_embedded_thumbnail |= lengths.iter().any(|length| *length != 0);
            }
            ExifTag::MakerNote(value) => {
                details.has_maker_specific_data |= !value.is_empty();
            }
            _ => {}
        }
    }
    details.camera = combined_camera(make, model);
}

fn detected_format_from_source(path: &Path, source: &crate::fs::ImageSource) -> Option<String> {
    source
        .clone_for_decode()
        .ok()
        .map(BufReader::new)
        .map(image::ImageReader::new)
        .and_then(|reader| reader.with_guessed_format().ok())
        .and_then(|reader| reader.format())
        .map(format_label)
        .or_else(|| extension_format_label(path).map(str::to_owned))
}

fn extension_format_label(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "jpg" | "jpeg" => Some("JPEG"),
        "png" => Some("PNG"),
        "gif" => Some("GIF"),
        "webp" => Some("WebP"),
        "bmp" => Some("BMP"),
        "tif" | "tiff" => Some("TIFF"),
        "ico" => Some("ICO"),
        "pnm" | "pbm" | "pgm" | "ppm" | "pam" => Some("PNM"),
        "tga" => Some("TGA"),
        "qoi" => Some("QOI"),
        "dds" => Some("DDS"),
        "hdr" => Some("HDR"),
        "exr" => Some("OpenEXR"),
        "ff" | "farbfeld" => Some("farbfeld"),
        "jxl" => Some("JPEG XL"),
        "svg" => Some("SVG"),
        "avif" => Some("AVIF"),
        "heic" | "heif" | "hif" => Some("HEIF"),
        "cr2" | "nef" | "arw" | "dng" | "rw2" | "orf" | "raf" => Some("camera RAW"),
        _ => None,
    }
}

/// Parse supported EXIF containers without allowing metadata payloads to grow
/// past the viewer's fixed safety budget. Malformed, unsupported, or absent
/// metadata is intentionally indistinguishable from an empty metadata set.
pub(crate) fn load_bounded_metadata(path: &Path) -> Option<Metadata> {
    let file = crate::fs::open_file_no_atime(path).ok()?;
    load_bounded_metadata_from_file(file)
}

pub(crate) fn load_bounded_metadata_from_source(
    source: &crate::fs::ImageSource,
) -> Option<Metadata> {
    load_bounded_metadata_from_source_while(source, || true)
}

fn load_bounded_metadata_from_source_while(
    source: &crate::fs::ImageSource,
    mut keep_going: impl FnMut() -> bool,
) -> Option<Metadata> {
    if !source.version_is_current_while(&mut keep_going) {
        return None;
    }
    let metadata = load_bounded_metadata_from_file(source.clone_for_decode().ok()?);
    source
        .version_is_current_while(&mut keep_going)
        .then_some(metadata)
        .flatten()
}

fn load_bounded_metadata_from_file(file: impl Read + Seek) -> Option<Metadata> {
    let tiff = read_bounded_exif_file(file)?;
    if !validate_tiff_payload(&tiff) {
        return None;
    }
    std::panic::catch_unwind(|| Metadata::new_from_vec(&tiff, FileExtension::TIFF))
        .ok()?
        .ok()
}

#[cfg(test)]
fn read_bounded_exif(path: &Path) -> Option<Vec<u8>> {
    let file = crate::fs::open_file_no_atime(path).ok()?;
    read_bounded_exif_file(file)
}

fn read_bounded_exif_file(file: impl Read + Seek) -> Option<Vec<u8>> {
    let mut reader = BufReader::new(file);
    let mut signature = [0_u8; 12];
    let signature_len = reader.read(&mut signature).ok()?;
    reader.rewind().ok()?;

    if signature_len >= 8 && &signature[..8] == PNG_SIGNATURE {
        return read_png_exif(&mut reader);
    }
    if signature_len >= 2 && signature[..2] == [0xff, 0xd8] {
        return read_jpeg_exif(&mut reader);
    }
    if signature_len >= 12 && &signature[..4] == b"RIFF" && &signature[8..12] == b"WEBP" {
        return read_webp_exif(&mut reader);
    }
    if signature_len >= 4 && (signature[..4] == *b"II*\0" || signature[..4] == *b"MM\0*") {
        return read_tiff_metadata(&mut reader);
    }
    None
}

fn read_png_exif(reader: &mut (impl Read + Seek)) -> Option<Vec<u8>> {
    reader.seek(SeekFrom::Start(8)).ok()?;
    let mut scanned = 8_u64;
    for _ in 0..MAX_CONTAINER_CHUNKS {
        let length = u64::from(read_u32_be(reader)?);
        let mut kind = [0_u8; 4];
        reader.read_exact(&mut kind).ok()?;
        add_scanned(&mut scanned, length.checked_add(12)?)?;
        if &kind == b"eXIf" {
            if length > MAX_EXIF_BYTES {
                return None;
            }
            let payload = read_exact_vec(reader, length)?;
            let expected_crc = read_u32_be(reader)?;
            if png_crc_matches(kind, &payload, expected_crc) {
                return normalize_tiff_payload(payload);
            }
            continue;
        }
        if matches!(&kind, b"tEXt" | b"zTXt" | b"iTXt") && length <= MAX_PNG_RAW_PROFILE_BYTES {
            let payload = read_exact_vec(reader, length)?;
            let expected_crc = read_u32_be(reader)?;
            if png_crc_matches(kind, &payload, expected_crc) && png_text_has_exif_keyword(&payload)
            {
                return read_png_text_exif(kind, &payload);
            }
            continue;
        }
        let skip = length.checked_add(4)?;
        reader
            .seek(SeekFrom::Current(i64::try_from(skip).ok()?))
            .ok()?;
        if &kind == b"IEND" {
            return None;
        }
    }
    None
}

fn png_text_has_exif_keyword(payload: &[u8]) -> bool {
    payload
        .iter()
        .position(|byte| *byte == 0)
        .and_then(|keyword_end| payload.get(..keyword_end))
        == Some(PNG_RAW_PROFILE_KEYWORD)
}

fn read_png_text_exif(kind: [u8; 4], payload: &[u8]) -> Option<Vec<u8>> {
    let keyword_end = payload.iter().position(|byte| *byte == 0)?;
    if payload.get(..keyword_end)? != PNG_RAW_PROFILE_KEYWORD {
        return None;
    }
    let text = match &kind {
        b"tEXt" => payload.get(keyword_end.checked_add(1)?..)?.to_vec(),
        b"zTXt" => {
            let method = *payload.get(keyword_end.checked_add(1)?)?;
            if method != 0 {
                return None;
            }
            inflate_png_text(payload.get(keyword_end.checked_add(2)?..)?)?
        }
        b"iTXt" => read_itxt_data(payload, keyword_end)?,
        _ => return None,
    };
    decode_png_raw_profile(&text)
}

fn read_itxt_data(payload: &[u8], keyword_end: usize) -> Option<Vec<u8>> {
    let control_start = keyword_end.checked_add(1)?;
    let compression_flag = *payload.get(control_start)?;
    let compression_method = *payload.get(control_start.checked_add(1)?)?;
    if !matches!(compression_flag, 0 | 1) || compression_method != 0 {
        return None;
    }

    let language_start = control_start.checked_add(2)?;
    let language_end = find_nul(payload, language_start)?;
    let translated_start = language_end.checked_add(1)?;
    let translated_end = find_nul(payload, translated_start)?;
    let text = payload.get(translated_end.checked_add(1)?..)?;
    if compression_flag == 0 {
        if u64::try_from(text.len()).ok()? > MAX_PNG_RAW_PROFILE_BYTES {
            return None;
        }
        Some(text.to_vec())
    } else {
        inflate_png_text(text)
    }
}

fn find_nul(bytes: &[u8], start: usize) -> Option<usize> {
    bytes
        .get(start..)?
        .iter()
        .position(|byte| *byte == 0)
        .and_then(|offset| start.checked_add(offset))
}

fn inflate_png_text(compressed: &[u8]) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    let output_limit = MAX_PNG_RAW_PROFILE_BYTES.checked_add(1)?;
    ZlibDecoder::new(compressed)
        .take(output_limit)
        .read_to_end(&mut output)
        .ok()?;
    (u64::try_from(output.len()).ok()? <= MAX_PNG_RAW_PROFILE_BYTES).then_some(output)
}

fn decode_png_raw_profile(encoded: &[u8]) -> Option<Vec<u8>> {
    let encoded = encoded.strip_prefix(b"\nexif\n")?;
    let size_end = encoded.iter().position(|byte| *byte == b'\n')?;
    if size_end == 0 || size_end > 8 {
        return None;
    }
    let size_text = std::str::from_utf8(encoded.get(..size_end)?).ok()?.trim();
    if size_text.is_empty() || !size_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let declared_size = size_text.parse::<u64>().ok()?;
    if declared_size == 0 || declared_size > MAX_EXIF_BYTES.checked_add(7)? {
        return None;
    }

    let capacity = usize::try_from(declared_size).ok()?;
    let mut decoded = Vec::new();
    decoded.try_reserve_exact(capacity).ok()?;
    let mut high_nibble = None;
    for byte in encoded.get(size_end.checked_add(1)?..)? {
        if byte.is_ascii_whitespace() {
            continue;
        }
        let nibble = hex_nibble(*byte)?;
        if let Some(high) = high_nibble.take() {
            if decoded.len() == capacity {
                return None;
            }
            decoded.push((high << 4) | nibble);
        } else {
            high_nibble = Some(nibble);
        }
    }
    if high_nibble.is_some() || decoded.len() != capacity {
        return None;
    }
    if decoded.pop() != Some(0) {
        return None;
    }
    normalize_tiff_payload(decoded)
}

const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn png_crc_matches(kind: [u8; 4], payload: &[u8], expected: u32) -> bool {
    let mut hasher = Hasher::new();
    hasher.update(&kind);
    hasher.update(payload);
    hasher.finalize() == expected
}

fn read_jpeg_exif(reader: &mut (impl Read + Seek)) -> Option<Vec<u8>> {
    reader.seek(SeekFrom::Start(2)).ok()?;
    let mut scanned = 2_u64;
    for _ in 0..MAX_CONTAINER_CHUNKS {
        let mut marker_prefix = [0_u8; 1];
        reader.read_exact(&mut marker_prefix).ok()?;
        add_scanned(&mut scanned, 1)?;
        while marker_prefix[0] != 0xff {
            reader.read_exact(&mut marker_prefix).ok()?;
            add_scanned(&mut scanned, 1)?;
        }
        let mut marker = [0_u8; 1];
        reader.read_exact(&mut marker).ok()?;
        add_scanned(&mut scanned, 1)?;
        while marker[0] == 0xff {
            reader.read_exact(&mut marker).ok()?;
            add_scanned(&mut scanned, 1)?;
        }
        if matches!(marker[0], 0xd9 | 0xda) {
            return None;
        }
        if matches!(marker[0], 0x01 | 0xd0..=0xd8) {
            continue;
        }
        let segment_length = usize::from(read_u16_be(reader)?);
        add_scanned(&mut scanned, 2)?;
        let payload_length = segment_length.checked_sub(2)?;
        add_scanned(&mut scanned, u64::try_from(payload_length).ok()?)?;
        if marker[0] == 0xe1 {
            let payload = read_exact_vec(reader, u64::try_from(payload_length).ok()?)?;
            if payload.starts_with(b"Exif\0\0") {
                return normalize_tiff_payload(payload);
            }
        } else {
            reader
                .seek(SeekFrom::Current(i64::try_from(payload_length).ok()?))
                .ok()?;
        }
    }
    None
}

fn read_webp_exif(reader: &mut (impl Read + Seek)) -> Option<Vec<u8>> {
    reader.seek(SeekFrom::Start(12)).ok()?;
    let mut scanned = 12_u64;
    for _ in 0..MAX_CONTAINER_CHUNKS {
        let mut kind = [0_u8; 4];
        reader.read_exact(&mut kind).ok()?;
        let length = u64::from(read_u32_le(reader)?);
        let padded = length.checked_add(length % 2)?;
        add_scanned(&mut scanned, padded.checked_add(8)?)?;
        if &kind == b"EXIF" {
            if length > MAX_EXIF_BYTES {
                return None;
            }
            let payload = read_exact_vec(reader, length)?;
            return normalize_tiff_payload(payload);
        }
        reader
            .seek(SeekFrom::Current(i64::try_from(padded).ok()?))
            .ok()?;
    }
    None
}

fn read_exact_vec(reader: &mut impl Read, length: u64) -> Option<Vec<u8>> {
    let length = usize::try_from(length).ok()?;
    let mut payload = Vec::new();
    payload.try_reserve_exact(length).ok()?;
    payload.resize(length, 0);
    reader.read_exact(&mut payload).ok()?;
    Some(payload)
}

fn add_scanned(scanned: &mut u64, bytes: u64) -> Option<()> {
    *scanned = scanned.checked_add(bytes)?;
    (*scanned <= MAX_CONTAINER_SCAN_BYTES).then_some(())
}

fn read_tiff_metadata(reader: &mut (impl Read + Seek)) -> Option<Vec<u8>> {
    let file_len = reader.seek(SeekFrom::End(0)).ok()?;
    let header = read_tiff_range(reader, file_len, 0, 8)?;
    let endian = match header.get(..2)? {
        b"II" => TiffEndian::Little,
        b"MM" => TiffEndian::Big,
        _ => return None,
    };
    if endian.read_u16(header.get(2..4)?) != Some(42) {
        return None;
    }
    let first_ifd = endian.read_u32(header.get(4..8)?)?;
    if first_ifd == 0 {
        return None;
    }

    let mut pending = vec![first_ifd];
    let mut visited = std::collections::HashSet::new();
    let mut directories = Vec::new();
    let mut total_tags = 0_usize;
    let mut allocation_bytes = 0_usize;
    while let Some(old_offset) = pending.pop() {
        if old_offset == 0 {
            continue;
        }
        if !visited.insert(old_offset) || visited.len() > MAX_IFD_DIRECTORIES {
            return None;
        }
        let count_bytes = read_tiff_range(reader, file_len, u64::from(old_offset), 2)?;
        let entry_count = usize::from(endian.read_u16(&count_bytes)?);
        total_tags = total_tags.checked_add(entry_count)?;
        if total_tags > MAX_EXIF_TAGS {
            return None;
        }
        let directory_len = entry_count.checked_mul(12)?.checked_add(6)?;
        let directory = read_tiff_range(reader, file_len, u64::from(old_offset), directory_len)?;
        let entries_end = 2_usize.checked_add(entry_count.checked_mul(12)?)?;
        let mut entries = Vec::new();
        entries.try_reserve_exact(entry_count).ok()?;
        for raw in directory.get(2..entries_end)?.chunks_exact(12) {
            let tag = endian.read_u16(raw)?;
            if is_tiff_pixel_data_tag(tag) {
                continue;
            }
            let format = endian.read_u16(raw.get(2..)?)?;
            let components = endian.read_u32(raw.get(4..)?)?;
            let value_bytes =
                tiff_component_bytes(format)?.checked_mul(usize::try_from(components).ok()?)?;
            let peak_bytes = value_bytes.checked_mul(3)?;
            allocation_bytes = allocation_bytes.checked_add(peak_bytes)?;
            if allocation_bytes > MAX_EXIF_ALLOCATION_BYTES {
                return None;
            }
            let value_field: [u8; 4] = raw.get(8..12)?.try_into().ok()?;
            let value = if matches!(tag, 0x8769 | 0x8825 | 0xa005) {
                if format != 4 || components != 1 || value_bytes != 4 {
                    return None;
                }
                let target = endian.read_u32(&value_field)?;
                if target == 0 {
                    return None;
                }
                pending.push(target);
                TiffSourceValue::Directory(target)
            } else if value_bytes > 4 {
                let source_offset = endian.read_u32(&value_field)?;
                let data =
                    read_tiff_range(reader, file_len, u64::from(source_offset), value_bytes)?;
                TiffSourceValue::External(data)
            } else {
                TiffSourceValue::Inline(value_field)
            };
            entries.push(TiffSourceEntry {
                tag,
                format,
                components,
                value,
            });
        }
        let next_ifd = endian.read_u32(directory.get(entries_end..entries_end.checked_add(4)?)?)?;
        if next_ifd != 0 {
            pending.push(next_ifd);
        }
        directories.push(TiffSourceDirectory {
            old_offset,
            entries,
            next_ifd,
        });
    }
    compact_tiff_metadata(endian, first_ifd, &directories)
}

struct TiffSourceEntry {
    tag: u16,
    format: u16,
    components: u32,
    value: TiffSourceValue,
}

enum TiffSourceValue {
    Inline([u8; 4]),
    External(Vec<u8>),
    Directory(u32),
}

struct TiffSourceDirectory {
    old_offset: u32,
    entries: Vec<TiffSourceEntry>,
    next_ifd: u32,
}

fn read_tiff_range(
    reader: &mut (impl Read + Seek),
    file_len: u64,
    offset: u64,
    length: usize,
) -> Option<Vec<u8>> {
    let length_u64 = u64::try_from(length).ok()?;
    if offset.checked_add(length_u64)? > file_len {
        return None;
    }
    reader.seek(SeekFrom::Start(offset)).ok()?;
    read_exact_vec(reader, length_u64)
}

const fn is_tiff_pixel_data_tag(tag: u16) -> bool {
    matches!(
        tag,
        0x0111 | 0x0117 | 0x0120 | 0x0121 | 0x0144 | 0x0145 | 0x014a | 0x0201 | 0x0202
    )
}

fn compact_tiff_metadata(
    endian: TiffEndian,
    first_ifd: u32,
    directories: &[TiffSourceDirectory],
) -> Option<Vec<u8>> {
    let mut directory_offsets = std::collections::HashMap::new();
    let mut output_len = 8_usize;
    for directory in directories {
        let new_offset = u32::try_from(output_len).ok()?;
        if directory_offsets
            .insert(directory.old_offset, new_offset)
            .is_some()
        {
            return None;
        }
        output_len = output_len
            .checked_add(2)?
            .checked_add(directory.entries.len().checked_mul(12)?)?
            .checked_add(4)?;
    }

    let mut external_offsets = Vec::new();
    external_offsets.try_reserve_exact(directories.len()).ok()?;
    for directory in directories {
        let mut offsets = Vec::new();
        offsets.try_reserve_exact(directory.entries.len()).ok()?;
        for entry in &directory.entries {
            if let TiffSourceValue::External(data) = &entry.value {
                output_len = output_len.checked_add(output_len % 2)?;
                let offset = u32::try_from(output_len).ok()?;
                output_len = output_len.checked_add(data.len())?;
                offsets.push(Some(offset));
            } else {
                offsets.push(None);
            }
        }
        external_offsets.push(offsets);
    }
    if u64::try_from(output_len).ok()? > MAX_EXIF_BYTES {
        return None;
    }

    let mut output = vec![0_u8; output_len];
    output[..2].copy_from_slice(match endian {
        TiffEndian::Little => b"II",
        TiffEndian::Big => b"MM",
    });
    output[2..4].copy_from_slice(&endian.u16_bytes(42));
    let compact_first = *directory_offsets.get(&first_ifd)?;
    output[4..8].copy_from_slice(&endian.u32_bytes(compact_first));

    for (directory_index, directory) in directories.iter().enumerate() {
        let start = usize::try_from(*directory_offsets.get(&directory.old_offset)?).ok()?;
        let entry_count = u16::try_from(directory.entries.len()).ok()?;
        output
            .get_mut(start..start.checked_add(2)?)?
            .copy_from_slice(&endian.u16_bytes(entry_count));
        let mut cursor = start.checked_add(2)?;
        for (entry_index, entry) in directory.entries.iter().enumerate() {
            let entry_end = cursor.checked_add(12)?;
            let target = output.get_mut(cursor..entry_end)?;
            target[..2].copy_from_slice(&endian.u16_bytes(entry.tag));
            target[2..4].copy_from_slice(&endian.u16_bytes(entry.format));
            target[4..8].copy_from_slice(&endian.u32_bytes(entry.components));
            match &entry.value {
                TiffSourceValue::Inline(value) => target[8..12].copy_from_slice(value),
                TiffSourceValue::Directory(old_target) => {
                    let new_target = *directory_offsets.get(old_target)?;
                    target[8..12].copy_from_slice(&endian.u32_bytes(new_target));
                }
                TiffSourceValue::External(data) => {
                    let data_offset = *external_offsets.get(directory_index)?.get(entry_index)?;
                    let data_offset = data_offset?;
                    target[8..12].copy_from_slice(&endian.u32_bytes(data_offset));
                    let data_start = usize::try_from(data_offset).ok()?;
                    output
                        .get_mut(data_start..data_start.checked_add(data.len())?)?
                        .copy_from_slice(data);
                }
            }
            cursor = entry_end;
        }
        let compact_next = if directory.next_ifd == 0 {
            0
        } else {
            *directory_offsets.get(&directory.next_ifd)?
        };
        output
            .get_mut(cursor..cursor.checked_add(4)?)?
            .copy_from_slice(&endian.u32_bytes(compact_next));
    }
    validate_tiff_payload(&output).then_some(output)
}

fn normalize_tiff_payload(mut payload: Vec<u8>) -> Option<Vec<u8>> {
    if payload.starts_with(b"Exif\0\0") {
        payload.drain(..6);
    }
    (u64::try_from(payload.len()).ok()? <= MAX_EXIF_BYTES
        && (payload.starts_with(b"II*\0") || payload.starts_with(b"MM\0*")))
    .then_some(payload)
}

#[derive(Clone, Copy)]
enum TiffEndian {
    Little,
    Big,
}

impl TiffEndian {
    fn read_u16(self, bytes: &[u8]) -> Option<u16> {
        let bytes: [u8; 2] = bytes.get(..2)?.try_into().ok()?;
        Some(match self {
            Self::Little => u16::from_le_bytes(bytes),
            Self::Big => u16::from_be_bytes(bytes),
        })
    }

    fn read_u32(self, bytes: &[u8]) -> Option<u32> {
        let bytes: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
        Some(match self {
            Self::Little => u32::from_le_bytes(bytes),
            Self::Big => u32::from_be_bytes(bytes),
        })
    }

    const fn u16_bytes(self, value: u16) -> [u8; 2] {
        match self {
            Self::Little => value.to_le_bytes(),
            Self::Big => value.to_be_bytes(),
        }
    }

    const fn u32_bytes(self, value: u32) -> [u8; 4] {
        match self {
            Self::Little => value.to_le_bytes(),
            Self::Big => value.to_be_bytes(),
        }
    }
}

/// Validate every allocation-driving TIFF field before handing the payload to
/// `little_exif`. A bounded input alone is insufficient because a four-byte
/// component count or cyclic IFD pointer can otherwise request unbounded work.
fn validate_tiff_payload(payload: &[u8]) -> bool {
    let Some(byte_order) = payload.get(..2) else {
        return false;
    };
    let endian = match byte_order {
        b"II" => TiffEndian::Little,
        b"MM" => TiffEndian::Big,
        _ => return false,
    };
    if endian.read_u16(payload.get(2..4).unwrap_or_default()) != Some(42) {
        return false;
    }
    let Some(first_ifd) = endian.read_u32(payload.get(4..8).unwrap_or_default()) else {
        return false;
    };
    if first_ifd == 0 {
        return false;
    }

    let mut validation = TiffValidation::new(first_ifd);
    while let Some(ifd_offset) = validation.pending.pop() {
        if ifd_offset == 0 {
            continue;
        }
        if !validation.visited.insert(ifd_offset)
            || validation.visited.len() > MAX_IFD_DIRECTORIES
            || !validate_tiff_directory(payload, endian, ifd_offset, &mut validation)
        {
            return false;
        }
    }
    true
}

struct TiffValidation {
    pending: Vec<u32>,
    visited: std::collections::HashSet<u32>,
    total_tags: usize,
    allocation_bytes: usize,
}

impl TiffValidation {
    fn new(first_ifd: u32) -> Self {
        Self {
            pending: vec![first_ifd],
            visited: std::collections::HashSet::new(),
            total_tags: 0,
            allocation_bytes: 0,
        }
    }

    fn add_tags(&mut self, count: usize) -> bool {
        self.total_tags = self.total_tags.saturating_add(count);
        self.total_tags <= MAX_EXIF_TAGS
    }

    fn add_allocation(&mut self, bytes: usize) -> bool {
        self.allocation_bytes = self.allocation_bytes.saturating_add(bytes);
        self.allocation_bytes <= MAX_EXIF_ALLOCATION_BYTES
    }
}

#[derive(Default)]
struct TiffExternalData {
    strip_offsets: Option<Vec<u32>>,
    strip_byte_counts: Option<Vec<u32>>,
    thumbnail_offset: Option<u32>,
    thumbnail_length: Option<u32>,
}

fn validate_tiff_directory(
    payload: &[u8],
    endian: TiffEndian,
    ifd_offset: u32,
    validation: &mut TiffValidation,
) -> bool {
    let Ok(ifd_start) = usize::try_from(ifd_offset) else {
        return false;
    };
    let Some(entry_count) = endian.read_u16(payload.get(ifd_start..).unwrap_or_default()) else {
        return false;
    };
    let entry_count = usize::from(entry_count);
    if !validation.add_tags(entry_count) {
        return false;
    }
    let Some(entries_start) = ifd_start.checked_add(2) else {
        return false;
    };
    let Some(entries_end) = entry_count
        .checked_mul(12)
        .and_then(|bytes| entries_start.checked_add(bytes))
    else {
        return false;
    };
    let Some(next_end) = entries_end.checked_add(4) else {
        return false;
    };
    if next_end > payload.len() {
        return false;
    }

    let mut external = TiffExternalData::default();
    for entry in payload[entries_start..entries_end].chunks_exact(12) {
        if !validate_tiff_entry(payload, endian, entry, validation, &mut external) {
            return false;
        }
    }
    if !validate_tiff_external_references(payload, &external, validation) {
        return false;
    }
    let Some(next_ifd) = endian.read_u32(&payload[entries_end..next_end]) else {
        return false;
    };
    validation.pending.push(next_ifd);
    true
}

fn validate_tiff_entry(
    payload: &[u8],
    endian: TiffEndian,
    entry: &[u8],
    validation: &mut TiffValidation,
    external: &mut TiffExternalData,
) -> bool {
    let (Some(tag), Some(format), Some(components)) = (
        endian.read_u16(entry),
        endian.read_u16(&entry[2..]),
        endian.read_u32(&entry[4..]),
    ) else {
        return false;
    };
    let Some(value_bytes) = tiff_component_bytes(format).and_then(|component_bytes| {
        usize::try_from(components)
            .ok()
            .and_then(|count| count.checked_mul(component_bytes))
    }) else {
        return false;
    };
    if !value_bytes
        .checked_mul(3)
        .is_some_and(|peak| validation.add_allocation(peak))
    {
        return false;
    }

    let value_field = &entry[8..12];
    if value_bytes > 4 && !tiff_external_range_exists(payload, endian, value_field, value_bytes) {
        return false;
    }
    if matches!(tag, 0x8769 | 0x8825 | 0xa005) {
        if format != 4 || components != 1 || value_bytes != 4 {
            return false;
        }
        let Some(offset) = endian.read_u32(value_field).filter(|offset| *offset != 0) else {
            return false;
        };
        validation.pending.push(offset);
    }

    let values = || {
        tiff_u32_values(
            payload,
            endian,
            format,
            components,
            value_field,
            value_bytes,
        )
    };
    match tag {
        0x0111 => external.strip_offsets = values(),
        0x0117 => external.strip_byte_counts = values(),
        0x0201 => {
            external.thumbnail_offset = values().and_then(|values| single_tiff_value(&values));
        }
        0x0202 => {
            external.thumbnail_length = values().and_then(|values| single_tiff_value(&values));
        }
        _ => return true,
    }
    match tag {
        0x0111 => external.strip_offsets.is_some(),
        0x0117 => external.strip_byte_counts.is_some(),
        0x0201 => external.thumbnail_offset.is_some(),
        0x0202 => external.thumbnail_length.is_some(),
        _ => true,
    }
}

fn single_tiff_value(values: &[u32]) -> Option<u32> {
    (values.len() == 1).then(|| values[0])
}

fn tiff_external_range_exists(
    payload: &[u8],
    endian: TiffEndian,
    value_field: &[u8],
    value_bytes: usize,
) -> bool {
    endian
        .read_u32(value_field)
        .and_then(|offset| usize::try_from(offset).ok())
        .and_then(|offset| offset.checked_add(value_bytes))
        .is_some_and(|end| end <= payload.len())
}

fn validate_tiff_external_references(
    payload: &[u8],
    external: &TiffExternalData,
    validation: &mut TiffValidation,
) -> bool {
    match (&external.strip_offsets, &external.strip_byte_counts) {
        (Some(offsets), Some(counts)) if offsets.len() == counts.len() => {
            if !validate_external_tiff_data(
                payload,
                offsets.iter().copied().zip(counts.iter().copied()),
                validation,
            ) {
                return false;
            }
        }
        (None, None) => {}
        _ => return false,
    }
    match (external.thumbnail_offset, external.thumbnail_length) {
        (Some(offset), Some(length)) => {
            if !validate_external_tiff_data(payload, std::iter::once((offset, length)), validation)
            {
                return false;
            }
        }
        (None, None) => {}
        _ => return false,
    }
    true
}

const fn tiff_component_bytes(format: u16) -> Option<usize> {
    match format {
        1 | 2 | 6 | 7 => Some(1),
        3 | 8 => Some(2),
        4 | 9 | 11 => Some(4),
        5 | 10 | 12 => Some(8),
        _ => None,
    }
}

fn tiff_u32_values(
    payload: &[u8],
    endian: TiffEndian,
    format: u16,
    components: u32,
    value_field: &[u8],
    value_bytes: usize,
) -> Option<Vec<u32>> {
    if format != 4 || value_bytes == 0 {
        return None;
    }
    let data = if value_bytes <= 4 {
        value_field.get(..value_bytes)?
    } else {
        let offset = usize::try_from(endian.read_u32(value_field)?).ok()?;
        payload.get(offset..offset.checked_add(value_bytes)?)?
    };
    let component_count = usize::try_from(components).ok()?;
    let mut values = Vec::new();
    values.try_reserve_exact(component_count).ok()?;
    for bytes in data.chunks_exact(4) {
        values.push(endian.read_u32(bytes)?);
    }
    (values.len() == component_count).then_some(values)
}

fn validate_external_tiff_data(
    payload: &[u8],
    regions: impl Iterator<Item = (u32, u32)>,
    validation: &mut TiffValidation,
) -> bool {
    for (offset, length) in regions {
        let (Ok(offset), Ok(length)) = (usize::try_from(offset), usize::try_from(length)) else {
            return false;
        };
        if offset
            .checked_add(length)
            .is_none_or(|end| end > payload.len())
        {
            return false;
        }
        if !validation.add_allocation(length) {
            return false;
        }
    }
    true
}

fn read_u16_be(reader: &mut impl Read) -> Option<u16> {
    let mut bytes = [0_u8; 2];
    reader.read_exact(&mut bytes).ok()?;
    Some(u16::from_be_bytes(bytes))
}

fn read_u32_be(reader: &mut impl Read) -> Option<u32> {
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes).ok()?;
    Some(u32::from_be_bytes(bytes))
}

fn read_u32_le(reader: &mut impl Read) -> Option<u32> {
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes).ok()?;
    Some(u32::from_le_bytes(bytes))
}

/// Format bytes using compact binary units.
#[must_use]
pub fn format_file_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Reduced pixel aspect string, such as `3:2`.
#[must_use]
pub fn aspect_ratio(width: u32, height: u32) -> Option<String> {
    if width == 0 || height == 0 {
        return None;
    }
    let divisor = greatest_common_divisor(width, height);
    Some(format!("{}:{}", width / divisor, height / divisor))
}

fn format_label(format: image::ImageFormat) -> String {
    match format {
        image::ImageFormat::Jpeg => "JPEG",
        image::ImageFormat::Png => "PNG",
        image::ImageFormat::Gif => "GIF",
        image::ImageFormat::WebP => "WebP",
        image::ImageFormat::Pnm => "PNM",
        image::ImageFormat::Tiff => "TIFF",
        image::ImageFormat::Tga => "TGA",
        image::ImageFormat::Dds => "DDS",
        image::ImageFormat::Bmp => "BMP",
        image::ImageFormat::Ico => "ICO",
        image::ImageFormat::Hdr => "HDR",
        image::ImageFormat::OpenExr => "OpenEXR",
        image::ImageFormat::Farbfeld => "farbfeld",
        image::ImageFormat::Avif => "AVIF",
        image::ImageFormat::Qoi => "QOI",
        _ => return format!("{format:?}"),
    }
    .to_owned()
}

fn clean_text(value: &str) -> Option<String> {
    let value = value.trim_matches('\0').trim();
    (!value.is_empty() && value.len() <= MAX_TEXT_BYTES).then(|| value.to_owned())
}

fn combined_camera(make: Option<String>, model: Option<String>) -> Option<String> {
    match (make, model) {
        (Some(make), Some(model)) if model.to_lowercase().starts_with(&make.to_lowercase()) => {
            Some(model)
        }
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (Some(make), None) => Some(make),
        (None, Some(model)) => Some(model),
        (None, None) => None,
    }
}

fn normalize_exif_date(value: String) -> String {
    let mut bytes = value.into_bytes();
    if bytes.len() >= 10 && bytes.get(4) == Some(&b':') && bytes.get(7) == Some(&b':') {
        if let Some(b) = bytes.get_mut(4) {
            *b = b'-';
        }
        if let Some(b) = bytes.get_mut(7) {
            *b = b'-';
        }
    }
    String::from_utf8(bytes).unwrap_or_default()
}

fn rational_value(value: &uR64) -> Option<f64> {
    (value.denominator != 0).then(|| f64::from(value.clone()))
}

fn format_exposure(value: &uR64) -> Option<String> {
    let seconds = rational_value(value)?;
    if seconds <= 0.0 {
        return None;
    }
    if seconds < 1.0 {
        Some(format!("1/{:.0} s", 1.0 / seconds))
    } else {
        Some(format!("{seconds:.1} s"))
    }
}

const fn greatest_common_divisor(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ephemeral::TempWorkspace;
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::{Cursor, Write};

    fn minimal_tiff(endian: TiffEndian) -> Vec<u8> {
        let mut bytes = match endian {
            TiffEndian::Little => b"II*\0".to_vec(),
            TiffEndian::Big => b"MM\0*".to_vec(),
        };
        match endian {
            TiffEndian::Little => bytes.extend_from_slice(&8_u32.to_le_bytes()),
            TiffEndian::Big => bytes.extend_from_slice(&8_u32.to_be_bytes()),
        }
        bytes.extend_from_slice(&[0; 6]);
        bytes
    }

    fn png_raw_profile(tiff: &[u8]) -> Vec<u8> {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut framed = b"Exif\0\0".to_vec();
        framed.extend_from_slice(tiff);
        framed.push(0);

        let mut encoded = b"\nexif\n".to_vec();
        encoded.extend_from_slice(format!("{:>8}\n", framed.len()).as_bytes());
        for byte in framed {
            encoded.push(HEX[usize::from(byte >> 4)]);
            encoded.push(HEX[usize::from(byte & 0x0f)]);
        }
        encoded.push(b'\n');
        encoded
    }

    fn compressed(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn text_chunk_payload(kind: [u8; 4], text: &[u8], compressed_text: bool) -> Vec<u8> {
        let mut payload = PNG_RAW_PROFILE_KEYWORD.to_vec();
        payload.push(0);
        match &kind {
            b"tEXt" => payload.extend_from_slice(text),
            b"zTXt" => {
                payload.push(0);
                payload.extend_from_slice(&compressed(text));
            }
            b"iTXt" => {
                payload.extend_from_slice(&[u8::from(compressed_text), 0]);
                payload.extend_from_slice(b"en\0EXIF\0");
                if compressed_text {
                    payload.extend_from_slice(&compressed(text));
                } else {
                    payload.extend_from_slice(text);
                }
            }
            _ => unreachable!("test helper only builds PNG text chunks"),
        }
        payload
    }

    fn append_png_chunk(png: &mut Vec<u8>, kind: [u8; 4], payload: &[u8], crc: Option<u32>) {
        png.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_be_bytes());
        png.extend_from_slice(&kind);
        png.extend_from_slice(payload);
        let calculated_crc = {
            let mut hasher = Hasher::new();
            hasher.update(&kind);
            hasher.update(payload);
            hasher.finalize()
        };
        png.extend_from_slice(&crc.unwrap_or(calculated_crc).to_be_bytes());
    }

    fn complex_tiff() -> Vec<u8> {
        fn entry(bytes: &mut Vec<u8>, tag: u16, format: u16, count: u32, value: u32) {
            bytes.extend_from_slice(&tag.to_le_bytes());
            bytes.extend_from_slice(&format.to_le_bytes());
            bytes.extend_from_slice(&count.to_le_bytes());
            bytes.extend_from_slice(&value.to_le_bytes());
        }

        let mut bytes = b"II*\0".to_vec();
        bytes.extend_from_slice(&8_u32.to_le_bytes());
        bytes.extend_from_slice(&6_u16.to_le_bytes());
        entry(&mut bytes, 0x010f, 2, 5, 86);
        entry(&mut bytes, 0x0111, 4, 2, 91);
        entry(&mut bytes, 0x0117, 4, 2, 99);
        entry(&mut bytes, 0x0201, 4, 1, 120);
        entry(&mut bytes, 0x0202, 4, 1, 4);
        entry(&mut bytes, 0xc001, 4, 2, 107);
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(b"Acme\0");
        bytes.extend_from_slice(&115_u32.to_le_bytes());
        bytes.extend_from_slice(&118_u32.to_le_bytes());
        bytes.extend_from_slice(&3_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u32.to_le_bytes());
        bytes.extend_from_slice(b"abcdeWXYZ");
        bytes
    }

    #[test]
    fn file_size_and_aspect_labels_cover_boundaries() {
        assert_eq!(format_file_size(999), "999 B");
        assert_eq!(format_file_size(1024), "1.0 KiB");
        assert_eq!(format_file_size(1024 * 1024), "1.0 MiB");
        assert_eq!(format_file_size(1024 * 1024 * 1024), "1.00 GiB");
        assert_eq!(aspect_ratio(6000, 4000).as_deref(), Some("3:2"));
        assert_eq!(aspect_ratio(1920, 1080).as_deref(), Some("16:9"));
        assert_eq!(aspect_ratio(0, 1080), None);
    }

    #[test]
    fn camera_metadata_is_read_and_location_is_only_disclosed_as_present() {
        let workspace = TempWorkspace::new("image_details").unwrap();
        let path = workspace.path().join("details.jpg");
        image::RgbImage::from_pixel(4, 3, image::Rgb([1, 2, 3]))
            .save(&path)
            .unwrap();
        let mut metadata = Metadata::new();
        metadata.set_tag(ExifTag::Make("Acme".into()));
        metadata.set_tag(ExifTag::Model("Acme One".into()));
        metadata.set_tag(ExifTag::LensModel("Prime 50".into()));
        metadata.set_tag(ExifTag::DateTimeOriginal("2026:07:25 12:34:56".into()));
        metadata.set_tag(ExifTag::ExposureTime(vec![uR64 {
            nominator: 1,
            denominator: 125,
        }]));
        metadata.set_tag(ExifTag::FNumber(vec![uR64 {
            nominator: 28,
            denominator: 10,
        }]));
        metadata.set_tag(ExifTag::ISO(vec![400]));
        metadata.set_tag(ExifTag::FocalLength(vec![uR64 {
            nominator: 50,
            denominator: 1,
        }]));
        metadata.set_tag(ExifTag::GPSLatitude(vec![uR64 {
            nominator: 1,
            denominator: 1,
        }]));
        metadata.write_to_file(&path).unwrap();

        let details = ImageDetails::load(&path);
        assert_eq!(details.format.as_deref(), Some("JPEG"));
        assert_eq!(details.camera.as_deref(), Some("Acme One"));
        assert_eq!(details.lens.as_deref(), Some("Prime 50"));
        assert_eq!(details.captured_at.as_deref(), Some("2026-07-25 12:34:56"));
        assert_eq!(details.exposure.as_deref(), Some("1/125 s"));
        assert_eq!(details.aperture.as_deref(), Some("f/2.8"));
        assert_eq!(details.iso.as_deref(), Some("ISO 400"));
        assert_eq!(details.focal_length.as_deref(), Some("50.0 mm"));
        assert!(details.exif_tag_count >= 9);
        assert!(details.has_location);
        assert!(details.file_bytes.is_some_and(|bytes| bytes > 0));
    }

    #[test]
    fn privacy_summary_classifies_presence_without_retaining_sensitive_values() {
        let mut metadata = Metadata::new();
        metadata.set_tag(ExifTag::GPSLatitude(vec![uR64 {
            nominator: 1,
            denominator: 1,
        }]));
        metadata.set_tag(ExifTag::OwnerName("Private Owner".into()));
        metadata.set_tag(ExifTag::SerialNumber("PRIVATE-SERIAL".into()));
        metadata.set_tag(ExifTag::UserComment(b"private comment".to_vec()));
        metadata.set_tag(ExifTag::Software("Private Editor".into()));
        metadata.set_tag(ExifTag::ThumbnailLength(vec![128]));
        metadata.set_tag(ExifTag::MakerNote(b"private maker data".to_vec()));

        let mut details = ImageDetails::default();
        inspect_metadata(&mut details, &metadata);

        assert_eq!(details.exif_tag_count, 7);
        assert!(details.has_location);
        assert!(details.has_owner_or_author);
        assert!(details.has_device_identifier);
        assert!(details.has_description_or_comment);
        assert!(details.has_software_history);
        assert!(details.has_embedded_thumbnail);
        assert!(details.has_maker_specific_data);
        assert_eq!(details.camera, None);
        assert_eq!(details.lens, None);
    }

    #[test]
    fn accepted_source_details_fail_closed_after_a_later_path_replacement() {
        let workspace = TempWorkspace::new("image_details_source_binding").unwrap();
        let path = workspace.path().join("details.jpg");
        image::RgbImage::from_pixel(4, 3, image::Rgb([1, 2, 3]))
            .save(&path)
            .unwrap();
        let mut original_metadata = Metadata::new();
        original_metadata.set_tag(ExifTag::Model("Original Camera".into()));
        original_metadata.write_to_file(&path).unwrap();
        let source = crate::fs::ImageSource::open(&path).unwrap();

        std::fs::rename(&path, workspace.path().join("original.jpg")).unwrap();
        image::RgbImage::from_pixel(4, 3, image::Rgb([9, 8, 7]))
            .save(&path)
            .unwrap();
        let mut replacement_metadata = Metadata::new();
        replacement_metadata.set_tag(ExifTag::Model("Replacement Camera".into()));
        replacement_metadata.write_to_file(&path).unwrap();

        let details = ImageDetails::load_from_source(&path, &source);
        assert_eq!(details, ImageDetails::default());
    }

    #[test]
    fn accepted_source_details_reject_an_in_place_rewrite() {
        let workspace = TempWorkspace::new("image_details_source_version").unwrap();
        let path = workspace.path().join("details.jpg");
        let replacement = workspace.path().join("replacement.jpg");
        image::RgbImage::from_pixel(4, 3, image::Rgb([1, 2, 3]))
            .save(&path)
            .unwrap();
        let mut original_metadata = Metadata::new();
        original_metadata.set_tag(ExifTag::Model("Original Camera".into()));
        original_metadata.write_to_file(&path).unwrap();
        let source = crate::fs::ImageSource::open(&path).unwrap();

        image::RgbImage::from_pixel(4, 3, image::Rgb([9, 8, 7]))
            .save(&replacement)
            .unwrap();
        let mut replacement_metadata = Metadata::new();
        replacement_metadata.set_tag(ExifTag::Model("Replacement Camera".into()));
        replacement_metadata.write_to_file(&replacement).unwrap();
        std::fs::write(&path, std::fs::read(&replacement).unwrap()).unwrap();

        assert_eq!(
            source.matches_path(&path),
            crate::fs::ImageSourceMatch::Changed
        );
        let details = ImageDetails::load_from_source(&path, &source);
        assert_eq!(details, ImageDetails::default());
    }

    #[test]
    fn accepted_source_details_stop_at_a_superseded_version_boundary() {
        let workspace = TempWorkspace::new("image_details_cancellation").unwrap();
        let path = workspace.path().join("details.jpg");
        image::RgbImage::from_pixel(4, 3, image::Rgb([1, 2, 3]))
            .save(&path)
            .unwrap();
        let source = crate::fs::ImageSource::open(&path).unwrap();
        let checks = std::cell::Cell::new(0_u8);

        let details = ImageDetails::load_from_source_while(&path, &source, || {
            let next = checks.get().saturating_add(1);
            checks.set(next);
            next < 3
        });

        assert_eq!(checks.get(), 3);
        assert_eq!(details, ImageDetails::default());
    }

    #[test]
    fn png_raw_profile_metadata_written_by_little_exif_is_read() {
        let workspace = TempWorkspace::new("image_details_png_metadata").unwrap();
        let path = workspace.path().join("details.png");
        image::RgbImage::from_pixel(4, 3, image::Rgb([1, 2, 3]))
            .save(&path)
            .unwrap();
        let mut metadata = Metadata::new();
        metadata.set_tag(ExifTag::Make("Acme".into()));
        metadata.set_tag(ExifTag::Model("Two".into()));
        metadata.write_to_file(&path).unwrap();

        let details = ImageDetails::load(&path);
        assert_eq!(details.format.as_deref(), Some("PNG"));
        assert_eq!(details.camera.as_deref(), Some("Acme Two"));
    }

    #[test]
    fn malformed_or_absent_metadata_is_best_effort() {
        let details = ImageDetails::load(Path::new("missing-viewr-image.jpg"));
        assert_eq!(details.file_bytes, None);
        assert_eq!(details.camera, None);
        assert_eq!(details.exif_tag_count, 0);
        assert!(!details.has_location);
        assert_eq!(clean_text("\0  \0"), None);
        assert_eq!(
            combined_camera(Some("A".into()), Some("B".into())).as_deref(),
            Some("A B")
        );
        assert_eq!(
            format_exposure(&uR64 {
                nominator: 1,
                denominator: 0
            }),
            None
        );
        assert_eq!(clean_text(&"x".repeat(MAX_TEXT_BYTES + 1)), None);
    }

    #[test]
    fn metadata_formatting_handles_partial_and_edge_values() {
        assert_eq!(clean_text("\0 Camera \0").as_deref(), Some("Camera"));
        assert_eq!(
            combined_camera(Some("Acme".into()), None).as_deref(),
            Some("Acme")
        );
        assert_eq!(
            combined_camera(None, Some("One".into())).as_deref(),
            Some("One")
        );
        assert_eq!(combined_camera(None, None), None);
        assert_eq!(normalize_exif_date("short".into()), "short");
        assert_eq!(
            normalize_exif_date("2026:07:25 12:34:56".into()),
            "2026-07-25 12:34:56"
        );
        assert_eq!(
            format_exposure(&uR64 {
                nominator: 2,
                denominator: 1,
            })
            .as_deref(),
            Some("2.0 s")
        );
        assert_eq!(
            format_exposure(&uR64 {
                nominator: 0,
                denominator: 1,
            }),
            None
        );
    }

    #[test]
    #[allow(deprecated)]
    fn all_supported_extension_and_detected_format_labels_are_stable() {
        let extension_cases = [
            ("image.JPG", "JPEG"),
            ("image.png", "PNG"),
            ("image.gif", "GIF"),
            ("image.webp", "WebP"),
            ("image.bmp", "BMP"),
            ("image.tiff", "TIFF"),
            ("image.ico", "ICO"),
            ("image.pam", "PNM"),
            ("image.tga", "TGA"),
            ("image.qoi", "QOI"),
            ("image.dds", "DDS"),
            ("image.hdr", "HDR"),
            ("image.exr", "OpenEXR"),
            ("image.farbfeld", "farbfeld"),
            ("image.jxl", "JPEG XL"),
            ("image.svg", "SVG"),
            ("image.avif", "AVIF"),
            ("image.heic", "HEIF"),
            ("image.dng", "camera RAW"),
        ];
        for (path, expected) in extension_cases {
            assert_eq!(extension_format_label(Path::new(path)), Some(expected));
        }
        assert_eq!(extension_format_label(Path::new("image.unknown")), None);
        assert_eq!(extension_format_label(Path::new("no-extension")), None);

        let format_cases = [
            (image::ImageFormat::Jpeg, "JPEG"),
            (image::ImageFormat::Png, "PNG"),
            (image::ImageFormat::Gif, "GIF"),
            (image::ImageFormat::WebP, "WebP"),
            (image::ImageFormat::Pnm, "PNM"),
            (image::ImageFormat::Tiff, "TIFF"),
            (image::ImageFormat::Tga, "TGA"),
            (image::ImageFormat::Dds, "DDS"),
            (image::ImageFormat::Bmp, "BMP"),
            (image::ImageFormat::Ico, "ICO"),
            (image::ImageFormat::Hdr, "HDR"),
            (image::ImageFormat::OpenExr, "OpenEXR"),
            (image::ImageFormat::Farbfeld, "farbfeld"),
            (image::ImageFormat::Avif, "AVIF"),
            (image::ImageFormat::Qoi, "QOI"),
            (image::ImageFormat::Pcx, "Pcx"),
        ];
        for (format, expected) in format_cases {
            assert_eq!(format_label(format), expected);
        }
    }

    #[test]
    fn format_uses_file_content_before_a_misleading_extension() {
        let workspace = TempWorkspace::new("image_details_format").unwrap();
        let path = workspace.path().join("actually-png.jpg");
        image::save_buffer_with_format(
            &path,
            &[1, 2, 3],
            1,
            1,
            image::ColorType::Rgb8,
            image::ImageFormat::Png,
        )
        .unwrap();

        assert_eq!(ImageDetails::load(&path).format.as_deref(), Some("PNG"));
    }

    #[test]
    fn oversized_png_exif_is_rejected_from_its_header_without_allocation() {
        let workspace = TempWorkspace::new("image_details_exif_bound").unwrap();
        let path = workspace.path().join("oversized.png");
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend_from_slice(&u32::try_from(MAX_EXIF_BYTES + 1).unwrap().to_be_bytes());
        bytes.extend_from_slice(b"eXIf");
        std::fs::write(&path, bytes).unwrap();

        assert_eq!(read_bounded_exif(&path), None);
    }

    #[test]
    fn png_text_exif_variants_decode_with_crc_validation() {
        let tiff = minimal_tiff(TiffEndian::Little);
        let raw_profile = png_raw_profile(&tiff);
        for (kind, compressed_text) in [
            (*b"tEXt", false),
            (*b"zTXt", true),
            (*b"iTXt", false),
            (*b"iTXt", true),
        ] {
            let payload = text_chunk_payload(kind, &raw_profile, compressed_text);
            assert_eq!(read_png_text_exif(kind, &payload), Some(tiff.clone()));
        }

        let workspace = TempWorkspace::new("image_details_png_crc").unwrap();
        let path = workspace.path().join("metadata.png");
        let payload = text_chunk_payload(*b"zTXt", &raw_profile, true);
        let mut png = PNG_SIGNATURE.to_vec();
        append_png_chunk(&mut png, *b"zTXt", &payload, Some(0));
        append_png_chunk(&mut png, *b"zTXt", &payload, None);
        std::fs::write(&path, png).unwrap();
        assert_eq!(read_bounded_exif(&path), Some(tiff));
    }

    #[test]
    fn malformed_png_text_metadata_and_decompression_bombs_fail_closed() {
        assert_eq!(read_png_text_exif(*b"tEXt", b"missing separator"), None);
        assert_eq!(read_png_text_exif(*b"tEXt", b"Comment\0value"), None);
        assert_eq!(
            read_png_text_exif(*b"zTXt", b"Raw profile type exif\0\x01data"),
            None
        );
        assert_eq!(
            read_png_text_exif(*b"iTXt", b"Raw profile type exif\0\x02\0en\0EXIF\0data"),
            None
        );
        assert_eq!(
            read_png_text_exif(*b"iTXt", b"Raw profile type exif\0\x01\x01en\0EXIF\0data"),
            None
        );
        assert_eq!(
            read_png_text_exif(*b"iTXt", b"Raw profile type exif\0\0\0unterminated"),
            None
        );

        for malformed in [
            b"not a profile".as_slice(),
            b"\nexif\n\n00\n",
            b"\nexif\n123456789\n00\n",
            b"\nexif\n  nope  \n00\n",
            b"\nexif\n       0\n00\n",
            b"\nexif\n       2\n0\n",
            b"\nexif\n       2\nzz\n",
            b"\nexif\n       1\n0000\n",
            b"\nexif\n       2\n00\n",
        ] {
            assert_eq!(decode_png_raw_profile(malformed), None);
        }

        let oversized = vec![0_u8; usize::try_from(MAX_PNG_RAW_PROFILE_BYTES + 1).unwrap()];
        assert_eq!(inflate_png_text(&compressed(&oversized)), None);

        let workspace = TempWorkspace::new("image_details_png_work_bound").unwrap();
        let path = workspace.path().join("duplicate-raw-profile.png");
        let mut invalid = PNG_RAW_PROFILE_KEYWORD.to_vec();
        invalid.extend_from_slice(b"\0\0not-zlib");
        let valid = text_chunk_payload(
            *b"zTXt",
            &png_raw_profile(&minimal_tiff(TiffEndian::Little)),
            true,
        );
        let mut png = PNG_SIGNATURE.to_vec();
        append_png_chunk(&mut png, *b"zTXt", &invalid, None);
        append_png_chunk(&mut png, *b"zTXt", &valid, None);
        std::fs::write(&path, png).unwrap();
        assert_eq!(read_bounded_exif(&path), None);
    }

    #[test]
    fn jpeg_webp_and_tiff_container_readers_are_bounded_and_content_driven() {
        let tiff = minimal_tiff(TiffEndian::Little);
        let mut exif = b"Exif\0\0".to_vec();
        exif.extend_from_slice(&tiff);

        let mut jpeg = vec![0xff, 0xd8, 0, 0xff, 0xd0, 0xff, 0xe0, 0, 3, 7, 0xff, 0xe1];
        jpeg.extend_from_slice(&u16::try_from(exif.len() + 2).unwrap().to_be_bytes());
        jpeg.extend_from_slice(&exif);
        assert_eq!(read_jpeg_exif(&mut Cursor::new(jpeg)), Some(tiff.clone()));
        assert_eq!(
            read_jpeg_exif(&mut Cursor::new(vec![0xff, 0xd8, 0xff, 0xda])),
            None
        );
        assert_eq!(
            read_jpeg_exif(&mut Cursor::new(vec![0xff, 0xd8, 0xff, 0xe1, 0, 1])),
            None
        );

        let mut webp = b"RIFF\0\0\0\0WEBP".to_vec();
        webp.extend_from_slice(b"JUNK");
        webp.extend_from_slice(&3_u32.to_le_bytes());
        webp.extend_from_slice(b"abc\0");
        webp.extend_from_slice(b"EXIF");
        webp.extend_from_slice(&u32::try_from(exif.len()).unwrap().to_le_bytes());
        webp.extend_from_slice(&exif);
        assert_eq!(read_webp_exif(&mut Cursor::new(webp)), Some(tiff.clone()));

        assert_eq!(
            read_tiff_metadata(&mut Cursor::new(tiff.clone())),
            Some(tiff)
        );
        assert_eq!(read_tiff_metadata(&mut Cursor::new(b"not tiff")), None);
        let mut scanned = MAX_CONTAINER_SCAN_BYTES;
        assert_eq!(add_scanned(&mut scanned, 0), Some(()));
        assert_eq!(add_scanned(&mut scanned, 1), None);
        let mut overflowed = u64::MAX;
        assert_eq!(add_scanned(&mut overflowed, 1), None);
    }

    #[test]
    fn sparse_tiff_metadata_beyond_the_prefix_is_compacted_without_pixel_strips() {
        fn entry(file: &mut std::fs::File, tag: u16, format: u16, count: u32, value: u32) {
            file.write_all(&tag.to_le_bytes()).unwrap();
            file.write_all(&format.to_le_bytes()).unwrap();
            file.write_all(&count.to_le_bytes()).unwrap();
            file.write_all(&value.to_le_bytes()).unwrap();
        }

        let workspace = TempWorkspace::new("sparse_tiff_metadata").unwrap();
        let path = workspace.path().join("large.tiff");
        let first_ifd = 3 * 1024 * 1024_u32;
        let make_offset = first_ifd + 2 + 3 * 12 + 4;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.write_all(b"II*\0").unwrap();
        file.write_all(&first_ifd.to_le_bytes()).unwrap();
        file.seek(SeekFrom::Start(u64::from(first_ifd))).unwrap();
        file.write_all(&3_u16.to_le_bytes()).unwrap();
        entry(&mut file, 0x010f, 2, 5, make_offset);
        entry(&mut file, 0x0111, 4, 1, make_offset + 5);
        entry(&mut file, 0x0117, 4, 1, 4 * 1024 * 1024);
        file.write_all(&0_u32.to_le_bytes()).unwrap();
        file.write_all(b"Acme\0").unwrap();
        file.flush().unwrap();
        file.rewind().unwrap();

        let compact = read_tiff_metadata(&mut file).expect("bounded metadata should be retained");
        assert!(compact.len() < 64);
        assert!(validate_tiff_payload(&compact));
        assert_eq!(u16::from_le_bytes([compact[8], compact[9]]), 1);
        assert_eq!(u16::from_le_bytes([compact[10], compact[11]]), 0x010f);

        drop(file);
        let details = ImageDetails::load(&path);
        assert_eq!(details.camera.as_deref(), Some("Acme"));
    }

    #[test]
    fn tiff_preflight_accepts_valid_endianness_and_bounded_external_data() {
        assert!(validate_tiff_payload(&minimal_tiff(TiffEndian::Little)));
        assert!(validate_tiff_payload(&minimal_tiff(TiffEndian::Big)));

        let valid = complex_tiff();
        assert!(validate_tiff_payload(&valid));
        let mut bad_strip = valid.clone();
        bad_strip[91..95].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(!validate_tiff_payload(&bad_strip));
        let mut bad_thumbnail = valid;
        bad_thumbnail[54..58].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(!validate_tiff_payload(&bad_thumbnail));
    }

    #[test]
    fn tiff_preflight_rejects_hostile_allocation_counts_and_ifd_cycles() {
        let mut oversized_value = b"II*\0".to_vec();
        oversized_value.extend_from_slice(&8_u32.to_le_bytes());
        oversized_value.extend_from_slice(&1_u16.to_le_bytes());
        oversized_value.extend_from_slice(&0x010f_u16.to_le_bytes());
        oversized_value.extend_from_slice(&1_u16.to_le_bytes());
        oversized_value.extend_from_slice(&u32::MAX.to_le_bytes());
        oversized_value.extend_from_slice(&0_u32.to_le_bytes());
        oversized_value.extend_from_slice(&0_u32.to_le_bytes());
        assert!(!validate_tiff_payload(&oversized_value));

        let mut cycle = b"II*\0".to_vec();
        cycle.extend_from_slice(&8_u32.to_le_bytes());
        cycle.extend_from_slice(&1_u16.to_le_bytes());
        cycle.extend_from_slice(&0x8769_u16.to_le_bytes());
        cycle.extend_from_slice(&4_u16.to_le_bytes());
        cycle.extend_from_slice(&1_u32.to_le_bytes());
        cycle.extend_from_slice(&8_u32.to_le_bytes());
        cycle.extend_from_slice(&0_u32.to_le_bytes());
        assert!(!validate_tiff_payload(&cycle));
    }

    #[test]
    fn tiff_preflight_rejects_malformed_directories_types_and_reference_pairs() {
        for malformed in [
            Vec::new(),
            b"ZZ*\0\x08\0\0\0".to_vec(),
            b"II+\0\x08\0\0\0".to_vec(),
            b"II*\0".to_vec(),
            b"II*\0\0\0\0\0".to_vec(),
            b"II*\0\xff\xff\xff\xff".to_vec(),
        ] {
            assert!(!validate_tiff_payload(&malformed));
        }

        let mut too_many_tags = b"II*\0".to_vec();
        too_many_tags.extend_from_slice(&8_u32.to_le_bytes());
        too_many_tags.extend_from_slice(&u16::try_from(MAX_EXIF_TAGS + 1).unwrap().to_le_bytes());
        assert!(!validate_tiff_payload(&too_many_tags));

        let mut unknown_type = b"II*\0".to_vec();
        unknown_type.extend_from_slice(&8_u32.to_le_bytes());
        unknown_type.extend_from_slice(&1_u16.to_le_bytes());
        unknown_type.extend_from_slice(&0x010f_u16.to_le_bytes());
        unknown_type.extend_from_slice(&13_u16.to_le_bytes());
        unknown_type.extend_from_slice(&1_u32.to_le_bytes());
        unknown_type.extend_from_slice(&0_u32.to_le_bytes());
        unknown_type.extend_from_slice(&0_u32.to_le_bytes());
        assert!(!validate_tiff_payload(&unknown_type));

        let mut bad_pointer = unknown_type.clone();
        bad_pointer[10..12].copy_from_slice(&0x8769_u16.to_le_bytes());
        bad_pointer[12..14].copy_from_slice(&3_u16.to_le_bytes());
        assert!(!validate_tiff_payload(&bad_pointer));

        let mut unpaired_strip = b"II*\0".to_vec();
        unpaired_strip.extend_from_slice(&8_u32.to_le_bytes());
        unpaired_strip.extend_from_slice(&1_u16.to_le_bytes());
        unpaired_strip.extend_from_slice(&0x0111_u16.to_le_bytes());
        unpaired_strip.extend_from_slice(&4_u16.to_le_bytes());
        unpaired_strip.extend_from_slice(&1_u32.to_le_bytes());
        unpaired_strip.extend_from_slice(&8_u32.to_le_bytes());
        unpaired_strip.extend_from_slice(&0_u32.to_le_bytes());
        assert!(!validate_tiff_payload(&unpaired_strip));

        let mut directories = b"II*\0".to_vec();
        directories.extend_from_slice(&8_u32.to_le_bytes());
        for index in 0..=MAX_IFD_DIRECTORIES {
            directories.extend_from_slice(&0_u16.to_le_bytes());
            let next = if index == MAX_IFD_DIRECTORIES {
                0
            } else {
                u32::try_from(8 + (index + 1) * 6).unwrap()
            };
            directories.extend_from_slice(&next.to_le_bytes());
        }
        assert!(!validate_tiff_payload(&directories));
    }
}
