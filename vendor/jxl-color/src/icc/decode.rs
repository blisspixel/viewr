use std::io::Cursor;
use std::io::prelude::*;

use jxl_bitstream::Bitstream;

use crate::{Error, Result};

/// Maximum encoded or decoded ICC bytes accepted from a JPEG XL codestream.
///
/// This application boundary is intentionally stricter than the JPEG XL level
/// limit. It matches the profile ceiling enforced for viewr's other decoders.
pub const MAX_EMBEDDED_ICC_BYTES: u64 = 10 * 1024 * 1024;

/// Reads the encoded ICC profile stream from the given bitstream.
pub fn read_icc(bitstream: &mut Bitstream) -> Result<Vec<u8>> {
    let enc_size = bitstream.read_u64()?;
    tracing::trace!(enc_size);

    if enc_size > MAX_EMBEDDED_ICC_BYTES {
        return Err(
            jxl_bitstream::Error::ProfileConformance("Too large encoded ICC profile").into(),
        );
    }

    let mut decoder = jxl_coding::Decoder::parse(bitstream, 41)?;

    let max_size_header_len = enc_size.min(18) as usize;
    let mut encoded_icc = vec![0u8; max_size_header_len];
    let mut b1 = 0u8;
    let mut b2 = 0u8;
    decoder.begin(bitstream)?;
    for (idx, b) in encoded_icc.iter_mut().enumerate() {
        let sym = decoder.read_varint(bitstream, get_icc_ctx(idx, b1, b2))?;
        if sym >= 256 {
            return Err(Error::InvalidIccStream("decoded value out of range"));
        }
        *b = sym as u8;

        b2 = b1;
        b1 = *b;
    }

    let mut tmp_cursor = Cursor::new(&*encoded_icc);
    let output_size = varint(&mut tmp_cursor)?;
    let commands_size = varint(&mut tmp_cursor)?;
    let stream_offset = tmp_cursor.position();
    tracing::trace!(
        enc_size,
        output_size,
        commands_size,
        stream_offset,
        "Quick ICC validity check",
    );

    if stream_offset + commands_size > enc_size {
        return Err(Error::InvalidIccStream("invalid commands_size"));
    }

    if output_size > MAX_EMBEDDED_ICC_BYTES {
        return Err(jxl_bitstream::Error::ProfileConformance("ICC output_size too large").into());
    }

    if output_size + 65536 < enc_size {
        return Err(Error::InvalidIccStream(
            "reported output_size is far smaller than enc_size",
        ));
    }

    // Read remaining data
    encoded_icc.resize(enc_size as usize, 0);
    for (idx, b) in encoded_icc.iter_mut().enumerate().skip(max_size_header_len) {
        let sym = decoder.read_varint(bitstream, get_icc_ctx(idx, b1, b2))?;
        if sym >= 256 {
            return Err(Error::InvalidIccStream("decoded value out of range"));
        }
        *b = sym as u8;

        b2 = b1;
        b1 = *b;
    }

    decoder.finalize()?;
    Ok(encoded_icc)
}

fn get_icc_ctx(idx: usize, b1: u8, b2: u8) -> u32 {
    if idx <= 128 {
        return 0;
    }

    let p1 = match b1 {
        b'a'..=b'z' | b'A'..=b'Z' => 0,
        b'0'..=b'9' | b'.' | b',' => 1,
        0..=1 => 2 + b1 as u32,
        2..=15 => 4,
        241..=254 => 5,
        255 => 6,
        _ => 7,
    };
    let p2 = match b2 {
        b'a'..=b'z' | b'A'..=b'Z' => 0,
        b'0'..=b'9' | b'.' | b',' => 1,
        0..=15 => 2,
        241..=255 => 3,
        _ => 4,
    };

    1 + p1 + 8 * p2
}

