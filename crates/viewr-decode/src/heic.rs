//! Bounded ownership and color-evidence layer around the libheif decode API.
//!
//! # Safety
//!
//! Every libheif allocation has one owning wrapper and one matching destructor.
//! The encoded input and output-profile target outlive every C call that borrows
//! them. Dimensions, profile sizes, strides, and plane lengths are validated
//! before allocation or slice construction. The version-8 options mirror is used
//! only when libheif's own allocated structure reports that append-only ABI
//! version. Version 10 is likewise required before enabling bitstream-profile
//! passthrough. Both critical field offsets are checked against current bindings
//! in the latest-libheif CI configuration.

#![allow(unsafe_code)] // all libheif pointer ownership is confined and reviewed here

use std::ffi::{CStr, c_void};
use std::marker::PhantomData;
use std::ptr::{self, NonNull};

use libheif_sys as ffi;
use viewr_protocol::{CicpColor, WorkerColorProfile};

use super::{DecodedOutput, copy_strided_rgba};

const MAX_DECODER_THREADS: i32 = 4;
const DECODED_SRGB: CicpColor = CicpColor {
    color_primaries: 1,
    transfer_characteristics: 13,
    matrix_coefficients: 0,
    full_range: true,
};

pub(super) fn decode(encoded: &[u8]) -> Result<DecodedOutput, String> {
    let context = HeifContext::read(encoded)?;
    let handle = context.primary_image()?;
    let width = handle.width()?;
    let height = handle.height()?;
    viewr_protocol::checked_rgba_len(width, height).map_err(|error| error.to_string())?;

    // Read source evidence before decode. ICC retrieval is size-first and
    // fallible, so an encoded declaration cannot allocate past the IPC limit.
    let source_icc = handle.bounded_icc()?;
    let source_nclx = handle.nclx()?;

    let output_target = source_nclx.map(OwnedNclx::from_cicp).transpose()?;
    let options = DecodingOptions::new(output_target.as_ref())?;
    let image = handle.decode(&options)?;
    let output_nclx = image.nclx()?;
    let rgba = image.rgba(width, height)?;
    let color_profile =
        select_color_profile(source_icc, source_nclx, output_nclx, options.color_contract);

    Ok(DecodedOutput {
        width,
        height,
        rgba,
        color_profile,
    })
}

fn select_color_profile(
    source_icc: Option<Vec<u8>>,
    source_nclx: Option<CicpColor>,
    output_nclx: Option<CicpColor>,
    color_contract: OutputColorContract,
) -> WorkerColorProfile {
    // libheif 1.21 and newer can convert a tagged NCLX source to a different
    // output gamut while retaining the source ICC on the decoded image. A
    // primaries or transfer change is therefore authoritative output evidence.
    // Matrix and range changes alone describe the YUV-to-RGB storage conversion
    // and do not supersede an ICC profile.
    if let (Some(source), Some(output)) = (source_nclx, output_nclx)
        && !same_color_encoding(source, output)
    {
        return WorkerColorProfile::Cicp(output);
    }
    // libheif defines ICC as the primary profile when ICC and NCLX evidence
    // coexist. Under the v10 passthrough contract, no additional color-space
    // conversion is requested, so the source ICC still describes the RGB
    // output even when the decoder also exposes bitstream-only NCLX metadata.
    // Older v8/v9 decoders target sRGB implicitly; only that contract makes a
    // retained source ICC unverifiable without an explicit source NCLX.
    if let Some(profile) = source_icc {
        return match color_contract {
            OutputColorContract::SourcePreserved => WorkerColorProfile::Icc(profile),
            OutputColorContract::DecoderSrgbTarget => {
                output_nclx.map_or(WorkerColorProfile::Unknown, WorkerColorProfile::Cicp)
            }
        };
    }
    output_nclx.or(source_nclx).map_or(
        WorkerColorProfile::Cicp(DECODED_SRGB),
        WorkerColorProfile::Cicp,
    )
}

const fn same_color_encoding(left: CicpColor, right: CicpColor) -> bool {
    left.color_primaries == right.color_primaries
        && left.transfer_characteristics == right.transfer_characteristics
}

