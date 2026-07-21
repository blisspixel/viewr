//! Smoke-check that SVG decoding is linked and callable via [`viewr::decode::DecodedImage`].

use std::io::Write;

fn main() {
    let dir = std::env::temp_dir().join(format!("viewr_example_svg_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("smoke.svg");
    let mut file = std::fs::File::create(&path).expect("temp svg");
    writeln!(
        file,
        r#"<svg width="10" height="10" xmlns="http://www.w3.org/2000/svg"><rect width="10" height="10" fill="blue"/></svg>"#
    )
    .expect("write svg");

    let image = viewr::decode::DecodedImage::load(&path).expect("decode svg");
    assert_eq!(image.width, 10);
    assert_eq!(image.height, 10);
    println!("svg ok: {}x{}", image.width, image.height);
    let _ = std::fs::remove_dir_all(&dir);
}
