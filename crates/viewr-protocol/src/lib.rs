//! Versioned IPC and decoded-image invariants shared by `viewr` and its worker.
//!
//! Encoded image bytes are framed with a validated format identifier rather than
//! a filesystem path. The main process opens user-selected files and the worker
//! therefore needs no dynamic filesystem grant.

use std::io::{self, Read, Write};

const REQUEST_FRAME_MAGIC: [u8; 4] = *b"VWI1";
const ACK_FRAME: [u8; 4] = *b"ACK1";
const RESPONSE_FRAME_MAGIC: [u8; 4] = *b"VRS1";
const REQUEST_FRAME_HEADER_BYTES: usize = 16;
const RESPONSE_FRAME_HEADER_BYTES: usize = 9;
/// Reserved format identifier for the package-level protocol handshake.
pub const PROBE_FORMAT: &str = "probe";
/// Largest format identifier accepted by the worker protocol.
pub const MAX_FORMAT_BYTES: usize = 16;
/// Largest encoded image accepted by the worker protocol.
pub const MAX_ENCODED_INPUT_BYTES: u64 = 512 * 1024 * 1024;
/// Largest typed response payload accepted by the worker protocol.
pub const MAX_RESPONSE_PAYLOAD_BYTES: usize = 4096;
const RESPONSE_PIXEL_STREAM: u8 = 1;
const RESPONSE_ERROR: u8 = 2;
const RESPONSE_PROBE: u8 = 3;
const PROBE_PAYLOAD: [u8; 4] = *b"VWI1";

/// Longest decoded image edge accepted at either side of the trust boundary.
pub const MAX_DECODE_DIMENSION: u32 = u16::MAX as u32;
/// Maximum decoded pixel count, equal to 512 MiB of tightly packed RGBA8.
pub const MAX_DECODE_PIXELS: u64 = 128 * 1024 * 1024;
/// Maximum byte length of a tightly packed decoded RGBA8 image.
pub const MAX_RGBA_BYTES: u64 = MAX_DECODE_PIXELS * 4;

/// One complete decode request received at the worker boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodeRequest {
    /// Lowercase ASCII image format identifier, such as `avif` or `heic`.
    pub format: String,
    /// Complete encoded image payload, bounded by [`MAX_ENCODED_INPUT_BYTES`].
    pub encoded: Vec<u8>,
}

/// Why a decoded image shape is invalid at the process boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShapeError {
    /// Either edge is zero.
    ZeroDimension,
    /// An edge exceeds [`MAX_DECODE_DIMENSION`].
    DimensionLimit,
    /// The pixel count or byte length exceeds the bounded RGBA allocation.
    AllocationLimit,
}

/// One complete response from the isolated decode worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerResponse {
    /// Tightly packed RGBA8 bytes follow this frame on the same stream.
    PixelStream {
        /// Decoded pixel width.
        width: u32,
        /// Decoded pixel height.
        height: u32,
    },
    /// A bounded, user-displayable decoder failure.
    Error(String),
    /// Exact acknowledgement of the encoded-input protocol handshake.
    Probe,
}

impl std::fmt::Display for ShapeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::ZeroDimension => "image dimensions must be non-zero",
            Self::DimensionLimit | Self::AllocationLimit => "image dimensions exceed safety limit",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ShapeError {}

/// Validate decoded dimensions and return the exact tightly packed RGBA8 length.
///
/// # Errors
/// Returns [`ShapeError`] when either dimension is zero, a dimension exceeds
/// the protocol limit, or the resulting allocation would exceed the pixel or
/// address-space limit.
pub fn checked_rgba_len(width: u32, height: u32) -> Result<usize, ShapeError> {
    if width == 0 || height == 0 {
        return Err(ShapeError::ZeroDimension);
    }
    if width > MAX_DECODE_DIMENSION || height > MAX_DECODE_DIMENSION {
        return Err(ShapeError::DimensionLimit);
    }
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(ShapeError::AllocationLimit)?;
    if pixels > MAX_DECODE_PIXELS {
        return Err(ShapeError::AllocationLimit);
    }
    pixels
        .checked_mul(4)
        .and_then(|bytes| usize::try_from(bytes).ok())
        .ok_or(ShapeError::AllocationLimit)
}