struct HeifContext<'a> {
    raw: NonNull<ffi::heif_context>,
    _encoded: PhantomData<&'a [u8]>,
}

impl<'a> HeifContext<'a> {
    fn read(encoded: &'a [u8]) -> Result<Self, String> {
        // SAFETY: the returned context is either null or uniquely owned and is
        // released by Drop.
        let raw = NonNull::new(unsafe { ffi::heif_context_alloc() })
            .ok_or_else(|| "failed to allocate HEIC context".to_string())?;
        let context = Self {
            raw,
            _encoded: PhantomData,
        };
        // SAFETY: the context is live and uniquely configured before decode.
        unsafe {
            ffi::heif_context_set_max_decoding_threads(context.raw.as_ptr(), MAX_DECODER_THREADS);
        };
        check_error(
            // SAFETY: `encoded` remains borrowed for the context lifetime, its
            // exact byte length is supplied, and libheif does not retain the
            // null reading-options pointer.
            unsafe {
                ffi::heif_context_read_from_memory_without_copy(
                    context.raw.as_ptr(),
                    encoded.as_ptr().cast::<c_void>(),
                    encoded.len(),
                    ptr::null(),
                )
            },
            "HEIC input",
        )?;
        Ok(context)
    }

    fn primary_image(&self) -> Result<HeifHandle<'_>, String> {
        let mut raw = ptr::null_mut();
        let error =
            // SAFETY: the context is live and `raw` points to writable storage.
            unsafe { ffi::heif_context_get_primary_image_handle(self.raw.as_ptr(), &raw mut raw) };
        if error.code != ffi::heif_error_code_heif_error_Ok {
            if !raw.is_null() {
                // SAFETY: a defensive cleanup for a non-null error output.
                unsafe { ffi::heif_image_handle_release(raw) };
            }
            return Err(error_message(error, "HEIC primary image"));
        }
        let raw = NonNull::new(raw)
            .ok_or_else(|| "HEIC primary-image lookup returned no handle".to_string())?;
        Ok(HeifHandle {
            raw,
            _context: PhantomData,
        })
    }
}

impl Drop for HeifContext<'_> {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the live context.
        unsafe { ffi::heif_context_free(self.raw.as_ptr()) };
    }
}

struct HeifHandle<'a> {
    raw: NonNull<ffi::heif_image_handle>,
    _context: PhantomData<&'a ffi::heif_context>,
}

