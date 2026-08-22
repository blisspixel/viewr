//! Bounded embedded ratings for ordinary JPEG files.
//!
//! The durable value is XMP `xmp:Rating`. Reading also reconciles the Windows
//! `System.SimpleRating` EXIF mirror at IFD0 tag `0x4746`. Writes update an
//! existing valid mirror in place but never grow or relocate a TIFF IFD. This
//! keeps MakerNote and unknown offset-bearing metadata byte-for-byte intact.
//! Unix writes use a verified same-directory replace and refuse extra hard links.

use crate::fs::{ImageSource, ImageSourceMatch, RatingScanSource};
use quick_xml::Writer;
use quick_xml::events::BytesStart;
use quick_xml::events::Event;
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;
use std::fmt;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const XMP_APP1_PREFIX: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const EXTENDED_XMP_APP1_PREFIX: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
const EXIF_APP1_PREFIX: &[u8] = b"Exif\0\0";
const XMP_META_NAMESPACE: &[u8] = b"adobe:ns:meta/";
const XMP_NAMESPACE: &[u8] = b"http://ns.adobe.com/xap/1.0/";
const RDF_NAMESPACE: &[u8] = b"http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const SIMPLE_RATING_TAG: u16 = 0x4746;
const MAX_JPEG_BYTES: u64 = viewr_protocol::MAX_ENCODED_INPUT_BYTES;
const MAX_HEADER_BYTES: usize = 16 * 1024 * 1024;
const MAX_HEADER_SEGMENTS: usize = 1024;
const MAX_XMP_PACKET_BYTES: usize = 65_502;
const MAX_XML_DEPTH: usize = 32;
const MAX_XML_ATTRIBUTES_PER_ELEMENT: usize = 64;
const MAX_XML_ATTRIBUTES_TOTAL: usize = 4096;
const MAX_XML_NAMESPACE_DECLARATIONS: usize = 32;
const MAX_XML_EVENTS: usize = 4096;
const COPY_BUFFER_BYTES: usize = 128 * 1024;

/// A validated user rating.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Rating(u8);

impl Rating {
    /// Every assignable rating in ascending order.
    pub const ALL: [Self; 5] = [Self(1), Self(2), Self(3), Self(4), Self(5)];

    /// Construct a rating only when `value` is in the interoperable 1-to-5 range.
    #[must_use]
    pub const fn new(value: u8) -> Option<Self> {
        if value >= 1 && value <= 5 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Return the integer value.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl fmt::Display for Rating {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// The explicit source mutation requested by a user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RatingAssignment {
    /// Remove the supported rating value.
    Clear,
    /// Store one rating.
    Set(Rating),
}

impl RatingAssignment {
    /// State expected after a verified write.
    #[must_use]
    pub const fn expected_state(self) -> RatingState {
        match self {
            Self::Clear => RatingState::Unrated,
            Self::Set(rating) => RatingState::Rated(rating),
        }
    }
}

/// A fully distinguished rating read state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RatingState {
    /// Rating discovery has not completed for this catalog entry.
    #[default]
    Loading,
    /// No supported rating is embedded.
    Unrated,
    /// An interoperable 1-to-5 rating.
    Rated(Rating),
    /// The external XMP value `-1`.
    Rejected,
    /// XMP and EXIF contain different otherwise-valid values.
    Conflict,
    /// Metadata exists but falls outside the narrow safe profile.
    Unsupported,
    /// The container or metadata could not be read completely.
    Unreadable,
}

impl RatingState {
    /// Whether this state appears in a folder threshold projection.
    #[must_use]
    pub const fn matches(self, filter: RatingFilter) -> bool {
        match filter {
            RatingFilter::All => true,
            RatingFilter::AtLeast(minimum) => {
                matches!(self, Self::Rated(rating) if rating.get() >= minimum.get())
            }
        }
    }
}

/// Session-only folder rating filter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RatingFilter {
    /// Show the complete canonical folder catalog.
    #[default]
    All,
    /// Show numeric ratings at or above the threshold.
    AtLeast(Rating),
}

/// Why the current rating can or cannot be changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RatingWriteCapability {
    /// A regular, identity-bound JPEG using the supported metadata profile.
    WritableJpeg,
    /// The container can be viewed but has no proven source writer.
    ReadOnlyFormat,
    /// The selected object is a link, reparse point, or lacks identity evidence.
    UnsafeSource,
    /// Rating observation ended before source metadata could be classified.
    ObservationFailed,
    /// The JPEG metadata cannot be changed without unsupported repair.
    UnsupportedMetadata,
}

/// Rating plus write capability for one accepted source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RatingObservation {
    /// Embedded rating state.
    pub state: RatingState,
    /// Source-write capability.
    pub capability: RatingWriteCapability,
}

/// Fixed, privacy-safe write outcomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RatingWriteError {
    /// No writer exists for this container on the current platform.
    #[error("rating is read-only for this format")]
    ReadOnlyFormat,
    /// Existing metadata is outside the supported safe profile.
    #[error("rating metadata is unsupported")]
    UnsupportedMetadata,
    /// The accepted source could not be read safely.
    #[error("rating metadata is unreadable")]
    UnreadableMetadata,
    /// The source object or its bytes changed before commit.
    #[error("source changed before rating commit")]
    SourceChanged,
    /// The file or directory denied a required operation.
    #[error("rating write was denied")]
    PermissionDenied,
    /// A pre-commit write, sync, or replacement step failed safely.
    #[error("rating write failed safely")]
    WriteFailed,
    /// Verification failed and the exact original was restored.
    #[error("rating verification failed and the original was restored")]
    VerificationRestored,
    /// Neither verification nor exact restoration could be proven.
    #[error("rating verification and restoration failed")]
    RecoveryFailed,
}

/// Successful source replacement, including the newly accepted identity handle.
#[derive(Debug)]
pub(crate) struct VerifiedRatingWrite {
    pub(crate) source: ImageSource,
    pub(crate) state: RatingState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MetadataError {
    NotJpeg,
    Unsupported,
    Unreadable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct JpegSegment {
    marker: u8,
    payload_offset: usize,
    raw: Vec<u8>,
}

impl JpegSegment {
    fn payload(&self) -> &[u8] {
        &self.raw[self.payload_offset..]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct JpegHeader {
    segments: Vec<JpegSegment>,
    encoded_len: u64,
}

#[derive(Debug, PartialEq, Eq)]
struct RatingHeaderSnapshot {
    bytes: Vec<u8>,
    parsed: Result<JpegHeader, MetadataError>,
}

struct SnapshotReader<R> {
    inner: R,
    bytes: Vec<u8>,
}

impl<R> SnapshotReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            bytes: Vec::new(),
        }
    }

    fn finish(self, parsed: Result<JpegHeader, MetadataError>) -> RatingHeaderSnapshot {
        RatingHeaderSnapshot {
            bytes: self.bytes,
            parsed,
        }
    }
}

impl<R: Read> Read for SnapshotReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        let next_len = self
            .bytes
            .len()
            .checked_add(read)
            .filter(|length| *length <= MAX_HEADER_BYTES)
            .ok_or_else(|| std::io::Error::other("rating header snapshot exceeds its bound"))?;
        self.bytes
            .try_reserve(read)
            .map_err(|_| std::io::Error::other("rating header snapshot allocation failed"))?;
        self.bytes.extend_from_slice(&buffer[..read]);
        debug_assert_eq!(self.bytes.len(), next_len);
        Ok(read)
    }
}

impl JpegHeader {
    fn encode(&self) -> Result<Vec<u8>, MetadataError> {
        let capacity = usize::try_from(self.encoded_len).map_err(|_| MetadataError::Unsupported)?;
        let mut encoded = Vec::new();
        encoded
            .try_reserve_exact(capacity)
            .map_err(|_| MetadataError::Unsupported)?;
        encoded.extend_from_slice(&[0xff, 0xd8]);
        for segment in &self.segments {
            encoded.extend_from_slice(&segment.raw);
        }
        Ok(encoded)
    }
}

/// Read the current rating and determine whether the exact accepted source is writable.
#[must_use]
pub(crate) fn observe_source(source: &ImageSource, path: &Path) -> RatingObservation {
    observe_source_while(source, path, || true)
}

/// Inspect one accepted rating while cooperatively stopping superseded work.
#[must_use]
pub(crate) fn observe_source_while(
    source: &ImageSource,
    path: &Path,
    keep_going: impl FnMut() -> bool,
) -> RatingObservation {
    observe_source_while_with_hook(source, path, || {}, keep_going)
}

#[cfg(test)]
fn observe_source_with_hook(
    source: &ImageSource,
    path: &Path,
    before_final_version_check: impl FnOnce(),
) -> RatingObservation {
    observe_source_while_with_hook(source, path, before_final_version_check, || true)
}

fn observe_source_while_with_hook(
    source: &ImageSource,
    path: &Path,
    before_final_version_check: impl FnOnce(),
    mut keep_going: impl FnMut() -> bool,
) -> RatingObservation {
    if !source.version_is_current_while(&mut keep_going) {
        return unsafe_rating_observation();
    }
    let mut reader = match source.clone_for_decode() {
        Ok(file) => BufReader::new(file),
        Err(_) => return unsafe_rating_observation(),
    };
    let header = read_jpeg_header(&mut reader);
    drop(reader);
    before_final_version_check();
    if !source.version_is_current_while(&mut keep_going) {
        return unsafe_rating_observation();
    }
    let header = match header {
        Ok(header) => header,
        Err(MetadataError::NotJpeg) => {
            return RatingObservation {
                state: RatingState::Unrated,
                capability: RatingWriteCapability::ReadOnlyFormat,
            };
        }
        Err(MetadataError::Unsupported) => {
            return RatingObservation {
                state: RatingState::Unsupported,
                capability: RatingWriteCapability::UnsupportedMetadata,
            };
        }
        Err(MetadataError::Unreadable) => {
            return RatingObservation {
                state: RatingState::Unreadable,
                capability: RatingWriteCapability::UnsupportedMetadata,
            };
        }
    };
    let state = rating_from_header(&header).unwrap_or(RatingState::Unsupported);
    let capability = if matches!(state, RatingState::Unsupported | RatingState::Unreadable) {
        RatingWriteCapability::UnsupportedMetadata
    } else if source.matches_path_while(path, &mut keep_going) != ImageSourceMatch::Same
        || source.extra_hard_links()
    {
        RatingWriteCapability::UnsafeSource
    } else {
        RatingWriteCapability::WritableJpeg
    };
    RatingObservation { state, capability }
}

const fn unsafe_rating_observation() -> RatingObservation {
    RatingObservation {
        state: RatingState::Unreadable,
        capability: RatingWriteCapability::UnsafeSource,
    }
}

/// Discover ratings for a folder. Independent header reads may use extra cores.
///
/// Returns `None` when cancelled. Result order matches `files`. This seam still
/// returns no write capability.
#[must_use]
pub(crate) fn scan_folder_ratings_while(
    files: Vec<(PathBuf, Option<crate::fs::ScanProvenance>)>,
    keep_going: impl Fn() -> bool + Sync,
    parallelism: usize,
) -> Option<Vec<(PathBuf, RatingState)>> {
    if files.is_empty() {
        return Some(Vec::new());
    }
    if !keep_going() {
        return None;
    }
    let workers = parallelism.max(1).min(files.len());
    if workers == 1 {
        let mut ratings = Vec::with_capacity(files.len());
        for (path, provenance) in files {
            if !keep_going() {
                return None;
            }
            let rating = scan_path_rating_while(&path, provenance, &keep_going);
            ratings.push((path, rating));
        }
        return Some(ratings);
    }

    let next = AtomicUsize::new(0);
    let cancelled = AtomicBool::new(false);
    let slots: Vec<Mutex<Option<RatingState>>> =
        (0..files.len()).map(|_| Mutex::new(None)).collect();

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    if cancelled.load(Ordering::Acquire) || !keep_going() {
                        cancelled.store(true, Ordering::Release);
                        return;
                    }
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some((path, provenance)) = files.get(index) else {
                        return;
                    };
                    let rating = scan_path_rating_while(path, *provenance, || {
                        !cancelled.load(Ordering::Acquire) && keep_going()
                    });
                    if cancelled.load(Ordering::Acquire) || !keep_going() {
                        cancelled.store(true, Ordering::Release);
                        return;
                    }
                    *slots[index]
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(rating);
                }
            });
        }
    });

    if cancelled.load(Ordering::Acquire) || !keep_going() {
        return None;
    }

    let mut ratings = Vec::with_capacity(files.len());
    for ((path, _), slot) in files.into_iter().zip(slots) {
        let rating = slot
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)?;
        ratings.push((path, rating));
    }
    Some(ratings)
}