/// Write one versioned, length-prefixed encoded-image request.
///
/// # Errors
/// Returns an I/O error when the format or payload violates the protocol
/// bounds, or when the destination cannot accept the complete frame.
pub fn write_decode_request(
    writer: &mut impl Write,
    format: &str,
    encoded: &[u8],
) -> io::Result<()> {
    if !valid_format(format) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid worker format identifier",
        ));
    }
    let format_length = u8::try_from(format.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid worker format identifier",
        )
    })?;
    let encoded_length = u64::try_from(encoded.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "encoded input exceeds IPC safety limit",
        )
    })?;
    if encoded_length > MAX_ENCODED_INPUT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "encoded input exceeds IPC safety limit",
        ));
    }

    writer.write_all(&REQUEST_FRAME_MAGIC)?;
    writer.write_all(&[format_length, 0, 0, 0])?;
    writer.write_all(&encoded_length.to_le_bytes())?;
    writer.write_all(format.as_bytes())?;
    writer.write_all(encoded)
}

/// Read one encoded-image request, returning `None` only for clean stream EOF.
///
/// # Errors
/// Returns an I/O error for a malformed, truncated, oversized, or unreadable
/// request. Allocation failure is reported instead of aborting the process.
pub fn read_decode_request(reader: &mut impl Read) -> io::Result<Option<DecodeRequest>> {
    let mut header = [0_u8; REQUEST_FRAME_HEADER_BYTES];
    match reader.read_exact(&mut header[..1]) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    reader.read_exact(&mut header[1..])?;
    if header[..4] != REQUEST_FRAME_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported worker protocol frame",
        ));
    }
    if header[5..8] != [0, 0, 0] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid worker protocol header",
        ));
    }
    let format_length = usize::from(header[4]);
    if format_length == 0 || format_length > MAX_FORMAT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid worker format identifier",
        ));
    }

    let encoded_length = u64::from_le_bytes(header[8..16].try_into().map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, "invalid worker protocol header")
    })?);
    if encoded_length > MAX_ENCODED_INPUT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "encoded input exceeds IPC safety limit",
        ));
    }
    let encoded_length = usize::try_from(encoded_length).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "encoded input length is not representable",
        )
    })?;

    let mut format = vec![0_u8; format_length];
    reader.read_exact(&mut format)?;
    let format = String::from_utf8(format)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "worker format is not UTF-8"))?;
    if !valid_format(&format) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid worker format identifier",
        ));
    }

    let mut encoded = Vec::new();
    encoded.try_reserve_exact(encoded_length).map_err(|_| {
        io::Error::new(
            io::ErrorKind::OutOfMemory,
            "encoded input allocation failed",
        )
    })?;
    encoded.resize(encoded_length, 0);
    reader.read_exact(&mut encoded)?;
    Ok(Some(DecodeRequest { format, encoded }))
}

fn valid_format(format: &str) -> bool {
    !format.is_empty()
        && format.len() <= MAX_FORMAT_BYTES
        && format
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// Write one typed, length-prefixed worker response.
///
/// # Errors
/// Returns an I/O error when the response violates the protocol bounds or the
/// destination cannot accept the complete frame.
pub fn write_worker_response(writer: &mut impl Write, response: &WorkerResponse) -> io::Result<()> {
    let (tag, payload) = match response {
        WorkerResponse::PixelStream { width, height } => {
            checked_rgba_len(*width, *height)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
            let mut payload = Vec::with_capacity(8);
            payload.extend_from_slice(&width.to_le_bytes());
            payload.extend_from_slice(&height.to_le_bytes());
            (RESPONSE_PIXEL_STREAM, payload)
        }
        WorkerResponse::Error(message) => {
            if !valid_worker_error(message) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "worker error is empty or contains control characters",
                ));
            }
            (RESPONSE_ERROR, message.as_bytes().to_vec())
        }
        WorkerResponse::Probe => (RESPONSE_PROBE, PROBE_PAYLOAD.to_vec()),
    };
    if payload.len() > MAX_RESPONSE_PAYLOAD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "worker response exceeds IPC safety limit",
        ));
    }
    let length = u32::try_from(payload.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "worker response exceeds IPC safety limit",
        )
    })?;
    writer.write_all(&RESPONSE_FRAME_MAGIC)?;
    writer.write_all(&[tag])?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(&payload)
}

