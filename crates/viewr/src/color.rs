//! Explicit contracts between decoded source pixels, working pixels, and display output.
//!
//! The current shipping path is deliberately narrow: RGBA8 pixels encoded in
//! sRGB are sampled through an sRGB GPU texture and presented to an sRGB surface.
//! Keeping that contract typed prevents a future wide-gamut or higher-precision
//! decoder from being accepted by the RGBA8 path and clipped implicitly.

/// Color space used by pixels after source-profile normalization.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkingColorSpace {
    /// IEC 61966-2-1 sRGB primaries and transfer function.
    Srgb,
    /// Display P3 primaries with the sRGB transfer function.
    DisplayP3,
}

/// In-memory representation of normalized working pixels.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkingPixelFormat {
    /// Four interleaved unsigned normalized 8-bit channels in RGBA order.
    Rgba8Unorm,
}

/// Complete interpretation required to consume a working pixel buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkingColorEncoding {
    color_space: WorkingColorSpace,
    pixel_format: WorkingPixelFormat,
}

impl WorkingColorEncoding {
    /// The current SDR working contract.
    pub const SRGB_RGBA8: Self = Self {
        color_space: WorkingColorSpace::Srgb,
        pixel_format: WorkingPixelFormat::Rgba8Unorm,
    };

    /// A representable wide-gamut contract used to prove unsupported paths fail
    /// closed. No shipping decoder or renderer produces or accepts it yet.
    #[cfg(test)]
    pub(crate) const DISPLAY_P3_RGBA8: Self = Self {
        color_space: WorkingColorSpace::DisplayP3,
        pixel_format: WorkingPixelFormat::Rgba8Unorm,
    };

    /// Return the color space represented by the component values.
    #[must_use]
    pub const fn color_space(self) -> WorkingColorSpace {
        self.color_space
    }

    /// Return the in-memory component layout and precision.
    #[must_use]
    pub const fn pixel_format(self) -> WorkingPixelFormat {
        self.pixel_format
    }
}

/// Color space expected by the presentation surface.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputColorSpace {
    /// Standard dynamic-range sRGB output.
    Srgb,
}

/// A renderer-owned transform from working pixels to one presentation target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputColorTransform {
    source: WorkingColorEncoding,
    destination: OutputColorSpace,
}

impl OutputColorTransform {
    /// Identity color transform implemented by sRGB texture sampling and an
    /// sRGB presentation surface.
    pub const SRGB_TO_SRGB: Self = Self {
        source: WorkingColorEncoding::SRGB_RGBA8,
        destination: OutputColorSpace::Srgb,
    };

    /// Return the only working encoding accepted by this transform.
    #[must_use]
    pub const fn source(self) -> WorkingColorEncoding {
        self.source
    }

    /// Return the presentation color space produced by this transform.
    #[must_use]
    pub const fn destination(self) -> OutputColorSpace {
        self.destination
    }

    pub(crate) fn accepts(self, source: WorkingColorEncoding) -> bool {
        self.source == source
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OutputColorSpace, OutputColorTransform, WorkingColorEncoding, WorkingColorSpace,
        WorkingPixelFormat,
    };

    #[test]
    fn shipping_output_contract_is_exactly_srgb_rgba8_to_srgb() {
        let transform = OutputColorTransform::SRGB_TO_SRGB;

        assert_eq!(transform.source(), WorkingColorEncoding::SRGB_RGBA8);
        assert_eq!(transform.source().color_space(), WorkingColorSpace::Srgb);
        assert_eq!(
            transform.source().pixel_format(),
            WorkingPixelFormat::Rgba8Unorm
        );
        assert_eq!(transform.destination(), OutputColorSpace::Srgb);
        assert!(transform.accepts(WorkingColorEncoding::SRGB_RGBA8));
        assert!(!transform.accepts(WorkingColorEncoding::DISPLAY_P3_RGBA8));
    }
}
