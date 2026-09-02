//! Pure dock layout and menu presentation policy.
//!
//! The event loop owns every mutable fact. This module projects those facts into
//! immutable control state so egui and native accessibility remain thin adapters.

/// Logical height reserved for the persistent menu and image status bar.
pub const TOP_BAR_HEIGHT: f32 = 40.0;
/// Logical width of the collapsed tools rail.
pub const TOOLS_RAIL_WIDTH: f32 = 44.0;
/// Logical width of the expanded tools panel.
pub const TOOLS_PANEL_WIDTH: f32 = 64.0;
/// Logical width of the temporary spot-heal inspector.
pub const HEAL_PANEL_WIDTH: f32 = 248.0;
/// Logical height of the collapsed folder-preview rail.
pub const FILMSTRIP_RAIL_HEIGHT: f32 = 44.0;
/// Logical height of the expanded folder-preview panel.
pub const FILMSTRIP_PANEL_HEIGHT: f32 = 112.0;
/// Logical width of the Image Information panel.
pub const IMAGE_INFO_PANEL_WIDTH: f32 = 304.0;
pub(crate) const RATING_RECOVERY_STATUS: &str = "Rating update is not settled. Restore this image from a trusted backup, then press F5 to reload.";
pub(crate) const RATING_DISCOVERY_WRITE_STATUS: &str =
    "Wait for folder ratings to finish loading before changing this rating.";
pub(crate) const SAVE_RECOVERY_STATUS: &str =
    "Save As stopped unexpectedly. Close and reopen viewr before saving again.";

/// Horizontal edge used by a docked side panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockSide {
    /// Dock against the left edge of the available window.
    Left,
    /// Dock against the right edge of the available window.
    Right,
}

/// Space reservation state for a docked panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockState {
    /// The panel is not applicable to the current content.
    Hidden,
    /// Only the panel's narrow disclosure rail is visible.
    Collapsed,
    /// The panel and its controls are visible.
    Expanded,
}

/// Persistent chrome layout used to derive the image-safe viewport.
#[derive(Clone, Copy, Debug)]
pub struct ChromeLayout {
    /// Tools rail or expanded tools panel.
    pub tools: DockState,
    /// Edge used by the tools rail or panel.
    pub tools_side: DockSide,
    /// Whether the temporary spot-heal inspector is visible beside Tools.
    pub heal: bool,
    /// Folder-preview rail or expanded preview panel.
    pub filmstrip: DockState,
    /// Edge used by Image Information when the panel is visible.
    pub image_info: Option<DockSide>,
    /// Physical pixels per logical UI point.
    pub scale_factor: f64,
    /// Fullscreen hides persistent chrome so the photo uses the whole window.
    pub immersive: bool,
}

