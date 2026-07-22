#![no_main]

use std::io::Cursor;
use std::path::Path;

use arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;
use viewr_protocol::{
    MAX_PATH_BYTES, MAX_RESPONSE_PAYLOAD_BYTES, WorkerResponse, checked_rgba_len, read_ack,
    read_path_request, read_worker_response, write_ack, write_path_request, write_worker_response,
};

const MAX_FUZZ_INPUT_BYTES: usize = MAX_PATH_BYTES + 16;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_FUZZ_INPUT_BYTES {
        return;
    }

    let mut unstructured = Unstructured::new(data);
    let selector = u8::arbitrary(&mut unstructured).unwrap_or_default();
    let payload = unstructured.take_rest();

    match selector % 8 {
        0 => {
            let _ = read_path_request(&mut Cursor::new(payload));
        }
        1 => round_trip_path(payload),
        2 => {
            let _ = read_worker_response(&mut Cursor::new(payload));
        }
        3 => round_trip_error(payload),
        4 => {
            let _ = read_ack(&mut Cursor::new(payload));
            let mut frame = Vec::new();
            write_ack(&mut frame).expect("Vec writes cannot fail");
            read_ack(&mut Cursor::new(frame)).expect("writer output must be readable");
        }
        5 => exercise_shape(payload),
        6 => round_trip_shape(payload),
        _ => {
            let mut truncated = b"VWR1".to_vec();
            truncated.extend_from_slice(payload);
            let _ = read_path_request(&mut Cursor::new(truncated));
        }
    }
});

fn round_trip_path(payload: &[u8]) {
    let Ok(path) = std::str::from_utf8(payload) else {
        return;
    };
    if path.is_empty() {
        return;
    }
    let mut frame = Vec::new();
    if write_path_request(&mut frame, Path::new(path)).is_ok() {
        let decoded = read_path_request(&mut Cursor::new(frame))
            .expect("writer output must be readable")
            .expect("writer emits one request");
        assert_eq!(decoded, Path::new(path));
    }
}

fn round_trip_error(payload: &[u8]) {
    let mut message = String::new();
    for character in String::from_utf8_lossy(payload)
        .chars()
        .filter(|character| !character.is_control())
    {
        if message.len() + character.len_utf8() > MAX_RESPONSE_PAYLOAD_BYTES {
            break;
        }
        message.push(character);
    }
    if message.is_empty() {
        return;
    }
    let expected = WorkerResponse::Error(message);
    let mut frame = Vec::new();
    write_worker_response(&mut frame, &expected).expect("bounded error must encode");
    assert_eq!(
        read_worker_response(&mut Cursor::new(frame)).expect("writer output must be readable"),
        expected
    );
}

fn exercise_shape(payload: &[u8]) {
    if let Some((width, height)) = dimensions(payload) {
        let _ = checked_rgba_len(width, height);
    }
}

fn round_trip_shape(payload: &[u8]) {
    let Some((width, height)) = dimensions(payload) else {
        return;
    };
    let expected = WorkerResponse::PixelStream { width, height };
    let mut frame = Vec::new();
    if write_worker_response(&mut frame, &expected).is_ok() {
        assert_eq!(
            read_worker_response(&mut Cursor::new(frame))
                .expect("valid shape frame must be readable"),
            expected
        );
    }
}

fn dimensions(payload: &[u8]) -> Option<(u32, u32)> {
    let bytes: [u8; 8] = payload.get(..8)?.try_into().ok()?;
    Some((
        u32::from_le_bytes(bytes[..4].try_into().ok()?),
        u32::from_le_bytes(bytes[4..].try_into().ok()?),
    ))
}
