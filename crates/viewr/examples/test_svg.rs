//! Smoke-check that SVG decoding is linked and callable via [`viewr::decode::DecodedImage`].
//!
//! Fully in-memory — does not touch the system temp directory.

fn main() {
    let svg = br#"<svg width="10" height="10" xmlns="http://www.w3.org/2000/svg"><rect width="10" height="10" fill="blue"/></svg>"#;
    let image = viewr::decode::DecodedImage::load_from_memory(svg).expect("decode svg");
    assert_eq!(image.width, 10);
    assert_eq!(image.height, 10);
    println!("svg ok: {}x{}", image.width, image.height);
}