impl HeifHandle<'_> {
    fn width(&self) -> Result<u32, String> {
        // SAFETY: the handle is live for this call.
        positive_dimension(
            unsafe { ffi::heif_image_handle_get_width(self.raw.as_ptr()) },
            "width",
        )
    }

    fn height(&self) -> Result<u32, String> {
        // SAFETY: the handle is live for this call.
        positive_dimension(
            unsafe { ffi::heif_image_handle_get_height(self.raw.as_ptr()) },
            "height",
        )
    }

    fn bounded_icc(&self) -> Result<Option<Vec<u8>>, String> {
        // SAFETY: profile queries borrow a live immutable handle.
        let profile_type =
            unsafe { ffi::heif_image_handle_get_color_profile_type(self.raw.as_ptr()) };
        if !matches!(
            profile_type,
            ffi::heif_color_profile_type_heif_color_profile_type_prof
                | ffi::heif_color_profile_type_heif_color_profile_type_rICC
        ) {
            return Ok(None);
        }
        // SAFETY: the size query does not write through the handle.
        let length =
            unsafe { ffi::heif_image_handle_get_raw_color_profile_size(self.raw.as_ptr()) };
        if length == 0 {
            return Err("HEIC ICC profile has an invalid empty payload".into());
        }
        if length > viewr_protocol::MAX_COLOR_PROFILE_BYTES {
            return Err("HEIC ICC profile exceeds worker safety limit".into());
        }
        let mut profile = Vec::new();
        profile
            .try_reserve_exact(length)
            .map_err(|_| "not enough memory for HEIC ICC profile".to_string())?;
        profile.resize(length, 0);
        check_error(
            // SAFETY: `profile` has exactly the size reported by the same live
            // immutable handle, and libheif writes that many bytes at most.
            unsafe {
                ffi::heif_image_handle_get_raw_color_profile(
                    self.raw.as_ptr(),
                    profile.as_mut_ptr().cast::<c_void>(),
                )
            },
            "HEIC ICC profile",
        )?;
        Ok(Some(profile))
    }

    fn nclx(&self) -> Result<Option<CicpColor>, String> {
        let mut raw = ptr::null_mut();
        let error =
            // SAFETY: the handle is live and `raw` points to writable storage.
            unsafe {
                ffi::heif_image_handle_get_nclx_color_profile(self.raw.as_ptr(), &raw mut raw)
            };
        owned_nclx(error, raw, "HEIC source NCLX")
    }

    fn decode<'a>(&'a self, options: &DecodingOptions) -> Result<HeifImage<'a>, String> {
        let mut raw = ptr::null_mut();
        let error =
            // SAFETY: the handle and options are live, `raw` is writable, and
            // the requested color layout is interleaved eight-bit RGBA.
            unsafe {
                ffi::heif_decode_image(
                    self.raw.as_ptr(),
                    &raw mut raw,
                    ffi::heif_colorspace_heif_colorspace_RGB,
                    ffi::heif_chroma_heif_chroma_interleaved_RGBA,
                    options.raw.as_ptr(),
                )
            };
        if error.code != ffi::heif_error_code_heif_error_Ok {
            if !raw.is_null() {
                // SAFETY: a defensive cleanup for a non-null error output.
                unsafe { ffi::heif_image_release(raw) };
            }
            return Err(error_message(error, "HEIC decode"));
        }
        let raw = NonNull::new(raw).ok_or_else(|| "HEIC decode returned no image".to_string())?;
        Ok(HeifImage {
            raw,
            _handle: PhantomData,
        })
    }
}

impl Drop for HeifHandle<'_> {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the live image handle.
        unsafe { ffi::heif_image_handle_release(self.raw.as_ptr()) };
    }
}

struct DecodingOptions {
    raw: NonNull<ffi::heif_decoding_options>,
    color_contract: OutputColorContract,
}