fn varint(stream: &mut Cursor<&[u8]>) -> Result<u64> {
    let mut value = 0u64;
    let mut shift = 0;
    let mut b = 0;
    while shift < 63 {
        stream
            .read_exact(std::slice::from_mut(&mut b))
            .map_err(|_| Error::InvalidIccStream("stream is too short"))?;
        value |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    Ok(value)
}

fn predict_header(idx: usize, output_size: u32, header: &[u8]) -> u8 {
    match idx {
        0..=3 => output_size.to_be_bytes()[idx],
        8 => 4,
        12..=23 => b"mntrRGB XYZ "[idx - 12],
        36..=39 => b"acsp"[idx - 36],
        // APPL
        41 | 42 if header[40] == b'A' => b'P',
        43 if header[40] == b'A' => b'L',
        // MSFT
        41 if header[40] == b'M' => b'S',
        42 if header[40] == b'M' => b'F',
        43 if header[40] == b'M' => b'T',
        // SGI_
        42 if header[40] == b'S' && header[41] == b'G' => b'I',
        43 if header[40] == b'S' && header[41] == b'G' => b' ',
        // SUNW
        42 if header[40] == b'S' && header[41] == b'U' => b'N',
        43 if header[40] == b'S' && header[41] == b'U' => b'W',
        70 => 246,
        71 => 214,
        73 => 1,
        78 => 211,
        79 => 45,
        80..=83 => header[4 + idx - 80],
        _ => 0,
    }
}

fn shuffle2(bytes: &[u8]) -> Vec<u8> {
    let len = bytes.len();
    let mut out = Vec::with_capacity(bytes.len());
    let height = len / 2;
    let odd = len % 2;
    for idx in 0..height {
        out.push(bytes[idx]);
        out.push(bytes[idx + height + odd]);
    }
    if odd != 0 {
        out.push(bytes[height]);
    }
    out
}

fn shuffle4(bytes: &[u8]) -> Vec<u8> {
    let len = bytes.len();
    let mut out = Vec::with_capacity(bytes.len());
    let step = len / 4;
    let wide_count = len % 4;
    for idx in 0..step {
        let mut base = idx;
        for _ in 0..wide_count {
            out.push(bytes[base]);
            base += step + 1;
        }
        for _ in wide_count..4 {
            out.push(bytes[base]);
            base += step;
        }
    }
    for idx in 1..=wide_count {
        out.push(bytes[(step + 1) * idx - 1]);
    }
    out
}

fn ensure_output_room(out: &[u8], additional: usize, output_limit: usize) -> Result<()> {
    if out
        .len()
        .checked_add(additional)
        .is_none_or(|length| length > output_limit)
    {
        return Err(Error::InvalidIccStream(
            "decoded ICC profile exceeds declared output_size",
        ));
    }
    Ok(())
}

fn append_output(out: &mut Vec<u8>, bytes: &[u8], output_limit: usize) -> Result<()> {
    ensure_output_room(out, bytes.len(), output_limit)?;
    out.extend_from_slice(bytes);
    Ok(())
}

fn append_tag_entry(
    out: &mut Vec<u8>,
    tag: &[u8],
    start: u32,
    size: u32,
    output_limit: usize,
) -> Result<()> {
    ensure_output_room(out, 12, output_limit)?;
    out.extend_from_slice(tag);
    out.extend_from_slice(&start.to_be_bytes());
    out.extend_from_slice(&size.to_be_bytes());
    Ok(())
}

/// Decodes the given ICC profile stream.
pub fn decode_icc(stream: &[u8]) -> Result<Vec<u8>> {
    use std::num::Wrapping;

    const COMMON_TAGS: [&[u8]; 19] = [
        b"rTRC", b"rXYZ", b"cprt", b"wtpt", b"bkpt", b"rXYZ", b"gXYZ", b"bXYZ", b"kXYZ", b"rTRC",
        b"gTRC", b"bTRC", b"kTRC", b"chad", b"desc", b"chrm", b"dmnd", b"dmdd", b"lumi",
    ];

    const COMMON_DATA: [&[u8]; 8] = [
        b"XYZ ", b"desc", b"text", b"mluc", b"para", b"curv", b"sf32", b"gbd ",
    ];

    let mut tmp_cursor = Cursor::new(stream);
    let output_size = varint(&mut tmp_cursor)?;
    let commands_size = varint(&mut tmp_cursor)?;
    let stream_offset = tmp_cursor.position();
    let commands_end = stream_offset
        .checked_add(commands_size)
        .ok_or(Error::InvalidIccStream("invalid commands_size"))?;
    if commands_end > stream.len() as u64 {
        return Err(Error::InvalidIccStream("invalid commands_size"));
    }

    if output_size > MAX_EMBEDDED_ICC_BYTES {
        return Err(jxl_bitstream::Error::ProfileConformance("ICC output_size too large").into());
    }
    let output_limit = usize::try_from(output_size)
        .map_err(|_| Error::InvalidIccStream("ICC output_size is not addressable"))?;

    let stream_offset = usize::try_from(stream_offset)
        .map_err(|_| Error::InvalidIccStream("invalid commands_size"))?;
    let commands_size = usize::try_from(commands_size)
        .map_err(|_| Error::InvalidIccStream("invalid commands_size"))?;
    let (commands, data) = stream[stream_offset..].split_at(commands_size);
    let header_size = output_size.min(128) as usize;
    if data.len() < header_size {
        return Err(Error::InvalidIccStream("invalid output_size"));
    }
    let (header_data, mut data) = data.split_at(header_size);
    let mut commands_stream = Cursor::new(commands);
    let mut out = Vec::with_capacity(output_limit);

    // Header
    for (idx, &e) in header_data.iter().enumerate() {
        let p = predict_header(idx, output_size as u32, header_data);
        out.push(p.wrapping_add(e));
    }
    if output_size <= 128 {
        return Ok(out);
    }

    // Tag
    let v = varint(&mut commands_stream)?;
    if let Some(num_tags) = v.checked_sub(1) {
        if (output_size - 128) / 12 < num_tags {
            return Err(Error::InvalidIccStream("num_tags too large"));
        }
        let num_tags =
            u32::try_from(num_tags).map_err(|_| Error::InvalidIccStream("num_tags too large"))?;
        append_output(&mut out, &num_tags.to_be_bytes(), output_limit)?;

        let mut prev_tagstart = num_tags
            .checked_mul(12)
            .and_then(|value| value.checked_add(128))
            .ok_or(Error::InvalidIccStream("ICC tag offset overflowed"))?;
        let mut prev_tagsize = 0u32;

        loop {
            let mut command = 0u8;
            if commands_stream
                .read_exact(std::slice::from_mut(&mut command))
                .is_err()
            {
                return if out.len() == output_limit {
                    Ok(out)
                } else {
                    Err(Error::InvalidIccStream("decoded ICC profile size mismatch"))
                };
            }
            let tagcode = command & 63;
            let tag = match tagcode {
                0 => break,
                1 => {
                    if data.len() < 4 {
                        return Err(Error::InvalidIccStream("unexpected end of data stream"));
                    }
                    let (tag, next_data) = data.split_at(4);
                    data = next_data;
                    tag
                }
                2..=20 => COMMON_TAGS[(tagcode - 2) as usize],
                _ => return Err(Error::InvalidIccStream("invalid tagcode")),
            };

            let tagstart = if command & 64 == 0 {
                prev_tagstart
                    .checked_add(prev_tagsize)
                    .ok_or(Error::InvalidIccStream("ICC tag offset overflowed"))?
            } else {
                u32::try_from(varint(&mut commands_stream)?)
                    .map_err(|_| Error::InvalidIccStream("ICC tag offset overflowed"))?
            };
            let tagsize = match tag {
                _ if command & 128 != 0 => u32::try_from(varint(&mut commands_stream)?)
                    .map_err(|_| Error::InvalidIccStream("ICC tag size overflowed"))?,
                b"rXYZ" | b"gXYZ" | b"bXYZ" | b"kXYZ" | b"wtpt" | b"bkpt" | b"lumi" => 20,
                _ => prev_tagsize,
            };
            if (tagstart as u64 + tagsize as u64) > output_size {
                return Err(Error::InvalidIccStream("ICC profile size mismatch"));
            }

            prev_tagstart = tagstart;
            prev_tagsize = tagsize;

            append_tag_entry(&mut out, tag, tagstart, tagsize, output_limit)?;
            if tagcode == 2 {
                append_tag_entry(&mut out, b"gTRC", tagstart, tagsize, output_limit)?;
                append_tag_entry(&mut out, b"bTRC", tagstart, tagsize, output_limit)?;
            } else if tagcode == 3 {
                let green_start = tagstart
                    .checked_add(tagsize)
                    .ok_or(Error::InvalidIccStream("ICC tag offset overflowed"))?;
                let blue_start = green_start
                    .checked_add(tagsize)
                    .ok_or(Error::InvalidIccStream("ICC tag offset overflowed"))?;
                if u64::from(blue_start) + u64::from(tagsize) > output_size {
                    return Err(Error::InvalidIccStream("ICC profile size mismatch"));
                }
                append_tag_entry(&mut out, b"gXYZ", green_start, tagsize, output_limit)?;
                append_tag_entry(&mut out, b"bXYZ", blue_start, tagsize, output_limit)?;
            }
        }
    }

    // Main
    let mut command = 0u8;
    while commands_stream
        .read_exact(std::slice::from_mut(&mut command))
        .is_ok()
    {
        match command {
            1 => {
                let num = usize::try_from(varint(&mut commands_stream)?)
                    .map_err(|_| Error::InvalidIccStream("ICC command size overflowed"))?;
                if num > data.len() {
                    return Err(Error::InvalidIccStream("stream is too short"));
                }
                ensure_output_room(&out, num, output_limit)?;
                let (bytes, next_data) = data.split_at(num);
                data = next_data;
                out.extend_from_slice(bytes);
            }
            2 | 3 => {
                let num = usize::try_from(varint(&mut commands_stream)?)
                    .map_err(|_| Error::InvalidIccStream("ICC command size overflowed"))?;
                if num > data.len() {
                    return Err(Error::InvalidIccStream("stream is too short"));
                }
                ensure_output_room(&out, num, output_limit)?;
                let (bytes, next_data) = data.split_at(num);
                data = next_data;
                let bytes = if command == 2 {
                    shuffle2(bytes)
                } else {
                    shuffle4(bytes)
                };
                out.extend_from_slice(&bytes);
            }
            4 => {
                let mut flags = 0u8;
                commands_stream
                    .read_exact(std::slice::from_mut(&mut flags))
                    .map_err(|_| Error::InvalidIccStream("stream is too short"))?;
                let width = ((flags & 3) + 1) as usize;
                let order = (flags >> 2) & 3;
                if width == 3 || order == 3 {
                    return Err(Error::InvalidIccStream("width == 3 || order == 3"));
                }

                let stride = if (flags & 16) == 0 {
                    width
                } else {
                    let stride = usize::try_from(varint(&mut commands_stream)?)
                        .map_err(|_| Error::InvalidIccStream("ICC command stride overflowed"))?;
                    if stride < width {
                        return Err(Error::InvalidIccStream("stride < width"));
                    }
                    stride
                };
                if stride.saturating_mul(4) >= out.len() {
                    return Err(Error::InvalidIccStream("stride * 4 >= out.len()"));
                }

                let num = usize::try_from(varint(&mut commands_stream)?)
                    .map_err(|_| Error::InvalidIccStream("ICC command size overflowed"))?;
                if data.len() < num {
                    return Err(Error::InvalidIccStream("stream is too short"));
                }
                ensure_output_room(&out, num, output_limit)?;
                let (bytes, next_data) = data.split_at(num);
                data = next_data;
                let shuffled;
                let bytes = match width {
                    1 => bytes,
                    2 => {
                        shuffled = shuffle2(bytes);
                        &shuffled
                    }
                    4 => {
                        shuffled = shuffle4(bytes);
                        &shuffled
                    }
                    _ => unreachable!(),
                };

                for i in (0..num).step_by(width) {
                    let mut prev = [Wrapping(0u32); 3];
                    for (j, p) in prev[..=order as usize].iter_mut().enumerate() {
                        let offset = out.len() - stride * (j + 1);
                        let mut bytes = [0u8; 4];
                        bytes[(4 - width)..].copy_from_slice(&out[offset..][..width]);
                        p.0 = u32::from_be_bytes(bytes);
                    }
                    let p = match order {
                        0 => prev[0],
                        1 => Wrapping(2) * prev[0] - prev[1],
                        2 => Wrapping(3) * (prev[0] - prev[1]) + prev[2],
                        _ => unreachable!(),
                    };

                    for j in 0..width.min(num - i) {
                        let val = Wrapping(bytes[i + j] as u32) + (p >> (8 * (width - 1 - j)));
                        out.push(val.0 as u8);
                    }
                }
            }
            10 => {
                if data.len() < 12 {
                    return Err(Error::InvalidIccStream("stream is too short"));
                }
                ensure_output_room(&out, 20, output_limit)?;
                out.extend_from_slice(&[b'X', b'Y', b'Z', b' ', 0, 0, 0, 0]);
                let (bytes, next_data) = data.split_at(12);
                data = next_data;
                out.extend_from_slice(bytes);
            }
            16..=23 => {
                ensure_output_room(&out, 8, output_limit)?;
                out.extend_from_slice(COMMON_DATA[command as usize - 16]);
                out.extend_from_slice(&[0, 0, 0, 0]);
            }
            _ => {
                return Err(Error::InvalidIccStream("invalid command"));
            }
        }
    }
    if out.len() != output_limit {
        return Err(Error::InvalidIccStream("decoded ICC profile size mismatch"));
    }
    Ok(out)
}
