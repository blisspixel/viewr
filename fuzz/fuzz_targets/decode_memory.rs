#![no_main]

use arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;
use viewr::decode::DecodedImage;
use viewr::fs::CORE_EXTENSIONS;
use viewr_protocol::checked_rgba_len;

const MAX_FUZZ_INPUT_BYTES: usize = 1024 * 1024;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_FUZZ_INPUT_BYTES {
        return;
    }

    let mut unstructured = Unstructured::new(data);
    let selector = u8::arbitrary(&mut unstructured).unwrap_or_default();
    let payload = unstructured.take_rest();
    let extension = CORE_EXTENSIONS[usize::from(selector) % CORE_EXTENSIONS.len()];

    validate_decode(DecodedImage::load_from_memory_with_extension(
        payload, extension,
    ));
    validate_decode(DecodedImage::load_from_memory(payload));
});

fn validate_decode(result: Result<DecodedImage, viewr::Error>) {
    if let Ok(image) = result {
        let expected = checked_rgba_len(image.width, image.height)
            .expect("a successful decode must satisfy the shared shape policy");
        assert_eq!(image.rgba.len(), expected);
    }
}