/// Read only the bounded rating header for in-memory folder discovery.
///
/// This seam returns no write capability. Mutating paths must reopen a fully
/// witnessed [`ImageSource`] and perform their independent guarded transaction.
#[must_use]
pub(crate) fn scan_path_rating_while(
    path: &Path,
    provenance: Option<crate::fs::ScanProvenance>,
    keep_going: impl FnMut() -> bool,
) -> RatingState {
    scan_path_rating_while_with_hook(path, provenance, keep_going, || {})
}

fn scan_path_rating_while_with_hook(
    path: &Path,
    provenance: Option<crate::fs::ScanProvenance>,
    mut keep_going: impl FnMut() -> bool,
    before_confirmation: impl FnOnce(),
) -> RatingState {
    let Ok(source) = RatingScanSource::open_while(path, provenance, &mut keep_going) else {
        return RatingState::Unreadable;
    };
    if !source.native_version_is_current_while(&mut keep_going) {
        return RatingState::Unreadable;
    }
    let header = read_rating_header_snapshot_while(&source, &mut keep_going);
    before_confirmation();
    if !source.native_version_is_current_while(&mut keep_going) {
        return RatingState::Unreadable;
    }
    let confirmation = read_rating_header_snapshot_while(&source, &mut keep_going);
    if !source.native_version_is_current_while(&mut keep_going) {
        return RatingState::Unreadable;
    }
    rating_from_matching_header_snapshots(&header, &confirmation)
}

fn rating_from_matching_header_snapshots(
    header: &RatingHeaderSnapshot,
    confirmation: &RatingHeaderSnapshot,
) -> RatingState {
    if confirmation != header {
        return RatingState::Unreadable;
    }
    match &header.parsed {
        Ok(header) => rating_from_header(header).unwrap_or(RatingState::Unsupported),
        Err(MetadataError::NotJpeg) => RatingState::Unrated,
        Err(MetadataError::Unsupported) => RatingState::Unsupported,
        Err(MetadataError::Unreadable) => RatingState::Unreadable,
    }
}

fn read_rating_header_snapshot_while(
    source: &RatingScanSource,
    keep_going: &mut impl FnMut() -> bool,
) -> RatingHeaderSnapshot {
    let Ok(file) = source.clone_for_read() else {
        return RatingHeaderSnapshot {
            bytes: Vec::new(),
            parsed: Err(MetadataError::Unreadable),
        };
    };
    read_jpeg_header_snapshot_from_reader_while(file, keep_going)
}

fn read_jpeg_header_snapshot_from_reader_while(
    reader: impl Read,
    keep_going: &mut impl FnMut() -> bool,
) -> RatingHeaderSnapshot {
    let mut reader = SnapshotReader::new(reader);
    let parsed = read_jpeg_header_while(&mut reader, keep_going);
    reader.finish(parsed)
}

fn read_jpeg_header(reader: &mut impl Read) -> Result<JpegHeader, MetadataError> {
    read_jpeg_header_while(reader, || true)
}

fn read_jpeg_header_while(
    reader: &mut impl Read,
    mut keep_going: impl FnMut() -> bool,
) -> Result<JpegHeader, MetadataError> {
    ensure_rating_work_current(&mut keep_going)?;
    let mut soi = [0_u8; 2];
    reader
        .read_exact(&mut soi)
        .map_err(|_| MetadataError::Unreadable)?;
    if soi != [0xff, 0xd8] {
        return Err(MetadataError::NotJpeg);
    }

    let mut total = 2_usize;
    let mut segments = Vec::new();
    loop {
        ensure_rating_work_current(&mut keep_going)?;
        if segments.len() >= MAX_HEADER_SEGMENTS {
            return Err(MetadataError::Unsupported);
        }
        let mut raw = Vec::with_capacity(256);
        let mut byte = read_byte(reader)?;
        if byte != 0xff {
            return Err(MetadataError::Unsupported);
        }
        raw.push(byte);
        loop {
            ensure_rating_work_current(&mut keep_going)?;
            byte = read_byte(reader)?;
            raw.push(byte);
            if total
                .checked_add(raw.len())
                .is_none_or(|value| value > MAX_HEADER_BYTES)
            {
                return Err(MetadataError::Unsupported);
            }
            if byte != 0xff {
                break;
            }
        }
        let marker = byte;
        if marker == 0x00
            || marker == 0xd8
            || marker == 0xd9
            || marker == 0x01
            || (0xd0..=0xd7).contains(&marker)
        {
            return Err(MetadataError::Unsupported);
        }

        let mut length_bytes = [0_u8; 2];
        reader
            .read_exact(&mut length_bytes)
            .map_err(|_| MetadataError::Unreadable)?;
        raw.extend_from_slice(&length_bytes);
        let declared = usize::from(u16::from_be_bytes(length_bytes));
        if declared < 2 {
            return Err(MetadataError::Unsupported);
        }
        let payload_len = declared - 2;
        let payload_offset = raw.len();
        let next_total = total
            .checked_add(raw.len())
            .and_then(|value| value.checked_add(payload_len))
            .ok_or(MetadataError::Unsupported)?;
        if next_total > MAX_HEADER_BYTES {
            return Err(MetadataError::Unsupported);
        }
        raw.try_reserve_exact(payload_len)
            .map_err(|_| MetadataError::Unsupported)?;
        raw.resize(payload_offset + payload_len, 0);
        reader
            .read_exact(&mut raw[payload_offset..])
            .map_err(|_| MetadataError::Unreadable)?;
        ensure_rating_work_current(&mut keep_going)?;
        total = next_total;
        segments.push(JpegSegment {
            marker,
            payload_offset,
            raw,
        });
        if marker == 0xda {
            break;
        }
    }

    Ok(JpegHeader {
        segments,
        encoded_len: u64::try_from(total).map_err(|_| MetadataError::Unsupported)?,
    })
}

fn ensure_rating_work_current(keep_going: &mut impl FnMut() -> bool) -> Result<(), MetadataError> {
    if keep_going() {
        Ok(())
    } else {
        Err(MetadataError::Unreadable)
    }
}

fn read_byte(reader: &mut impl Read) -> Result<u8, MetadataError> {
    let mut byte = [0_u8; 1];
    reader
        .read_exact(&mut byte)
        .map_err(|_| MetadataError::Unreadable)?;
    Ok(byte[0])
}

fn rating_from_header(header: &JpegHeader) -> Result<RatingState, MetadataError> {
    let mut xmp = None;
    let mut exif = None;
    let mut exif_packets = 0_usize;
    for segment in &header.segments {
        if segment.marker != 0xe1 {
            continue;
        }
        let payload = segment.payload();
        if payload.starts_with(EXTENDED_XMP_APP1_PREFIX) {
            return Err(MetadataError::Unsupported);
        }
        if let Some(packet) = payload.strip_prefix(XMP_APP1_PREFIX) {
            if xmp.is_some() {
                return Err(MetadataError::Unsupported);
            }
            xmp = Some(parse_xmp_rating(packet)?);
        } else if let Some(tiff) = payload.strip_prefix(EXIF_APP1_PREFIX) {
            exif_packets = exif_packets.saturating_add(1);
            if exif_packets > 1 {
                return Err(MetadataError::Unsupported);
            }
            exif = parse_exif_rating(tiff)?;
        }
    }
    Ok(reconcile_ratings(xmp.flatten(), exif))
}

fn reconcile_ratings(xmp: Option<RatingState>, exif: Option<RatingState>) -> RatingState {
    match (xmp, exif) {
        (None, None) => RatingState::Unrated,
        (Some(state), None) | (None, Some(state)) => state,
        (Some(left), Some(right)) if left == right => left,
        (Some(_), Some(_)) => RatingState::Conflict,
    }
}

fn parse_rating_text(value: &str) -> Result<RatingState, MetadataError> {
    match value.trim() {
        "-1" => Ok(RatingState::Rejected),
        "0" => Ok(RatingState::Unrated),
        "1" => Ok(RatingState::Rated(Rating(1))),
        "2" => Ok(RatingState::Rated(Rating(2))),
        "3" => Ok(RatingState::Rated(Rating(3))),
        "4" => Ok(RatingState::Rated(Rating(4))),
        "5" => Ok(RatingState::Rated(Rating(5))),
        _ => Err(MetadataError::Unsupported),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one bounded streaming state machine keeps XML limits and RDF topology coupled"
)]
fn parse_xmp_rating(packet: &[u8]) -> Result<Option<RatingState>, MetadataError> {
    if packet.len() > MAX_XMP_PACKET_BYTES || std::str::from_utf8(packet).is_err() {
        return Err(MetadataError::Unsupported);
    }
    let mut reader = NsReader::from_reader(packet);
    reader
        .resolver_mut()
        .set_max_declarations_per_element(MAX_XML_NAMESPACE_DECLARATIONS);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut events = 0_usize;
    let mut attributes_total = 0_usize;
    let mut rating = None;
    let mut rating_element_depth = None;
    let mut rating_text = String::new();
    let mut root_seen = false;
    let mut xmpmeta_depth = None;
    let mut rdf_depth = None;
    let mut rdf_seen = false;
    let mut rdf_closed = false;
    let mut description_depth = None;
    let mut description_count = 0_usize;
    let mut subject = None;

    loop {
        events = events.saturating_add(1);
        if events > MAX_XML_EVENTS {
            return Err(MetadataError::Unsupported);
        }
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|_| MetadataError::Unsupported)?;
        match event {
            Event::Start(element) => {
                let element_depth = depth.checked_add(1).ok_or(MetadataError::Unsupported)?;
                if element_depth > MAX_XML_DEPTH {
                    return Err(MetadataError::Unsupported);
                }
                let is_xmpmeta =
                    resolved_element_is(&reader, &element, XMP_META_NAMESPACE, b"xmpmeta")?;
                let is_rdf = resolved_element_is(&reader, &element, RDF_NAMESPACE, b"RDF")?;
                let is_description =
                    resolved_element_is(&reader, &element, RDF_NAMESPACE, b"Description")?;
                let is_rating = resolved_element_is(&reader, &element, XMP_NAMESPACE, b"Rating")?;
                if rating_element_depth.is_some() {
                    return Err(MetadataError::Unsupported);
                }

                let is_top_description = rdf_depth == Some(depth) && is_description;
                validate_xmp_element_position(
                    depth,
                    is_xmpmeta,
                    is_rdf,
                    is_description,
                    root_seen,
                    xmpmeta_depth,
                    rdf_depth,
                    rdf_seen,
                )?;
                let about = inspect_attributes(
                    &reader,
                    &element,
                    &mut attributes_total,
                    &mut rating,
                    is_top_description,
                )?;

                if depth == 0 {
                    root_seen = true;
                    if is_xmpmeta {
                        xmpmeta_depth = Some(element_depth);
                    } else {
                        rdf_depth = Some(element_depth);
                        rdf_seen = true;
                    }
                } else if xmpmeta_depth == Some(depth) {
                    rdf_depth = Some(element_depth);
                    rdf_seen = true;
                } else if is_top_description {
                    record_xmp_subject(&mut subject, about)?;
                    description_count = description_count.saturating_add(1);
                    description_depth = Some(element_depth);
                }

                if is_rating {
                    if description_depth != Some(depth) || rating.is_some() {
                        return Err(MetadataError::Unsupported);
                    }
                    rating_element_depth = Some(element_depth);
                    rating_text.clear();
                }
                depth = element_depth;
            }
            Event::Empty(element) => {
                let element_depth = depth.checked_add(1).ok_or(MetadataError::Unsupported)?;
                if element_depth > MAX_XML_DEPTH {
                    return Err(MetadataError::Unsupported);
                }
                let is_xmpmeta =
                    resolved_element_is(&reader, &element, XMP_META_NAMESPACE, b"xmpmeta")?;
                let is_rdf = resolved_element_is(&reader, &element, RDF_NAMESPACE, b"RDF")?;
                let is_description =
                    resolved_element_is(&reader, &element, RDF_NAMESPACE, b"Description")?;
                let is_rating = resolved_element_is(&reader, &element, XMP_NAMESPACE, b"Rating")?;
                if is_rating || rating_element_depth.is_some() {
                    return Err(MetadataError::Unsupported);
                }
                let is_top_description = rdf_depth == Some(depth) && is_description;
                validate_xmp_element_position(
                    depth,
                    is_xmpmeta,
                    is_rdf,
                    is_description,
                    root_seen,
                    xmpmeta_depth,
                    rdf_depth,
                    rdf_seen,
                )?;
                let about = inspect_attributes(
                    &reader,
                    &element,
                    &mut attributes_total,
                    &mut rating,
                    is_top_description,
                )?;
                if depth == 0 {
                    root_seen = true;
                    if is_rdf {
                        rdf_seen = true;
                        rdf_closed = true;
                    }
                } else if xmpmeta_depth == Some(depth) {
                    rdf_seen = true;
                    rdf_closed = true;
                } else if is_top_description {
                    record_xmp_subject(&mut subject, about)?;
                    description_count = description_count.saturating_add(1);
                }
            }
            Event::Text(text) if rating_element_depth == Some(depth) => {
                let decoded = text.decode().map_err(|_| MetadataError::Unsupported)?;
                let unescaped = quick_xml::escape::unescape(&decoded)
                    .map_err(|_| MetadataError::Unsupported)?;
                if rating_text.len().saturating_add(unescaped.len()) > 32 {
                    return Err(MetadataError::Unsupported);
                }
                rating_text.push_str(&unescaped);
            }
            Event::CData(text) if rating_element_depth == Some(depth) => {
                let decoded = text.decode().map_err(|_| MetadataError::Unsupported)?;
                if rating_text.len().saturating_add(decoded.len()) > 32 {
                    return Err(MetadataError::Unsupported);
                }
                rating_text.push_str(&decoded);
            }
            Event::End(_) => {
                if rating_element_depth == Some(depth) {
                    rating = Some(parse_rating_text(&rating_text)?);
                    rating_element_depth = None;
                }
                if description_depth == Some(depth) {
                    description_depth = None;
                }
                if rdf_depth == Some(depth) {
                    rdf_depth = None;
                    rdf_closed = true;
                }
                if xmpmeta_depth == Some(depth) {
                    xmpmeta_depth = None;
                }
                depth = depth.checked_sub(1).ok_or(MetadataError::Unsupported)?;
            }
            Event::DocType(_) => return Err(MetadataError::Unsupported),
            Event::Text(text) if description_depth.is_none() && rating_element_depth.is_none() => {
                let decoded = text.decode().map_err(|_| MetadataError::Unsupported)?;
                if !decoded.trim().is_empty() {
                    return Err(MetadataError::Unsupported);
                }
            }
            Event::CData(_) if description_depth.is_none() => {
                return Err(MetadataError::Unsupported);
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if depth != 0
        || rating_element_depth.is_some()
        || !root_seen
        || !rdf_seen
        || !rdf_closed
        || xmpmeta_depth.is_some()
        || description_count == 0
    {
        return Err(MetadataError::Unsupported);
    }
    Ok(rating)
}

#[allow(
    clippy::fn_params_excessive_bools,
    clippy::too_many_arguments,
    reason = "the booleans are resolved names from one XML event, not independent options"
)]
fn validate_xmp_element_position(
    depth: usize,
    is_xmpmeta: bool,
    is_rdf: bool,
    is_description: bool,
    root_seen: bool,
    xmpmeta_depth: Option<usize>,
    rdf_depth: Option<usize>,
    rdf_seen: bool,
) -> Result<(), MetadataError> {
    if depth == 0 {
        if root_seen || (!is_xmpmeta && !is_rdf) {
            return Err(MetadataError::Unsupported);
        }
    } else if xmpmeta_depth == Some(depth) {
        if !is_rdf || rdf_seen {
            return Err(MetadataError::Unsupported);
        }
    } else if rdf_depth == Some(depth) {
        if !is_description {
            return Err(MetadataError::Unsupported);
        }
    } else if is_xmpmeta || is_rdf {
        return Err(MetadataError::Unsupported);
    }
    Ok(())
}