impl DecodingOptions {
    fn new(output_target: Option<&OwnedNclx>) -> Result<Self, String> {
        // SAFETY: libheif initializes the complete versioned options structure.
        let mut raw = NonNull::new(unsafe { ffi::heif_decoding_options_alloc() })
            .ok_or_else(|| "failed to allocate HEIC decoding options".to_string())?;
        // SAFETY: this wrapper has unique access. All fields are part of the
        // v1.17 prefix and values are documented libheif enumerators.
        let options = unsafe { raw.as_mut() };
        options.strict_decoding = 1;
        options.convert_hdr_to_8bit = 1;
        options
            .color_conversion_options
            .preferred_chroma_upsampling_algorithm =
            ffi::heif_chroma_upsampling_algorithm_heif_chroma_upsampling_bilinear;
        options
            .color_conversion_options
            .only_use_preferred_chroma_algorithm = 1;
        let color_contract = configure_output_profile(raw, output_target);
        Ok(Self {
            raw,
            color_contract,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputColorContract {
    SourcePreserved,
    DecoderSrgbTarget,
}

type StartProgress = Option<unsafe extern "C" fn(ffi::heif_progress_step, i32, *mut c_void)>;
type UpdateProgress = Option<unsafe extern "C" fn(ffi::heif_progress_step, i32, *mut c_void)>;
type EndProgress = Option<unsafe extern "C" fn(ffi::heif_progress_step, *mut c_void)>;
type CancelDecoding = Option<unsafe extern "C" fn(*mut c_void) -> i32>;

// libheif's public decoding-options ABI is append-only. Version 8 added an
// output NCLX pointer, but the distro-floor Rust bindings intentionally expose
// only the version-5 prefix. This mirror lets a binary built against the floor
// request source-profile preservation when it runs with libheif 1.21 or newer.
// The latest-libheif CI configuration verifies the field offset against the
// current generated binding.
#[repr(C)]
struct DecodingOptionsV8 {
    version: u8,
    ignore_transformations: u8,
    start_progress: StartProgress,
    on_progress: UpdateProgress,
    end_progress: EndProgress,
    progress_user_data: *mut c_void,
    convert_hdr_to_8bit: u8,
    strict_decoding: u8,
    decoder_id: *const std::ffi::c_char,
    color_conversion_options: ffi::heif_color_conversion_options,
    cancel_decoding: CancelDecoding,
    color_conversion_options_ext: *mut c_void,
    ignore_sequence_editlist: i32,
    output_image_nclx_profile: *mut ffi::heif_color_profile_nclx,
    num_library_threads: i32,
    num_codec_threads: i32,
}

#[repr(C)]
struct DecodingOptionsV10 {
    v8: DecodingOptionsV8,
    autocorrect_broken_input: u8,
    output_image_nclx_profile_passthrough: u8,
}

fn configure_output_profile(
    raw: NonNull<ffi::heif_decoding_options>,
    output_target: Option<&OwnedNclx>,
) -> OutputColorContract {
    // SAFETY: `raw` comes from libheif's allocator. A runtime version of at
    // least 8 guarantees the V8 prefix; version 10 guarantees the full V10
    // mirror. An explicit target outlives the decode call.
    unsafe {
        let version = raw.as_ref().version;
        let contract = output_color_contract(version, output_target.is_some());
        if let Some(output_target) = output_target {
            if version >= 8 {
                raw.cast::<DecodingOptionsV8>()
                    .as_mut()
                    .output_image_nclx_profile = output_target.raw.as_ptr();
            }
        } else if version >= 10 {
            raw.cast::<DecodingOptionsV10>()
                .as_mut()
                .output_image_nclx_profile_passthrough = 1;
        }
        contract
    }
}

const fn output_color_contract(version: u8, has_explicit_target: bool) -> OutputColorContract {
    if has_explicit_target || version < 8 || version >= 10 {
        OutputColorContract::SourcePreserved
    } else {
        OutputColorContract::DecoderSrgbTarget
    }
}

impl Drop for DecodingOptions {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the live options allocation.
        unsafe { ffi::heif_decoding_options_free(self.raw.as_ptr()) };
    }
}

struct HeifImage<'a> {
    raw: NonNull<ffi::heif_image>,
    _handle: PhantomData<&'a ffi::heif_image_handle>,
}

impl HeifImage<'_> {
    fn nclx(&self) -> Result<Option<CicpColor>, String> {
        let mut raw = ptr::null_mut();
        let error =
            // SAFETY: the decoded image is live and `raw` is writable.
            unsafe { ffi::heif_image_get_nclx_color_profile(self.raw.as_ptr(), &raw mut raw) };
        owned_nclx(error, raw, "HEIC output NCLX")
    }

    fn rgba(&self, width: u32, height: u32) -> Result<Vec<u8>, String> {
        // SAFETY: both dimension queries borrow a live decoded image.
        let plane_width = positive_dimension(
            unsafe {
                ffi::heif_image_get_width(
                    self.raw.as_ptr(),
                    ffi::heif_channel_heif_channel_interleaved,
                )
            },
            "RGBA width",
        )?;
        // SAFETY: both dimension queries borrow a live decoded image.
        let plane_height = positive_dimension(
            unsafe {
                ffi::heif_image_get_height(
                    self.raw.as_ptr(),
                    ffi::heif_channel_heif_channel_interleaved,
                )
            },
            "RGBA height",
        )?;
        if (plane_width, plane_height) != (width, height) {
            return Err("decoder returned inconsistent RGBA plane dimensions".into());
        }

        let mut stride = 0;
        // SAFETY: the image is live and `stride` points to writable storage.
        let pixels = NonNull::new(
            unsafe {
                ffi::heif_image_get_plane_readonly(
                    self.raw.as_ptr(),
                    ffi::heif_channel_heif_channel_interleaved,
                    &raw mut stride,
                )
            }
            .cast_mut(),
        )
        .ok_or_else(|| "decoder returned no interleaved RGBA plane".to_string())?;
        let stride = usize::try_from(stride)
            .ok()
            .filter(|stride| *stride != 0)
            .ok_or_else(|| "decoder returned an invalid RGBA stride".to_string())?;
        let rows = usize::try_from(height)
            .map_err(|_| "decoder returned an invalid RGBA height".to_string())?;
        let length = stride
            .checked_mul(rows)
            .ok_or_else(|| "decoder returned an invalid RGBA buffer".to_string())?;
        // SAFETY: libheif exposes `stride` initialized bytes for every decoded
        // row, and the image retains ownership for the duration of this copy.
        let data = unsafe { std::slice::from_raw_parts(pixels.as_ptr(), length) };
        copy_strided_rgba(data, width, height, stride)
    }
}

impl Drop for HeifImage<'_> {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the live decoded image.
        unsafe { ffi::heif_image_release(self.raw.as_ptr()) };
    }
}

