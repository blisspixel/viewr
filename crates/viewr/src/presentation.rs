//! Pure selected-versus-presented policy consumed by the application adapter.
//!
//! These decisions own no image, path, cache, job, session, renderer, window, or
//! event-loop state.

use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PresentationKind {
    Loaded,
    Cropped,
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

pub(super) fn durable_presentation_error(kind: PresentationKind, message: &str) -> Option<String> {
    matches!(kind, PresentationKind::Loaded).then(|| message.to_owned())
}

pub(super) fn preview_job_matches(
    job_generation: u64,
    job_path: &Path,
    current_generation: u64,
    selected_path: Option<&Path>,
) -> bool {
    job_generation == current_generation && selected_path == Some(job_path)
}

#[cfg(test)]
mod tests {
    use super::{
        ImageReuseEligibility, NavigationImagePlan, PresentationKind, durable_presentation_error,
        image_open_in_progress, navigation_image_plan, preview_job_matches,
    };
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
