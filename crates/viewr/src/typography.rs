//! Bounded, local platform typography with bundled glyph fallbacks.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily};
use skrifa::{FontRef, MetadataProvider};

const MAX_FONT_BYTES: u64 = 8 * 1024 * 1024;

/// Read at most one sans and one monospace font, once during renderer startup.
pub(crate) fn font_definitions() -> FontDefinitions {
    let (sans, mono) = platform_candidates();
    with_platform_fonts(first_available(&sans), first_available(&mono))
}

fn first_available(paths: &[PathBuf]) -> Option<FontData> {
    paths.iter().find_map(|path| load_font(path))
}

fn load_font(path: &Path) -> Option<FontData> {
    // Linux restricts O_NOATIME to the file owner or CAP_FOWNER. Installed
    // system fonts are normally root-owned, so use the same read fallback as
    // ImageSource when that optimization is unavailable to a desktop user.
    let file = crate::fs::open_file_no_atime(path)
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                std::fs::File::open(path)
            } else {
                Err(error)
            }
        })
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || !(1..=MAX_FONT_BYTES).contains(&metadata.len()) {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_FONT_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_FONT_BYTES {
        return None;
    }
    let font = FontRef::from_index(&bytes, 0).ok()?;
    let metrics = font.metrics(
        skrifa::instance::Size::unscaled(),
        skrifa::instance::LocationRef::default(),
    );
    let glyph = font.charmap().map('M')?;
    if metrics.units_per_em == 0 || font.outline_glyphs().get(glyph).is_none() {
        return None;
    }
    let mut data = FontData::from_owned(bytes);
    // These coordinates are ignored by static faces. Variable UI faces use
    // regular weight and the optical size of the compact menu/body text.
    data.tweak.coords =
        egui::epaint::text::VariationCoords::new([(b"wght", 400.0), (b"opsz", 14.0)]);
    Some(data)
}

fn with_platform_fonts(sans: Option<FontData>, mono: Option<FontData>) -> FontDefinitions {
    let mut definitions = FontDefinitions::default();
    for (name, family, data) in [
        ("viewr_system_sans", FontFamily::Proportional, sans),
        ("viewr_system_mono", FontFamily::Monospace, mono),
    ] {
        if let Some(data) = data {
            definitions
                .font_data
                .insert(name.to_owned(), Arc::new(data));
            definitions
                .families
                .entry(family)
                .or_default()
                .insert(0, name.to_owned());
        }
    }
    definitions
}

fn platform_candidates() -> (Vec<PathBuf>, Vec<PathBuf>) {
    #[cfg(target_os = "windows")]
    {
        let root = std::env::var_os("WINDIR")
            .filter(|value| Path::new(value).is_absolute())
            .map_or_else(|| PathBuf::from("C:/Windows"), PathBuf::from)
            .join("Fonts");
        (
            ["SegUIVar.ttf", "segoeui.ttf"]
                .map(|name| root.join(name))
                .to_vec(),
            vec![root.join("consola.ttf")],
        )
    }
    #[cfg(target_os = "macos")]
    {
        (
            [
                "/System/Library/Fonts/SFNS.ttf",
                "/System/Library/Fonts/Helvetica.ttc",
            ]
            .map(PathBuf::from)
            .to_vec(),
            vec![PathBuf::from("/System/Library/Fonts/Menlo.ttc")],
        )
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        (
            [
                "/usr/share/fonts/adwaita-sans-fonts/AdwaitaSans-Regular.ttf",
                "/usr/share/fonts/truetype/adwaita/AdwaitaSans-Regular.ttf",
                "/usr/share/fonts/truetype/inter/InterVariable.ttf",
                "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
                "/usr/share/fonts/google-noto-vf/NotoSans[wght].ttf",
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            ]
            .map(PathBuf::from)
            .to_vec(),
            [
                "/usr/share/fonts/adwaita-mono-fonts/AdwaitaMono-Regular.ttf",
                "/usr/share/fonts/truetype/adwaita/AdwaitaMono-Regular.ttf",
                "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            ]
            .map(PathBuf::from)
            .to_vec(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundled_sans() -> FontData {
        let defaults = FontDefinitions::default();
        defaults.font_data[&defaults.families[&FontFamily::Proportional][0]]
            .as_ref()
            .clone()
    }

    #[test]
    fn missing_invalid_directory_and_oversized_fonts_use_the_next_candidate() {
        let directory = tempfile::tempdir().unwrap();
        let invalid = directory.path().join("invalid.ttf");
        std::fs::write(&invalid, b"not a font").unwrap();
        let empty = directory.path().join("empty.ttf");
        std::fs::write(&empty, []).unwrap();
        let oversized = directory.path().join("oversized.ttf");
        std::fs::File::create(&oversized)
            .unwrap()
            .set_len(MAX_FONT_BYTES + 1)
            .unwrap();
        let candidates = vec![
            directory.path().join("missing.ttf"),
            invalid,
            empty,
            oversized,
            directory.path().to_owned(),
        ];
        assert!(first_available(&candidates).is_none());
        let valid = directory.path().join("valid.ttf");
        std::fs::write(&valid, bundled_sans().font.as_ref()).unwrap();
        let mut candidates = candidates;
        candidates.push(valid);
        let loaded = first_available(&candidates).expect("valid font after rejected candidates");
        assert_eq!(loaded.font.as_ref(), bundled_sans().font.as_ref());
    }

    #[test]
    fn platform_fonts_precede_and_preserve_bundled_glyph_fallbacks() {
        let defaults = FontDefinitions::default();
        let definitions = with_platform_fonts(Some(bundled_sans()), None);
        let sans = &definitions.families[&FontFamily::Proportional];
        assert_eq!(sans[0], "viewr_system_sans");
        assert_eq!(&sans[1..], defaults.families[&FontFamily::Proportional]);
        assert_eq!(
            definitions.families[&FontFamily::Monospace],
            defaults.families[&FontFamily::Monospace]
        );
        let definitions = with_platform_fonts(None, Some(bundled_sans()));
        let mono = &definitions.families[&FontFamily::Monospace];
        assert_eq!(mono[0], "viewr_system_mono");
        assert_eq!(&mono[1..], defaults.families[&FontFamily::Monospace]);
    }

    #[test]
    fn readable_installed_fonts_do_not_require_ownership_of_system_files() {
        let (sans, mono) = platform_candidates();
        for path in sans.iter().chain(&mono) {
            if std::fs::File::open(path).is_ok() {
                assert!(
                    load_font(path).is_some(),
                    "readable installed font rejected"
                );
            }
        }
    }

    #[test]
    fn selected_font_draws_localized_menus_with_symbol_fallbacks() {
        let context = egui::Context::default();
        context.set_fonts(font_definitions());
        let _ = context.run_ui(egui::RawInput::default(), |ui| {
            ui.label("Fichier, Édition, Größe, Español");
        });
        context.fonts_mut(|fonts| {
            for text in ["Fichier, Édition, Größe, Español", "← →"] {
                assert!(fonts.has_glyphs(&egui::FontId::proportional(14.0), text));
            }
        });
    }
}