struct OwnedNclx {
    raw: NonNull<ffi::heif_color_profile_nclx>,
}

impl OwnedNclx {
    fn from_cicp(profile: CicpColor) -> Result<Self, String> {
        // SAFETY: the returned allocation is uniquely owned by this wrapper.
        let raw = NonNull::new(unsafe { ffi::heif_nclx_color_profile_alloc() })
            .ok_or_else(|| "failed to allocate HEIC output color profile".to_string())?;
        let mut owned = Self { raw };
        check_error(
            // SAFETY: the allocation is live and the CICP value is bounded by
            // its protocol representation.
            unsafe {
                ffi::heif_nclx_color_profile_set_color_primaries(
                    owned.raw.as_ptr(),
                    profile.color_primaries,
                )
            },
            "HEIC output color primaries",
        )?;
        check_error(
            // SAFETY: same live allocation and bounded protocol value.
            unsafe {
                ffi::heif_nclx_color_profile_set_transfer_characteristics(
                    owned.raw.as_ptr(),
                    profile.transfer_characteristics,
                )
            },
            "HEIC output transfer characteristics",
        )?;
        check_error(
            // SAFETY: same live allocation and bounded protocol value.
            unsafe {
                ffi::heif_nclx_color_profile_set_matrix_coefficients(
                    owned.raw.as_ptr(),
                    profile.matrix_coefficients,
                )
            },
            "HEIC output matrix coefficients",
        )?;
        // SAFETY: this wrapper has unique access to the live allocation.
        unsafe {
            owned.raw.as_mut().full_range_flag = u8::from(profile.full_range);
        }
        Ok(owned)
    }

    fn get(&self) -> &ffi::heif_color_profile_nclx {
        // SAFETY: the allocation remains live until this wrapper drops.
        unsafe { self.raw.as_ref() }
    }
}

impl Drop for OwnedNclx {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the libheif NCLX allocation.
        unsafe { ffi::heif_nclx_color_profile_free(self.raw.as_ptr()) };
    }
}

fn owned_nclx(
    error: ffi::heif_error,
    raw: *mut ffi::heif_color_profile_nclx,
    operation: &str,
) -> Result<Option<CicpColor>, String> {
    let profile = NonNull::new(raw).map(|raw| OwnedNclx { raw });
    if error.code == ffi::heif_error_code_heif_error_Color_profile_does_not_exist {
        return Ok(None);
    }
    check_error(error, operation)?;
    let profile = profile.ok_or_else(|| format!("{operation} returned no profile"))?;
    Ok(cicp_from_nclx(profile.get()))
}

fn cicp_from_nclx(profile: &ffi::heif_color_profile_nclx) -> Option<CicpColor> {
    let primaries = u16::try_from(profile.color_primaries).ok()?;
    let transfer = u16::try_from(profile.transfer_characteristics).ok()?;
    let matrix = u16::try_from(profile.matrix_coefficients).ok()?;
    if profile.version == 0
        || matches!(primaries, 0 | 2)
        || matches!(transfer, 0 | 2)
        || profile.full_range_flag > 1
    {
        return None;
    }
    Some(CicpColor {
        color_primaries: primaries,
        transfer_characteristics: transfer,
        matrix_coefficients: matrix,
        full_range: profile.full_range_flag == 1,
    })
}

