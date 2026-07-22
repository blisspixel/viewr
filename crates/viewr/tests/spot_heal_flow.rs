//! Integration coverage for the complete model-free spot-heal edit flow.

use viewr::decode::DecodedImage;
use viewr::edit;
use viewr::ephemeral::TempWorkspace;
use viewr::heal::{PatchHistory, SpotHealJob, StrokePoint, apply_patch};

fn patterned_image(width: u32, height: u32) -> DecodedImage {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            rgba.extend_from_slice(&[
                (x * 3 % 251) as u8,
                (y * 5 % 241) as u8,
                ((x + y) * 2 % 239) as u8,
                255,
            ]);
        }
    }
    DecodedImage {
        rgba,
        width,
        height,
    }
}

fn pixel(image: &DecodedImage, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * image.width + x) * 4) as usize;
    image.rgba[index..index + 4]
        .try_into()
        .expect("one RGBA pixel")
}

#[test]
fn spot_heal_undo_redo_and_export_round_trip() {
    let mut image = patterned_image(128, 128);
    for y in 58..70 {
        for x in 58..70 {
            let index = ((y * image.width + x) * 4) as usize;
            image.rgba[index..index + 4].copy_from_slice(&[255, 0, 0, 255]);
        }
    }
    let damaged = image.rgba.clone();

    let repair = SpotHealJob::prepare(&image, &[StrokePoint { x: 64.0, y: 64.0 }], 9)
        .expect("valid repair")
        .expect("stroke intersects the image")
        .run()
        .expect("repair has a valid source region");
    let inverse = apply_patch(&mut image, &repair).expect("repair applies to its source image");
    assert_ne!(pixel(&image, 64, 64), [255, 0, 0, 255]);

    let repaired = image.rgba.clone();
    let mut history = PatchHistory::new(1024 * 1024);
    history.record(inverse);
    let undone = history
        .undo_patch(&mut image)
        .expect("undo patch remains valid")
        .expect("undo history contains the repair");
    assert_eq!(undone.bounds, repair.bounds);
    assert_eq!(image.rgba, damaged);
    let redone = history
        .redo_patch(&mut image)
        .expect("redo patch remains valid")
        .expect("redo history contains the repair");
    assert_eq!(redone.bounds, repair.bounds);
    assert_eq!(image.rgba, repaired);

    let workspace = TempWorkspace::new("spot_heal_flow").expect("temporary workspace");
    let output = workspace.path().join("healed.png");
    edit::save(&image, &output).expect("pixel-only export succeeds");
    let reopened = DecodedImage::load(&output).expect("exported image reopens");
    assert_eq!((reopened.width, reopened.height), (128, 128));
    assert_eq!(reopened.rgba, repaired);
}