fn record_xmp_subject(
    expected: &mut Option<String>,
    about: Option<String>,
) -> Result<(), MetadataError> {
    let about = about.ok_or(MetadataError::Unsupported)?;
    if expected.as_ref().is_some_and(|value| value != &about) {
        return Err(MetadataError::Unsupported);
    }
    if expected.is_none() {
        *expected = Some(about);
    }
    Ok(())
}

fn inspect_attributes(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    total: &mut usize,
    rating: &mut Option<RatingState>,
    top_description: bool,
) -> Result<Option<String>, MetadataError> {
    let mut count = 0_usize;
    let mut about = None;
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| MetadataError::Unsupported)?;
        count = count.saturating_add(1);
        *total = total.saturating_add(1);
        if count > MAX_XML_ATTRIBUTES_PER_ELEMENT || *total > MAX_XML_ATTRIBUTES_TOTAL {
            return Err(MetadataError::Unsupported);
        }
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if matches!(namespace, ResolveResult::Unknown(_)) {
            return Err(MetadataError::Unsupported);
        }
        if namespace_is(&namespace, XMP_NAMESPACE) && local.as_ref() == b"Rating" {
            if !top_description || rating.is_some() {
                return Err(MetadataError::Unsupported);
            }
            let value = attribute
                .decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, reader.decoder())
                .map_err(|_| MetadataError::Unsupported)?;
            *rating = Some(parse_rating_text(&value)?);
        } else if namespace_is(&namespace, RDF_NAMESPACE) && local.as_ref() == b"about" {
            if !top_description || about.is_some() {
                return Err(MetadataError::Unsupported);
            }
            let value = attribute
                .decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, reader.decoder())
                .map_err(|_| MetadataError::Unsupported)?;
            about = Some(value.into_owned());
        }
    }
    Ok(about)
}

fn resolved_element_is(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    expected_namespace: &[u8],
    expected_local: &[u8],
) -> Result<bool, MetadataError> {
    let (namespace, local) = reader.resolver().resolve_element(element.name());
    if matches!(namespace, ResolveResult::Unknown(_)) {
        return Err(MetadataError::Unsupported);
    }
    Ok(namespace_is(&namespace, expected_namespace) && local.as_ref() == expected_local)
}

fn namespace_is(namespace: &ResolveResult<'_>, expected: &[u8]) -> bool {
    matches!(namespace, ResolveResult::Bound(Namespace(value)) if *value == expected)
}

fn parse_exif_rating(tiff: &[u8]) -> Result<Option<RatingState>, MetadataError> {
    let endian = Endian::read(tiff)?;
    if endian.u16(tiff, 2)? != 42 {
        return Err(MetadataError::Unsupported);
    }
    let ifd_offset =
        usize::try_from(endian.u32(tiff, 4)?).map_err(|_| MetadataError::Unsupported)?;
    let count = usize::from(endian.u16(tiff, ifd_offset)?);
    if count > 4096 {
        return Err(MetadataError::Unsupported);
    }
    let entries_start = ifd_offset
        .checked_add(2)
        .ok_or(MetadataError::Unsupported)?;
    let entries_bytes = count.checked_mul(12).ok_or(MetadataError::Unsupported)?;
    let entries_end = entries_start
        .checked_add(entries_bytes)
        .ok_or(MetadataError::Unsupported)?;
    if entries_end
        .checked_add(4)
        .ok_or(MetadataError::Unsupported)?
        > tiff.len()
    {
        return Err(MetadataError::Unsupported);
    }

    let mut rating = None;
    for index in 0..count {
        let entry = entries_start + index * 12;
        if endian.u16(tiff, entry)? != SIMPLE_RATING_TAG {
            continue;
        }
        if rating.is_some()
            || endian.u16(tiff, entry + 2)? != 3
            || endian.u32(tiff, entry + 4)? != 1
        {
            return Err(MetadataError::Unsupported);
        }
        let value = endian.u16(tiff, entry + 8)?;
        rating = Some(match value {
            0 => RatingState::Unrated,
            1..=5 => RatingState::Rated(Rating(value as u8)),
            _ => return Err(MetadataError::Unsupported),
        });
    }
    Ok(rating)
}

fn exif_rating_value_offset(tiff: &[u8]) -> Result<Option<(usize, Endian)>, MetadataError> {
    let endian = Endian::read(tiff)?;
    if endian.u16(tiff, 2)? != 42 {
        return Err(MetadataError::Unsupported);
    }
    let ifd_offset =
        usize::try_from(endian.u32(tiff, 4)?).map_err(|_| MetadataError::Unsupported)?;
    let count = usize::from(endian.u16(tiff, ifd_offset)?);
    if count > 4096 {
        return Err(MetadataError::Unsupported);
    }
    let entries_start = ifd_offset
        .checked_add(2)
        .ok_or(MetadataError::Unsupported)?;
    let entries_end = entries_start
        .checked_add(count.checked_mul(12).ok_or(MetadataError::Unsupported)?)
        .ok_or(MetadataError::Unsupported)?;
    if entries_end
        .checked_add(4)
        .ok_or(MetadataError::Unsupported)?
        > tiff.len()
    {
        return Err(MetadataError::Unsupported);
    }
    let mut result = None;
    for index in 0..count {
        let entry = entries_start + index * 12;
        if endian.u16(tiff, entry)? != SIMPLE_RATING_TAG {
            continue;
        }
        if result.is_some()
            || endian.u16(tiff, entry + 2)? != 3
            || endian.u32(tiff, entry + 4)? != 1
        {
            return Err(MetadataError::Unsupported);
        }
        let offset = entry.checked_add(8).ok_or(MetadataError::Unsupported)?;
        endian.u16(tiff, offset)?;
        result = Some((offset, endian));
    }
    Ok(result)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Endian {
    Little,
    Big,
}

impl Endian {
    fn read(bytes: &[u8]) -> Result<Self, MetadataError> {
        match bytes.get(..2) {
            Some(b"II") => Ok(Self::Little),
            Some(b"MM") => Ok(Self::Big),
            _ => Err(MetadataError::Unsupported),
        }
    }

    fn u16(self, bytes: &[u8], offset: usize) -> Result<u16, MetadataError> {
        let value: [u8; 2] = bytes
            .get(offset..offset.saturating_add(2))
            .and_then(|slice| slice.try_into().ok())
            .ok_or(MetadataError::Unsupported)?;
        Ok(match self {
            Self::Little => u16::from_le_bytes(value),
            Self::Big => u16::from_be_bytes(value),
        })
    }

    fn u32(self, bytes: &[u8], offset: usize) -> Result<u32, MetadataError> {
        let value: [u8; 4] = bytes
            .get(offset..offset.saturating_add(4))
            .and_then(|slice| slice.try_into().ok())
            .ok_or(MetadataError::Unsupported)?;
        Ok(match self {
            Self::Little => u32::from_le_bytes(value),
            Self::Big => u32::from_be_bytes(value),
        })
    }

    fn put_u16(self, bytes: &mut [u8], offset: usize, value: u16) -> Result<(), MetadataError> {
        let encoded = match self {
            Self::Little => value.to_le_bytes(),
            Self::Big => value.to_be_bytes(),
        };
        bytes
            .get_mut(offset..offset.saturating_add(2))
            .ok_or(MetadataError::Unsupported)?
            .copy_from_slice(&encoded);
        Ok(())
    }
}

fn rewrite_header(
    header: &JpegHeader,
    assignment: RatingAssignment,
) -> Result<Vec<u8>, MetadataError> {
    let current = rating_from_header(header)?;
    if matches!(current, RatingState::Unsupported | RatingState::Unreadable) {
        return Err(MetadataError::Unsupported);
    }
    let mut output_segments = Vec::with_capacity(header.segments.len() + 1);
    let mut found_xmp = false;
    for segment in &header.segments {
        let payload = segment.payload();
        if segment.marker == 0xe1 && payload.starts_with(XMP_APP1_PREFIX) {
            found_xmp = true;
            let packet = payload
                .strip_prefix(XMP_APP1_PREFIX)
                .ok_or(MetadataError::Unsupported)?;
            let rewritten = rewrite_xmp_packet(packet, assignment)?;
            output_segments.push(app1_segment(XMP_APP1_PREFIX, &rewritten)?);
        } else if segment.marker == 0xe1 && payload.starts_with(EXIF_APP1_PREFIX) {
            let mut rewritten = segment.clone();
            let tiff_start = rewritten
                .payload_offset
                .checked_add(EXIF_APP1_PREFIX.len())
                .ok_or(MetadataError::Unsupported)?;
            let tiff = rewritten
                .raw
                .get(tiff_start..)
                .ok_or(MetadataError::Unsupported)?;
            if let Some((value_offset, endian)) = exif_rating_value_offset(tiff)? {
                let absolute = tiff_start
                    .checked_add(value_offset)
                    .ok_or(MetadataError::Unsupported)?;
                let value = match assignment {
                    RatingAssignment::Clear => 0,
                    RatingAssignment::Set(rating) => u16::from(rating.get()),
                };
                endian.put_u16(&mut rewritten.raw, absolute, value)?;
            }
            output_segments.push(rewritten);
        } else {
            output_segments.push(segment.clone());
        }
    }

    if !found_xmp && let RatingAssignment::Set(rating) = assignment {
        let packet = new_xmp_packet(rating);
        let segment = app1_segment(XMP_APP1_PREFIX, packet.as_bytes())?;
        let insert_at = output_segments
            .iter()
            .take_while(|entry| entry.marker == 0xe0)
            .count();
        output_segments.insert(insert_at, segment);
    }

    let encoded_len = 2_u64
        + output_segments
            .iter()
            .map(|segment| u64::try_from(segment.raw.len()).unwrap_or(u64::MAX))
            .sum::<u64>();
    if encoded_len > u64::try_from(MAX_HEADER_BYTES).unwrap_or(u64::MAX) {
        return Err(MetadataError::Unsupported);
    }
    JpegHeader {
        segments: output_segments,
        encoded_len,
    }
    .encode()
}

fn app1_segment(prefix: &[u8], packet: &[u8]) -> Result<JpegSegment, MetadataError> {
    let payload_len = prefix
        .len()
        .checked_add(packet.len())
        .ok_or(MetadataError::Unsupported)?;
    let declared = payload_len
        .checked_add(2)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or(MetadataError::Unsupported)?;
    let mut raw = Vec::new();
    raw.try_reserve_exact(payload_len + 4)
        .map_err(|_| MetadataError::Unsupported)?;
    raw.extend_from_slice(&[0xff, 0xe1]);
    raw.extend_from_slice(&declared.to_be_bytes());
    let payload_offset = raw.len();
    raw.extend_from_slice(prefix);
    raw.extend_from_slice(packet);
    Ok(JpegSegment {
        marker: 0xe1,
        payload_offset,
        raw,
    })
}

fn new_xmp_packet(rating: Rating) -> String {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="{}"/></rdf:RDF></x:xmpmeta>"#,
        rating.get()
    )
}