fn positive_dimension(value: i32, label: &str) -> Result<u32, String> {
    u32::try_from(value)
        .ok()
        .filter(|value| *value != 0)
        .ok_or_else(|| format!("HEIC decoder returned an invalid {label}"))
}

fn check_error(error: ffi::heif_error, operation: &str) -> Result<(), String> {
    if error.code == ffi::heif_error_code_heif_error_Ok {
        Ok(())
    } else {
        Err(error_message(error, operation))
    }
}

fn error_message(error: ffi::heif_error, operation: &str) -> String {
    // SAFETY: libheif documents a NUL-terminated process-lifetime message for
    // every error. The null branch remains defensive against ABI violations.
    let detail = unsafe {
        if error.message.is_null() {
            format!("error code {}:{}", error.code, error.subcode)
        } else {
            CStr::from_ptr(error.message).to_string_lossy().into_owned()
        }
    };
    format!("{operation} failed: {detail}")
}

#[cfg(test)]
mod tests {
    use super::{
        DECODED_SRGB, OutputColorContract, cicp_from_nclx, output_color_contract,
        select_color_profile,
    };
    #[cfg(feature = "heic-latest-ci")]
    use super::{DecodingOptions, DecodingOptionsV8, DecodingOptionsV10, OwnedNclx};
    use libheif_sys as ffi;
    use viewr_protocol::{CicpColor, WorkerColorProfile};

    const SRGB: CicpColor = CicpColor {
        color_primaries: 1,
        transfer_characteristics: 13,
        matrix_coefficients: 0,
        full_range: true,
    };
    const DISPLAY_P3: CicpColor = CicpColor {
        color_primaries: 12,
        transfer_characteristics: 13,
        matrix_coefficients: 1,
        full_range: false,
    };

    #[cfg(feature = "heic-latest-ci")]
    #[test]
    fn compatibility_layout_matches_latest_libheif_binding() {
        assert_eq!(
            std::mem::offset_of!(DecodingOptionsV8, output_image_nclx_profile),
            std::mem::offset_of!(ffi::heif_decoding_options, output_image_nclx_profile)
        );
        assert_eq!(
            std::mem::offset_of!(DecodingOptionsV10, output_image_nclx_profile_passthrough),
            std::mem::offset_of!(
                ffi::heif_decoding_options,
                output_image_nclx_profile_passthrough
            )
        );
    }

    #[cfg(feature = "heic-latest-ci")]
    #[test]
    fn latest_libheif_receives_the_source_output_profile() {
        let target = OwnedNclx::from_cicp(DISPLAY_P3).expect("allocate output profile");
        let options = DecodingOptions::new(Some(&target)).expect("allocate decoding options");
        // SAFETY: the layout test above verifies this ABI mirror against the
        // same latest generated binding used by this test configuration.
        let configured = unsafe {
            options
                .raw
                .cast::<DecodingOptionsV8>()
                .as_ref()
                .output_image_nclx_profile
        };
        assert_eq!(configured, target.raw.as_ptr());
        assert_eq!(options.color_contract, OutputColorContract::SourcePreserved);
    }

    #[cfg(feature = "heic-latest-ci")]
    #[test]
    fn latest_libheif_preserves_bitstream_only_color_signaling() {
        let options = DecodingOptions::new(None).expect("allocate decoding options");
        // SAFETY: the layout test verifies this V10 mirror against the same
        // latest generated binding used by this test configuration.
        let passthrough = unsafe {
            options
                .raw
                .cast::<DecodingOptionsV10>()
                .as_ref()
                .output_image_nclx_profile_passthrough
        };
        assert_eq!(passthrough, 1);
        assert_eq!(options.color_contract, OutputColorContract::SourcePreserved);
    }

