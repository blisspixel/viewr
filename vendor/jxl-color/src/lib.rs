//! This crate provides a set of functions related to color encodings defined by the JPEG XL
//! specification.
//!
//! # Color transformation
//! jxl-color can handle the transformation between "well-known" color encodings without any
//! external color management system, since it is required by the specification. Such
//! transformations can be done by creating a [`ColorTransform`].
//!
//! # Modules
//! - [`consts`] defines constants used by the various colorspaces.
//! - [`icc`] provides functions related to ICC profiles.
#![allow(unsafe_op_in_unsafe_fn)]

mod ciexyz;
mod cms;
pub mod consts;
mod convert;
mod error;
mod fastmath;
mod gamut;
pub mod icc;
mod tf;
mod xyb;
mod ycbcr;

pub use jxl_image::color::*;

pub use ciexyz::{AsIlluminant, AsPrimaries};
pub use cms::*;
pub use convert::*;
pub use error::*;
pub use ycbcr::ycbcr_to_rgb;

#[cfg(any(test, target_arch = "x86_64"))]
const fn avx2_fma_dispatch(avx2: bool, fma: bool) -> bool {
    avx2 && fma
}

#[cfg(test)]
mod tests {
    use super::avx2_fma_dispatch;

    #[test]
    fn avx2_dispatch_requires_both_avx2_and_fma() {
        assert!(!avx2_fma_dispatch(false, false));
        assert!(!avx2_fma_dispatch(false, true));
        assert!(!avx2_fma_dispatch(true, false));
        assert!(avx2_fma_dispatch(true, true));
    }
}