/// Read and validate one typed, length-prefixed worker response.
///
/// # Errors
/// Returns an I/O error for a malformed, truncated, oversized, or unreadable
/// response.
pub fn read_worker_response(reader: &mut impl Read) -> io::Result<WorkerResponse> {
    let mut header = [0_u8; RESPONSE_FRAME_HEADER_BYTES];
    reader.read_exact(&mut header)?;
    if header[..4] != RESPONSE_FRAME_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported worker response frame",
        ));
    }
    let tag = header[4];
    let length = u32::from_le_bytes(header[5..9].try_into().map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, "invalid worker response header")
    })?) as usize;
    if length == 0 || length > MAX_RESPONSE_PAYLOAD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "worker response exceeds IPC safety limit",
        ));
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;

    match tag {
        RESPONSE_PIXEL_STREAM if payload.len() == 8 => {
            let width = u32::from_le_bytes(payload[..4].try_into().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid worker response width")
            })?);
            let height = u32::from_le_bytes(payload[4..8].try_into().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid worker response height")
            })?);
            checked_rgba_len(width, height)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
            Ok(WorkerResponse::PixelStream { width, height })
        }
        RESPONSE_ERROR => {
            let message = String::from_utf8(payload).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "worker error is not UTF-8")
            })?;
            if !valid_worker_error(&message) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "worker error is empty or contains control characters",
                ));
            }
            Ok(WorkerResponse::Error(message))
        }
        RESPONSE_PROBE if payload == PROBE_PAYLOAD => Ok(WorkerResponse::Probe),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unknown worker response type",
        )),
    }
}

fn valid_worker_error(message: &str) -> bool {
    !message.is_empty() && !message.chars().any(char::is_control)
}

/// Acknowledge that the parent received the complete pixel stream.
///
/// # Errors
/// Returns an I/O error when the acknowledgement cannot be written completely.
pub fn write_ack(writer: &mut impl Write) -> io::Result<()> {
    writer.write_all(&ACK_FRAME)
}