    #[test]
    fn older_runtime_contracts_discard_only_unverifiable_icc() {
        for version in 0..8 {
            assert_eq!(
                output_color_contract(version, false),
                OutputColorContract::SourcePreserved
            );
        }
        for version in [8, 9] {
            assert_eq!(
                output_color_contract(version, false),
                OutputColorContract::DecoderSrgbTarget
            );
            assert_eq!(
                output_color_contract(version, true),
                OutputColorContract::SourcePreserved
            );
        }
        assert_eq!(
            output_color_contract(10, false),
            OutputColorContract::SourcePreserved
        );
    }

    #[test]
    fn color_selection_distinguishes_output_conversion_from_storage_changes() {
        let icc = vec![1, 2, 3, 4];
        assert_eq!(
            select_color_profile(
                Some(icc.clone()),
                Some(DISPLAY_P3),
                Some(SRGB),
                OutputColorContract::SourcePreserved,
            ),
            WorkerColorProfile::Cicp(SRGB)
        );

        let storage_only_change = CicpColor {
            matrix_coefficients: 0,
            full_range: true,
            ..DISPLAY_P3
        };
        assert_eq!(
            select_color_profile(
                Some(icc.clone()),
                Some(DISPLAY_P3),
                Some(storage_only_change),
                OutputColorContract::SourcePreserved,
            ),
            WorkerColorProfile::Icc(icc.clone())
        );
        assert_eq!(
            select_color_profile(
                Some(icc.clone()),
                None,
                Some(SRGB),
                OutputColorContract::SourcePreserved,
            ),
            WorkerColorProfile::Icc(icc.clone())
        );
        assert_eq!(
            select_color_profile(
                Some(icc.clone()),
                None,
                None,
                OutputColorContract::SourcePreserved,
            ),
            WorkerColorProfile::Icc(icc)
        );
        assert_eq!(
            select_color_profile(
                None,
                Some(DISPLAY_P3),
                Some(SRGB),
                OutputColorContract::SourcePreserved,
            ),
            WorkerColorProfile::Cicp(SRGB)
        );
        assert_eq!(
            select_color_profile(
                None,
                Some(DISPLAY_P3),
                None,
                OutputColorContract::SourcePreserved,
            ),
            WorkerColorProfile::Cicp(DISPLAY_P3)
        );
        assert_eq!(
            select_color_profile(None, None, None, OutputColorContract::SourcePreserved,),
            WorkerColorProfile::Cicp(DECODED_SRGB)
        );
        let unverified_icc = vec![9, 8, 7];
        assert_eq!(
            select_color_profile(
                Some(unverified_icc.clone()),
                None,
                Some(SRGB),
                OutputColorContract::DecoderSrgbTarget,
            ),
            WorkerColorProfile::Cicp(SRGB)
        );
        assert_eq!(
            select_color_profile(
                Some(unverified_icc),
                None,
                None,
                OutputColorContract::DecoderSrgbTarget,
            ),
            WorkerColorProfile::Unknown
        );
    }

    #[test]
    fn nclx_conversion_rejects_ambiguous_or_invalid_metadata() {
        let raw = |primaries, transfer, full_range_flag| ffi::heif_color_profile_nclx {
            version: 1,
            color_primaries: primaries,
            transfer_characteristics: transfer,
            matrix_coefficients: 1,
            full_range_flag,
            color_primary_red_x: 0.0,
            color_primary_red_y: 0.0,
            color_primary_green_x: 0.0,
            color_primary_green_y: 0.0,
            color_primary_blue_x: 0.0,
            color_primary_blue_y: 0.0,
            color_primary_white_x: 0.0,
            color_primary_white_y: 0.0,
        };

        assert_eq!(
            cicp_from_nclx(&raw(1, 13, 1)),
            Some(CicpColor {
                matrix_coefficients: 1,
                ..SRGB
            })
        );
        assert_eq!(cicp_from_nclx(&raw(2, 13, 1)), None);
        assert_eq!(cicp_from_nclx(&raw(1, 2, 1)), None);
        assert_eq!(cicp_from_nclx(&raw(1, 13, 2)), None);
    }
}