fn rewrite_xmp_packet(
    packet: &[u8],
    assignment: RatingAssignment,
) -> Result<Vec<u8>, MetadataError> {
    parse_xmp_rating(packet)?;
    let mut reader = NsReader::from_reader(packet);
    reader
        .resolver_mut()
        .set_max_declarations_per_element(MAX_XML_NAMESPACE_DECLARATIONS);
    let mut writer = Writer::new(Vec::with_capacity(packet.len().saturating_add(96)));
    let mut buffer = Vec::new();
    let mut skip_depth = 0_usize;
    let mut inserted = false;
    let mut depth = 0_usize;
    let mut rdf_depth = None;

    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|_| MetadataError::Unsupported)?;
        match event {
            Event::Start(element) => {
                let element_depth = depth.checked_add(1).ok_or(MetadataError::Unsupported)?;
                let is_rdf = resolved_element_is(&reader, &element, RDF_NAMESPACE, b"RDF")?;
                let is_description = rdf_depth == Some(depth)
                    && resolved_element_is(&reader, &element, RDF_NAMESPACE, b"Description")?;
                if skip_depth > 0 {
                    skip_depth = skip_depth.saturating_add(1);
                } else if resolved_element_is(&reader, &element, XMP_NAMESPACE, b"Rating")? {
                    skip_depth = 1;
                } else {
                    let rewritten =
                        rewrite_start(&reader, element, is_description, assignment, &mut inserted)?;
                    writer
                        .write_event(Event::Start(rewritten))
                        .map_err(|_| MetadataError::Unsupported)?;
                }
                if is_rdf {
                    rdf_depth = Some(element_depth);
                }
                depth = element_depth;
            }
            Event::Empty(element) => {
                let is_description = rdf_depth == Some(depth)
                    && resolved_element_is(&reader, &element, RDF_NAMESPACE, b"Description")?;
                if skip_depth == 0
                    && !resolved_element_is(&reader, &element, XMP_NAMESPACE, b"Rating")?
                {
                    let rewritten =
                        rewrite_start(&reader, element, is_description, assignment, &mut inserted)?;
                    writer
                        .write_event(Event::Empty(rewritten))
                        .map_err(|_| MetadataError::Unsupported)?;
                }
            }
            Event::End(end) => {
                if skip_depth > 0 {
                    skip_depth -= 1;
                } else {
                    writer
                        .write_event(Event::End(end))
                        .map_err(|_| MetadataError::Unsupported)?;
                }
                if rdf_depth == Some(depth) {
                    rdf_depth = None;
                }
                depth = depth.checked_sub(1).ok_or(MetadataError::Unsupported)?;
            }
            Event::Eof => break,
            other if skip_depth == 0 => writer
                .write_event(other)
                .map_err(|_| MetadataError::Unsupported)?,
            _ => {}
        }
        buffer.clear();
    }
    if matches!(assignment, RatingAssignment::Set(_)) && !inserted {
        return Err(MetadataError::Unsupported);
    }
    let output = writer.into_inner();
    if output.len() > MAX_XMP_PACKET_BYTES {
        return Err(MetadataError::Unsupported);
    }
    Ok(output)
}

fn rewrite_start(
    reader: &NsReader<&[u8]>,
    element: BytesStart<'_>,
    is_description: bool,
    assignment: RatingAssignment,
    inserted: &mut bool,
) -> Result<BytesStart<'static>, MetadataError> {
    let mut retained = Vec::<(Vec<u8>, Vec<u8>)>::new();
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| MetadataError::Unsupported)?;
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if namespace_is(&namespace, XMP_NAMESPACE) && local.as_ref() == b"Rating" {
            continue;
        }
        retained.push((
            attribute.key.as_ref().to_vec(),
            attribute.value.as_ref().to_vec(),
        ));
    }
    let mut rewritten = element.into_owned();
    rewritten.clear_attributes();
    for (key, value) in &retained {
        rewritten.push_attribute((key.as_slice(), value.as_slice()));
    }
    if is_description && !*inserted {
        if let RatingAssignment::Set(rating) = assignment {
            let prefix = unique_rating_prefix(&retained);
            let mut declaration = b"xmlns:".to_vec();
            declaration.extend_from_slice(&prefix);
            let mut key = prefix;
            key.extend_from_slice(b":Rating");
            let value = rating.get().to_string();
            rewritten.push_attribute((declaration.as_slice(), XMP_NAMESPACE));
            rewritten.push_attribute((key.as_slice(), value.as_bytes()));
        }
        *inserted = true;
    }
    Ok(rewritten)
}

fn unique_rating_prefix(attributes: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    for suffix in 0_u8..=64 {
        let candidate = if suffix == 0 {
            b"viewrRating".to_vec()
        } else {
            format!("viewrRating{suffix}").into_bytes()
        };
        let mut declaration = b"xmlns:".to_vec();
        declaration.extend_from_slice(&candidate);
        if attributes.iter().all(|(key, _)| key != &declaration) {
            return candidate;
        }
    }
    b"viewrRatingSafe".to_vec()
}

fn map_metadata_write_error(error: MetadataError) -> RatingWriteError {
    match error {
        MetadataError::NotJpeg => RatingWriteError::ReadOnlyFormat,
        MetadataError::Unsupported => RatingWriteError::UnsupportedMetadata,
        MetadataError::Unreadable => RatingWriteError::UnreadableMetadata,
    }
}

fn map_io_error(error: &io::Error) -> RatingWriteError {
    if error.kind() == io::ErrorKind::PermissionDenied {
        RatingWriteError::PermissionDenied
    } else {
        RatingWriteError::WriteFailed
    }
}

pub(crate) fn write_rating(
    path: &Path,
    source: &ImageSource,
    assignment: RatingAssignment,
) -> Result<VerifiedRatingWrite, RatingWriteError> {
    #[cfg(target_os = "windows")]
    {
        write_rating_windows(path, source, assignment)
    }
    #[cfg(not(target_os = "windows"))]
    {
        write_rating_unix(path, source, assignment)
    }
}

#[cfg(not(target_os = "windows"))]
#[allow(
    clippy::too_many_lines,
    reason = "the Unix transaction stays linear so every failure has an adjacent rollback decision"
)]
fn write_rating_unix(
    path: &Path,
    source: &ImageSource,
    assignment: RatingAssignment,
) -> Result<VerifiedRatingWrite, RatingWriteError> {
    if source.matches_path(path) != ImageSourceMatch::Same {
        return Err(RatingWriteError::SourceChanged);
    }
    if source.extra_hard_links() {
        return Err(RatingWriteError::SourceChanged);
    }
    let parent = path.parent().ok_or(RatingWriteError::WriteFailed)?;
    let source_file = source
        .clone_for_decode()
        .map_err(|error| map_io_error(&error))?;
    let source_metadata = source_file
        .metadata()
        .map_err(|error| map_io_error(&error))?;
    if source_metadata.len() > MAX_JPEG_BYTES {
        return Err(RatingWriteError::UnsupportedMetadata);
    }
    drop(source_file);

    let mut pristine =
        unix_named_temp(parent, ".viewr-rating-pristine-").map_err(|error| map_io_error(&error))?;
    copy_accepted_source(source, pristine.as_file_mut()).map_err(|error| map_io_error(&error))?;
    pristine
        .as_file_mut()
        .flush()
        .and_then(|()| pristine.as_file().sync_all())
        .map_err(|error| map_io_error(&error))?;

    let mut pristine_reader = BufReader::new(
        pristine
            .as_file()
            .try_clone()
            .map_err(|error| map_io_error(&error))?,
    );
    pristine_reader
        .seek(SeekFrom::Start(0))
        .map_err(|error| map_io_error(&error))?;
    let original_header =
        read_jpeg_header(&mut pristine_reader).map_err(map_metadata_write_error)?;
    reject_metadata_signatures_in_tail(&mut pristine_reader).map_err(map_metadata_write_error)?;
    let expected_header =
        rewrite_header(&original_header, assignment).map_err(map_metadata_write_error)?;
    let pristine_len = pristine
        .as_file()
        .metadata()
        .map_err(|error| map_io_error(&error))?
        .len();
    let candidate_len = pristine_len
        .checked_sub(original_header.encoded_len)
        .and_then(|tail| tail.checked_add(u64::try_from(expected_header.len()).ok()?))
        .ok_or(RatingWriteError::UnsupportedMetadata)?;
    if candidate_len > MAX_JPEG_BYTES {
        return Err(RatingWriteError::UnsupportedMetadata);
    }

    let mut work =
        unix_named_temp(parent, ".viewr-rating-work-").map_err(|error| map_io_error(&error))?;
    work.as_file()
        .set_permissions(source_metadata.permissions())
        .map_err(|error| map_io_error(&error))?;
    work.write_all(&expected_header)
        .map_err(|error| map_io_error(&error))?;
    pristine_reader
        .seek(SeekFrom::Start(original_header.encoded_len))
        .map_err(|error| map_io_error(&error))?;
    copy_bounded(&mut pristine_reader, work.as_file_mut(), MAX_JPEG_BYTES)
        .map_err(|error| map_io_error(&error))?;
    work.as_file_mut()
        .flush()
        .and_then(|()| work.as_file().sync_all())
        .map_err(|error| map_io_error(&error))?;
    verify_candidate(
        work.as_file(),
        pristine.as_file(),
        original_header.encoded_len,
        &expected_header,
        assignment,
    )
    .map_err(map_metadata_write_error)?;

    if source.matches_path(path) != ImageSourceMatch::Same
        || !accepted_matches_snapshot(source, pristine.as_file())
            .map_err(|error| map_io_error(&error))?
    {
        return Err(RatingWriteError::SourceChanged);
    }

    let backup = unix_backup_path(path, parent).map_err(|error| map_io_error(&error))?;
    if !source.same_object_at_path(&backup)
        || !accepted_matches_snapshot(source, pristine.as_file()).unwrap_or(false)
    {
        let _ = std::fs::remove_file(&backup);
        return Err(RatingWriteError::SourceChanged);
    }
    if work.persist(path).is_err() {
        let _ = std::fs::remove_file(&backup);
        return Err(RatingWriteError::WriteFailed);
    }

    let rollback = || rollback_unix_rating(path, &backup);
    let verified = ImageSource::open(path).map_err(|_| rollback())?;
    let verified_file = verified.clone_for_decode().map_err(|_| rollback())?;
    let candidate_is_valid = verify_candidate(
        &verified_file,
        pristine.as_file(),
        original_header.encoded_len,
        &expected_header,
        assignment,
    )
    .is_ok();
    drop(verified_file);
    if !candidate_is_valid
        || candidate_binding_error(&verified, path).is_some()
        || !source.same_object_at_path(&backup)
        || !accepted_matches_snapshot(source, pristine.as_file()).unwrap_or(false)
    {
        return Err(rollback());
    }
    if std::fs::remove_file(&backup).is_err() {
        return Err(rollback());
    }
    Ok(VerifiedRatingWrite {
        source: verified,
        state: assignment.expected_state(),
    })
}

#[cfg(not(target_os = "windows"))]
fn unix_named_temp(parent: &Path, prefix: &str) -> io::Result<tempfile::NamedTempFile> {
    tempfile::Builder::new().prefix(prefix).tempfile_in(parent)
}

#[cfg(not(target_os = "windows"))]
fn unix_backup_path(path: &Path, parent: &Path) -> io::Result<std::path::PathBuf> {
    let backup = vacant_transaction_path(parent, ".viewr-rating-backup-")?;
    if let Err(error) = std::fs::hard_link(path, &backup) {
        let _ = std::fs::remove_file(&backup);
        return Err(error);
    }
    Ok(backup)
}

#[cfg(not(target_os = "windows"))]
fn rollback_unix_rating(path: &Path, backup: &Path) -> RatingWriteError {
    match std::fs::rename(backup, path) {
        Ok(()) => RatingWriteError::VerificationRestored,
        Err(_) => RatingWriteError::RecoveryFailed,
    }
}

