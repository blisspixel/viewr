//! Convert working sRGB pixels into an unmanaged display's ICC encoding.
//!
//! Managed compositors keep tagged sRGB and convert themselves. This module
//! exists for the remaining Windows-legacy and real X11 paths, where the
//! framebuffer bytes are shown without a compositor transform. Export, edit,
//! and thumbnail buffers stay in working sRGB.

use std::borrow::Cow;
use std::sync::Arc;

use crate::error::Error;

/// Optional sRGB-to-display transform applied at GPU upload.
#[derive(Clone)]
pub(crate) struct DisplayOutputNormalizer {
    transform: Option<Arc<moxcms::Transform8BitExecutor>>,
}

impl DisplayOutputNormalizer {
    /// Leave working sRGB pixels unchanged.
    #[must_use]
    pub(crate) const fn identity() -> Self {
        Self { transform: None }
    }

    /// Build a display transform from admitted RGB ICC bytes.
    ///
    /// Bound, parse, and require an 8-bit RGBA transform from working sRGB
    /// into the display encoding. Anything else is `None` so the caller keeps
    /// the deterministic sRGB fallback instead of presenting a guessed result.
    #[must_use]
    pub(crate) fn from_profile_bytes(bytes: &[u8]) -> Option<Self> {
        if !crate::display_state::admit_display_profile(bytes) {
            return None;
        }
        let destination = moxcms::ColorProfile::new_from_slice(bytes).ok()?;
        let source = moxcms::ColorProfile::new_srgb();
        let transform = source
            .create_transform_8bit(
                moxcms::Layout::Rgba,
                &destination,
                moxcms::Layout::Rgba,
                moxcms::TransformOptions::default(),
            )
            .ok()?;
        Some(Self {
            transform: Some(transform),
        })
    }

    /// Whether presented pixels are converted for a display ICC.
    #[must_use]
    pub(crate) const fn is_applied(&self) -> bool {
        self.transform.is_some()
    }

    /// Convert tightly packed RGBA8 working sRGB into display-referred bytes.
    ///
    /// Identity returns the input unchanged. A live transform copies, so the
    /// working buffer used for export and edits is never mutated.
    pub(crate) fn apply<'a>(&self, rgba: &'a [u8]) -> Result<Cow<'a, [u8]>, Error> {
        let Some(transform) = self.transform.as_ref() else {
            return Ok(Cow::Borrowed(rgba));
        };
        if !rgba.len().is_multiple_of(4) {
            return Err(Error::Gpu(
                "display color conversion source is not packed RGBA".into(),
            ));
        }
        let mut converted = Vec::new();
        converted.try_reserve_exact(rgba.len()).map_err(|error| {
            Error::Gpu(format!(
                "could not allocate display color conversion: {error}"
            ))
        })?;
        converted.resize(rgba.len(), 0);
        transform
            .transform(rgba, &mut converted)
            .map_err(|_| Error::Gpu("display color conversion failed".into()))?;
        Ok(Cow::Owned(converted))
    }
}

#[cfg(test)]
mod tests {
    use super::DisplayOutputNormalizer;
    use crate::display_state::admit_display_profile;

    const TOLERANCE: i32 = 1;

    fn encode(profile: &moxcms::ColorProfile) -> Vec<u8> {
        profile.encode().expect("encode display ICC fixture")
    }

    fn assert_channels(what: &str, actual: &[u8], expected: [u8; 4]) {
        for (channel, (value, target)) in actual.iter().zip(expected.iter()).enumerate() {
            let drift = i32::from(*value) - i32::from(*target);
            assert!(
                drift.abs() <= TOLERANCE,
                "{what}: channel {channel} was {value}, expected {target}"
            );
        }
        assert_eq!(actual[3], expected[3], "{what}: alpha");
    }

    #[test]
    fn identity_leaves_working_pixels_borrowed_and_unchanged() {
        let pixels = [10_u8, 20, 30, 40, 200, 150, 100, 255];
        let output = DisplayOutputNormalizer::identity().apply(&pixels).unwrap();
        assert!(!DisplayOutputNormalizer::identity().is_applied());
        assert!(matches!(output, std::borrow::Cow::Borrowed(_)));
        assert_eq!(&*output, &pixels);
    }