/// Convert persistent panel state into physical-pixel image insets.
#[must_use]
pub fn viewport_insets(layout: ChromeLayout) -> crate::view::ViewportInsets {
    let scale = layout.scale_factor.max(0.0) as f32;
    let tools_width = match layout.tools {
        DockState::Hidden => 0.0,
        DockState::Collapsed => TOOLS_RAIL_WIDTH,
        DockState::Expanded => TOOLS_PANEL_WIDTH,
    };
    let edit_width = if layout.heal { HEAL_PANEL_WIDTH } else { 0.0 };
    let left = if layout.tools_side == DockSide::Left {
        tools_width + edit_width
    } else {
        0.0
    } + if layout.image_info == Some(DockSide::Left) {
        IMAGE_INFO_PANEL_WIDTH
    } else {
        0.0
    };
    let right = if layout.tools_side == DockSide::Right {
        tools_width + edit_width
    } else {
        0.0
    } + if layout.image_info == Some(DockSide::Right) {
        IMAGE_INFO_PANEL_WIDTH
    } else {
        0.0
    };
    crate::view::ViewportInsets {
        left: left * scale,
        right: right * scale,
        top: if layout.immersive {
            0.0
        } else {
            TOP_BAR_HEIGHT * scale
        },
        bottom: match layout.filmstrip {
            DockState::Hidden => 0.0,
            DockState::Collapsed => FILMSTRIP_RAIL_HEIGHT,
            DockState::Expanded => FILMSTRIP_PANEL_HEIGHT,
        } * scale,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // independent dock facts projected from the event loop
pub(crate) struct DockInput {
    pub has_image: bool,
    pub has_multiple_images: bool,
    pub show_tools: bool,
    pub tools_expanded: bool,
    pub tools_side: DockSide,
    pub show_filmstrip: bool,
    pub filmstrip_expanded: bool,
    pub show_image_info: bool,
    pub image_info_side: DockSide,
    pub heal_active: bool,
    /// Fullscreen viewing: persistent chrome hides without changing stored panel flags.
    pub immersive: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanelKind {
    Tools,
    Filmstrip,
    ImageInfo,
}

impl PanelKind {
    #[cfg(test)]
    pub const ALL: [Self; 3] = [Self::Tools, Self::Filmstrip, Self::ImageInfo];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PanelToggleView {
    pub kind: PanelKind,
    pub label: &'static str,
    pub shortcut: &'static str,
    pub enabled: bool,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisclosureDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DisclosureView {
    pub direction: DisclosureDirection,
    pub label: &'static str,
    pub expanded: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SideDockView {
    pub state: DockState,
    pub side: DockSide,
}

impl SideDockView {
    pub fn disclosure(self) -> Option<DisclosureView> {
        match (self.side, self.state) {
            (_, DockState::Hidden) => None,
            (DockSide::Left, DockState::Expanded) => Some(DisclosureView {
                direction: DisclosureDirection::Left,
                label: "Collapse tools panel",
                expanded: true,
            }),
            (DockSide::Left, DockState::Collapsed) => Some(DisclosureView {
                direction: DisclosureDirection::Right,
                label: "Expand tools panel",
                expanded: false,
            }),
            (DockSide::Right, DockState::Expanded) => Some(DisclosureView {
                direction: DisclosureDirection::Right,
                label: "Collapse tools panel",
                expanded: true,
            }),
            (DockSide::Right, DockState::Collapsed) => Some(DisclosureView {
                direction: DisclosureDirection::Left,
                label: "Expand tools panel",
                expanded: false,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BottomDockView {
    pub state: DockState,
}

impl BottomDockView {
    pub fn disclosure(self) -> Option<DisclosureView> {
        match self.state {
            DockState::Hidden => None,
            DockState::Collapsed => Some(DisclosureView {
                direction: DisclosureDirection::Up,
                label: "Expand folder previews",
                expanded: false,
            }),
            DockState::Expanded => Some(DisclosureView {
                direction: DisclosureDirection::Down,
                label: "Collapse folder previews",
                expanded: true,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DockViewModel {
    pub tools: SideDockView,
    pub filmstrip: BottomDockView,
    pub image_info: Option<DockSide>,
    pub image_info_side: DockSide,
    pub heal: bool,
    pub immersive: bool,
    panels: [PanelToggleView; 3],
}

impl DockViewModel {
    #[must_use]
    pub fn new(input: DockInput) -> Self {
        let show_tools = if input.immersive {
            input.heal_active
        } else {
            input.show_tools
        };
        let tools_expanded = input.tools_expanded || (input.immersive && input.heal_active);
        let tools_state = panel_state(input.has_image, show_tools, tools_expanded);
        let filmstrip_state = panel_state(
            input.has_image && input.has_multiple_images,
            input.show_filmstrip && !input.immersive,
            input.filmstrip_expanded,
        );
        Self {
            tools: SideDockView {
                state: tools_state,
                side: input.tools_side,
            },
            filmstrip: BottomDockView {
                state: filmstrip_state,
            },
            image_info: (input.has_image && input.show_image_info && !input.immersive)
                .then_some(input.image_info_side),
            image_info_side: input.image_info_side,
            heal: input.has_image && input.heal_active,
            immersive: input.immersive,
            panels: [
                PanelToggleView {
                    kind: PanelKind::Tools,
                    label: "Tools",
                    shortcut: "T",
                    enabled: input.has_image,
                    selected: input.show_tools,
                },
                PanelToggleView {
                    kind: PanelKind::Filmstrip,
                    label: "Folder Previews",
                    shortcut: "G",
                    enabled: input.has_image && input.has_multiple_images,
                    selected: input.show_filmstrip,
                },
                PanelToggleView {
                    kind: PanelKind::ImageInfo,
                    label: "Image Information",
                    shortcut: "I",
                    enabled: input.has_image,
                    selected: input.show_image_info,
                },
            ],
        }
    }

    pub const fn panel_toggles(&self) -> &[PanelToggleView; 3] {
        &self.panels
    }

    #[must_use]
    pub const fn layout(self, scale_factor: f64) -> ChromeLayout {
        ChromeLayout {
            tools: self.tools.state,
            tools_side: self.tools.side,
            heal: self.heal,
            filmstrip: self.filmstrip.state,
            image_info: self.image_info,
            scale_factor,
            immersive: self.immersive,
        }
    }
}

const fn panel_state(applicable: bool, visible: bool, expanded: bool) -> DockState {
    if !applicable || !visible {
        DockState::Hidden
    } else if expanded {
        DockState::Expanded
    } else {
        DockState::Collapsed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PositionedPanel {
    Tools,
    ImageInfo,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DockSideChoice {
    pub side: DockSide,
    pub label: &'static str,
    pub selected: bool,
    pub accessibility_label: String,
}

pub(crate) fn dock_side_choices(panel: PositionedPanel, current: DockSide) -> [DockSideChoice; 2] {
    let panel_name = match panel {
        PositionedPanel::Tools => "Tools",
        PositionedPanel::ImageInfo => "Image Information",
    };
    [
        DockSideChoice {
            side: DockSide::Left,
            label: "Left",
            selected: current == DockSide::Left,
            accessibility_label: format!("{panel_name}: Left"),
        },
        DockSideChoice {
            side: DockSide::Right,
            label: "Right",
            selected: current == DockSide::Right,
            accessibility_label: format!("{panel_name}: Right"),
        },
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // independent event-loop facts, not derived policy
pub(crate) struct ChromeInput {
    pub dock: DockInput,
    pub is_loading: bool,
    pub is_opening: bool,
    pub load_failed: bool,
    pub save_busy: bool,
    pub crop_busy: bool,
    pub heal_busy: bool,
    pub heal_painting: bool,
    pub curation_busy: bool,
    /// The active curation owner is the serialized move-to-Trash queue.
    pub trash_move_busy: bool,
    pub source_verification_busy: bool,
    pub folder_scan_busy: bool,
    pub is_cropping: bool,
    pub heal_supported: bool,
    pub has_alternate_heal_source: bool,
    pub has_undo_edit: bool,
    pub has_redo_edit: bool,
    pub has_undo_trash: bool,
    pub restore_recovery_unsettled: bool,
    pub save_recovery_unsettled: bool,
    pub crop_recovery_unsettled: bool,
    pub preview_recovery_unsettled: bool,
    pub preview_retry_blocked: bool,
    pub rating_state: crate::ratings::RatingState,
    pub rating_capability: crate::ratings::RatingWriteCapability,
    pub rating_filter: crate::ratings::RatingFilter,
    pub rating_write_busy: bool,
    pub rating_discovery_busy: bool,
    pub rating_recovery_unsettled: bool,
    pub rating_folder_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChromeControl {
    OpenSource,
    Reload,
    OpenWith,
    SaveAs,
    MoveToTrash,
    PermanentDelete,
    UndoTrash,
    Crop,
    ApplyCrop,
    HealToggle,
    HealAdjust,
    HealRefreshSource,
    EditTransform,
    ViewImage,
    UndoEdit,
    RedoEdit,
    RetryLoad,
    NavigateFilmstrip,
    RatingMenu,
    RatingChoice,
    RatingFilterMenu,
}

impl ChromeControl {
    #[cfg(test)]
    pub const ALL: &'static [Self] = &[
        Self::OpenSource,
        Self::Reload,
        Self::OpenWith,
        Self::SaveAs,
        Self::MoveToTrash,
        Self::PermanentDelete,
        Self::UndoTrash,
        Self::Crop,
        Self::ApplyCrop,
        Self::HealToggle,
        Self::HealAdjust,
        Self::HealRefreshSource,
        Self::EditTransform,
        Self::ViewImage,
        Self::UndoEdit,
        Self::RedoEdit,
        Self::RetryLoad,
        Self::NavigateFilmstrip,
        Self::RatingMenu,
        Self::RatingChoice,
        Self::RatingFilterMenu,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ToolControlView {
    pub label: &'static str,
    pub shortcut: &'static str,
    pub enabled: bool,
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UndoTrashView {
    pub enabled: bool,
    pub label: &'static str,
    pub shortcut: &'static str,
    pub help: &'static str,
    pub accessibility_label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RatingChoiceView {
    pub assignment: crate::ratings::RatingAssignment,
    pub label: &'static str,
    pub shortcut: &'static str,
    pub enabled: bool,
    pub selected: bool,
    pub accessibility_label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RatingFilterChoiceView {
    pub filter: crate::ratings::RatingFilter,
    pub label: String,
    pub selected: bool,
    pub accessibility_label: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BackgroundChoiceView {
    pub label: &'static str,
    pub value: Option<[f64; 4]>,
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AppearanceChoiceView {
    pub preference: crate::theme::Preference,
    pub label: &'static str,
    pub description: String,
    pub selected: bool,
    pub accessibility_label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChromeViewModel {
    input: ChromeInput,
    pub dock: DockViewModel,
}

impl ChromeViewModel {
    #[must_use]
    pub fn new(input: ChromeInput) -> Self {
        Self {
            dock: DockViewModel::new(input.dock),
            input,
        }
    }

    #[must_use]
    pub fn is_enabled(self, control: ChromeControl) -> bool {
        let current = self.current_selection_ready();
        match control {
            ChromeControl::OpenSource => !self.input.curation_busy,
            ChromeControl::NavigateFilmstrip => self.browse_action_ready(),
            ChromeControl::Reload => {
                current && !self.input.heal_painting && !self.input.rating_discovery_busy
            }
            ChromeControl::PermanentDelete => self.exclusive_current_action_ready(),
            ChromeControl::EditTransform => self.transform_action_ready(),
            ChromeControl::OpenWith => {
                self.exclusive_current_action_ready()
                    && matches!(
                        crate::file_coherence::open_with_availability(),
                        crate::file_coherence::OpenWithAvailability::NativeChooser
                    )
            }
            ChromeControl::SaveAs => {
                current
                    && !self.input.heal_painting
                    && !self.input.is_cropping
                    && !self.input.save_recovery_unsettled
            }
            ChromeControl::MoveToTrash => {
                self.trash_submission_ready() && !self.input.restore_recovery_unsettled
            }
            ChromeControl::UndoTrash => {
                self.input.has_undo_trash
                    && !self.input.heal_painting
                    && self.curation_action_ready()
            }
            ChromeControl::Crop => self.crop_toggle_ready(),
            ChromeControl::ApplyCrop => self.crop_apply_ready(),
            ChromeControl::HealToggle => self.heal_toggle_ready(),
            ChromeControl::HealAdjust => {
                self.input.dock.has_image
                    && !self.input.heal_busy
                    && !self.input.heal_painting
                    && !self.input.curation_busy
            }
            ChromeControl::HealRefreshSource => {
                current && self.input.has_alternate_heal_source && self.edit_history_action_ready()
            }
            ChromeControl::UndoEdit => self.input.has_undo_edit && self.edit_history_action_ready(),
            ChromeControl::RedoEdit => self.input.has_redo_edit && self.edit_history_action_ready(),
            ChromeControl::RetryLoad => !self.input.preview_retry_blocked,
            ChromeControl::ViewImage => self.input.dock.has_image,
            ChromeControl::RatingMenu => !self.input.rating_write_busy,
            ChromeControl::RatingChoice => {
                self.exclusive_current_action_ready()
                    && !self.input.rating_discovery_busy
                    && !self.input.rating_recovery_unsettled
                    && self.input.rating_capability
                        == crate::ratings::RatingWriteCapability::WritableJpeg
            }
            ChromeControl::RatingFilterMenu => {
                self.input.rating_folder_count > 0 && self.exclusive_work_clear()
            }
        }
    }

    #[must_use]
    pub fn crop_control(self) -> ToolControlView {
        ToolControlView {
            label: if self.input.is_cropping {
                "Cancel Crop"
            } else {
                "Crop"
            },
            shortcut: if self.input.is_cropping { "Esc" } else { "C" },
            enabled: self.is_enabled(ChromeControl::Crop),
            selected: self.input.is_cropping,
        }
    }

    #[must_use]
    pub fn heal_control(self) -> ToolControlView {
        ToolControlView {
            label: if self.input.dock.heal_active {
                "Finish Spot Heal"
            } else if self.input.heal_busy {
                "Finishing Spot Heal..."
            } else {
                "Spot Heal"
            },
            shortcut: if self.input.dock.heal_active {
                "Esc"
            } else {
                "J"
            },
            enabled: self.is_enabled(ChromeControl::HealToggle),
            selected: self.input.dock.heal_active,
        }
    }

    #[must_use]
    pub fn undo_trash(self) -> UndoTrashView {
        let unsettled = self.input.curation_busy || self.input.restore_recovery_unsettled;
        let help = if unsettled {
            "Trash restore state is not settled. Follow the current status or recovery guidance before using Undo Trash."
        } else if self.input.has_undo_trash {
            "Restores the latest safely recoverable Trash action. It may belong to another folder."
        } else {
            "No safely recoverable Trash action is available."
        };
        let accessibility_label = if !self.input.has_undo_trash && !unsettled {
            "Undo Trash".to_owned()
        } else {
            format!("Undo Trash. {help}")
        };
        UndoTrashView {
            enabled: self.is_enabled(ChromeControl::UndoTrash),
            label: "Undo Trash",
            shortcut: "U",
            help,
            accessibility_label,
        }
    }

    #[must_use]
    pub fn rating_menu_label(self) -> &'static str {
        if self.input.dock.has_image {
            rating_status_label(self.input.rating_state)
        } else if self.input.is_opening {
            "Rating: Loading image"
        } else if self.input.load_failed {
            "Rating: Image unavailable"
        } else {
            "Rating: Open an image"
        }
    }

    #[must_use]
    pub fn rating_filter_menu_label(self) -> String {
        if self.input.folder_scan_busy {
            "Rating Filter: Reading folder...".to_owned()
        } else if self.input.rating_folder_count == 0 {
            "Rating Filter: Open a folder".to_owned()
        } else {
            rating_filter_label(self.input.rating_filter)
        }
    }

    #[must_use]
    pub fn rating_unavailable_text(self) -> &'static str {
        if self.input.rating_recovery_unsettled {
            return RATING_RECOVERY_STATUS;
        }
        if self.input.rating_discovery_busy {
            return RATING_DISCOVERY_WRITE_STATUS;
        }
        if !self.input.dock.has_image {
            if self.input.is_opening {
                return "Wait for the selected image to finish loading.";
            }
            if self.input.load_failed {
                return "Reload or open another image before assigning a rating.";
            }
            return "Open an image to assign a rating.";
        }
        match self.input.rating_capability {
            crate::ratings::RatingWriteCapability::WritableJpeg => "Rating is not ready yet.",
            crate::ratings::RatingWriteCapability::ReadOnlyFormat => {
                "This image's rating is read-only in viewr."
            }
            crate::ratings::RatingWriteCapability::UnsafeSource => {
                "Safe source identity is unavailable for rating writes."
            }
            crate::ratings::RatingWriteCapability::ObservationFailed => {
                "Rating could not be read. Close and reopen viewr before changing it."
            }
            crate::ratings::RatingWriteCapability::UnsupportedMetadata => {
                "This image has unsupported rating metadata."
            }
        }
    }

    #[must_use]
    pub fn rating_choices(self) -> Vec<RatingChoiceView> {
        let enabled = self.is_enabled(ChromeControl::RatingChoice);
        std::iter::once((
            crate::ratings::RatingAssignment::Clear,
            "Unrated",
            "0",
            self.input.rating_state == crate::ratings::RatingState::Unrated,
        ))
        .chain(crate::ratings::Rating::ALL.into_iter().map(|rating| {
            let label = rating_value_label(rating);
            let shortcut = rating_value_shortcut(rating);
            (
                crate::ratings::RatingAssignment::Set(rating),
                label,
                shortcut,
                self.input.rating_state == crate::ratings::RatingState::Rated(rating),
            )
        }))
        .map(|(assignment, label, shortcut, selected)| RatingChoiceView {
            assignment,
            label,
            shortcut,
            enabled,
            selected,
            accessibility_label: format!("Rating {label}, shortcut {shortcut}"),
        })
        .collect()
    }

    #[must_use]
    pub fn rating_filter_choices(self) -> Vec<RatingFilterChoiceView> {
        std::iter::once((
            crate::ratings::RatingFilter::All,
            "All images".to_owned(),
            "All images".to_owned(),
        ))
        .chain(crate::ratings::Rating::ALL.into_iter().map(|rating| {
            let label = format!("At least {}", rating.get());
            (
                crate::ratings::RatingFilter::AtLeast(rating),
                label.clone(),
                format!("Rating filter: {label}"),
            )
        }))
        .map(
            |(filter, label, accessibility_label)| RatingFilterChoiceView {
                filter,
                label,
                selected: self.input.rating_filter == filter,
                accessibility_label,
            },
        )
        .collect()
    }

    const fn current_selection_ready(self) -> bool {
        self.stable_selection_ready() && !self.input.heal_busy
    }

    const fn browse_action_ready(self) -> bool {
        !self.input.crop_busy
            && !self.input.save_busy
            && !self.input.heal_busy
            && !self.input.heal_painting
            && !self.input.curation_busy
            && !self.input.folder_scan_busy
            && !self.input.is_cropping
            && !self.input.dock.heal_active
            && !self.input.rating_write_busy
    }

    const fn crop_toggle_ready(self) -> bool {
        self.input.is_cropping
            || (self.current_selection_ready()
                && !self.input.crop_recovery_unsettled
                && !self.input.preview_recovery_unsettled
                && !self.input.heal_painting
                && !self.input.source_verification_busy
                && !self.input.folder_scan_busy)
    }

    const fn crop_apply_ready(self) -> bool {
        self.input.is_cropping
            && self.current_selection_ready()
            && !self.input.crop_recovery_unsettled
            && !self.input.preview_recovery_unsettled
            && !self.input.heal_painting
            && !self.input.source_verification_busy
            && !self.input.folder_scan_busy
    }

    const fn exclusive_current_action_ready(self) -> bool {
        self.current_selection_ready() && self.exclusive_work_clear()
    }

    const fn trash_submission_ready(self) -> bool {
        self.input.dock.has_image
            && !self.input.is_loading
            && !self.input.load_failed
            && !self.input.crop_busy
            && !self.input.save_busy
            && !self.input.heal_busy
            && !self.input.heal_painting
            && (!self.input.curation_busy || self.input.trash_move_busy)
            && !self.input.source_verification_busy
            && !self.input.folder_scan_busy
            && !self.input.is_cropping
            && !self.input.dock.heal_active
            && !self.input.rating_write_busy
    }

    const fn transform_action_ready(self) -> bool {
        self.current_selection_ready()
            && !self.input.heal_painting
            && !self.input.source_verification_busy
            && !self.input.folder_scan_busy
            && !self.input.dock.heal_active
    }

    const fn exclusive_work_clear(self) -> bool {
        !self.input.is_loading
            && !self.input.crop_busy
            && !self.input.save_busy
            && !self.input.heal_busy
            && !self.input.heal_painting
            && !self.input.curation_busy
            && !self.input.source_verification_busy
            && !self.input.folder_scan_busy
            && !self.input.is_cropping
            && !self.input.dock.heal_active
            && !self.input.rating_write_busy
    }

    const fn heal_toggle_ready(self) -> bool {
        if self.input.dock.heal_active {
            true
        } else {
            self.stable_selection_ready()
                && self.input.heal_supported
                && !self.input.heal_busy
                && !self.input.source_verification_busy
                && !self.input.folder_scan_busy
        }
    }

    const fn stable_selection_ready(self) -> bool {
        self.input.dock.has_image
            && !self.input.is_loading
            && !self.input.load_failed
            && !self.input.crop_busy
            && !self.input.save_busy
            && !self.input.curation_busy
            && !self.input.rating_write_busy
    }

    const fn curation_action_ready(self) -> bool {
        !self.input.is_loading
            && !self.input.crop_busy
            && !self.input.save_busy
            && !self.input.heal_busy
            && !self.input.is_cropping
            && !self.input.dock.heal_active
            && !self.input.folder_scan_busy
            && !self.input.curation_busy
            && !self.input.source_verification_busy
            && !self.input.rating_write_busy
    }

    const fn edit_history_action_ready(self) -> bool {
        !self.input.is_loading
            && !self.input.heal_busy
            && !self.input.heal_painting
            && !self.input.crop_busy
            && !self.input.save_busy
            && !self.input.curation_busy
            && !self.input.source_verification_busy
            && !self.input.folder_scan_busy
            && !self.input.is_cropping
            && !self.input.rating_write_busy
    }
}

pub(crate) const fn rating_status_label(state: crate::ratings::RatingState) -> &'static str {
    match state {
        crate::ratings::RatingState::Loading => "Rating: Reading...",
        crate::ratings::RatingState::Unrated => "Rating: Unrated",
        crate::ratings::RatingState::Rated(rating) => rating_status_value_label(rating),
        crate::ratings::RatingState::Rejected => "Rating: Rejected",
        crate::ratings::RatingState::Conflict => "Rating: Conflict",
        crate::ratings::RatingState::Unsupported => "Rating: Unsupported",
        crate::ratings::RatingState::Unreadable => "Rating: Unreadable",
    }
}

const fn rating_status_value_label(rating: crate::ratings::Rating) -> &'static str {
    match rating.get() {
        1 => "Rating: 1 of 5",
        2 => "Rating: 2 of 5",
        3 => "Rating: 3 of 5",
        4 => "Rating: 4 of 5",
        5 => "Rating: 5 of 5",
        _ => "Rating: Unsupported",
    }
}

const fn rating_value_label(rating: crate::ratings::Rating) -> &'static str {
    match rating.get() {
        1 => "1 of 5",
        2 => "2 of 5",
        3 => "3 of 5",
        4 => "4 of 5",
        5 => "5 of 5",
        _ => "Unsupported",
    }
}

const fn rating_value_shortcut(rating: crate::ratings::Rating) -> &'static str {
    match rating.get() {
        1 => "1",
        2 => "2",
        3 => "3",
        4 => "4",
        5 => "5",
        _ => "",
    }
}

pub(crate) fn rating_filter_label(filter: crate::ratings::RatingFilter) -> String {
    match filter {
        crate::ratings::RatingFilter::All => "Rating Filter: All images".to_owned(),
        crate::ratings::RatingFilter::AtLeast(rating) => {
            format!("Rating Filter: At least {}", rating.get())
        }
    }
}

pub(crate) fn appearance_menu_label(preference: crate::theme::Preference) -> String {
    format!("Appearance: {}", preference.name())
}

pub(crate) fn appearance_choices(
    current: crate::theme::Preference,
    resolved: crate::theme::Mode,
) -> Vec<AppearanceChoiceView> {
    let current_system_mode = (current == crate::theme::Preference::System).then_some(resolved);
    crate::theme::Preference::ALL
        .into_iter()
        .map(|preference| AppearanceChoiceView {
            preference,
            label: preference.name(),
            description: preference.description(current_system_mode),
            selected: current == preference,
            accessibility_label: preference.accessible_label(current_system_mode),
        })
        .collect()
}

pub(crate) fn background_choices(current: Option<[f64; 4]>) -> [BackgroundChoiceView; 4] {
    [
        BackgroundChoiceView {
            label: "Theme Default",
            value: None,
            selected: current.is_none(),
        },
        BackgroundChoiceView {
            label: "Black",
            value: Some([0.0, 0.0, 0.0, 1.0]),
            selected: current == Some([0.0, 0.0, 0.0, 1.0]),
        },
        BackgroundChoiceView {
            label: "Neutral Gray",
            value: Some([0.2, 0.2, 0.2, 1.0]),
            selected: current == Some([0.2, 0.2, 0.2, 1.0]),
        },
        BackgroundChoiceView {
            label: "White",
            value: Some([1.0, 1.0, 1.0, 1.0]),
            selected: current == Some([1.0, 1.0, 1.0, 1.0]),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::{
        ChromeControl, ChromeInput, ChromeLayout, ChromeViewModel, DisclosureDirection, DockInput,
        DockSide, DockState, DockViewModel, FILMSTRIP_PANEL_HEIGHT, FILMSTRIP_RAIL_HEIGHT,
        HEAL_PANEL_WIDTH, IMAGE_INFO_PANEL_WIDTH, PanelKind, PositionedPanel,
        RATING_DISCOVERY_WRITE_STATUS, TOOLS_PANEL_WIDTH, TOOLS_RAIL_WIDTH, TOP_BAR_HEIGHT,
        appearance_choices, background_choices, dock_side_choices, viewport_insets,
    };

    fn ready_input() -> ChromeInput {
        ChromeInput {
            dock: DockInput {
                has_image: true,
                has_multiple_images: true,
                show_tools: true,
                tools_expanded: true,
                tools_side: DockSide::Left,
                show_filmstrip: true,
                filmstrip_expanded: true,
                show_image_info: true,
                image_info_side: DockSide::Right,
                heal_active: false,
                immersive: false,
            },
            is_loading: false,
            is_opening: false,
            load_failed: false,
            save_busy: false,
            crop_busy: false,
            heal_busy: false,
            heal_painting: false,
            curation_busy: false,
            trash_move_busy: false,
            source_verification_busy: false,
            folder_scan_busy: false,
            is_cropping: false,
            heal_supported: true,
            has_alternate_heal_source: true,
            has_undo_edit: true,
            has_redo_edit: true,
            has_undo_trash: true,
            restore_recovery_unsettled: false,
            save_recovery_unsettled: false,
            crop_recovery_unsettled: false,
            preview_recovery_unsettled: false,
            preview_retry_blocked: false,
            rating_state: crate::ratings::RatingState::Unrated,
            rating_capability: crate::ratings::RatingWriteCapability::WritableJpeg,
            rating_filter: crate::ratings::RatingFilter::All,
            rating_write_busy: false,
            rating_discovery_busy: false,
            rating_recovery_unsettled: false,
            rating_folder_count: 2,
        }
    }

    #[test]
    fn ready_state_defines_every_control_without_windowing() {
        let model = ChromeViewModel::new(ready_input());
        assert_eq!(ChromeControl::ALL.len(), 21);
        for &control in ChromeControl::ALL {
            let expected = control != ChromeControl::ApplyCrop;
            assert_eq!(model.is_enabled(control), expected, "{control:?}");
        }
    }

    #[test]
    fn every_current_selection_blocker_disables_the_same_commands() {
        for blocker in 0..7 {
            let mut input = ready_input();
            match blocker {
                0 => input.dock.has_image = false,
                1 => input.is_loading = true,
                2 => input.load_failed = true,
                3 => input.crop_busy = true,
                4 => input.save_busy = true,
                5 => input.heal_busy = true,
                6 => input.rating_write_busy = true,
                _ => unreachable!(),
            }
            let model = ChromeViewModel::new(input);
            for control in [
                ChromeControl::Reload,
                ChromeControl::OpenWith,
                ChromeControl::SaveAs,
                ChromeControl::MoveToTrash,
                ChromeControl::PermanentDelete,
                ChromeControl::Crop,
                ChromeControl::HealToggle,
                ChromeControl::EditTransform,
                ChromeControl::RatingChoice,
            ] {
                assert!(!model.is_enabled(control), "blocker {blocker}: {control:?}");
            }
            assert_eq!(
                model.is_enabled(ChromeControl::ViewImage),
                blocker != 0,
                "view controls must follow presented pixels for blocker {blocker}"
            );
        }
    }

    #[test]
    fn edit_history_stays_available_in_idle_heal_mode_only() {
        let mut input = ready_input();
        input.dock.heal_active = true;
        let model = ChromeViewModel::new(input);
        assert!(model.is_enabled(ChromeControl::UndoEdit));
        assert!(model.is_enabled(ChromeControl::RedoEdit));

        for blocker in 0..6 {
            let mut input = ready_input();
            input.dock.heal_active = true;
            match blocker {
                0 => input.heal_busy = true,
                1 => input.heal_painting = true,
                2 => input.folder_scan_busy = true,
                3 => input.is_cropping = true,
                4 => input.rating_write_busy = true,
                5 => input.source_verification_busy = true,
                _ => unreachable!(),
            }
            let model = ChromeViewModel::new(input);
            assert!(
                !model.is_enabled(ChromeControl::UndoEdit),
                "blocker {blocker}"
            );
            assert!(
                !model.is_enabled(ChromeControl::RedoEdit),
                "blocker {blocker}"
            );
        }
    }

    #[test]
    fn exclusive_controls_match_every_background_and_edit_owner() {
        for blocker in 0..4 {
            let mut input = ready_input();
            match blocker {
                0 => input.source_verification_busy = true,
                1 => input.folder_scan_busy = true,
                2 => input.is_cropping = true,
                3 => input.dock.heal_active = true,
                _ => unreachable!(),
            }
            let model = ChromeViewModel::new(input);
            for control in [
                ChromeControl::OpenWith,
                ChromeControl::MoveToTrash,
                ChromeControl::PermanentDelete,
                ChromeControl::RatingChoice,
                ChromeControl::RatingFilterMenu,
            ] {
                assert!(!model.is_enabled(control), "blocker {blocker}: {control:?}");
            }
            assert_eq!(
                model.is_enabled(ChromeControl::EditTransform),
                blocker == 2,
                "blocker {blocker}: EditTransform"
            );
            assert!(model.is_enabled(ChromeControl::Reload));
            assert_eq!(
                model.is_enabled(ChromeControl::SaveAs),
                blocker != 2,
                "blocker {blocker}: SaveAs"
            );
            assert!(model.is_enabled(ChromeControl::ViewImage));
        }
    }

    #[test]
    fn reload_and_save_controls_do_not_offer_rejected_edit_states() {
        let mut input = ready_input();
        input.heal_painting = true;
        let model = ChromeViewModel::new(input);
        assert!(!model.is_enabled(ChromeControl::Reload));
        assert!(!model.is_enabled(ChromeControl::SaveAs));

        input = ready_input();
        input.rating_discovery_busy = true;
        let model = ChromeViewModel::new(input);
        assert!(!model.is_enabled(ChromeControl::Reload));
        assert!(model.is_enabled(ChromeControl::SaveAs));

        input = ready_input();
        input.is_cropping = true;
        let model = ChromeViewModel::new(input);
        assert!(model.is_enabled(ChromeControl::Reload));
        assert!(!model.is_enabled(ChromeControl::SaveAs));
    }

    #[test]
    fn filmstrip_navigation_matches_the_browse_preflight() {
        for blocker in 0..9 {
            let mut input = ready_input();
            match blocker {
                0 => input.crop_busy = true,
                1 => input.save_busy = true,
                2 => input.heal_busy = true,
                3 => input.heal_painting = true,
                4 => input.curation_busy = true,
                5 => input.folder_scan_busy = true,
                6 => input.is_cropping = true,
                7 => input.dock.heal_active = true,
                8 => input.rating_write_busy = true,
                _ => unreachable!(),
            }
            assert!(
                !ChromeViewModel::new(input).is_enabled(ChromeControl::NavigateFilmstrip),
                "blocker {blocker}"
            );
        }

        let mut input = ready_input();
        input.is_loading = true;
        assert!(ChromeViewModel::new(input).is_enabled(ChromeControl::NavigateFilmstrip));
        input.is_loading = false;
        input.source_verification_busy = true;
        assert!(ChromeViewModel::new(input).is_enabled(ChromeControl::NavigateFilmstrip));
    }

    #[test]
    fn tool_switches_wait_for_background_owners_but_active_tools_can_exit() {
        for blocker in 0..3 {
            let mut input = ready_input();
            match blocker {
                0 => input.source_verification_busy = true,
                1 => input.folder_scan_busy = true,
                2 => input.heal_painting = true,
                _ => unreachable!(),
            }
            let model = ChromeViewModel::new(input);
            assert!(!model.is_enabled(ChromeControl::Crop), "blocker {blocker}");
            if blocker != 2 {
                assert!(
                    !model.is_enabled(ChromeControl::HealToggle),
                    "blocker {blocker}"
                );
            }
        }

        let mut input = ready_input();
        input.is_cropping = true;
        input.source_verification_busy = true;
        assert!(ChromeViewModel::new(input).is_enabled(ChromeControl::Crop));

        input = ready_input();
        input.dock.heal_active = true;
        input.source_verification_busy = true;
        assert!(ChromeViewModel::new(input).is_enabled(ChromeControl::HealToggle));
        input.curation_busy = true;
        assert!(ChromeViewModel::new(input).is_enabled(ChromeControl::HealToggle));
    }

    #[test]
    fn rating_write_waits_for_discovery_while_filter_controls_remain_available() {
        let mut input = ready_input();
        input.rating_discovery_busy = true;
        let model = ChromeViewModel::new(input);
        assert!(!model.is_enabled(ChromeControl::RatingChoice));
        assert!(model.is_enabled(ChromeControl::RatingMenu));
        assert!(model.is_enabled(ChromeControl::RatingFilterMenu));
        assert_eq!(
            model.rating_unavailable_text(),
            RATING_DISCOVERY_WRITE_STATUS
        );
    }

    #[test]
    fn recovery_blocks_only_actions_that_need_the_unsettled_owner() {
        let mut input = ready_input();
        input.save_recovery_unsettled = true;
        let model = ChromeViewModel::new(input);
        assert!(!model.is_enabled(ChromeControl::SaveAs));
        assert!(model.is_enabled(ChromeControl::Crop));

        input = ready_input();
        input.crop_recovery_unsettled = true;
        let model = ChromeViewModel::new(input);
        assert!(!model.is_enabled(ChromeControl::Crop));
        assert!(model.is_enabled(ChromeControl::SaveAs));
        input.is_cropping = true;
        let model = ChromeViewModel::new(input);
        assert!(model.is_enabled(ChromeControl::Crop));
        assert!(!model.is_enabled(ChromeControl::ApplyCrop));

        input = ready_input();
        input.preview_recovery_unsettled = true;
        let model = ChromeViewModel::new(input);
        assert!(!model.is_enabled(ChromeControl::Crop));
        assert!(model.is_enabled(ChromeControl::SaveAs));

        input = ready_input();
        input.restore_recovery_unsettled = true;
        let model = ChromeViewModel::new(input);
        assert!(!model.is_enabled(ChromeControl::MoveToTrash));
        assert!(model.is_enabled(ChromeControl::PermanentDelete));

        input = ready_input();
        input.preview_retry_blocked = true;
        let model = ChromeViewModel::new(input);
        assert!(!model.is_enabled(ChromeControl::RetryLoad));
        assert!(model.is_enabled(ChromeControl::Reload));
    }

    #[test]
    fn curation_and_history_controls_follow_their_exact_conflicts() {
        for blocker in 0..10 {
            let mut input = ready_input();
            match blocker {
                0 => input.is_loading = true,
                1 => input.crop_busy = true,
                2 => input.save_busy = true,
                3 => input.heal_busy = true,
                4 => input.heal_painting = true,
                5 => input.is_cropping = true,
                6 => input.dock.heal_active = true,
                7 => input.folder_scan_busy = true,
                8 => input.source_verification_busy = true,
                9 => input.rating_write_busy = true,
                _ => unreachable!(),
            }
            let model = ChromeViewModel::new(input);
            assert!(
                !model.is_enabled(ChromeControl::UndoTrash),
                "blocker {blocker}"
            );
        }

        let mut input = ready_input();
        input.curation_busy = true;
        let model = ChromeViewModel::new(input);
        assert!(!model.is_enabled(ChromeControl::OpenSource));
        assert!(!model.is_enabled(ChromeControl::NavigateFilmstrip));
        assert!(!model.is_enabled(ChromeControl::HealToggle));
        assert!(!model.is_enabled(ChromeControl::HealAdjust));
        assert!(!model.is_enabled(ChromeControl::HealRefreshSource));
        assert!(!model.is_enabled(ChromeControl::UndoEdit));
        assert!(!model.is_enabled(ChromeControl::UndoTrash));

        input.trash_move_busy = true;
        let model = ChromeViewModel::new(input);
        assert!(model.is_enabled(ChromeControl::MoveToTrash));
        assert!(!model.is_enabled(ChromeControl::PermanentDelete));

        input = ready_input();
        input.has_undo_edit = false;
        input.has_redo_edit = false;
        input.has_undo_trash = false;
        let model = ChromeViewModel::new(input);
        assert!(!model.is_enabled(ChromeControl::UndoEdit));
        assert!(!model.is_enabled(ChromeControl::RedoEdit));
        assert!(!model.is_enabled(ChromeControl::UndoTrash));
    }

    #[test]
    fn tool_labels_selection_and_surface_enablement_are_exact() {
        let model = ChromeViewModel::new(ready_input());
        assert_eq!(
            model.crop_control(),
            super::ToolControlView {
                label: "Crop",
                shortcut: "C",
                enabled: true,
                selected: false,
            }
        );
        assert_eq!(model.heal_control().label, "Spot Heal");

        let mut input = ready_input();
        input.is_cropping = true;
        let model = ChromeViewModel::new(input);
        let crop = model.crop_control();
        assert_eq!(
            (crop.label, crop.shortcut, crop.selected),
            ("Cancel Crop", "Esc", true)
        );

        input = ready_input();
        input.dock.heal_active = true;
        input.heal_supported = false;
        input.is_loading = true;
        input.load_failed = true;
        input.heal_busy = true;
        input.rating_write_busy = true;
        input.save_busy = true;
        let model = ChromeViewModel::new(input);
        let menu_heal = model.heal_control();
        assert_eq!(
            (
                menu_heal.label,
                menu_heal.shortcut,
                menu_heal.enabled,
                menu_heal.selected
            ),
            ("Finish Spot Heal", "Esc", true, true)
        );
        input = ready_input();
        input.heal_supported = false;
        input.heal_busy = true;
        let model = ChromeViewModel::new(input);
        let heal = model.heal_control();
        assert_eq!(heal.label, "Finishing Spot Heal...");
        assert!(!heal.enabled);
    }

    #[test]
    fn heal_secondary_controls_follow_worker_and_source_ownership() {
        let model = ChromeViewModel::new(ready_input());
        assert!(model.is_enabled(ChromeControl::HealAdjust));
        assert!(model.is_enabled(ChromeControl::HealRefreshSource));

        let mut input = ready_input();
        input.has_alternate_heal_source = false;
        let model = ChromeViewModel::new(input);
        assert!(model.is_enabled(ChromeControl::HealAdjust));
        assert!(!model.is_enabled(ChromeControl::HealRefreshSource));

        input.heal_busy = true;
        let model = ChromeViewModel::new(input);
        assert!(!model.is_enabled(ChromeControl::HealAdjust));
        assert!(!model.is_enabled(ChromeControl::HealRefreshSource));

        input = ready_input();
        input.heal_painting = true;
        let model = ChromeViewModel::new(input);
        assert!(!model.is_enabled(ChromeControl::HealAdjust));
        assert!(!model.is_enabled(ChromeControl::HealRefreshSource));

        for blocker in 0..3 {
            input = ready_input();
            match blocker {
                0 => input.folder_scan_busy = true,
                1 => input.source_verification_busy = true,
                2 => input.is_cropping = true,
                _ => unreachable!(),
            }
            assert!(
                !ChromeViewModel::new(input).is_enabled(ChromeControl::HealRefreshSource),
                "blocker {blocker}"
            );
        }
    }

    #[test]
    fn undo_trash_copy_and_accessibility_state_are_derived_together() {
        let available = ChromeViewModel::new(ready_input()).undo_trash();
        assert!(available.enabled);
        assert_eq!(available.label, "Undo Trash");
        assert_eq!(available.shortcut, "U");
        assert!(available.accessibility_label.contains("another folder"));

        let mut input = ready_input();
        input.has_undo_trash = false;
        let unavailable = ChromeViewModel::new(input).undo_trash();
        assert!(!unavailable.enabled);
        assert_eq!(unavailable.accessibility_label, "Undo Trash");

        input.restore_recovery_unsettled = true;
        let unsettled = ChromeViewModel::new(input).undo_trash();
        assert!(!unsettled.enabled);
        assert!(unsettled.help.contains("not settled"));
        assert!(unsettled.accessibility_label.contains("not settled"));
    }

    #[test]
    fn rating_choices_cover_every_value_and_one_selected_state() {
        for selected in 0..=5 {
            let mut input = ready_input();
            input.rating_state = if selected == 0 {
                crate::ratings::RatingState::Unrated
            } else {
                crate::ratings::RatingState::Rated(
                    crate::ratings::Rating::new(selected).expect("valid rating"),
                )
            };
            let choices = ChromeViewModel::new(input).rating_choices();
            assert_eq!(choices.len(), 6);
            assert_eq!(choices.iter().filter(|choice| choice.selected).count(), 1);
            for (index, choice) in choices.iter().enumerate() {
                assert_eq!(choice.shortcut, index.to_string());
                assert!(choice.enabled);
                assert!(choice.accessibility_label.contains("shortcut"));
            }
        }
    }

    #[test]
    fn rating_labels_and_unavailable_reasons_cover_every_state() {
        let states = [
            (crate::ratings::RatingState::Loading, "Rating: Reading..."),
            (crate::ratings::RatingState::Unrated, "Rating: Unrated"),
            (crate::ratings::RatingState::Rejected, "Rating: Rejected"),
            (crate::ratings::RatingState::Conflict, "Rating: Conflict"),
            (
                crate::ratings::RatingState::Unsupported,
                "Rating: Unsupported",
            ),
            (
                crate::ratings::RatingState::Unreadable,
                "Rating: Unreadable",
            ),
        ];
        for (state, expected) in states {
            let mut input = ready_input();
            input.rating_state = state;
            assert_eq!(ChromeViewModel::new(input).rating_menu_label(), expected);
        }

        let capabilities = [
            (
                crate::ratings::RatingWriteCapability::ReadOnlyFormat,
                "read-only",
            ),
            (
                crate::ratings::RatingWriteCapability::UnsafeSource,
                "identity",
            ),
            (
                crate::ratings::RatingWriteCapability::ObservationFailed,
                "Close and reopen",
            ),
            (
                crate::ratings::RatingWriteCapability::UnsupportedMetadata,
                "unsupported",
            ),
        ];
        for (capability, expected) in capabilities {
            let mut input = ready_input();
            input.rating_capability = capability;
            let model = ChromeViewModel::new(input);
            assert!(!model.is_enabled(ChromeControl::RatingChoice));
            assert!(model.rating_unavailable_text().contains(expected));
        }
    }

    #[test]
    fn rating_filter_model_has_all_thresholds_and_one_selection() {
        let mut input = ready_input();
        input.rating_filter = crate::ratings::RatingFilter::AtLeast(
            crate::ratings::Rating::new(3).expect("valid rating"),
        );
        let model = ChromeViewModel::new(input);
        assert_eq!(
            model.rating_filter_menu_label(),
            "Rating Filter: At least 3"
        );
        let choices = model.rating_filter_choices();
        assert_eq!(choices.len(), 6);
        assert_eq!(choices.iter().filter(|choice| choice.selected).count(), 1);
        assert!(model.is_enabled(ChromeControl::RatingFilterMenu));
        assert_eq!(choices[3].accessibility_label, "Rating filter: At least 3");

        input.rating_folder_count = 0;
        let model = ChromeViewModel::new(input);
        assert!(!model.is_enabled(ChromeControl::RatingFilterMenu));
        assert_eq!(
            model.rating_filter_menu_label(),
            "Rating Filter: Open a folder"
        );
        input.folder_scan_busy = true;
        assert_eq!(
            ChromeViewModel::new(input).rating_filter_menu_label(),
            "Rating Filter: Reading folder..."
        );
    }

    #[test]
    fn dock_model_exhausts_applicability_visibility_and_expansion() {
        for has_image in [false, true] {
            for visible in [false, true] {
                for expanded in [false, true] {
                    let mut input = ready_input().dock;
                    input.has_image = has_image;
                    input.show_tools = visible;
                    input.tools_expanded = expanded;
                    let model = DockViewModel::new(input);
                    let expected = if !has_image || !visible {
                        DockState::Hidden
                    } else if expanded {
                        DockState::Expanded
                    } else {
                        DockState::Collapsed
                    };
                    assert_eq!(model.tools.state, expected);
                }
            }
        }

        for has_multiple in [false, true] {
            for visible in [false, true] {
                for expanded in [false, true] {
                    let mut input = ready_input().dock;
                    input.has_multiple_images = has_multiple;
                    input.show_filmstrip = visible;
                    input.filmstrip_expanded = expanded;
                    let model = DockViewModel::new(input);
                    let expected = if !has_multiple || !visible {
                        DockState::Hidden
                    } else if expanded {
                        DockState::Expanded
                    } else {
                        DockState::Collapsed
                    };
                    assert_eq!(model.filmstrip.state, expected);
                }
            }
        }
    }

    #[test]
    fn panel_toggles_and_disclosures_publish_exact_accessibility_state() {
        let model = DockViewModel::new(ready_input().dock);
        assert_eq!(model.panel_toggles().len(), PanelKind::ALL.len());
        for (toggle, kind, label, shortcut) in [
            (&model.panel_toggles()[0], PanelKind::Tools, "Tools", "T"),
            (
                &model.panel_toggles()[1],
                PanelKind::Filmstrip,
                "Folder Previews",
                "G",
            ),
            (
                &model.panel_toggles()[2],
                PanelKind::ImageInfo,
                "Image Information",
                "I",
            ),
        ] {
            assert_eq!(
                (toggle.kind, toggle.label, toggle.shortcut),
                (kind, label, shortcut)
            );
            assert!(toggle.enabled && toggle.selected);
        }

        let left_expanded = model.tools.disclosure().expect("visible tools");
        assert_eq!(left_expanded.direction, DisclosureDirection::Left);
        assert_eq!(left_expanded.label, "Collapse tools panel");
        assert!(left_expanded.expanded);
        let filmstrip = model.filmstrip.disclosure().expect("visible filmstrip");
        assert_eq!(filmstrip.direction, DisclosureDirection::Down);
        assert_eq!(filmstrip.label, "Collapse folder previews");

        let mut input = ready_input().dock;
        input.tools_side = DockSide::Right;
        input.tools_expanded = false;
        input.filmstrip_expanded = false;
        let model = DockViewModel::new(input);
        assert_eq!(
            model.tools.disclosure().expect("visible tools").direction,
            DisclosureDirection::Left
        );
        assert_eq!(
            model
                .filmstrip
                .disclosure()
                .expect("visible filmstrip")
                .direction,
            DisclosureDirection::Up
        );
    }

    #[test]
    fn dock_side_choices_have_one_selected_accessible_radio() {
        for panel in [PositionedPanel::Tools, PositionedPanel::ImageInfo] {
            for side in [DockSide::Left, DockSide::Right] {
                let choices = dock_side_choices(panel, side);
                assert_eq!(choices.iter().filter(|choice| choice.selected).count(), 1);
                assert_eq!(
                    choices
                        .iter()
                        .find(|choice| choice.selected)
                        .map(|c| c.side),
                    Some(side)
                );
                assert!(
                    choices
                        .iter()
                        .all(|choice| choice.accessibility_label.contains(':'))
                );
            }
        }
    }

    #[test]
    fn background_and_appearance_choices_have_one_selected_radio() {
        for value in [
            None,
            Some([0.0, 0.0, 0.0, 1.0]),
            Some([0.2, 0.2, 0.2, 1.0]),
            Some([1.0, 1.0, 1.0, 1.0]),
        ] {
            let choices = background_choices(value);
            assert_eq!(choices.iter().filter(|choice| choice.selected).count(), 1);
        }
        for preference in crate::theme::Preference::ALL {
            let choices = appearance_choices(preference, crate::theme::Mode::Dark);
            assert_eq!(choices.len(), 4);
            assert_eq!(choices.iter().filter(|choice| choice.selected).count(), 1);
            assert!(choices.iter().all(|choice| !choice.description.is_empty()));
            assert!(
                choices
                    .iter()
                    .all(|choice| !choice.accessibility_label.is_empty())
            );
        }
    }

    #[test]
    fn dock_layout_reserves_exact_scaled_physical_pixels() {
        let insets = viewport_insets(ChromeLayout {
            tools: DockState::Expanded,
            tools_side: DockSide::Left,
            heal: false,
            filmstrip: DockState::Expanded,
            image_info: Some(DockSide::Right),
            scale_factor: 1.5,
            immersive: false,
        });
        assert!((insets.left - TOOLS_PANEL_WIDTH * 1.5).abs() < f32::EPSILON);
        assert!((insets.right - IMAGE_INFO_PANEL_WIDTH * 1.5).abs() < f32::EPSILON);
        assert!((insets.top - TOP_BAR_HEIGHT * 1.5).abs() < f32::EPSILON);
        assert!((insets.bottom - FILMSTRIP_PANEL_HEIGHT * 1.5).abs() < f32::EPSILON);

        let insets = viewport_insets(ChromeLayout {
            tools: DockState::Collapsed,
            tools_side: DockSide::Right,
            heal: true,
            filmstrip: DockState::Collapsed,
            image_info: None,
            scale_factor: 1.25,
            immersive: false,
        });
        assert!((insets.right - (TOOLS_RAIL_WIDTH + HEAL_PANEL_WIDTH) * 1.25).abs() < f32::EPSILON);
        assert!((insets.bottom - FILMSTRIP_RAIL_HEIGHT * 1.25).abs() < f32::EPSILON);

        let insets = viewport_insets(ChromeLayout {
            tools: DockState::Expanded,
            tools_side: DockSide::Right,
            heal: false,
            filmstrip: DockState::Hidden,
            image_info: Some(DockSide::Right),
            scale_factor: 1.0,
            immersive: false,
        });
        assert!(insets.left.abs() < f32::EPSILON);
        assert!((insets.right - TOOLS_PANEL_WIDTH - IMAGE_INFO_PANEL_WIDTH).abs() < f32::EPSILON);
        assert!((insets.top - TOP_BAR_HEIGHT).abs() < f32::EPSILON);
        assert!(insets.bottom.abs() < f32::EPSILON);

        let insets = viewport_insets(ChromeLayout {
            tools: DockState::Hidden,
            tools_side: DockSide::Right,
            heal: false,
            filmstrip: DockState::Hidden,
            image_info: None,
            scale_factor: -1.0,
            immersive: false,
        });
        assert!(insets.left.abs() < f32::EPSILON);
        assert!(insets.right.abs() < f32::EPSILON);
        assert!(insets.top.abs() < f32::EPSILON);
        assert!(insets.bottom.abs() < f32::EPSILON);
    }

    #[test]
    fn immersive_fullscreen_hides_persistent_chrome() {
        let mut dock = ready_input().dock;
        dock.immersive = true;
        let layout = DockViewModel::new(dock).layout(1.0);
        assert_eq!(layout.tools, DockState::Hidden);
        assert_eq!(layout.filmstrip, DockState::Hidden);
        assert!(layout.image_info.is_none());
        let insets = viewport_insets(layout);
        assert!(insets.left.abs() < f32::EPSILON);
        assert!(insets.right.abs() < f32::EPSILON);
        assert!(insets.top.abs() < f32::EPSILON);
        assert!(insets.bottom.abs() < f32::EPSILON);

        dock.heal_active = true;
        let layout = DockViewModel::new(dock).layout(1.0);
        assert_eq!(layout.tools, DockState::Expanded);
        assert!(layout.heal);
        assert_eq!(layout.filmstrip, DockState::Hidden);
        let insets = viewport_insets(layout);
        assert!((insets.left - (TOOLS_PANEL_WIDTH + HEAL_PANEL_WIDTH)).abs() < f32::EPSILON);
        assert!(insets.top.abs() < f32::EPSILON);
        assert!(insets.bottom.abs() < f32::EPSILON);
    }
}