#[cfg(target_os = "windows")]
#[allow(
    clippy::too_many_lines,
    reason = "the transaction remains linear so every failure has an adjacent rollback decision"
)]
fn write_rating_windows(
    path: &Path,
    source: &ImageSource,
    assignment: RatingAssignment,
) -> Result<VerifiedRatingWrite, RatingWriteError> {
    if source.matches_path(path) != ImageSourceMatch::Same {
        return Err(RatingWriteError::SourceChanged);
    }
    let parent = path.parent().ok_or(RatingWriteError::WriteFailed)?;
    let source_file = source
        .clone_for_decode()
        .map_err(|error| map_io_error(&error))?;
    let source_metadata = source_file
        .metadata()
        .map_err(|error| map_io_error(&error))?;
    if source_metadata.len() > MAX_JPEG_BYTES {
        return Err(RatingWriteError::UnsupportedMetadata);
    }
    let mut security_descriptor =
        file_security_descriptor(&source_file).map_err(|error| map_io_error(&error))?;
    drop(source_file);

    let pristine_temp = secured_named_temp_file(
        parent,
        ".viewr-rating-pristine-",
        &mut security_descriptor,
        true,
    )
    .map_err(|error| map_io_error(&error))?;
    let (mut pristine, pristine_path) = pristine_temp.into_parts();
    drop(pristine_path);
    copy_accepted_source(source, &mut pristine).map_err(|error| map_io_error(&error))?;
    pristine
        .flush()
        .and_then(|()| pristine.sync_all())
        .map_err(|error| map_io_error(&error))?;

    let mut pristine_reader =
        BufReader::new(pristine.try_clone().map_err(|error| map_io_error(&error))?);
    pristine_reader
        .seek(SeekFrom::Start(0))
        .map_err(|error| map_io_error(&error))?;
    let original_header =
        read_jpeg_header(&mut pristine_reader).map_err(map_metadata_write_error)?;
    reject_metadata_signatures_in_tail(&mut pristine_reader).map_err(map_metadata_write_error)?;
    let expected_header =
        rewrite_header(&original_header, assignment).map_err(map_metadata_write_error)?;
    let pristine_len = pristine
        .metadata()
        .map_err(|error| map_io_error(&error))?
        .len();
    let candidate_len = pristine_len
        .checked_sub(original_header.encoded_len)
        .and_then(|tail| tail.checked_add(u64::try_from(expected_header.len()).ok()?))
        .ok_or(RatingWriteError::UnsupportedMetadata)?;
    if candidate_len > MAX_JPEG_BYTES {
        return Err(RatingWriteError::UnsupportedMetadata);
    }

    let mut work = secured_named_temp_file(
        parent,
        ".viewr-rating-work-",
        &mut security_descriptor,
        false,
    )
    .map_err(|error| map_io_error(&error))?;
    work.as_file()
        .set_permissions(source_metadata.permissions())
        .map_err(|error| map_io_error(&error))?;
    work.write_all(&expected_header)
        .map_err(|error| map_io_error(&error))?;
    pristine_reader
        .seek(SeekFrom::Start(original_header.encoded_len))
        .map_err(|error| map_io_error(&error))?;
    copy_bounded(&mut pristine_reader, work.as_file_mut(), MAX_JPEG_BYTES)
        .map_err(|error| map_io_error(&error))?;
    work.as_file_mut()
        .flush()
        .and_then(|()| work.as_file().sync_all())
        .map_err(|error| map_io_error(&error))?;

    verify_candidate(
        work.as_file(),
        &pristine,
        original_header.encoded_len,
        &expected_header,
        assignment,
    )
    .map_err(map_metadata_write_error)?;

    if source.matches_path(path) != ImageSourceMatch::Same
        || !accepted_matches_snapshot(source, &pristine).map_err(|error| map_io_error(&error))?
    {
        return Err(RatingWriteError::SourceChanged);
    }

    let backup_path = vacant_transaction_path(parent, ".viewr-rating-backup-")
        .map_err(|error| map_io_error(&error))?;
    let work_path = work.into_temp_path();
    if let Err(error) = crate::fs::replace_file(path, &work_path, Some(&backup_path)) {
        let recovery = reconcile_failed_replace(path, &backup_path, source, &pristine, &error);
        if recovery == RatingWriteError::RecoveryFailed {
            let _ = work_path.keep();
        }
        return Err(recovery);
    }

    if !source.same_object_at_path(&backup_path)
        || !accepted_matches_snapshot(source, &pristine).unwrap_or(false)
    {
        return Err(rollback_after_verification_failure(
            path,
            &backup_path,
            source,
            &pristine,
            None,
        ));
    }

    let verified = ImageSource::open(path).map_err(|_| {
        rollback_after_verification_failure(path, &backup_path, source, &pristine, None)
    })?;
    let verified_file = verified.clone_for_decode().map_err(|_| {
        rollback_after_verification_failure(path, &backup_path, source, &pristine, Some(&verified))
    })?;
    let candidate_is_valid = verify_candidate(
        &verified_file,
        &pristine,
        original_header.encoded_len,
        &expected_header,
        assignment,
    )
    .is_ok();
    drop(verified_file);
    if !candidate_is_valid {
        return Err(rollback_after_verification_failure(
            path,
            &backup_path,
            source,
            &pristine,
            Some(&verified),
        ));
    }
    if verified.matches_path(path) != ImageSourceMatch::Same
        || !source.same_object_at_path(&backup_path)
        || !accepted_matches_snapshot(source, &pristine).unwrap_or(false)
    {
        return Err(rollback_after_verification_failure(
            path,
            &backup_path,
            source,
            &pristine,
            Some(&verified),
        ));
    }
    if std::fs::remove_file(&backup_path).is_err() {
        return Err(rollback_after_verification_failure(
            path,
            &backup_path,
            source,
            &pristine,
            Some(&verified),
        ));
    }
    if let Some(error) = candidate_binding_error(&verified, path) {
        return Err(error);
    }

    Ok(VerifiedRatingWrite {
        source: verified,
        state: assignment.expected_state(),
    })
}

fn candidate_binding_error(source: &ImageSource, path: &Path) -> Option<RatingWriteError> {
    match source.matches_path(path) {
        ImageSourceMatch::Same => None,
        ImageSourceMatch::Changed | ImageSourceMatch::Missing | ImageSourceMatch::Unsupported => {
            Some(RatingWriteError::SourceChanged)
        }
        ImageSourceMatch::Unavailable => Some(RatingWriteError::RecoveryFailed),
    }
}

fn copy_accepted_source(source: &ImageSource, destination: &mut File) -> io::Result<()> {
    let mut reader = source.clone_for_decode()?;
    let length = reader.metadata()?.len();
    if length > MAX_JPEG_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "rating source exceeds the encoded input limit",
        ));
    }
    let copied = copy_bounded(&mut reader, destination, MAX_JPEG_BYTES)?;
    if copied != length {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "rating source changed while it was copied",
        ));
    }
    Ok(())
}

fn copy_bounded(reader: &mut impl Read, writer: &mut impl Write, limit: u64) -> io::Result<u64> {
    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(copied);
        }
        copied = copied
            .checked_add(u64::try_from(read).unwrap_or(u64::MAX))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "copy limit exceeded"))?;
        if copied > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "copy limit exceeded",
            ));
        }
        writer.write_all(&buffer[..read])?;
    }
}

fn reject_metadata_signatures_in_tail(reader: &mut impl Read) -> Result<(), MetadataError> {
    let longest = [
        XMP_APP1_PREFIX.len(),
        EXTENDED_XMP_APP1_PREFIX.len(),
        EXIF_APP1_PREFIX.len(),
    ]
    .into_iter()
    .max()
    .unwrap_or(1);
    let mut carry = Vec::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut scanned = 0_u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| MetadataError::Unreadable)?;
        if read == 0 {
            return Ok(());
        }
        scanned = scanned
            .checked_add(u64::try_from(read).unwrap_or(u64::MAX))
            .ok_or(MetadataError::Unsupported)?;
        if scanned > MAX_JPEG_BYTES {
            return Err(MetadataError::Unsupported);
        }
        carry.extend_from_slice(&buffer[..read]);
        if [XMP_APP1_PREFIX, EXTENDED_XMP_APP1_PREFIX, EXIF_APP1_PREFIX]
            .into_iter()
            .any(|signature| {
                carry
                    .windows(signature.len())
                    .any(|window| window == signature)
            })
        {
            return Err(MetadataError::Unsupported);
        }
        let retain = longest.saturating_sub(1).min(carry.len());
        carry.drain(..carry.len().saturating_sub(retain));
    }
}

fn accepted_matches_snapshot(source: &ImageSource, snapshot: &File) -> io::Result<bool> {
    let mut current = source.clone_for_decode()?;
    let mut pristine = snapshot.try_clone()?;
    pristine.seek(SeekFrom::Start(0))?;
    files_equal(&mut current, &mut pristine, MAX_JPEG_BYTES)
}

fn files_equal(left: &mut impl Read, right: &mut impl Read, limit: u64) -> io::Result<bool> {
    let mut compared = 0_u64;
    let mut left_buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut right_buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let left_read = left.read(&mut left_buffer)?;
        let right_read = right.read(&mut right_buffer)?;
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
        compared = compared
            .checked_add(u64::try_from(left_read).unwrap_or(u64::MAX))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "compare limit exceeded"))?;
        if compared > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "compare limit exceeded",
            ));
        }
    }
}

fn verify_candidate(
    candidate: &File,
    pristine: &File,
    pristine_header_len: u64,
    expected_header: &[u8],
    assignment: RatingAssignment,
) -> Result<(), MetadataError> {
    let mut candidate_reader = BufReader::new(
        candidate
            .try_clone()
            .map_err(|_| MetadataError::Unreadable)?,
    );
    candidate_reader
        .seek(SeekFrom::Start(0))
        .map_err(|_| MetadataError::Unreadable)?;
    let header = read_jpeg_header(&mut candidate_reader)?;
    if header.encode()? != expected_header
        || rating_from_header(&header)? != assignment.expected_state()
    {
        return Err(MetadataError::Unsupported);
    }
    let mut pristine_reader = pristine
        .try_clone()
        .map_err(|_| MetadataError::Unreadable)?;
    pristine_reader
        .seek(SeekFrom::Start(pristine_header_len))
        .map_err(|_| MetadataError::Unreadable)?;
    candidate_reader
        .seek(SeekFrom::Start(header.encoded_len))
        .map_err(|_| MetadataError::Unreadable)?;
    if !files_equal(&mut candidate_reader, &mut pristine_reader, MAX_JPEG_BYTES)
        .map_err(|_| MetadataError::Unreadable)?
    {
        return Err(MetadataError::Unsupported);
    }
    Ok(())
}

fn vacant_transaction_path(parent: &Path, prefix: &str) -> io::Result<std::path::PathBuf> {
    let temporary = tempfile::Builder::new()
        .prefix(prefix)
        .tempfile_in(parent)?;
    let path = temporary.path().to_owned();
    temporary.close()?;
    Ok(path)
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)] // bounded read of the accepted handle's self-relative security descriptor
fn file_security_descriptor(file: &File) -> io::Result<Vec<u8>> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, GetKernelObjectSecurity,
        OWNER_SECURITY_INFORMATION,
    };

    const MAX_SECURITY_DESCRIPTOR_BYTES: u32 = 64 * 1024;
    let requested =
        OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut needed = 0_u32;
    // SAFETY: `file` retains a valid handle. A null zero-length output buffer is the
    // documented size query, and `needed` is writable for the duration of the call.
    let succeeded = unsafe {
        GetKernelObjectSecurity(
            file.as_raw_handle(),
            requested,
            std::ptr::null_mut(),
            0,
            &raw mut needed,
        )
    };
    let size_error = io::Error::last_os_error();
    if succeeded != 0
        || size_error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER.cast_signed())
        || needed == 0
        || needed > MAX_SECURITY_DESCRIPTOR_BYTES
    {
        return Err(if succeeded == 0 {
            size_error
        } else {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "source security descriptor has an invalid size",
            )
        });
    }
    let mut descriptor = vec![
        0_u8;
        usize::try_from(needed).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "source security descriptor is too large",
            )
        })?
    ];
    // SAFETY: `descriptor` provides `needed` writable bytes and the API does not
    // retain the file handle or output pointer.
    let succeeded = unsafe {
        GetKernelObjectSecurity(
            file.as_raw_handle(),
            requested,
            descriptor.as_mut_ptr().cast(),
            needed,
            &raw mut needed,
        )
    };
    if succeeded == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(descriptor)
    }
}