    #[test]
    fn working_srgb_converts_to_published_display_reference_values() {
        // Destination values are derived from the published matrices and
        // transfer functions, not recorded from this library. sRGB, Display
        // P3, and Adobe RGB share a D65 white point, so neutrals stay
        // neutral. Display P3 shares the sRGB transfer function. Adobe RGB
        // uses gamma 563/256, so mid gray drops one code value. Saturated
        // sRGB orange moves by tens of code values against a one-value
        // tolerance, which rejects a wrong matrix or transfer function.
        struct Case {
            profile: fn() -> moxcms::ColorProfile,
            what: &'static str,
            input: [u8; 4],
            expected: [u8; 4],
        }
        let cases = [
            Case {
                profile: moxcms::ColorProfile::new_display_p3,
                what: "Display P3 black",
                input: [0, 0, 0, 255],
                expected: [0, 0, 0, 255],
            },
            Case {
                profile: moxcms::ColorProfile::new_display_p3,
                what: "Display P3 white",
                input: [255, 255, 255, 255],
                expected: [255, 255, 255, 255],
            },
            Case {
                profile: moxcms::ColorProfile::new_display_p3,
                what: "Display P3 mid gray",
                input: [128, 128, 128, 200],
                expected: [128, 128, 128, 200],
            },
            Case {
                profile: moxcms::ColorProfile::new_display_p3,
                what: "Display P3 saturated sRGB orange",
                input: [210, 120, 35, 17],
                expected: [198, 124, 56, 17],
            },
            Case {
                profile: moxcms::ColorProfile::new_adobe_rgb,
                what: "Adobe RGB black",
                input: [0, 0, 0, 255],
                expected: [0, 0, 0, 255],
            },
            Case {
                profile: moxcms::ColorProfile::new_adobe_rgb,
                what: "Adobe RGB white",
                input: [255, 255, 255, 255],
                expected: [255, 255, 255, 255],
            },
            Case {
                profile: moxcms::ColorProfile::new_adobe_rgb,
                what: "Adobe RGB mid gray",
                input: [128, 128, 128, 255],
                expected: [127, 127, 127, 255],
            },
            Case {
                profile: moxcms::ColorProfile::new_adobe_rgb,
                what: "Adobe RGB saturated sRGB orange",
                input: [210, 120, 35, 255],
                expected: [188, 119, 47, 255],
            },
            Case {
                profile: moxcms::ColorProfile::new_srgb,
                what: "sRGB display is its own destination",
                input: [210, 120, 35, 90],
                expected: [210, 120, 35, 90],
            },
        ];

        for case in cases {
            let bytes = encode(&(case.profile)());
            let output = DisplayOutputNormalizer::from_profile_bytes(&bytes)
                .expect(case.what)
                .apply(&case.input)
                .unwrap();
            assert_channels(case.what, &output, case.expected);
        }
    }

    #[test]
    fn unusable_profiles_do_not_build_a_display_transform() {
        assert!(DisplayOutputNormalizer::from_profile_bytes(&[]).is_none());
        assert!(DisplayOutputNormalizer::from_profile_bytes(b"not an ICC profile").is_none());
        assert!(
            DisplayOutputNormalizer::from_profile_bytes(&vec![
                0;
                viewr_protocol::MAX_COLOR_PROFILE_BYTES
                    + 1
            ])
            .is_none()
        );

        let mut cmyk = moxcms::ColorProfile::new_srgb();
        cmyk.color_space = moxcms::DataColorSpace::Cmyk;
        let encoded = encode(&cmyk);
        assert!(!admit_display_profile(&encoded));
        assert!(DisplayOutputNormalizer::from_profile_bytes(&encoded).is_none());

        let gray = encode(&moxcms::ColorProfile::new_gray_with_gamma(2.2));
        assert!(DisplayOutputNormalizer::from_profile_bytes(&gray).is_none());
    }

    #[test]
    fn applied_transform_rejects_a_truncated_rgba_buffer() {
        let bytes = encode(&moxcms::ColorProfile::new_display_p3());
        let output = DisplayOutputNormalizer::from_profile_bytes(&bytes).unwrap();
        let error = output.apply(&[1, 2, 3]).unwrap_err();
        assert!(
            error.to_string().contains("packed RGBA"),
            "unexpected error: {error}"
        );
    }
}
