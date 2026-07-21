//! Filesystem helpers: recognizing image files and ordering them the way people
//! expect, so `img2` comes before `img10`. These are pure functions, fully
//! unit-tested, and never block the UI thread when used by the scanner.

use std::cmp::Ordering;
use std::iter::Peekable;
use std::path::Path;
use std::str::Chars;

/// File extensions viewr recognizes as images in the always-on core build.
/// Formats that need heavy or C-backed decoders are added behind Cargo features
/// later (see `docs/STANDARDS.md` dependency policy), not here.
const CORE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff", "ico", "qoi", "tga", "ppm", "pgm",
    "pbm", "pnm", "hdr", "exr", "ff", "dds", "jxl", "svg",
];

/// Return `true` if `path` has an extension viewr can open in the core build.
#[must_use]
pub fn is_supported_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|e| CORE_EXTENSIONS.contains(&e.as_str()))
}

/// Compare two file names the way a human reads them: runs of digits are
/// compared by numeric value, so `img2` orders before `img10`. Comparison is
/// case-insensitive elsewhere.
#[must_use]
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    match take_number(&mut ai).cmp(&take_number(&mut bi)) {
                        Ordering::Equal => {}
                        non_eq => return non_eq,
                    }
                } else {
                    match ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase()) {
                        Ordering::Equal => {
                            ai.next();
                            bi.next();
                        }
                        non_eq => return non_eq,
                    }
                }
            }
        }
    }
}

/// Consume a run of ASCII digits from `it` and return its numeric value,
/// saturating rather than overflowing on absurdly long runs.
fn take_number(it: &mut Peekable<Chars>) -> u64 {
    let mut n: u64 = 0;
    while let Some(d) = it.peek().and_then(|c| c.to_digit(10)) {
        n = n.saturating_mul(10).saturating_add(u64::from(d));
        it.next();
    }
    n
}

#[cfg(test)]
mod tests {
    use super::{is_supported_image, natural_cmp};
    use std::cmp::Ordering;
    use std::path::Path;

    #[test]
    fn recognizes_common_images() {
        assert!(is_supported_image(Path::new("a.jpg")));
        assert!(is_supported_image(Path::new("A.PNG")));
        assert!(is_supported_image(Path::new("x.jxl")));
        assert!(is_supported_image(Path::new("/some/dir/photo.webp")));
    }

    #[test]
    fn rejects_non_images() {
        assert!(!is_supported_image(Path::new("notes.txt")));
        assert!(!is_supported_image(Path::new("noext")));
        assert!(!is_supported_image(Path::new("archive.zip")));
    }

    #[test]
    fn natural_order_numbers() {
        assert_eq!(natural_cmp("img2.jpg", "img10.jpg"), Ordering::Less);
        assert_eq!(natural_cmp("img10.jpg", "img2.jpg"), Ordering::Greater);
        assert_eq!(natural_cmp("a.jpg", "a.jpg"), Ordering::Equal);
    }

    #[test]
    fn natural_order_case_insensitive() {
        assert_eq!(natural_cmp("Photo.jpg", "photo.jpg"), Ordering::Equal);
    }

    #[test]
    fn natural_sort_orders_a_list() {
        let mut v = vec!["img10", "img2", "img1", "img100"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["img1", "img2", "img10", "img100"]);
    }
}