#[cfg(target_os = "windows")]
fn secured_named_temp_file(
    parent: &Path,
    prefix: &str,
    security_descriptor: &mut [u8],
    delete_on_close: bool,
) -> io::Result<tempfile::NamedTempFile> {
    const MAX_NAME_ATTEMPTS: usize = 16;
    for _ in 0..MAX_NAME_ATTEMPTS {
        let candidate = vacant_transaction_path(parent, prefix)?;
        let candidate = if candidate.is_absolute() {
            candidate
        } else {
            std::env::current_dir()?.join(candidate)
        };
        match create_secured_file(&candidate, security_descriptor, delete_on_close) {
            Ok(file) => {
                let path = tempfile::TempPath::try_from_path(candidate)?;
                return Ok(tempfile::NamedTempFile::from_parts(file, path));
            }
            Err(error) if matches!(error.raw_os_error(), Some(80 | 183)) => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve a rating transaction path",
    ))
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)] // one audited ACL-at-create Win32 file boundary
fn create_secured_file(
    path: &Path,
    security_descriptor: &mut [u8],
    delete_on_close: bool,
) -> io::Result<File> {
    use std::iter;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_NEW, CreateFileW, DELETE, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_TEMPORARY,
        FILE_FLAG_DELETE_ON_CLOSE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
    };

    let path = path
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>())
            .map_err(|_| io::Error::other("security attributes are too large"))?,
        lpSecurityDescriptor: security_descriptor.as_mut_ptr().cast(),
        bInheritHandle: 0,
    };
    let access = GENERIC_READ | GENERIC_WRITE | if delete_on_close { DELETE } else { 0 };
    let flags = if delete_on_close {
        FILE_ATTRIBUTE_TEMPORARY | FILE_FLAG_DELETE_ON_CLOSE | FILE_FLAG_OPEN_REPARSE_POINT
    } else {
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT
    };
    // SAFETY: `path` is stable NUL-terminated UTF-16, `attributes` points to a
    // live self-relative descriptor copied from the accepted source, CREATE_NEW
    // prevents traversal or clobbering, and ownership of a successful handle is
    // transferred immediately to `File`.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            access,
            FILE_SHARE_DELETE,
            &raw const attributes,
            CREATE_NEW,
            flags,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: `CreateFileW` returned one uniquely owned valid handle.
        Ok(unsafe { File::from_raw_handle(handle) })
    }
}

#[cfg(target_os = "windows")]
fn rollback_after_verification_failure(
    target: &Path,
    backup: &Path,
    source: &ImageSource,
    pristine: &File,
    replacement: Option<&ImageSource>,
) -> RatingWriteError {
    match restore_retained_source(target, backup, source, pristine, replacement) {
        RestoreSourceOutcome::RestoredExact => RatingWriteError::VerificationRestored,
        RestoreSourceOutcome::RestoredChanged => RatingWriteError::SourceChanged,
        RestoreSourceOutcome::BackupMismatch
        | RestoreSourceOutcome::UnsupportedTarget
        | RestoreSourceOutcome::OperationFailed(_)
        | RestoreSourceOutcome::IdentityMismatch
        | RestoreSourceOutcome::SnapshotUnreadable => RatingWriteError::RecoveryFailed,
    }
}

