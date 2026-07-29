#![allow(clippy::expect_used)]

//! Build script to embed Windows resources like application icons.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS must be set") == "windows" {
        let icon = "../../assets/icon.ico";
        let manifest = "../../packaging/windows/viewr.exe.manifest";
        println!("cargo:rerun-if-changed={icon}");
        println!("cargo:rerun-if-changed={manifest}");
        let mut res = winres::WindowsResource::new();
        res.set_icon(icon);
        res.set_manifest_file(manifest);
        res.compile().expect("failed to compile resources");
    }
}
