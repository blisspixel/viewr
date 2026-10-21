//! Pure selected-versus-presented policy consumed by the application adapter.
//!
//! These decisions own no image, path, cache, job, session, renderer, window, or
//! event-loop state.

use std::path::Path;

use crate::locale::{Language, Localized, tr};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PresentationKind {
    Loaded,
    Cropped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PresentedFrameTransition {
    RetainForReplacement,
    Invalidate,
    Present(PresentationKind),
}

impl PresentationKind {
    pub(super) const fn image_reuse(self) -> ImageReuseEligibility {
        match self {
            Self::Loaded => ImageReuseEligibility::PristineSource,
            Self::Cropped => ImageReuseEligibility::Ineligible,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ImageReuseEligibility {
    Ineligible,
    PristineSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NavigationImagePlan {
    ReusePresented,
    RetainPresented,
    LoadOnly,
}

pub(super) fn navigation_image_plan(
    current_index: usize,
    target_index: usize,
    current_path: &Path,
    target_path: &Path,
    presented_path: Option<&Path>,
    has_image: bool,
    reuse: ImageReuseEligibility,
) -> NavigationImagePlan {
    if !has_image || reuse != ImageReuseEligibility::PristineSource {
        return NavigationImagePlan::LoadOnly;
    }
    if presented_path == Some(target_path) {
        return NavigationImagePlan::ReusePresented;
    }
    if presented_path == Some(current_path) && current_index.abs_diff(target_index) <= 2 {
        NavigationImagePlan::RetainPresented
    } else {
        NavigationImagePlan::LoadOnly
    }
}

pub(super) const fn image_open_in_progress(
    foreground_decode_pending: bool,
    preview_kind: Option<PresentationKind>,
) -> bool {
    foreground_decode_pending || matches!(preview_kind, Some(PresentationKind::Loaded))
}

pub(super) const fn external_edit_pending_after_frame_transition(
    was_pending: bool,
    transition: PresentedFrameTransition,
) -> bool {
    match transition {
        PresentedFrameTransition::RetainForReplacement
        | PresentedFrameTransition::Present(PresentationKind::Cropped) => was_pending,
        PresentedFrameTransition::Invalidate
        | PresentedFrameTransition::Present(PresentationKind::Loaded) => false,
    }
}

pub(super) fn durable_presentation_error(kind: PresentationKind, message: &str) -> Option<String> {
    matches!(kind, PresentationKind::Loaded).then(|| message.to_owned())
}

const DECODE_ERROR_PREFIXES: &[&str] = &[
    "Could not decode: ",
    "could not decode: ",
    "could not open image: ",
    "open/decode failed: ",
    "decode failed: ",
];

/// Strip stacked decoder prefixes so one failure can be classified once.
#[must_use]
pub(super) fn stripped_decode_error(error: impl std::fmt::Display) -> String {
    let raw = error.to_string();
    let mut current = raw.lines().next().unwrap_or(&raw).trim();
    while let Some(next) = DECODE_ERROR_PREFIXES.iter().find_map(|prefix| {
        current.get(..prefix.len()).and_then(|head| {
            head.eq_ignore_ascii_case(prefix)
                .then(|| current[prefix.len()..].trim())
        })
    }) {
        current = next;
    }
    current.to_owned()
}

/// Prefix-stripped decoder text, or the generic sentence when nothing remains.
///
/// The card uses [`explain_open_failure`]. This check keeps the empty-line
/// fallback and the prefix strip honest.
#[cfg(test)]
#[must_use]
pub(super) fn user_facing_decode_error(
    language: Language,
    error: impl std::fmt::Display,
) -> String {
    let current = stripped_decode_error(error);
    if current.is_empty() {
        language
            .text(tr!("The image could not be decoded"))
            .to_owned()
    } else {
        current
    }
}

/// Why a selected image did not open, in terms a later try can act on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OpenFailureCause {
    /// The bytes are corrupt, truncated, or otherwise not a valid image.
    Damaged,
    /// The extension or container is outside the decoders viewr ships.
    Unsupported,
    /// AVIF or HEIC needs the optional `viewr-decode` helper.
    WorkerAbsent,
    /// The operating system refused to read the file.
    Permission,
    /// The path no longer names a file.
    Gone,
    /// Width, height, or decoded memory is above the safety ceiling.
    TooLarge,
    /// A decode failure that does not match a more specific cause.
    Other,
}

impl OpenFailureCause {
    /// Retry can help only when the file, permission, or helper may change.
    #[must_use]
    pub(crate) const fn allows_retry(self) -> bool {
        matches!(
            self,
            Self::WorkerAbsent | Self::Permission | Self::Gone | Self::Other
        )
    }

    /// Catalog sentence for this cause. The English text is the product copy.
    #[must_use]
    pub(crate) fn sentence(self, language: Language) -> String {
        let english = match self {
            Self::Damaged => tr!("The file is damaged or incomplete."),
            Self::Unsupported => tr!("viewr can't read this kind of file."),
            Self::WorkerAbsent => {
                tr!("This format needs the viewr-decode helper. Run viewr doctor.")
            }
            Self::Permission => tr!("viewr doesn't have permission to read this file."),
            Self::Gone => tr!("The file was moved or deleted."),
            Self::TooLarge => tr!("The image is larger than viewr opens safely."),
            Self::Other => tr!("The image could not be decoded."),
        };
        language.text(english).to_owned()
    }
}

/// One classified open failure: the card sentence, and the raw decoder line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OpenFailure {
    /// Cause used for Retry and the sentence.
    pub cause: OpenFailureCause,
    /// Translated sentence shown on the card and in the toast.
    pub sentence: String,
    /// First decoder or I/O line, with viewr's own prefixes removed.
    pub detail: String,
}

impl OpenFailure {
    /// Whether a later attempt at the same path can succeed.
    #[must_use]
    pub(crate) const fn allows_retry(&self) -> bool {
        self.cause.allows_retry()
    }
}

/// Classify a decode or I/O error into one catalog sentence.
#[must_use]
pub(crate) fn explain_open_failure(
    language: Language,
    error: impl std::fmt::Display,
) -> OpenFailure {
    let detail = stripped_decode_error(error);
    let cause = open_failure_cause(&detail);
    let sentence = cause.sentence(language);
    OpenFailure {
        cause,
        sentence,
        detail,
    }
}

fn open_failure_cause(detail: &str) -> OpenFailureCause {
    let text = detail.to_ascii_lowercase();
    if text.is_empty() {
        return OpenFailureCause::Other;
    }
    if text.contains("viewr-decode worker executable not found")
        || text.contains("configured viewr-decode worker executable is unavailable")
    {
        return OpenFailureCause::WorkerAbsent;
    }
    if text.contains("permission denied")
        || text.contains("access is denied")
        || text.contains("os error 5")
        || text.contains("os error 13")
    {
        return OpenFailureCause::Permission;
    }
    if text.contains("no such file")
        || text.contains("cannot find the file")
        || text.contains("system cannot find the file")
        || text.contains("os error 2")
    {
        return OpenFailureCause::Gone;
    }
    if text.contains("exceeds safety limit")
        || text.contains("exceeds the")
        || text.contains("exceeds limit")
        || text.contains("not enough memory")
        || text.contains("memory limit exceeded")
        || text.contains("image dimensions exceed")
        || text.contains("allocation exceeds")
    {
        return OpenFailureCause::TooLarge;
    }
    if text.contains("unsupported") {
        return OpenFailureCause::Unsupported;
    }
    if text.contains("format error")
        || text.contains("truncated")
        || text.contains("corrupt")
        || text.contains("illegal")
        || text.contains("malformed")
        || text.contains("invalid signature")
        || text.contains("invalid png")
        || text.contains("invalid pixel dimensions")
        || text.contains("dimensions must be non-zero")
        || text.contains("do not match their dimensions")
        || text.contains("unexpected end")
        || text.contains("failed to decode")
        || text.contains("failed to parse")
    {
        return OpenFailureCause::Damaged;
    }
    OpenFailureCause::Other
}

/// Toast for a classified failure. Retry is named only when a later try can help.
///
/// A sentence with no closing mark gets one period so the next clause stays a
/// separate sentence. Catalog sentences already end with a period.
#[must_use]
pub(crate) fn failure_notice(
    language: Language,
    sentence: &str,
    allows_retry: bool,
    previous_image_visible: bool,
) -> Localized {
    let sentence = ended_sentence(sentence);
    let template = match (previous_image_visible, allows_retry) {
        (false, false) => return Localized::from_translated_seam(sentence.into_owned()),
        (true, true) => tr!("{detail} The previous image remains visible. Retry is available."),
        (true, false) => tr!("{detail} The previous image remains visible."),
        (false, true) => tr!("{detail} Retry is available."),
    };
    language.fill(template, &[("detail", sentence.as_ref())])
}

/// Keep a toast sentence closed before the previous-image or Retry clause.
fn ended_sentence(sentence: &str) -> std::borrow::Cow<'_, str> {
    let trimmed = sentence.trim();
    if trimmed.is_empty() || trimmed.ends_with(['.', '!', '?']) {
        std::borrow::Cow::Borrowed(trimmed)
    } else {
        std::borrow::Cow::Owned(format!("{trimmed}."))
    }
}

/// Decode-failure toast copy. The last-good-frame clause is true only when a
/// previous picture is still on the canvas.
#[must_use]
pub(super) fn decode_failure_toast(
    language: Language,
    error: &str,
    previous_image_visible: bool,
) -> Localized {
    let failure = explain_open_failure(language, error);
    failure_notice(
        language,
        &failure.sentence,
        failure.allows_retry(),
        previous_image_visible,
    )
}

pub(super) fn preview_job_matches(
    job_generation: u64,
    job_path: &Path,
    current_generation: u64,
    selected_path: Option<&Path>,
) -> bool {
    crate::work_currency::selected_work_is_current(
        job_generation,
        job_path,
        current_generation,
        selected_path,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        ImageReuseEligibility, NavigationImagePlan, OpenFailureCause, PresentationKind,
        PresentedFrameTransition, decode_failure_toast, durable_presentation_error,
        explain_open_failure, external_edit_pending_after_frame_transition, failure_notice,
        image_open_in_progress, navigation_image_plan, preview_job_matches,
        user_facing_decode_error,
    };
    use crate::locale::Language;
    use std::path::Path;

    #[test]
    fn presentation_kind_allows_reuse_only_for_pristine_loaded_pixels() {
        assert_eq!(
            PresentationKind::Loaded.image_reuse(),
            ImageReuseEligibility::PristineSource
        );
        assert_eq!(
            PresentationKind::Cropped.image_reuse(),
            ImageReuseEligibility::Ineligible
        );
    }

    #[test]
    fn navigation_reuses_the_exact_pristine_presented_target() {
        let current = Path::new("current.png");
        let target = Path::new("target.png");

        assert_eq!(
            navigation_image_plan(
                50,
                1,
                current,
                target,
                Some(target),
                true,
                ImageReuseEligibility::PristineSource,
            ),
            NavigationImagePlan::ReusePresented
        );
        for (has_image, reuse) in [
            (false, ImageReuseEligibility::PristineSource),
            (true, ImageReuseEligibility::Ineligible),
        ] {
            assert_eq!(
                navigation_image_plan(50, 1, current, target, Some(target), has_image, reuse),
                NavigationImagePlan::LoadOnly
            );
        }
    }

    #[test]
    fn navigation_retains_only_a_near_pristine_current_presentation() {
        let current = Path::new("current.png");
        let target = Path::new("target.png");
        let other = Path::new("other.png");

        for target_index in [1, 4] {
            assert_eq!(
                navigation_image_plan(
                    3,
                    target_index,
                    current,
                    target,
                    Some(current),
                    true,
                    ImageReuseEligibility::PristineSource,
                ),
                NavigationImagePlan::RetainPresented
            );
        }
        for (current_index, presented) in [(3, Some(other)), (3, None), (4, Some(current))] {
            assert_eq!(
                navigation_image_plan(
                    current_index,
                    1,
                    current,
                    target,
                    presented,
                    true,
                    ImageReuseEligibility::PristineSource,
                ),
                NavigationImagePlan::LoadOnly
            );
        }
    }

    #[test]
    fn opening_state_counts_foreground_decode_and_loaded_preview_only() {
        assert!(image_open_in_progress(true, None));
        assert!(image_open_in_progress(
            false,
            Some(PresentationKind::Loaded)
        ));
        assert!(!image_open_in_progress(
            false,
            Some(PresentationKind::Cropped)
        ));
        assert!(!image_open_in_progress(false, None));
    }

    #[test]
    fn external_edit_reminder_follows_presented_frame_ownership() {
        assert!(external_edit_pending_after_frame_transition(
            true,
            PresentedFrameTransition::RetainForReplacement
        ));
        assert!(!external_edit_pending_after_frame_transition(
            true,
            PresentedFrameTransition::Invalidate
        ));
        assert!(!external_edit_pending_after_frame_transition(
            true,
            PresentedFrameTransition::Present(PresentationKind::Loaded)
        ));
        assert!(external_edit_pending_after_frame_transition(
            true,
            PresentedFrameTransition::Present(PresentationKind::Cropped)
        ));
        assert!(!external_edit_pending_after_frame_transition(
            false,
            PresentedFrameTransition::RetainForReplacement
        ));
        assert!(!external_edit_pending_after_frame_transition(
            false,
            PresentedFrameTransition::Invalidate
        ));
        assert!(!external_edit_pending_after_frame_transition(
            false,
            PresentedFrameTransition::Present(PresentationKind::Loaded)
        ));
        assert!(!external_edit_pending_after_frame_transition(
            false,
            PresentedFrameTransition::Present(PresentationKind::Cropped)
        ));
    }

    #[test]
    fn only_loaded_presentation_errors_are_durable() {
        let message = "Could not prepare image preview";
        assert_eq!(
            durable_presentation_error(PresentationKind::Loaded, message).as_deref(),
            Some(message)
        );
        assert_eq!(
            durable_presentation_error(PresentationKind::Cropped, message),
            None
        );
    }

    #[test]
    fn decode_failure_toast_mentions_a_previous_image_only_when_one_is_visible() {
        assert_eq!(
            user_facing_decode_error(
                Language::English,
                "Could not decode: could not open image: open/decode failed: truncated"
            ),
            "truncated"
        );
        let unsupported = explain_open_failure(
            Language::English,
            "could not open image: unsupported core image format",
        );
        assert_eq!(unsupported.cause, OpenFailureCause::Unsupported);
        assert!(!unsupported.allows_retry());
        assert_eq!(
            decode_failure_toast(
                Language::English,
                "could not open image: unsupported core image format",
                false
            )
            .as_str(),
            "viewr can't read this kind of file."
        );
        assert_eq!(
            decode_failure_toast(Language::English, "format error", true).as_str(),
            "The file is damaged or incomplete. The previous image remains visible."
        );
        assert_eq!(
            decode_failure_toast(
                Language::English,
                "viewr-decode worker executable not found beside viewr",
                false
            )
            .as_str(),
            "This format needs the viewr-decode helper. Run viewr doctor. Retry is available."
        );
        assert_eq!(
            decode_failure_toast(Language::Spanish, "format error", false).as_str(),
            "El archivo está dañado o incompleto."
        );
        // Executor loss is its own status. Classifying that sentence would hide it.
        assert_eq!(
            failure_notice(
                Language::English,
                "The image decoder stopped unexpectedly",
                true,
                true
            )
            .as_str(),
            "The image decoder stopped unexpectedly. The previous image remains visible. Retry is available."
        );
    }

    #[test]
    fn open_failures_use_one_sentence_and_retry_only_when_a_later_try_can_help() {
        let cases = [
            (
                "open/decode failed: Format error decoding Png: invalid PNG signature",
                OpenFailureCause::Damaged,
                "The file is damaged or incomplete.",
                false,
            ),
            (
                "unsupported core image format",
                OpenFailureCause::Unsupported,
                "viewr can't read this kind of file.",
                false,
            ),
            (
                "configured viewr-decode worker executable is unavailable",
                OpenFailureCause::WorkerAbsent,
                "This format needs the viewr-decode helper. Run viewr doctor.",
                true,
            ),
            (
                "failed to open worker input: Access is denied. (os error 5)",
                OpenFailureCause::Permission,
                "viewr doesn't have permission to read this file.",
                true,
            ),
            (
                "open/decode failed: No such file or directory (os error 2)",
                OpenFailureCause::Gone,
                "The file was moved or deleted.",
                true,
            ),
            (
                "decoder output allocation exceeds safety limit",
                OpenFailureCause::TooLarge,
                "The image is larger than viewr opens safely.",
                false,
            ),
            (
                "image dimensions exceed safety limit",
                OpenFailureCause::TooLarge,
                "The image is larger than viewr opens safely.",
                false,
            ),
            (
                "not enough memory to read worker input",
                OpenFailureCause::TooLarge,
                "The image is larger than viewr opens safely.",
                false,
            ),
            (
                "image dimensions must be non-zero",
                OpenFailureCause::Damaged,
                "The file is damaged or incomplete.",
                false,
            ),
            (
                "source pixels do not match their dimensions",
                OpenFailureCause::Damaged,
                "The file is damaged or incomplete.",
                false,
            ),
            (
                "SVG produced invalid pixel dimensions",
                OpenFailureCause::Damaged,
                "The file is damaged or incomplete.",
                false,
            ),
            (
                "foreground image executor stopped unexpectedly",
                OpenFailureCause::Other,
                "The image could not be decoded.",
                true,
            ),
        ];
        for (error, cause, sentence, retry) in cases {
            let failure = explain_open_failure(Language::English, error);
            assert_eq!(failure.cause, cause, "{error}");
            assert_eq!(failure.sentence, sentence, "{error}");
            assert_eq!(failure.allows_retry(), retry, "{error}");
            assert!(!failure.detail.is_empty(), "{error}");
            for language in [
                Language::English,
                Language::Spanish,
                Language::French,
                Language::German,
            ] {
                let translated = explain_open_failure(language, error);
                assert_eq!(translated.cause, cause, "{language:?} {error}");
                assert!(!translated.sentence.is_empty(), "{language:?} {error}");
                assert_ne!(
                    translated.sentence, error,
                    "{language:?} must not show the raw decoder line"
                );
            }
        }
    }

    #[test]
    fn preview_publication_requires_exact_generation_and_selected_path() {
        let path = Path::new("album/large.png");
        assert!(preview_job_matches(17, path, 17, Some(path)));
        assert!(!preview_job_matches(16, path, 17, Some(path)));
        assert!(!preview_job_matches(
            17,
            path,
            17,
            Some(Path::new("album/other.png"))
        ));
        assert!(!preview_job_matches(17, path, 17, None));
    }
}