#[cfg(target_os = "windows")]
fn reconcile_failed_replace(
    target: &Path,
    backup: &Path,
    source: &ImageSource,
    pristine: &File,
    replace_error: &io::Error,
) -> RatingWriteError {
    if source.same_object_at_path(target) {
        if backup.exists()
            && (!source.same_object_at_path(backup) || std::fs::remove_file(backup).is_err())
        {
            return RatingWriteError::RecoveryFailed;
        }
        return match accepted_matches_snapshot(source, pristine) {
            Ok(true) => map_io_error(replace_error),
            Ok(false) => RatingWriteError::SourceChanged,
            Err(_) => RatingWriteError::RecoveryFailed,
        };
    }
    match restore_retained_source(target, backup, source, pristine, None) {
        RestoreSourceOutcome::RestoredExact => RatingWriteError::VerificationRestored,
        RestoreSourceOutcome::RestoredChanged => RatingWriteError::SourceChanged,
        RestoreSourceOutcome::BackupMismatch
        | RestoreSourceOutcome::UnsupportedTarget
        | RestoreSourceOutcome::OperationFailed(_)
        | RestoreSourceOutcome::IdentityMismatch
        | RestoreSourceOutcome::SnapshotUnreadable => RatingWriteError::RecoveryFailed,
    }
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RestoreSourceOutcome {
    RestoredExact,
    RestoredChanged,
    BackupMismatch,
    UnsupportedTarget,
    OperationFailed(Option<i32>),
    IdentityMismatch,
    SnapshotUnreadable,
}

#[cfg(target_os = "windows")]
fn restore_retained_source(
    target: &Path,
    backup: &Path,
    source: &ImageSource,
    pristine: &File,
    replacement: Option<&ImageSource>,
) -> RestoreSourceOutcome {
    if !source.same_object_at_path(backup) {
        return RestoreSourceOutcome::BackupMismatch;
    }
    let restored = match std::fs::symlink_metadata(target) {
        Ok(metadata) if regular_windows_file(&metadata) => {
            let Some(replacement) = replacement else {
                return RestoreSourceOutcome::UnsupportedTarget;
            };
            if !replacement.same_object_at_path(target) {
                return RestoreSourceOutcome::IdentityMismatch;
            }
            let Some(parent) = target.parent() else {
                return RestoreSourceOutcome::OperationFailed(None);
            };
            let displaced = match vacant_transaction_path(parent, ".viewr-rating-displaced-") {
                Ok(path) => path,
                Err(error) => {
                    return RestoreSourceOutcome::OperationFailed(error.raw_os_error());
                }
            };
            if let Err(error) = std::fs::rename(target, &displaced) {
                return RestoreSourceOutcome::OperationFailed(error.raw_os_error());
            }
            if !replacement.same_object_at_path(&displaced) {
                let _ = std::fs::rename(&displaced, target);
                return RestoreSourceOutcome::IdentityMismatch;
            }
            if let Err(error) = std::fs::rename(backup, target) {
                let _ = std::fs::rename(&displaced, target);
                return RestoreSourceOutcome::OperationFailed(error.raw_os_error());
            }
            if std::fs::remove_file(&displaced).is_err() {
                return RestoreSourceOutcome::OperationFailed(None);
            }
            Ok(())
        }
        Ok(_) => return RestoreSourceOutcome::UnsupportedTarget,
        Err(error) if error.kind() == io::ErrorKind::NotFound => std::fs::rename(backup, target),
        Err(error) => return RestoreSourceOutcome::OperationFailed(error.raw_os_error()),
    };
    if let Err(error) = restored {
        RestoreSourceOutcome::OperationFailed(error.raw_os_error())
    } else if !source.same_object_at_path(target) {
        RestoreSourceOutcome::IdentityMismatch
    } else {
        match accepted_matches_snapshot(source, pristine) {
            Ok(true) => RestoreSourceOutcome::RestoredExact,
            Ok(false) => RestoreSourceOutcome::RestoredChanged,
            Err(_) => RestoreSourceOutcome::SnapshotUnreadable,
        }
    }
}

#[cfg(target_os = "windows")]
fn regular_windows_file(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.is_file() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ephemeral::TempWorkspace;
    use std::fmt::Write as FormatWrite;
    use std::fs;

    fn segment(marker: u8, payload: &[u8]) -> Vec<u8> {
        let length = u16::try_from(payload.len() + 2).unwrap();
        let mut bytes = vec![0xff, marker];
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    fn jpeg(segments: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xd8];
        for segment in segments {
            bytes.extend_from_slice(segment);
        }
        bytes.extend_from_slice(&segment(0xda, &[0, 1, 0, 0, 0, 0]));
        bytes.extend_from_slice(&[1, 2, 0xff, 0x00, 3, 0xff, 0xd9]);
        bytes
    }

    fn xmp_packet(body: &str) -> Vec<u8> {
        let mut payload = XMP_APP1_PREFIX.to_vec();
        payload.extend_from_slice(body.as_bytes());
        payload
    }

    fn xmp_with_rating(value: &str) -> Vec<u8> {
        xmp_packet(&format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="{rdf}"><rdf:Description rdf:about="" xmlns:xmp="{xmp}" xmp:Rating="{value}" keep="yes"/></rdf:RDF></x:xmpmeta>"#,
            rdf = std::str::from_utf8(RDF_NAMESPACE).unwrap(),
            xmp = std::str::from_utf8(XMP_NAMESPACE).unwrap(),
        ))
    }

    fn exif_with_rating(value: u16, endian: Endian) -> Vec<u8> {
        let mut tiff = match endian {
            Endian::Little => b"II".to_vec(),
            Endian::Big => b"MM".to_vec(),
        };
        let push_u16 = |bytes: &mut Vec<u8>, number: u16| match endian {
            Endian::Little => bytes.extend_from_slice(&number.to_le_bytes()),
            Endian::Big => bytes.extend_from_slice(&number.to_be_bytes()),
        };
        let push_u32 = |bytes: &mut Vec<u8>, number: u32| match endian {
            Endian::Little => bytes.extend_from_slice(&number.to_le_bytes()),
            Endian::Big => bytes.extend_from_slice(&number.to_be_bytes()),
        };
        push_u16(&mut tiff, 42);
        push_u32(&mut tiff, 8);
        push_u16(&mut tiff, 1);
        push_u16(&mut tiff, SIMPLE_RATING_TAG);
        push_u16(&mut tiff, 3);
        push_u32(&mut tiff, 1);
        push_u16(&mut tiff, value);
        push_u16(&mut tiff, 0);
        push_u32(&mut tiff, 0);
        let mut payload = EXIF_APP1_PREFIX.to_vec();
        payload.extend_from_slice(&tiff);
        payload
    }

    fn parse(bytes: &[u8]) -> Result<(JpegHeader, RatingState), MetadataError> {
        let mut reader = bytes;
        let header = read_jpeg_header(&mut reader)?;
        let rating = rating_from_header(&header)?;
        Ok((header, rating))
    }

    #[cfg(target_os = "windows")]
    fn comparable_security_descriptor(mut descriptor: Vec<u8>) -> Vec<u8> {
        use windows_sys::Win32::Security::SE_DACL_AUTO_INHERITED;

        if let Some(control) = descriptor.get_mut(2..4) {
            let value = u16::from_le_bytes([control[0], control[1]]) & !SE_DACL_AUTO_INHERITED;
            control.copy_from_slice(&value.to_le_bytes());
        }
        descriptor
    }

    #[test]
    fn validated_rating_domain_and_filter_semantics() {
        assert_eq!(Rating::new(0), None);
        assert_eq!(Rating::new(6), None);
        assert_eq!(Rating::new(4).unwrap().get(), 4);
        assert!(RatingState::Unrated.matches(RatingFilter::All));
        assert!(!RatingState::Rejected.matches(RatingFilter::AtLeast(Rating(1))));
        assert!(RatingState::Rated(Rating(4)).matches(RatingFilter::AtLeast(Rating(3))));
        assert!(!RatingState::Rated(Rating(3)).matches(RatingFilter::AtLeast(Rating(4))));
    }

    #[test]
    fn reads_attribute_and_element_xmp_forms() {
        let attribute = jpeg(&[segment(0xe1, &xmp_with_rating("4"))]);
        assert_eq!(parse(&attribute).unwrap().1, RatingState::Rated(Rating(4)));

        let element = xmp_packet(&format!(
            r#"<rdf:RDF xmlns:rdf="{rdf}" xmlns:score="{xmp}"><rdf:Description rdf:about=""><score:Rating> 5 </score:Rating></rdf:Description></rdf:RDF>"#,
            rdf = std::str::from_utf8(RDF_NAMESPACE).unwrap(),
            xmp = std::str::from_utf8(XMP_NAMESPACE).unwrap(),
        ));
        assert_eq!(
            parse(&jpeg(&[segment(0xe1, &element)])).unwrap().1,
            RatingState::Rated(Rating(5))
        );
    }

    #[test]
    fn distinguishes_unrated_rejected_conflict_and_unsupported() {
        assert_eq!(parse(&jpeg(&[])).unwrap().1, RatingState::Unrated);
        assert_eq!(
            parse(&jpeg(&[segment(0xe1, &xmp_with_rating("-1"))]))
                .unwrap()
                .1,
            RatingState::Rejected
        );
        assert_eq!(
            parse(&jpeg(&[
                segment(0xe1, &xmp_with_rating("4")),
                segment(0xe1, &exif_with_rating(3, Endian::Little)),
            ]))
            .unwrap()
            .1,
            RatingState::Conflict
        );
        for invalid in ["1.5", "6", "-2", "NaN"] {
            assert!(matches!(
                parse(&jpeg(&[segment(0xe1, &xmp_with_rating(invalid))])),
                Err(MetadataError::Unsupported)
            ));
        }
    }

    #[test]
    fn rejects_duplicate_extended_and_malformed_metadata() {
        let duplicate = xmp_packet(&format!(
            r#"<rdf:RDF xmlns:rdf="{rdf}" xmlns:xmp="{xmp}"><rdf:Description rdf:about="" xmp:Rating="3"><xmp:Rating>3</xmp:Rating></rdf:Description></rdf:RDF>"#,
            rdf = std::str::from_utf8(RDF_NAMESPACE).unwrap(),
            xmp = std::str::from_utf8(XMP_NAMESPACE).unwrap(),
        ));
        assert!(matches!(
            parse(&jpeg(&[segment(0xe1, &duplicate)])),
            Err(MetadataError::Unsupported)
        ));
        assert!(matches!(
            parse(&jpeg(&[
                segment(0xe1, &xmp_with_rating("3")),
                segment(0xe1, &xmp_with_rating("3")),
            ])),
            Err(MetadataError::Unsupported)
        ));
        assert!(matches!(
            parse(&jpeg(&[segment(0xe1, EXTENDED_XMP_APP1_PREFIX)])),
            Err(MetadataError::Unsupported)
        ));
        assert!(matches!(
            parse(&jpeg(&[segment(0xe1, &xmp_packet("<open>"))])),
            Err(MetadataError::Unsupported)
        ));
    }

    #[test]
    fn validates_xmp_rdf_structure_and_description_subjects() {
        let rdf = std::str::from_utf8(RDF_NAMESPACE).unwrap();
        let xmp = std::str::from_utf8(XMP_NAMESPACE).unwrap();
        let valid = format!(
            r#"<rdf:RDF xmlns:rdf="{rdf}" xmlns:xmp="{xmp}"><rdf:Description rdf:about="urn:photo" keep="one"/><rdf:Description rdf:about="urn:photo" xmp:Rating="4" keep="two"/></rdf:RDF>"#,
        );
        assert_eq!(
            parse_xmp_rating(valid.as_bytes()),
            Ok(Some(RatingState::Rated(Rating(4))))
        );
        let rewritten =
            rewrite_xmp_packet(valid.as_bytes(), RatingAssignment::Set(Rating(5))).unwrap();
        assert_eq!(
            parse_xmp_rating(&rewritten),
            Ok(Some(RatingState::Rated(Rating(5))))
        );
        assert_eq!(
            std::str::from_utf8(&rewritten)
                .unwrap()
                .matches(":Rating=")
                .count(),
            1
        );

        let invalid = [
            format!(
                r#"<rdf:RDF xmlns:rdf="{rdf}" xmlns:xmp="{xmp}"><rdf:Description xmp:Rating="4"/></rdf:RDF>"#,
            ),
            format!(
                r#"<rdf:RDF xmlns:rdf="{rdf}" xmlns:xmp="{xmp}"><rdf:Description rdf:about="one"/><rdf:Description rdf:about="two" xmp:Rating="4"/></rdf:RDF>"#,
            ),
            format!(
                r#"<rdf:RDF xmlns:rdf="{rdf}" xmlns:xmp="{xmp}" xmlns:n="urn:nested"><rdf:Description rdf:about=""><n:value><xmp:Rating>4</xmp:Rating></n:value></rdf:Description></rdf:RDF>"#,
            ),
            format!(
                r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" xmlns:rdf="{rdf}"><rdf:Description rdf:about=""/></x:xmpmeta>"#,
            ),
        ];
        for packet in invalid {
            assert_eq!(
                parse_xmp_rating(packet.as_bytes()),
                Err(MetadataError::Unsupported)
            );
        }

        let generated = new_xmp_packet(Rating(3));
        assert!(generated.contains("rdf:about=\"\""));
        assert_eq!(
            parse_xmp_rating(generated.as_bytes()),
            Ok(Some(RatingState::Rated(Rating(3))))
        );
    }

    #[test]
    fn parser_limits_depth_attributes_namespaces_and_doctype() {
        let deep = format!(
            "{}{}",
            "<a>".repeat(MAX_XML_DEPTH + 1),
            "</a>".repeat(MAX_XML_DEPTH + 1)
        );
        assert_eq!(
            parse_xmp_rating(deep.as_bytes()),
            Err(MetadataError::Unsupported)
        );

        let mut attributes = String::new();
        for index in 0..=MAX_XML_ATTRIBUTES_PER_ELEMENT {
            write!(&mut attributes, " a{index}=\"x\"").unwrap();
        }
        assert_eq!(
            parse_xmp_rating(format!("<a{attributes}/>").as_bytes()),
            Err(MetadataError::Unsupported)
        );

        let mut namespaces = String::new();
        for index in 0..=MAX_XML_NAMESPACE_DECLARATIONS {
            write!(&mut namespaces, " xmlns:n{index}=\"urn:{index}\"").unwrap();
        }
        assert_eq!(
            parse_xmp_rating(format!("<a{namespaces}/>").as_bytes()),
            Err(MetadataError::Unsupported)
        );
        assert_eq!(
            parse_xmp_rating(b"<!DOCTYPE x [<!ENTITY e '4'>]><x>&e;</x>"),
            Err(MetadataError::Unsupported)
        );

        let prefix = std::io::Cursor::new([0xff, 0xd8]);
        let fill = std::io::repeat(0xff).take(
            u64::try_from(MAX_HEADER_BYTES)
                .expect("header limit fits u64")
                .saturating_add(1),
        );
        let mut marker_fill = prefix.chain(fill);
        assert!(matches!(
            read_jpeg_header(&mut marker_fill),
            Err(MetadataError::Unsupported)
        ));
    }

    #[test]
    fn reads_little_and_big_endian_simple_rating() {
        for endian in [Endian::Little, Endian::Big] {
            let image = jpeg(&[segment(0xe1, &exif_with_rating(5, endian))]);
            assert_eq!(parse(&image).unwrap().1, RatingState::Rated(Rating(5)));
        }
    }

    #[test]
    fn rewrites_only_rating_metadata_and_preserves_scan_tail() {
        let comment = segment(0xfe, b"keep this comment exactly");
        let unknown = segment(0xef, b"unknown metadata");
        let image = jpeg(&[
            segment(0xe0, b"JFIF\0keep"),
            segment(0xe1, &xmp_with_rating("2")),
            segment(0xe1, &exif_with_rating(2, Endian::Big)),
            comment.clone(),
            unknown.clone(),
        ]);
        let (header, _) = parse(&image).unwrap();
        let rewritten = rewrite_header(&header, RatingAssignment::Set(Rating(4))).unwrap();
        let tail = &image[usize::try_from(header.encoded_len).unwrap()..];
        let mut candidate = rewritten.clone();
        candidate.extend_from_slice(tail);
        let (new_header, state) = parse(&candidate).unwrap();
        assert_eq!(state, RatingState::Rated(Rating(4)));
        assert_eq!(
            &candidate[usize::try_from(new_header.encoded_len).unwrap()..],
            tail
        );
        assert!(new_header.segments.iter().any(|entry| entry.raw == comment));
        assert!(new_header.segments.iter().any(|entry| entry.raw == unknown));
        let xmp = new_header
            .segments
            .iter()
            .find_map(|entry| entry.payload().strip_prefix(XMP_APP1_PREFIX))
            .unwrap();
        let xmp = std::str::from_utf8(xmp).unwrap();
        assert!(xmp.contains("keep=\"yes\""));
    }

    #[test]
    fn inserts_xmp_without_inserting_or_relocating_exif() {
        let exif = segment(0xe1, &exif_with_rating(0, Endian::Little));
        let image = jpeg(&[segment(0xe0, b"JFIF\0"), exif]);
        let (header, _) = parse(&image).unwrap();
        let rewritten = rewrite_header(&header, RatingAssignment::Set(Rating(3))).unwrap();
        let mut candidate = rewritten;
        candidate.extend_from_slice(&image[usize::try_from(header.encoded_len).unwrap()..]);
        assert_eq!(parse(&candidate).unwrap().1, RatingState::Rated(Rating(3)));
    }

    #[test]
    fn clear_removes_xmp_rating_and_zeroes_existing_mirror() {
        let image = jpeg(&[
            segment(0xe1, &xmp_with_rating("5")),
            segment(0xe1, &exif_with_rating(5, Endian::Little)),
        ]);
        let (header, _) = parse(&image).unwrap();
        let rewritten = rewrite_header(&header, RatingAssignment::Clear).unwrap();
        let mut candidate = rewritten;
        candidate.extend_from_slice(&image[usize::try_from(header.encoded_len).unwrap()..]);
        assert_eq!(parse(&candidate).unwrap().1, RatingState::Unrated);
        let packet = parse(&candidate)
            .unwrap()
            .0
            .segments
            .into_iter()
            .find_map(|entry| entry.payload().strip_prefix(XMP_APP1_PREFIX).map(Vec::from))
            .unwrap();
        assert!(!std::str::from_utf8(&packet).unwrap().contains("Rating"));
    }

    #[test]
    fn non_jpeg_is_read_only_and_truncation_is_unreadable() {
        let mut not_jpeg = b"not a jpeg".as_slice();
        assert!(matches!(
            read_jpeg_header(&mut not_jpeg),
            Err(MetadataError::NotJpeg)
        ));
        let mut truncated = [0xff, 0xd8, 0xff, 0xe1].as_slice();
        assert!(matches!(
            read_jpeg_header(&mut truncated),
            Err(MetadataError::Unreadable)
        ));
    }

    #[test]
    fn accepted_rating_is_rejected_if_the_source_changes_during_inspection() {
        let workspace = TempWorkspace::new("rating_source_version").unwrap();
        let path = workspace.path().join("photo.jpg");
        let replacement = jpeg(&[
            segment(0xe1, &xmp_with_rating("5")),
            segment(0xfe, b"replacement source version"),
        ]);
        fs::write(&path, jpeg(&[segment(0xe1, &xmp_with_rating("2"))])).unwrap();
        let source = ImageSource::open(&path).unwrap();

        let observation = observe_source_with_hook(&source, &path, || {
            fs::write(&path, replacement).unwrap();
        });

        assert_eq!(observation.state, RatingState::Unreadable);
        assert_eq!(observation.capability, RatingWriteCapability::UnsafeSource);
        assert!(!source.version_is_current());
    }

    #[test]
    fn accepted_rating_stops_at_a_superseded_version_boundary() {
        let workspace = TempWorkspace::new("rating_source_cancellation").unwrap();
        let path = workspace.path().join("photo.jpg");
        fs::write(&path, jpeg(&[segment(0xe1, &xmp_with_rating("2"))])).unwrap();
        let source = ImageSource::open(&path).unwrap();
        let checks = std::cell::Cell::new(0_u8);

        let observation = observe_source_while(&source, &path, || {
            let next = checks.get().saturating_add(1);
            checks.set(next);
            next < 3
        });

        assert_eq!(checks.get(), 3);
        assert_eq!(observation.state, RatingState::Unreadable);
        assert_eq!(observation.capability, RatingWriteCapability::UnsafeSource);
    }

    #[test]
    fn folder_rating_scan_returns_the_bounded_state_for_the_scanned_object() {
        let workspace = TempWorkspace::new("rating_folder_scan").unwrap();
        let path = workspace.path().join("photo.jpg");
        fs::write(&path, jpeg(&[segment(0xe1, &xmp_with_rating("4"))])).unwrap();
        let (_, provenance) = crate::fs::scan_image_entries_while(workspace.path(), || true)
            .unwrap()
            .pop()
            .unwrap()
            .into_parts();

        assert_eq!(
            scan_path_rating_while(&path, Some(provenance), || true),
            RatingState::Rated(Rating(4))
        );
    }

    #[test]
    fn folder_rating_scan_keeps_order_with_several_workers() {
        let workspace = TempWorkspace::new("rating_folder_parallel").unwrap();
        for (name, rating) in [("a.jpg", "1"), ("b.jpg", "2"), ("c.jpg", "5")] {
            fs::write(
                workspace.path().join(name),
                jpeg(&[segment(0xe1, &xmp_with_rating(rating))]),
            )
            .unwrap();
        }
        let files = crate::fs::scan_image_entries_while(workspace.path(), || true)
            .unwrap()
            .into_iter()
            .map(|entry| {
                let (path, provenance) = entry.into_parts();
                (path, Some(provenance))
            })
            .collect::<Vec<_>>();
        let sequential = scan_folder_ratings_while(files.clone(), || true, 1).unwrap();
        let parallel = scan_folder_ratings_while(files, || true, 4).unwrap();
        assert_eq!(sequential, parallel);
        assert_eq!(
            sequential
                .iter()
                .map(|(_, state)| *state)
                .collect::<Vec<_>>(),
            [
                RatingState::Rated(Rating(1)),
                RatingState::Rated(Rating(2)),
                RatingState::Rated(Rating(5)),
            ]
        );
    }

    #[test]
    fn folder_rating_scan_returns_none_when_cancelled() {
        let workspace = TempWorkspace::new("rating_folder_cancel").unwrap();
        for name in ["a.jpg", "b.jpg", "c.jpg"] {
            fs::write(
                workspace.path().join(name),
                jpeg(&[segment(0xe1, &xmp_with_rating("3"))]),
            )
            .unwrap();
        }
        let files = crate::fs::scan_image_entries_while(workspace.path(), || true)
            .unwrap()
            .into_iter()
            .map(|entry| {
                let (path, provenance) = entry.into_parts();
                (path, Some(provenance))
            })
            .collect::<Vec<_>>();
        let calls = AtomicUsize::new(0);
        let result =
            scan_folder_ratings_while(files, || calls.fetch_add(1, Ordering::Relaxed) < 1, 4);
        assert!(result.is_none());
    }

    #[test]
    fn folder_rating_scan_rejects_a_same_length_header_rewrite() {
        let workspace = TempWorkspace::new("rating_folder_rewrite").unwrap();
        let path = workspace.path().join("photo.jpg");
        let accepted = jpeg(&[segment(0xe1, &xmp_with_rating("2"))]);
        let replacement = jpeg(&[segment(0xe1, &xmp_with_rating("5"))]);
        assert_eq!(accepted.len(), replacement.len());
        fs::write(&path, accepted).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let (_, provenance) = crate::fs::scan_image_entries_while(workspace.path(), || true)
            .unwrap()
            .pop()
            .unwrap()
            .into_parts();

        let rating = scan_path_rating_while_with_hook(
            &path,
            Some(provenance),
            || true,
            || {
                fs::write(&path, replacement).unwrap();
                std::fs::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_times(std::fs::FileTimes::new().set_modified(modified))
                    .unwrap();
            },
        );

        assert_eq!(rating, RatingState::Unreadable);
    }

    #[test]
    fn folder_rating_snapshots_compare_consumed_bytes_for_parse_failures() {
        let mut current = || true;
        let first =
            read_jpeg_header_snapshot_from_reader_while(b"first non-JPEG".as_slice(), &mut current);
        let mut current = || true;
        let same =
            read_jpeg_header_snapshot_from_reader_while(b"first non-JPEG".as_slice(), &mut current);
        let mut current = || true;
        let changed = read_jpeg_header_snapshot_from_reader_while(
            b"second non-JPEG".as_slice(),
            &mut current,
        );

        assert_eq!(
            rating_from_matching_header_snapshots(&first, &changed),
            RatingState::Unreadable
        );
        let mut current = || true;
        let first =
            read_jpeg_header_snapshot_from_reader_while(b"first non-JPEG".as_slice(), &mut current);
        assert_eq!(
            rating_from_matching_header_snapshots(&first, &same),
            RatingState::Unrated
        );
    }

    #[test]
    fn folder_rating_header_stops_between_segments_when_cancelled() {
        let encoded = jpeg(&[
            segment(0xfe, &[0x41; 1024]),
            segment(0xe1, &xmp_with_rating("3")),
        ]);
        let mut reader = encoded.as_slice();
        let mut checks = 0_u8;

        let result = read_jpeg_header_while(&mut reader, || {
            checks = checks.saturating_add(1);
            checks < 4
        });

        assert!(matches!(result, Err(MetadataError::Unreadable)));
        assert_eq!(checks, 4);
        assert!(!reader.is_empty());
    }

    #[test]
    fn source_writer_rejects_hidden_metadata_signatures_after_first_scan() {
        let tail_bytes = [
            b"entropy".as_slice(),
            XMP_APP1_PREFIX,
            b"more entropy".as_slice(),
        ]
        .concat();
        let mut tail = tail_bytes.as_slice();
        assert_eq!(
            reject_metadata_signatures_in_tail(&mut tail),
            Err(MetadataError::Unsupported)
        );
        let mut ordinary = b"entropy without metadata marker".as_slice();
        assert_eq!(reject_metadata_signatures_in_tail(&mut ordinary), Ok(()));
    }

    #[test]
    fn jpeg_rating_write_replaces_verifies_and_rebinds_source() {
        let workspace = TempWorkspace::new("rating_transaction").unwrap();
        let path = workspace.path().join("photo.jpg");
        let original = jpeg(&[
            segment(0xe1, &xmp_with_rating("2")),
            segment(0xfe, b"preserve"),
        ]);
        fs::write(&path, &original).unwrap();
        let source = ImageSource::open(&path).unwrap();
        let written = write_rating(&path, &source, RatingAssignment::Set(Rating(5))).unwrap();
        assert_eq!(written.state, RatingState::Rated(Rating(5)));
        assert_eq!(
            observe_source(&written.source, &path).state,
            RatingState::Rated(Rating(5))
        );
        assert_eq!(source.matches_path(&path), ImageSourceMatch::Changed);
        let mut entries = fs::read_dir(workspace.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        entries.sort();
        assert_eq!(
            entries,
            [std::ffi::OsString::from("photo.jpg")],
            "unexpected transaction files"
        );
    }

    #[test]
    fn jpeg_rating_write_rejects_stale_source_without_mutation() {
        let workspace = TempWorkspace::new("rating_stale_source").unwrap();
        let path = workspace.path().join("photo.jpg");
        let original = jpeg(&[segment(0xe1, &xmp_with_rating("2"))]);
        fs::write(&path, &original).unwrap();
        let source = ImageSource::open(&path).unwrap();
        let externally_changed = jpeg(&[
            segment(0xe1, &xmp_with_rating("3")),
            segment(0xfe, b"external edit with a distinct source version"),
        ]);
        fs::write(&path, externally_changed).unwrap();
        assert!(matches!(
            write_rating(&path, &source, RatingAssignment::Set(Rating(5))),
            Err(RatingWriteError::SourceChanged)
        ));
        assert_eq!(
            observe_source(&ImageSource::open(&path).unwrap(), &path).state,
            RatingState::Rated(Rating(3))
        );
    }

    #[test]
    fn ordinary_jpeg_is_writable_when_the_source_is_a_single_regular_file() {
        let workspace = TempWorkspace::new("rating_writable_jpeg").unwrap();
        let path = workspace.path().join("photo.jpg");
        fs::write(&path, jpeg(&[segment(0xe1, &xmp_with_rating("2"))])).unwrap();
        let source = ImageSource::open(&path).unwrap();
        let observation = observe_source(&source, &path);
        assert_eq!(observation.state, RatingState::Rated(Rating(2)));
        assert_eq!(observation.capability, RatingWriteCapability::WritableJpeg);
    }

    #[cfg(unix)]
    #[test]
    fn extra_hard_links_stay_read_only() {
        let workspace = TempWorkspace::new("rating_hard_link").unwrap();
        let path = workspace.path().join("photo.jpg");
        let linked = workspace.path().join("alias.jpg");
        fs::write(&path, jpeg(&[segment(0xe1, &xmp_with_rating("2"))])).unwrap();
        std::fs::hard_link(&path, &linked).unwrap();
        let source = ImageSource::open(&path).unwrap();
        assert!(source.extra_hard_links());
        assert_eq!(
            observe_source(&source, &path).capability,
            RatingWriteCapability::UnsafeSource
        );
        assert!(matches!(
            write_rating(&path, &source, RatingAssignment::Set(Rating(5))),
            Err(RatingWriteError::SourceChanged)
        ));
        assert_eq!(
            observe_source(&ImageSource::open(&path).unwrap(), &path).state,
            RatingState::Rated(Rating(2))
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_rating_replacement_preserves_mode_and_removes_transaction_files() {
        use std::os::unix::fs::PermissionsExt;

        let workspace = TempWorkspace::new("rating_unix_mode").unwrap();
        let path = workspace.path().join("photo.jpg");
        fs::write(&path, jpeg(&[segment(0xe1, &xmp_with_rating("2"))])).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let source = ImageSource::open(&path).unwrap();

        let written = write_rating(&path, &source, RatingAssignment::Set(Rating(4))).unwrap();

        assert_eq!(written.state, RatingState::Rated(Rating(4)));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        let entries = fs::read_dir(workspace.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(entries, [std::ffi::OsString::from("photo.jpg")]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn secured_transaction_files_copy_source_acl_and_pristine_is_ephemeral() {
        let workspace = TempWorkspace::new("rating_secured_temps").unwrap();
        let source_path = workspace.path().join("photo.jpg");
        fs::write(&source_path, b"source").unwrap();
        let source_file = File::open(&source_path).unwrap();
        let expected = file_security_descriptor(&source_file).unwrap();
        let mut descriptor = expected.clone();

        let work = secured_named_temp_file(
            workspace.path(),
            ".viewr-rating-work-",
            &mut descriptor,
            false,
        )
        .unwrap();
        assert!(
            work.path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".viewr-rating-work-")
        );
        assert_eq!(
            comparable_security_descriptor(file_security_descriptor(work.as_file()).unwrap()),
            comparable_security_descriptor(expected.clone())
        );

        let pristine = secured_named_temp_file(
            workspace.path(),
            ".viewr-rating-pristine-",
            &mut descriptor,
            true,
        )
        .unwrap();
        assert_eq!(
            comparable_security_descriptor(file_security_descriptor(pristine.as_file()).unwrap()),
            comparable_security_descriptor(expected)
        );
        let (mut pristine_file, pristine_path) = pristine.into_parts();
        let path = pristine_path.to_path_buf();
        drop(pristine_path);
        pristine_file.write_all(b"retained only by handle").unwrap();
        drop(pristine_file);
        assert!(!path.exists());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn candidate_binding_detects_replaced_or_missing_success_path() {
        let workspace = TempWorkspace::new("rating_candidate_binding").unwrap();
        let path = workspace.path().join("photo.jpg");
        let displaced = workspace.path().join("displaced.jpg");
        fs::write(&path, jpeg(&[segment(0xe1, &xmp_with_rating("5"))])).unwrap();
        let candidate = ImageSource::open(&path).unwrap();
        assert_eq!(candidate_binding_error(&candidate, &path), None);

        fs::rename(&path, &displaced).unwrap();
        fs::write(&path, jpeg(&[segment(0xe1, &xmp_with_rating("3"))])).unwrap();
        assert_eq!(
            candidate_binding_error(&candidate, &path),
            Some(RatingWriteError::SourceChanged)
        );
        fs::remove_file(&path).unwrap();
        assert_eq!(
            candidate_binding_error(&candidate, &path),
            Some(RatingWriteError::SourceChanged)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn restore_rechecks_retained_bytes_after_renaming_backup() {
        let workspace = TempWorkspace::new("rating_restore_exactness").unwrap();
        let original = jpeg(&[segment(0xe1, &xmp_with_rating("2"))]);
        let changed = jpeg(&[segment(0xe1, &xmp_with_rating("5"))]);
        let path = workspace.path().join("photo.jpg");
        let pristine_path = workspace.path().join("pristine.jpg");
        let backup = workspace.path().join("backup.jpg");
        fs::write(&path, &original).unwrap();
        fs::write(&pristine_path, &original).unwrap();
        let source = ImageSource::open(&path).unwrap();
        let pristine = File::open(&pristine_path).unwrap();

        fs::rename(&path, &backup).unwrap();
        fs::write(&backup, &changed).unwrap();
        assert!(source.same_object_at_path(&backup));
        assert_eq!(
            restore_retained_source(&path, &backup, &source, &pristine, None),
            RestoreSourceOutcome::RestoredChanged
        );
        assert_eq!(fs::read(&path).unwrap(), changed);
        assert!(!backup.exists());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn failed_replace_reconciles_safe_partial_windows_states() {
        let workspace = TempWorkspace::new("rating_replace_reconciliation").unwrap();
        let original = jpeg(&[segment(0xe1, &xmp_with_rating("2"))]);
        let candidate = jpeg(&[segment(0xe1, &xmp_with_rating("5"))]);
        let path = workspace.path().join("photo.jpg");
        let pristine_path = workspace.path().join("pristine.jpg");
        let backup = workspace.path().join("backup.jpg");
        fs::write(&path, &original).unwrap();
        fs::write(&pristine_path, &original).unwrap();
        let source = ImageSource::open(&path).unwrap();
        let pristine = File::open(&pristine_path).unwrap();
        let partial_error = io::Error::from_raw_os_error(1177);

        fs::rename(&path, &backup).unwrap();
        assert_eq!(
            reconcile_failed_replace(&path, &backup, &source, &pristine, &partial_error),
            RatingWriteError::VerificationRestored
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(!backup.exists());

        fs::rename(&path, &backup).unwrap();
        fs::write(&path, &candidate).unwrap();
        let replacement = ImageSource::open(&path).unwrap();
        assert_eq!(
            restore_retained_source(&path, &backup, &source, &pristine, Some(&replacement)),
            RestoreSourceOutcome::RestoredExact
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(!backup.exists());

        let permission_error = io::Error::from_raw_os_error(5);
        assert_eq!(
            reconcile_failed_replace(&path, &backup, &source, &pristine, &permission_error),
            RatingWriteError::PermissionDenied
        );
        assert_eq!(fs::read(&path).unwrap(), original);

        fs::write(&path, &candidate).unwrap();
        assert_eq!(
            reconcile_failed_replace(&path, &backup, &source, &pristine, &partial_error),
            RatingWriteError::SourceChanged
        );
        assert_eq!(fs::read(&path).unwrap(), candidate);
    }
}