/// Read and validate a pixel-stream acknowledgement.
///
/// # Errors
/// Returns an I/O error when the acknowledgement is missing, truncated, or
/// invalid.
pub fn read_ack(reader: &mut impl Read) -> io::Result<()> {
    let mut ack = [0_u8; ACK_FRAME.len()];
    reader.read_exact(&mut ack)?;
    if ack != ACK_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid worker acknowledgement",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        DecodeRequest, WorkerResponse, checked_rgba_len, read_ack, read_decode_request,
        read_worker_response, write_ack, write_decode_request, write_worker_response,
    };
    use std::io::{self, Cursor, Read, Write};

    fn response_frame(tag: u8, payload: &[u8]) -> Vec<u8> {
        let mut frame = b"VRS1".to_vec();
        frame.push(tag);
        frame.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    struct FailingIo {
        writable_bytes: usize,
    }

    impl Write for FailingIo {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if self.writable_bytes == 0 {
                return Err(io::Error::other("injected write failure"));
            }
            let written = buffer.len().min(self.writable_bytes);
            self.writable_bytes -= written;
            Ok(written)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Read for FailingIo {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("injected read failure"))
        }
    }

    #[test]
    fn shape_validation_covers_zero_edges_limits_and_exact_length() {
        assert_eq!(checked_rgba_len(2, 3).unwrap(), 24);
        assert!(checked_rgba_len(0, 3).is_err());
        assert!(checked_rgba_len(u32::MAX, 1).is_err());
        assert!(checked_rgba_len(u16::MAX.into(), u16::MAX.into()).is_err());
    }

    #[test]
    fn decode_request_round_trips_format_and_binary_payload() {
        let expected = DecodeRequest {
            format: "avif".into(),
            encoded: vec![0, b'\n', 0xff, 42],
        };
        let mut frame = Vec::new();
        write_decode_request(&mut frame, &expected.format, &expected.encoded).unwrap();
        assert_eq!(
            read_decode_request(&mut Cursor::new(frame)).unwrap(),
            Some(expected)
        );
    }

    #[test]
    fn request_reader_rejects_bad_or_truncated_frames() {
        assert!(
            read_decode_request(&mut Cursor::new(Vec::new()))
                .unwrap()
                .is_none()
        );
        assert!(read_decode_request(&mut Cursor::new(b"BAD!\x04\0\0\0".to_vec())).is_err());
        assert!(read_decode_request(&mut Cursor::new(b"VWI1\x04\0\0\0".to_vec())).is_err());
    }

    #[test]
    fn requests_reject_invalid_formats_reserved_bytes_and_oversized_input() {
        for format in ["", "AVIF", "heic!", "abcdefghijklmnopq"] {
            assert!(write_decode_request(&mut Vec::new(), format, b"image").is_err());
        }

        let mut reserved = b"VWI1\x04\x01\0\0".to_vec();
        reserved.extend_from_slice(&0_u64.to_le_bytes());
        assert!(read_decode_request(&mut Cursor::new(reserved)).is_err());

        let mut oversized = b"VWI1\x04\0\0\0".to_vec();
        oversized.extend_from_slice(&(super::MAX_ENCODED_INPUT_BYTES + 1).to_le_bytes());
        oversized.extend_from_slice(b"avif");
        assert!(read_decode_request(&mut Cursor::new(oversized)).is_err());

        let mut invalid_format = b"VWI1\x04\0\0\0".to_vec();
        invalid_format.extend_from_slice(&0_u64.to_le_bytes());
        invalid_format.extend_from_slice(b"AVIF");
        assert!(read_decode_request(&mut Cursor::new(invalid_format)).is_err());
    }

    #[test]
    fn request_reader_rejects_truncated_format_and_payload() {
        for (format, payload) in [(b"avi".as_slice(), b"".as_slice()), (b"avif", b"xy")] {
            let mut frame = b"VWI1\x04\0\0\0".to_vec();
            frame.extend_from_slice(&3_u64.to_le_bytes());
            frame.extend_from_slice(format);
            frame.extend_from_slice(payload);
            assert!(read_decode_request(&mut Cursor::new(frame)).is_err());
        }
    }

    #[test]
    fn protocol_io_failures_propagate_without_partial_success() {
        assert!(read_decode_request(&mut FailingIo { writable_bytes: 0 }).is_err());
        for writable_bytes in [0, 4, 8, 16, 20] {
            assert!(
                write_decode_request(&mut FailingIo { writable_bytes }, "avif", b"image").is_err()
            );
        }

        let response = WorkerResponse::Error("failed".into());
        for writable_bytes in [0, 4, 5, 9] {
            assert!(write_worker_response(&mut FailingIo { writable_bytes }, &response).is_err());
        }
        assert!(write_ack(&mut FailingIo { writable_bytes: 0 }).is_err());
        assert!(read_ack(&mut Cursor::new(b"ACK".to_vec())).is_err());
    }

    #[test]
    fn acknowledgement_is_an_exact_versioned_frame() {
        let mut frame = Vec::new();
        write_ack(&mut frame).unwrap();
        read_ack(&mut Cursor::new(frame)).unwrap();
        assert!(read_ack(&mut Cursor::new(b"ACK0".to_vec())).is_err());
    }

    #[test]
    fn typed_responses_round_trip_without_line_delimiters() {
        for expected in [
            WorkerResponse::PixelStream {
                width: 2,
                height: 3,
            },
            WorkerResponse::Error("decoder failed without desynchronizing".into()),
            WorkerResponse::Probe,
        ] {
            let mut frame = Vec::new();
            write_worker_response(&mut frame, &expected).unwrap();
            assert_eq!(
                read_worker_response(&mut Cursor::new(frame)).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn response_reader_rejects_bad_truncated_and_oversized_frames() {
        assert!(read_worker_response(&mut Cursor::new(b"BAD!\x02\x01\0\0\0x".to_vec())).is_err());
        assert!(read_worker_response(&mut Cursor::new(b"VRS1\x02\x04\0\0\0x".to_vec())).is_err());
        assert!(read_worker_response(&mut Cursor::new(b"VRS1\x02\x01\x10\0\0".to_vec())).is_err());
    }

    #[test]
    fn response_writer_rejects_invalid_or_oversized_payloads() {
        for response in [
            WorkerResponse::PixelStream {
                width: 0,
                height: 1,
            },
            WorkerResponse::Error(String::new()),
            WorkerResponse::Error("forged\nsecond line".into()),
            WorkerResponse::Error("x".repeat(super::MAX_RESPONSE_PAYLOAD_BYTES + 1)),
        ] {
            assert!(write_worker_response(&mut Vec::new(), &response).is_err());
        }
    }

    #[test]
    fn response_reader_rejects_invalid_typed_payloads() {
        let mut invalid_shape = Vec::new();
        invalid_shape.extend_from_slice(&0_u32.to_le_bytes());
        invalid_shape.extend_from_slice(&1_u32.to_le_bytes());
        invalid_shape.extend_from_slice(b"mapping");

        for frame in [
            response_frame(super::RESPONSE_ERROR, &[]),
            response_frame(super::RESPONSE_PIXEL_STREAM, &[0; 7]),
            response_frame(super::RESPONSE_PIXEL_STREAM, &invalid_shape),
            response_frame(super::RESPONSE_ERROR, &[0xff]),
            response_frame(super::RESPONSE_ERROR, b"forged\nsecond line"),
            response_frame(super::RESPONSE_PROBE, b"wrong"),
            response_frame(99, b"unknown"),
        ] {
            assert!(read_worker_response(&mut Cursor::new(frame)).is_err());
        }
    }
}
