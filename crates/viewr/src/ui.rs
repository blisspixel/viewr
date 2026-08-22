//! Immediate-mode chrome: menus, docked collapsible panels, empty state, crop
//! overlay, and toasts.
//!
//! Design intent (see `docs/DESIGN.md`): persistent controls reserve their own
//! space and never cover the photo. Amber marks active tools only.

use crate::chrome::{
    ChromeControl, ChromeInput, ChromeViewModel, DisclosureDirection, DisclosureView, DockInput,
    PanelKind, PositionedPanel, ToolControlView,
};
pub use crate::chrome::{
    ChromeLayout, DockSide, DockState, FILMSTRIP_PANEL_HEIGHT, FILMSTRIP_RAIL_HEIGHT,
    HEAL_PANEL_WIDTH, IMAGE_INFO_PANEL_WIDTH, TOOLS_PANEL_WIDTH, TOOLS_RAIL_WIDTH, TOP_BAR_HEIGHT,
    viewport_insets,
};
pub(crate) use crate::chrome::{RATING_RECOVERY_STATUS, SAVE_RECOVERY_STATUS};
use egui::containers::scroll_area::ScrollBarVisibility;
use egui::text::LayoutJob;
use egui::{
    Align2, Area, Color32, CornerRadius, CursorIcon, FontId, Frame, Panel, Pos2, Rect, RichText,
    ScrollArea, Sense, Stroke, TextFormat, Vec2, WidgetInfo, WidgetType,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ChromeColors {
    accent: Color32,
    accent_ink: Color32,
    text: Color32,
    muted: Color32,
    panel: Color32,
    raised: Color32,
    active: Color32,
    border: Color32,
}

fn chrome_colors_for(mode: crate::theme::Mode) -> ChromeColors {
    let palette = crate::theme::chrome_palette_for(mode);
    let color = |rgba: [u8; 4]| {
        let [red, green, blue, alpha] = rgba;
        Color32::from_rgba_unmultiplied(red, green, blue, alpha)
    };
    ChromeColors {
        accent: color(palette.accent),
        accent_ink: color(palette.accent_ink),
        text: color(palette.text),
        muted: color(palette.muted),
        panel: color(palette.panel),
        raised: color(palette.raised),
        active: color(palette.active),
        border: color(palette.border),
    }
}

fn chrome_colors(ui: &egui::Ui) -> ChromeColors {
    ui.ctx()
        .data(|data| data.get_temp(egui::Id::new("viewr_chrome_colors")))
        .unwrap_or_else(|| chrome_colors_for(crate::theme::Mode::Dark))
}

fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    let [red, green, blue, _] = color.to_array();
    Color32::from_rgba_unmultiplied(red, green, blue, alpha)
}

#[cfg(target_os = "macos")]
const PRIMARY_MODIFIER: &str = "Cmd";
#[cfg(not(target_os = "macos"))]
const PRIMARY_MODIFIER: &str = "Ctrl";

const TOP_STATUS_MAX_WIDTH: f32 = 220.0;
const TOP_STATUS_COMPACT_MAX_WIDTH: f32 = 172.0;
const TOP_METADATA_GAP: f32 = 8.0;
/// Extra separation egui adds between the top-bar reading items.
///
/// The reading strip sets this itself so its 8px gaps stay legible whatever
/// spacing the menu titles beside it use.
const TOP_METADATA_SPACING: f32 = 2.0;

const OPEN_FILE_SCOPE_HELP: &str = "Open one image. When access allows, viewr also browses supported images in its folder for this session.";
const OPEN_FOLDER_SCOPE_HELP: &str =
    "Choose a folder explicitly and browse its supported images for this session.";
const OPEN_WITH_HELP: &str = "Opens the original file, including embedded metadata, in an app you choose. Unsaved viewr edits are not included. That app's privacy rules apply. If the other app changes the file, viewr reloads it when that is safe, or asks you to press F5 when unsaved edits would be lost.";
const LOCAL_PRIVACY_SUMMARY: &str = "Local only. No cloud or viewr activity log.";
const APPEARANCE_SCOPE_HELP: &str = "Changes app chrome and its default canvas. Image pixels stay unchanged; Image Background overrides the canvas separately.";
const EXTERNAL_EDIT_BADGE: &str = "External F5";
const EXTERNAL_EDIT_STANDALONE_STATUS: &str = "Source may have changed";
const EXTERNAL_EDIT_ACCESSIBLE_STATUS: &str = crate::file_coherence::reload_reminder_copy();
pub(crate) const CROP_RECOVERY_STATUS: &str =
    "Crop stopped unexpectedly. Close and reopen viewr before cropping again.";
pub(crate) const PREVIEW_RECOVERY_STATUS: &str = "Display preview preparation stopped unexpectedly. Close and reopen viewr before opening another over-limit image or cropping again.";
// Anchor the naturally sized startup card from a stable top-left point on its first sizing pass.
const EMPTY_STATE_EXPECTED_HEIGHT: f32 = 268.0;
const RATING_DISCLOSURE_FOCUS_STATE: &str = "rating_write_disclosure_focus_initialized";
const SAVE_OVERWRITE_FOCUS_STATE: &str = "save_overwrite_focus_initialized";

/// Actions dispatched from the UI to be handled by the main application logic.
pub enum UiAction {
    /// Open a new image file dialog.
    Open,
    /// Open a folder with explicit user consent for sibling navigation.
    OpenFolder,
    /// Decode the current file from disk again, bypassing the neighbor cache.
    Reload,
    /// Open a save as dialog.
    SaveAs,
    /// Confirm replacement of the exact captured Save As destination.
    ConfirmSaveOverwrite,
    /// Cancel a pending Save As overwrite without changing the destination.
    CancelSaveOverwrite,
    /// Ask the operating system which external app should open the current source.
    OpenWith,
    /// Move the current file to the trash.
    Trash,
    /// Undo the last trash operation.
    UndoTrash,
    /// Set the background color.
    SetBackground(Option<[f64; 4]>),
    /// Set the application appearance preference.
    SetTheme(crate::theme::Preference),
    /// Open the About viewr surface.
    ShowAbout,
    /// Close the About viewr surface.
    CloseAbout,
    /// Open the local update-instructions surface.
    ShowUpdate,
    /// Close the local update-instructions surface.
    CloseUpdate,
    /// Assign or clear the current image's embedded rating.
    AssignRating(crate::ratings::RatingAssignment),
    /// Change the session-only folder rating threshold.
    SetRatingFilter(crate::ratings::RatingFilter),
    /// Confirm the first embedded-metadata write in this process session.
    ConfirmRatingDisclosure,
    /// Cancel the pending embedded-metadata write.
    CancelRatingDisclosure,
    /// Clear the active threshold and return to the retained folder position.
    ShowAllRatings,
    /// Toggle the Image Info panel.
    ToggleImageInfo,
    /// Show or fully hide the docked tools panel.
    ToggleToolsPanelVisibility,
    /// Expand or collapse the visible tools panel.
    ToggleToolsPanelExpansion,
    /// Show or fully hide the docked folder-preview panel.
    ToggleFilmstripPanelVisibility,
    /// Expand or collapse the visible folder-preview panel.
    ToggleFilmstripPanelExpansion,
    /// Move the tools panel to a horizontal edge.
    SetToolsPanelSide(DockSide),
    /// Move Image Information to a horizontal edge.
    SetImageInfoSide(DockSide),
    /// Toggle whether Save As retains EXIF (default off = strip).
    ToggleRetainExif,
    /// Pause or resume the current animated image.
    ToggleAnimationPlayback,
    /// Step the current animation frame or document page without wrapping.
    StepSequence(isize),
    /// Retry decoding the selected path after a load failure.
    RetryLoad,
    /// Rotate the image clockwise.
    RotateCw,
    /// Rotate the image counter-clockwise.
    RotateCcw,
    /// Flip the image horizontally.
    FlipH,
    /// Flip the image vertically.
    FlipV,
    /// Toggle fullscreen mode.
    ToggleFullscreen,
    /// Reset the image to fit inside the available viewport.
    FitToView,
    /// Display one image pixel per physical screen pixel.
    ActualSize,
    /// Increase zoom around the image-safe viewport center.
    ZoomIn,
    /// Decrease zoom around the image-safe viewport center.
    ZoomOut,
    /// Navigate to a relative file index.
    Navigate(isize),
    /// Jump to an absolute playlist index.
    NavigateTo(usize),
    /// Toggle the crop tool mode.
    ToggleCrop,
    /// Apply the current crop area.
    ApplyCrop,
    /// Cancel crop without applying.
    CancelCrop,
    /// Set the aspect ratio for the crop tool.
    SetCropRatio(crate::crop::CropRatio),
    /// Update the session-local numeric custom crop ratio.
    SetCustomCropRatio(u16, u16),
    /// Swap the active crop ratio between landscape and portrait.
    SwapCropRatio,
    /// Move the crop selection using a logical-screen pointer delta.
    MoveCrop {
        /// Current logical-screen pointer position.
        pointer: [f32; 2],
        /// Logical-screen movement since the previous frame.
        delta: [f32; 2],
    },
    /// Resize from the handle at `handle_center` toward `pointer`.
    ResizeCrop {
        /// Logical-screen center of the active handle.
        handle_center: [f32; 2],
        /// Current logical-screen pointer position.
        pointer: [f32; 2],
    },
    /// Enter or leave the focused spot-heal tool.
    ToggleHeal,
    /// Close the right-click context menu.
    CloseContextMenu,
    /// Change the spot-heal radius in source-image pixels.
    SetHealBrushRadius(u32),
    /// Change the spot-heal feather as a percentage of brush radius.
    SetHealFeather(u8),
    /// Re-run the latest repair with the next ranked source patch.
    RefreshHealSource,
    /// Undo the latest in-memory pixel edit.
    UndoEdit,
    /// Reapply the latest undone in-memory pixel edit.
    RedoEdit,
    /// Permanently delete the current image (UI will confirm).
    PermanentDelete,
}

/// Owned frame inputs for drawing chrome.
#[allow(clippy::struct_excessive_bools)] // independent UI mode bits for one frame
pub(crate) struct UiFrameOwned {
    /// Raw dock facts captured once by the event loop for layout and paint.
    pub(crate) dock: DockInput,
    /// Whether Save As will retain EXIF (default false = strip).
    pub retain_exif: bool,
    /// Current image-background override; `None` follows the operating-system theme.
    pub background_override: Option<[f64; 4]>,
    /// User-selected appearance preference.
    pub theme_preference: crate::theme::Preference,
    /// Resolved appearance after applying the system choice.
    pub theme_mode: crate::theme::Mode,
    /// Whether the About viewr surface is open.
    pub show_about: bool,
    /// Whether the local update-instructions surface is open.
    pub show_update: bool,
    /// Current embedded rating, folder filter, and write state.
    pub rating: RatingUiState,
    /// Whether an external handoff may have made the displayed pixels stale.
    pub external_edit_pending: bool,
    /// Whether the selected path no longer names the presented file.
    pub source_gone: bool,
    /// Privacy-safe basename for the currently presented pixels (display only).
    pub file_path: Option<String>,
    /// Privacy-safe basename of the currently selected file.
    pub selected_file_name: Option<String>,
    /// Pixel dimensions of the current image, if any.
    pub img_size: Option<(u32, u32)>,
    /// Playback state for an animated image.
    pub animation: Option<AnimationUiInfo>,
    /// Still-page state for a multi-page TIFF or multi-size ICO.
    pub pages: Option<PageUiInfo>,
    /// Best-effort local file and camera metadata.
    pub details: Option<crate::image_info::ImageDetails>,
    /// How the displayed pixels were normalized for the sRGB render pipeline.
    pub color_profile: Option<crate::decode::ColorProfileStatus>,
    /// How the current sRGB swapchain relates to the display that owns the window.
    pub display_output: crate::display_state::DisplayOutputStatus,
    /// Crop tool active.
    pub is_cropping: bool,
    /// Active crop aspect lock.
    pub crop_ratio: crate::crop::CropRatio,
    /// Session-local custom ratio fields shown by the crop picker.
    pub custom_crop_ratio: (u16, u16),
    /// The displayed texture represents every source pixel Spot Heal can edit.
    pub heal_supported: bool,
    /// A spot-heal worker is processing the current stroke.
    pub heal_busy: bool,
    /// A pointer stroke is actively collecting spot-heal samples.
    pub heal_painting: bool,
    /// Spot-heal radius in source-image pixels.
    pub heal_brush_radius: u32,
    /// Spot-heal feather as a percentage of brush radius.
    pub heal_feather_percent: u8,
    /// Selected and total ranked source patches for the latest repair.
    pub heal_source: Option<(usize, usize)>,
    /// Whether the edit history contains an undo entry before UI gating.
    pub has_undo_edit: bool,
    /// Whether the edit history contains a redo entry before UI gating.
    pub has_redo_edit: bool,
    /// Whether a retained exact Trash receipt exists before UI gating.
    pub has_undo_trash: bool,
    /// Whether a prior Restore must reconcile before Undo ownership can be replaced.
    pub restore_recovery_unsettled: bool,
    /// Hand tool is currently dragging.
    pub is_panning: bool,
    /// Space is held for the temporary pan tool.
    pub space_held: bool,
    /// A decode or display-preparation job is blocking image actions.
    pub is_loading: bool,
    /// A selected source is decoding or preparing its first display preview.
    pub is_opening: bool,
    /// Most recent decode failure for the selected path.
    pub load_error: Option<String>,
    /// An explicit Save As encode is running.
    pub save_busy: bool,
    /// An existing captured destination awaits app-owned overwrite consent.
    pub save_overwrite_pending: bool,
    /// A lost Save As completion requires restart before another export.
    pub save_recovery_unsettled: bool,
    /// A full-resolution crop is being applied off the UI thread.
    pub crop_busy: bool,
    /// A lost crop completion requires restart before another crop.
    pub crop_recovery_unsettled: bool,
    /// A lost display-preview completion requires restart before more preview work.
    pub preview_recovery_unsettled: bool,
    /// Retry for the current load would require the lost display-preview executor.
    pub preview_load_retry_blocked: bool,
    /// Fixed, path-private description of active Trash, delete, or restore work.
    pub curation_status: Option<String>,
    /// Durable recovery guidance after an indeterminate curation worker loss.
    pub curation_recovery_status: Option<String>,
    /// A folder scan is still deciding the active playlist scope.
    pub folder_scan_busy: bool,
    /// 1-based index and total in the folder playlist, if known.
    pub playlist_pos: Option<(usize, usize)>,
    /// Physical display pixels per source-image pixel (`1.0` = actual size).
    pub pixel_scale: f32,
    /// Transient toast message (trash undo hint, etc.).
    pub toast: Option<String>,
    /// Neighbor filmstrip entries.
    pub filmstrip: Vec<FilmstripItem>,
    /// Crop rectangle in screen pixels `[x0, y0, x1, y1]` when previewing.
    pub crop_screen: Option<[f32; 4]>,
    /// Crop rectangle in source-image UV coordinates for exact pixel dimensions.
    pub crop_uv: Option<[f32; 4]>,
    /// Whether the visible/exported crop swaps source width and height.
    pub crop_swaps_axes: bool,
    /// Image-safe viewport in logical UI coordinates `[x0, y0, x1, y1]`.
    pub image_viewport: Option<[f32; 4]>,
    /// Current spot-heal stroke projected into logical screen coordinates.
    pub heal_stroke_screen: Vec<[f32; 2]>,
    /// Current pointer position in logical screen coordinates while healing.
    pub heal_cursor_screen: Option<[f32; 2]>,
    /// Brush radius in logical screen pixels.
    pub heal_brush_screen_radius: f32,
    /// Screen position of the right-click context menu, if open.
    pub context_menu_pos: Option<[f32; 2]>,
}

/// Immutable rating and folder-filter state for one rendered frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // independent write, discovery, filter, and recovery states
pub struct RatingUiState {
    /// Rating attached to the presented source.
    pub state: crate::ratings::RatingState,
    /// Whether source mutation is proven safe for this image.
    pub capability: crate::ratings::RatingWriteCapability,
    /// Active session-only folder threshold.
    pub filter: crate::ratings::RatingFilter,
    /// A rating replacement transaction is in progress.
    pub write_busy: bool,
    /// A prior rating replacement left the source mutation indeterminate.
    pub recovery_unsettled: bool,
    /// Folder rating discovery is in progress.
    pub discovery_busy: bool,
    /// The current image was retained after falling below the threshold.
    pub outside_filter: bool,
    /// One-based position and number of matching images.
    pub visible_position: Option<(usize, usize)>,
    /// Number of entries in the active projection.
    pub match_count: usize,
    /// Canonical zero-based index used by Trash receipts and filmstrip cells.
    pub current_catalog_index: Option<usize>,
    /// Total images in the canonical folder catalog.
    pub folder_count: usize,
    /// First-write assignment awaiting explicit disclosure confirmation.
    pub pending_disclosure: Option<crate::ratings::RatingAssignment>,
}

impl Default for RatingUiState {
    fn default() -> Self {
        Self {
            state: crate::ratings::RatingState::Loading,
            capability: crate::ratings::RatingWriteCapability::UnsafeSource,
            filter: crate::ratings::RatingFilter::All,
            write_busy: false,
            recovery_unsettled: false,
            discovery_busy: false,
            outside_filter: false,
            visible_position: None,
            match_count: 0,
            current_catalog_index: None,
            folder_count: 0,
            pending_disclosure: None,
        }
    }
}

/// Animation state shown in Image Information.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnimationUiInfo {
    /// Zero-based displayed frame index.
    pub frame_index: usize,
    /// Total decoded frames.
    pub frame_count: usize,
    /// Whether timed playback is active.
    pub is_playing: bool,
    /// Whether a previous-frame step is available.
    pub can_previous: bool,
    /// Whether a next-frame step is available.
    pub can_next: bool,
}

/// Still-page state shown in Image Information.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageUiInfo {
    /// Zero-based displayed page index.
    pub index: usize,
    /// Total decoded pages or icon frames.
    pub count: usize,
    /// User-facing noun: "Page" or "Icon".
    pub noun: &'static str,
    /// Whether a previous-page step is available.
    pub can_previous: bool,
    /// Whether a next-page step is available.
    pub can_next: bool,
    /// Accessible name including page identity and dimensions.
    pub accessibility_label: String,
    /// Visible page or icon identity, including ICO pixel size.
    pub visible_label: String,
}

impl UiFrameOwned {
    fn chrome_view_model(&self) -> ChromeViewModel {
        ChromeViewModel::new(ChromeInput {
            dock: self.dock,
            is_loading: self.is_loading,
            is_opening: self.is_opening,
            load_failed: self.load_error.is_some(),
            save_busy: self.save_busy,
            crop_busy: self.crop_busy,
            heal_busy: self.heal_busy,
            heal_painting: self.heal_painting,
            curation_busy: self.curation_status.is_some(),
            folder_scan_busy: self.folder_scan_busy,
            is_cropping: self.is_cropping,
            heal_supported: self.heal_supported,
            has_heal_source: self.heal_source.is_some(),
            has_undo_edit: self.has_undo_edit,
            has_redo_edit: self.has_redo_edit,
            has_undo_trash: self.has_undo_trash,
            restore_recovery_unsettled: self.restore_recovery_unsettled,
            save_recovery_unsettled: self.save_recovery_unsettled,
            crop_recovery_unsettled: self.crop_recovery_unsettled,
            preview_recovery_unsettled: self.preview_recovery_unsettled,
            preview_retry_blocked: self.preview_load_retry_blocked,
            rating_state: self.rating.state,
            rating_capability: self.rating.capability,
            rating_filter: self.rating.filter,
            rating_write_busy: self.rating.write_busy,
            rating_recovery_unsettled: self.rating.recovery_unsettled,
            rating_folder_count: self.rating.folder_count,
        })
    }
}

/// One cell in the progressive bottom filmstrip.
#[derive(Clone)]
pub struct FilmstripItem {
    /// Canonical playlist index used for navigation.
    pub index: usize,
    /// One-based position in the active rating-filter projection.
    pub position: usize,
    /// File basename for tooltip / fallback label.
    pub name: String,
    /// Thumbnail texture when ready.
    pub texture: Option<egui::TextureHandle>,
}

#[must_use]
pub(crate) const fn save_overwrite_action_allowed(action: &UiAction) -> bool {
    matches!(
        action,
        UiAction::ConfirmSaveOverwrite | UiAction::CancelSaveOverwrite
    )
}

fn actions_owned_by_modal(mut actions: Vec<UiAction>, frame: &UiFrameOwned) -> Vec<UiAction> {
    if frame.save_overwrite_pending {
        actions.retain(save_overwrite_action_allowed);
    } else if frame.rating.pending_disclosure.is_some() {
        actions.retain(|action| {
            matches!(
                action,
                UiAction::ConfirmRatingDisclosure | UiAction::CancelRatingDisclosure
            )
        });
    } else if frame.show_update {
        actions.retain(|action| matches!(action, UiAction::CloseUpdate));
    } else if frame.show_about {
        actions.retain(|action| matches!(action, UiAction::CloseAbout));
    }
    actions
}

/// Render the UI overlays and return a list of actions triggered by the user.
pub(crate) fn render(ui: &mut egui::Ui, frame: &UiFrameOwned) -> Vec<UiAction> {
    let mut actions = Vec::new();
    apply_chrome_theme(ui.ctx(), frame.theme_mode);
    let colors = chrome_colors(ui);
    let chrome = frame.chrome_view_model();
    let modal_active = frame.save_overwrite_pending
        || frame.rating.pending_disclosure.is_some()
        || frame.show_update
        || frame.show_about;

    ui.add_enabled_ui(!modal_active, |ui| {
        render_background(ui, &mut actions, frame, chrome, colors);
    });

    if frame.save_overwrite_pending {
        render_save_overwrite_confirmation(ui, &mut actions);
    } else if frame.rating.pending_disclosure.is_some() {
        render_rating_disclosure(ui, &mut actions, frame);
    } else if frame.show_update {
        render_update(ui, &mut actions);
    } else if frame.show_about {
        render_about(ui, &mut actions);
    }
    ui.ctx().data_mut(|data| {
        if !frame.save_overwrite_pending {
            data.remove_temp::<bool>(egui::Id::new(SAVE_OVERWRITE_FOCUS_STATE));
        }
        if frame.rating.pending_disclosure.is_none() {
            data.remove_temp::<bool>(egui::Id::new(RATING_DISCLOSURE_FOCUS_STATE));
        }
    });

    actions_owned_by_modal(actions, frame)
}

fn render_background(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    frame: &UiFrameOwned,
    chrome: ChromeViewModel,
    colors: ChromeColors,
) {
    if !frame.dock.immersive {
        render_top_menu(ui, actions, frame, chrome);
    }

    if rating_filter_is_empty(frame) {
        render_filtered_empty_state(ui, actions, frame);
        if let Some(msg) = &frame.toast {
            render_toast(ui, msg, frame);
        }
        return;
    }

    if !frame.dock.has_image {
        render_empty_state(ui, actions, frame, chrome);
        if let Some(msg) = &frame.toast {
            render_toast(ui, msg, frame);
        }
        return;
    }

    render_context_menu(ui, actions, frame, chrome, colors);

    if let Some(side) = chrome.dock.image_info {
        render_image_info_panel(ui, actions, frame, side);
    }

    if chrome.dock.filmstrip.state != DockState::Hidden {
        render_filmstrip(ui, actions, frame, chrome);
    }

    if chrome.dock.tools.state != DockState::Hidden {
        render_tools_panel(ui, actions, frame, chrome);
    }

    if frame.dock.heal_active {
        render_heal_panel(ui, actions, frame, chrome);
        render_heal_overlay(ui, frame);
    }

    if let Some(msg) = &frame.toast {
        render_toast(ui, msg, frame);
    }

    if frame.is_cropping {
        render_crop_overlay(ui, frame, chrome, actions);
    }

    apply_cursor(ui, frame);
}

fn render_context_menu(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    frame: &UiFrameOwned,
    chrome: ChromeViewModel,
    colors: ChromeColors,
) {
    let Some(pos) = frame.context_menu_pos else {
        return;
    };
    let mut close = false;
    egui::Window::new("Quick Tools")
        .fixed_pos(Pos2::new(pos[0], pos[1]))
        .constrain_to(ui.ctx().content_rect())
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            ui.set_max_width(320.0);
            ui.horizontal(|ui| {
                let heal = chrome.heal_control();
                if context_tool_button(ui, heal).clicked() {
                    actions.push(UiAction::ToggleHeal);
                    close = true;
                }
                let crop = chrome.crop_control();
                if context_tool_button(ui, crop).clicked() {
                    actions.push(UiAction::ToggleCrop);
                    close = true;
                }
            });
            if frame.dock.heal_active {
                ui.separator();
                let mut radius = frame.heal_brush_radius;
                ui.label(
                    RichText::new("Heal Brush Radius")
                        .size(11.5)
                        .color(colors.muted),
                );
                let slider = egui::Slider::new(
                    &mut radius,
                    crate::heal::MIN_BRUSH_RADIUS..=crate::heal::MAX_BRUSH_RADIUS,
                )
                .suffix(" px");
                let adjust_enabled = chrome.is_enabled(ChromeControl::HealAdjust);
                let response = ui.add_enabled(adjust_enabled, slider);
                response.widget_info(|| {
                    WidgetInfo::slider(
                        ui.is_enabled() && adjust_enabled,
                        f64::from(radius),
                        "Heal brush radius",
                    )
                });
                if response.changed() {
                    actions.push(UiAction::SetHealBrushRadius(radius));
                }
                let mut feather = frame.heal_feather_percent;
                ui.label(RichText::new("Heal Feather").size(11.5).color(colors.muted));
                let response = ui.add_enabled(
                    adjust_enabled,
                    egui::Slider::new(&mut feather, 0..=crate::heal::MAX_FEATHER_PERCENT)
                        .suffix("%"),
                );
                response.widget_info(|| {
                    WidgetInfo::slider(
                        ui.is_enabled() && adjust_enabled,
                        f64::from(feather),
                        "Heal feather",
                    )
                });
                if response.changed() {
                    actions.push(UiAction::SetHealFeather(feather));
                }
            }
            ui.separator();
            let enabled = chrome.is_enabled(ChromeControl::OpenWith);
            let open_with = ui.add_enabled(enabled, egui::Button::new("Open With..."));
            open_with
                .widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, "Open With..."));
            if open_with.on_hover_text(OPEN_WITH_HELP).clicked() {
                actions.push(UiAction::OpenWith);
                close = true;
            }
            ui.label(RichText::new(OPEN_WITH_HELP).size(11.0).color(colors.muted));
        });

    if close || (ui.ctx().input(|i| i.pointer.any_pressed()) && !ui.ctx().is_pointer_over_egui()) {
        actions.push(UiAction::CloseContextMenu);
    }
}

fn menu_tool_button(ui: &mut egui::Ui, view: ToolControlView) -> egui::Response {
    let enabled = ui.is_enabled() && view.enabled;
    let response = ui.add_enabled(
        view.enabled,
        egui::Button::new(view.label)
            .shortcut_text(view.shortcut)
            .selected(view.selected),
    );
    response.widget_info(|| {
        WidgetInfo::selected(WidgetType::Button, enabled, view.selected, view.label)
    });
    response.ctx.accesskit_node_builder(response.id, |node| {
        node.set_keyboard_shortcut(view.shortcut);
    });
    response
}

fn context_tool_button(ui: &mut egui::Ui, view: ToolControlView) -> egui::Response {
    let label = format!("{} ({})", view.label, view.shortcut);
    let enabled = ui.is_enabled() && view.enabled;
    let response = ui.add_enabled(
        view.enabled,
        egui::Button::new(&label).selected(view.selected),
    );
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::Button, enabled, view.selected, &label));
    response.ctx.accesskit_node_builder(response.id, |node| {
        node.set_keyboard_shortcut(view.shortcut);
    });
    response
}

fn apply_chrome_theme(ctx: &egui::Context, mode: crate::theme::Mode) {
    let colors = chrome_colors_for(mode);
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("viewr_chrome_colors"), colors));
    let mut visuals = if mode == crate::theme::Mode::Light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    visuals.override_text_color = Some(colors.text);
    visuals.panel_fill = colors.panel;
    visuals.window_fill = colors.panel;
    visuals.extreme_bg_color = colors.panel;
    visuals.faint_bg_color = colors.raised;
    visuals.window_stroke = Stroke::new(1.0, colors.border);
    visuals.widgets.noninteractive.fg_stroke.color = colors.text;
    visuals.widgets.inactive.fg_stroke.color = colors.text;
    visuals.widgets.hovered.fg_stroke.color = colors.text;
    visuals.widgets.hovered.bg_fill = colors.raised;
    visuals.widgets.hovered.weak_bg_fill = colors.raised;
    visuals.widgets.active.fg_stroke.color = colors.text;
    visuals.widgets.active.bg_fill = colors.active;
    visuals.widgets.active.weak_bg_fill = colors.active;
    visuals.widgets.open.fg_stroke.color = colors.text;
    visuals.widgets.open.bg_fill = colors.raised;
    visuals.widgets.open.weak_bg_fill = colors.raised;
    visuals.selection.bg_fill = with_alpha(colors.accent, 48);
    visuals.selection.stroke = Stroke::new(1.0, colors.accent);
    visuals.hyperlink_color = colors.accent;
    if ctx.style_of(ctx.theme()).visuals != visuals {
        ctx.set_visuals(visuals);
    }
    let desired_family = |text_style: &egui::TextStyle| {
        if mode == crate::theme::Mode::Console || matches!(text_style, egui::TextStyle::Monospace) {
            egui::FontFamily::Monospace
        } else {
            egui::FontFamily::Proportional
        }
    };
    let family_changed = ctx
        .style_of(ctx.theme())
        .text_styles
        .iter()
        .any(|(text_style, font)| font.family != desired_family(text_style));
    if family_changed {
        ctx.style_mut_of(ctx.theme(), |style| {
            for (text_style, font) in &mut style.text_styles {
                font.family = desired_family(text_style);
            }
        });
    }
}

fn menu_frame(colors: ChromeColors) -> Frame {
    Frame::new()
        .fill(colors.panel)
        .inner_margin(egui::Margin::symmetric(10, 4))
        .stroke(Stroke::new(1.0, colors.border))
}

fn docked_frame(colors: ChromeColors) -> Frame {
    Frame::new()
        .fill(colors.panel)
        .stroke(Stroke::new(1.0, colors.border))
        .inner_margin(4.0)
}

/// Horizontal padding on each side of a top-bar menu title.
///
/// Desktop menu bars separate titles by roughly twice this value. macOS sets
/// the gap with title padding alone and lets neighboring highlights meet, so
/// the pointer crossing an open menu bar never falls through a dead seam. viewr
/// follows that: 8 points a side, no extra spacing between titles.
const MENU_TITLE_PADDING_X: f32 = 8.0;

fn configure_top_menu_widgets(ui: &mut egui::Ui, colors: ChromeColors) {
    ui.spacing_mut().button_padding = Vec2::new(MENU_TITLE_PADDING_X, 4.0);
    ui.visuals_mut().widgets.inactive.bg_fill = Color32::TRANSPARENT;
    ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    ui.visuals_mut().widgets.hovered.bg_fill = colors.raised;
    ui.visuals_mut().widgets.hovered.weak_bg_fill = colors.raised;
    ui.visuals_mut().widgets.active.bg_fill = colors.active;
}

#[allow(
    clippy::too_many_lines,
    reason = "the menu bar keeps its ordered menus and responsive status strip together"
)]
fn render_top_menu(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    frame: &UiFrameOwned,
    chrome: ChromeViewModel,
) {
    let colors = chrome_colors(ui);
    Panel::top("top_panel")
        .exact_size(TOP_BAR_HEIGHT)
        .resizable(false)
        .frame(menu_frame(colors))
        .show(ui, |ui| {
            configure_top_menu_widgets(ui, colors);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                file_menu(ui, actions, chrome);
                edit_menu(ui, actions, frame, chrome);
                view_menu(ui, actions, frame, chrome);
                tools_menu(ui, actions, chrome);
                help_menu(ui, actions);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // The reading strip owns its own separation instead of
                    // inheriting whatever spacing the menu titles need.
                    ui.spacing_mut().item_spacing.x = TOP_METADATA_SPACING;
                    render_top_operation_status(ui, actions, frame, chrome, colors);
                    render_top_rating_position(ui, frame, colors);
                    render_top_image_facts(ui, frame, chrome, colors);
                });
            });
        });
}

fn render_top_operation_status(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    frame: &UiFrameOwned,
    chrome: ChromeViewModel,
    colors: ChromeColors,
) {
    let add_status = |ui: &mut egui::Ui, status: &str| {
        add_top_status_with_external_edit(ui, status, frame.external_edit_pending, colors);
    };
    if let Some(status) = frame.curation_status.as_deref() {
        ui.add(egui::Spinner::new().size(14.0).color(colors.accent));
        add_status(ui, status);
    } else if frame.rating.write_busy {
        ui.add(egui::Spinner::new().size(14.0).color(colors.accent));
        add_status(ui, "Saving rating...");
    } else if frame.rating.discovery_busy {
        ui.add(egui::Spinner::new().size(14.0).color(colors.accent));
        add_status(ui, "Reading folder ratings...");
    } else if frame.dock.has_image && frame.load_error.is_some() {
        ui.add_enabled_ui(chrome.is_enabled(ChromeControl::RetryLoad), |ui| {
            add_retry_button(ui, actions, frame.selected_file_name.as_deref());
        });
        if frame.save_recovery_unsettled {
            add_status(ui, SAVE_RECOVERY_STATUS);
        } else if frame.crop_recovery_unsettled {
            add_status(ui, CROP_RECOVERY_STATUS);
        } else if frame.preview_recovery_unsettled {
            add_status(ui, PREVIEW_RECOVERY_STATUS);
        } else if frame.rating.recovery_unsettled {
            add_status(ui, RATING_RECOVERY_STATUS);
        } else if let Some(status) =
            image_open_status(false, true, frame.selected_file_name.as_deref())
        {
            add_status(ui, &status);
        }
    } else if frame.dock.has_image && frame.is_opening {
        ui.add(egui::Spinner::new().size(14.0).color(colors.accent));
        if let Some(status) = image_open_status(true, false, frame.selected_file_name.as_deref()) {
            add_status(ui, &status);
        }
    } else if frame.dock.has_image && frame.is_loading {
        ui.add(egui::Spinner::new().size(14.0).color(colors.accent));
        add_status(ui, "Preparing preview...");
    } else if frame.dock.has_image && frame.save_busy {
        ui.add(egui::Spinner::new().size(14.0).color(colors.accent));
        add_status(ui, "Saving...");
    } else if frame.dock.has_image && frame.crop_busy {
        ui.add(egui::Spinner::new().size(14.0).color(colors.accent));
        add_status(ui, "Applying crop...");
    } else if frame.save_recovery_unsettled {
        add_status(ui, SAVE_RECOVERY_STATUS);
    } else if frame.crop_recovery_unsettled {
        add_status(ui, CROP_RECOVERY_STATUS);
    } else if frame.preview_recovery_unsettled {
        add_status(ui, PREVIEW_RECOVERY_STATUS);
    } else if frame.rating.recovery_unsettled {
        add_status(ui, RATING_RECOVERY_STATUS);
    } else if let Some(status) = frame.curation_recovery_status.as_deref() {
        add_status(ui, status);
    } else if frame.source_gone {
        add_top_status(ui, crate::file_coherence::current_gone_copy(), colors);
    } else if frame.external_edit_pending {
        add_top_status_with_external_edit(ui, EXTERNAL_EDIT_STANDALONE_STATUS, true, colors);
    } else if frame.rating.outside_filter {
        add_top_status(
            ui,
            "Outside current filter. Next or Previous returns to matching images.",
            colors,
        );
    } else if frame.folder_scan_busy && frame.dock.has_image {
        ui.add(egui::Spinner::new().size(14.0).color(colors.accent));
        add_status(ui, "Reading folder...");
    }
}

fn add_top_status_with_external_edit(
    ui: &mut egui::Ui,
    status: &str,
    external_edit_pending: bool,
    colors: ChromeColors,
) -> Option<(egui::Response, egui::Response)> {
    if !external_edit_pending {
        add_top_status(ui, status, colors);
        return None;
    }
    let accessible_status = if status == EXTERNAL_EDIT_STANDALONE_STATUS {
        EXTERNAL_EDIT_ACCESSIBLE_STATUS.to_owned()
    } else {
        format!("{EXTERNAL_EDIT_ACCESSIBLE_STATUS} {status}")
    };
    let max_width = top_status_max_width(ui);
    let responses = ui.scope(|ui| {
        ui.set_max_width(max_width);
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.horizontal(|ui| {
            let badge = ui
                .add(egui::Label::new(
                    RichText::new(EXTERNAL_EDIT_BADGE)
                        .size(11.5)
                        .color(colors.accent),
                ))
                .on_hover_text(EXTERNAL_EDIT_ACCESSIBLE_STATUS);
            badge.ctx.accesskit_node_builder(badge.id, |node| {
                node.set_hidden();
            });
            let response = ui
                .add(
                    egui::Label::new(RichText::new(status).size(12.5).color(colors.muted))
                        .truncate(),
                )
                .on_hover_text(&accessible_status);
            response.ctx.accesskit_node_builder(response.id, |node| {
                node.set_value(accessible_status);
                node.set_live(egui::accesskit::Live::Polite);
            });
            (badge, response)
        })
        .inner
    });
    Some(responses.inner)
}

fn render_top_rating_position(ui: &mut egui::Ui, frame: &UiFrameOwned, colors: ChromeColors) {
    let displayed_position = match frame.rating.filter {
        crate::ratings::RatingFilter::All => frame.playlist_pos,
        crate::ratings::RatingFilter::AtLeast(_) => frame.rating.visible_position,
    };
    let label = if let Some((index, total)) = displayed_position {
        Some(match frame.rating.filter {
            crate::ratings::RatingFilter::All => format!("{index} / {total}"),
            crate::ratings::RatingFilter::AtLeast(minimum) => format!(
                "{index} / {total} rated {}+ · {} total",
                minimum.get(),
                frame.rating.folder_count
            ),
        })
    } else if !matches!(frame.rating.filter, crate::ratings::RatingFilter::All)
        && frame.rating.match_count > 0
    {
        Some(format!(
            "{} matching · {} total",
            frame.rating.match_count, frame.rating.folder_count
        ))
    } else {
        None
    };
    if let Some(label) = label {
        Frame::new()
            .fill(colors.raised)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(8, 3))
            .show(ui, |ui| {
                ui.label(RichText::new(label).size(12.5).color(colors.muted));
            });
        ui.add_space(TOP_METADATA_GAP);
    }
}

fn render_top_image_facts(
    ui: &mut egui::Ui,
    frame: &UiFrameOwned,
    chrome: ChromeViewModel,
    colors: ChromeColors,
) {
    if frame.dock.has_image {
        Frame::new()
            .fill(colors.raised)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(8, 3))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(chrome.rating_menu_label())
                        .size(12.5)
                        .color(colors.muted),
                );
            });
        ui.add_space(TOP_METADATA_GAP);
    }
    if ui.ctx().content_rect().width() < 720.0 {
        return;
    }
    let mut has_detail = false;
    if frame.dock.has_image {
        ui.label(
            RichText::new(format!("{:.0}%", frame.pixel_scale * 100.0))
                .size(12.5)
                .color(colors.muted),
        );
        has_detail = true;
    }
    if let Some((width, height)) = frame.img_size {
        if has_detail {
            ui.add_space(TOP_METADATA_GAP);
        }
        ui.label(
            RichText::new(format!("{width} × {height}"))
                .size(12.5)
                .color(colors.muted),
        );
        has_detail = true;
    }
    if let Some(path) = frame.file_path.as_ref() {
        let name = crate::prefetch::privacy_safe_file_name(std::path::Path::new(path));
        if has_detail {
            ui.add_space(TOP_METADATA_GAP);
        }
        let response = ui.add(
            egui::Label::new(RichText::new(&name).size(13.5).strong().color(colors.text))
                .truncate(),
        );
        let _ = response.on_hover_text(name);
    }
}

fn file_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, chrome: ChromeViewModel) {
    let colors = chrome_colors(ui);
    ui.menu_button(RichText::new("File").size(13.5).color(colors.text), |ui| {
        ui.set_min_width(238.0);
        let open_file = ui
            .add_enabled(
                chrome.is_enabled(ChromeControl::OpenSource),
                egui::Button::new("Open File...").shortcut_text(format!("{PRIMARY_MODIFIER}+O")),
            )
            .on_hover_text(OPEN_FILE_SCOPE_HELP);
        if open_file.clicked() {
            actions.push(UiAction::Open);
            ui.close();
        }
        let open_folder = ui
            .add_enabled(
                chrome.is_enabled(ChromeControl::OpenSource),
                egui::Button::new("Open Folder...")
                    .shortcut_text(format!("{PRIMARY_MODIFIER}+Shift+O")),
            )
            .on_hover_text(OPEN_FOLDER_SCOPE_HELP);
        if open_folder.clicked() {
            actions.push(UiAction::OpenFolder);
            ui.close();
        }
        if ui
            .add_enabled(
                chrome.is_enabled(ChromeControl::Reload),
                egui::Button::new("Reload File").shortcut_text("F5"),
            )
            .clicked()
        {
            actions.push(UiAction::Reload);
            ui.close();
        }
        let open_with = ui
            .add_enabled(
                chrome.is_enabled(ChromeControl::OpenWith),
                egui::Button::new("Open With..."),
            )
            .on_hover_text(OPEN_WITH_HELP);
        if open_with.clicked() {
            actions.push(UiAction::OpenWith);
            ui.close();
        }
        if ui
            .add_enabled(
                chrome.is_enabled(ChromeControl::SaveAs),
                egui::Button::new("Save As...")
                    .shortcut_text(format!("{PRIMARY_MODIFIER}+Shift+S")),
            )
            .clicked()
        {
            actions.push(UiAction::SaveAs);
            ui.close();
        }
        ui.separator();
        if ui
            .add_enabled(
                chrome.is_enabled(ChromeControl::MoveToTrash),
                egui::Button::new("Move to Trash").shortcut_text("Delete"),
            )
            .clicked()
        {
            actions.push(UiAction::Trash);
            ui.close();
        }
        if ui
            .add_enabled(
                chrome.is_enabled(ChromeControl::PermanentDelete),
                egui::Button::new("Permanently Delete...").shortcut_text("Shift+Delete"),
            )
            .clicked()
        {
            actions.push(UiAction::PermanentDelete);
            ui.close();
        }
        undo_trash_menu_item(ui, actions, chrome);
    });
}

fn undo_trash_menu_item(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, chrome: ChromeViewModel) {
    let view = chrome.undo_trash();
    let response = ui.add_enabled(
        view.enabled,
        egui::Button::new(view.label).shortcut_text(view.shortcut),
    );
    response.widget_info(|| {
        WidgetInfo::labeled(WidgetType::Button, view.enabled, &view.accessibility_label)
    });
    if response.on_hover_text(view.help).clicked() {
        actions.push(UiAction::UndoTrash);
        ui.close();
    }
}

fn edit_menu(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    frame: &UiFrameOwned,
    chrome: ChromeViewModel,
) {
    let colors = chrome_colors(ui);
    ui.menu_button(RichText::new("Edit").size(13.5).color(colors.text), |ui| {
        ui.set_min_width(210.0);
        let crop = chrome.crop_control();
        if menu_tool_button(ui, crop).clicked() {
            actions.push(UiAction::ToggleCrop);
            ui.close();
        }
        if frame.is_cropping
            && ui
                .add_enabled(
                    chrome.is_enabled(ChromeControl::ApplyCrop),
                    egui::Button::new("Apply Crop").shortcut_text("Enter"),
                )
                .clicked()
        {
            actions.push(UiAction::ApplyCrop);
            ui.close();
        }
        spot_heal_menu_items(ui, actions, chrome);
        ui.separator();
        if ui
            .add_enabled(
                chrome.is_enabled(ChromeControl::EditTransform),
                egui::Button::new("Rotate Clockwise").shortcut_text("R"),
            )
            .clicked()
        {
            actions.push(UiAction::RotateCw);
            ui.close();
        }
        if ui
            .add_enabled(
                chrome.is_enabled(ChromeControl::EditTransform),
                egui::Button::new("Rotate Counterclockwise").shortcut_text("L"),
            )
            .clicked()
        {
            actions.push(UiAction::RotateCcw);
            ui.close();
        }
        if ui
            .add_enabled(
                chrome.is_enabled(ChromeControl::EditTransform),
                egui::Button::new("Flip Horizontally").shortcut_text("H"),
            )
            .clicked()
        {
            actions.push(UiAction::FlipH);
            ui.close();
        }
        if ui
            .add_enabled(
                chrome.is_enabled(ChromeControl::EditTransform),
                egui::Button::new("Flip Vertically").shortcut_text("V"),
            )
            .clicked()
        {
            actions.push(UiAction::FlipV);
            ui.close();
        }
        ui.separator();
        let label = chrome.rating_menu_label();
        ui.add_enabled_ui(chrome.is_enabled(ChromeControl::RatingMenu), |ui| {
            ui.menu_button(label, |ui| rating_menu(ui, actions, chrome));
        });
    });
}

fn rating_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, chrome: ChromeViewModel) {
    ui.set_min_width(220.0);
    let choices = chrome.rating_choices();
    for choice in choices {
        let response = ui.add_enabled(
            choice.enabled,
            egui::RadioButton::new(
                choice.selected,
                format!("{}    {}", choice.label, choice.shortcut),
            ),
        );
        response.widget_info(|| {
            WidgetInfo::selected(
                WidgetType::RadioButton,
                choice.enabled,
                choice.selected,
                &choice.accessibility_label,
            )
        });
        if response.clicked() {
            actions.push(UiAction::AssignRating(choice.assignment));
            ui.close();
        }
    }
    if !chrome.is_enabled(ChromeControl::RatingChoice) {
        ui.separator();
        ui.label(
            RichText::new(chrome.rating_unavailable_text())
                .size(11.0)
                .color(chrome_colors(ui).muted),
        );
    }
}

fn tools_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, chrome: ChromeViewModel) {
    let colors = chrome_colors(ui);
    ui.menu_button(RichText::new("Tools").size(13.5).color(colors.text), |ui| {
        ui.set_min_width(210.0);
        let crop = chrome.crop_control();
        if menu_tool_button(ui, crop).clicked() {
            actions.push(UiAction::ToggleCrop);
            ui.close();
        }

        let heal = chrome.heal_control();
        if menu_tool_button(ui, heal).clicked() {
            actions.push(UiAction::ToggleHeal);
            ui.close();
        }
    });
}

fn spot_heal_menu_items(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, chrome: ChromeViewModel) {
    let heal = chrome.heal_control();
    if menu_tool_button(ui, heal).clicked() {
        actions.push(UiAction::ToggleHeal);
        ui.close();
    }
    ui.separator();
    if ui
        .add_enabled(
            chrome.is_enabled(ChromeControl::UndoEdit),
            egui::Button::new("Undo Spot Heal").shortcut_text(format!("{PRIMARY_MODIFIER}+Z")),
        )
        .clicked()
    {
        actions.push(UiAction::UndoEdit);
        ui.close();
    }
    if ui
        .add_enabled(
            chrome.is_enabled(ChromeControl::RedoEdit),
            egui::Button::new("Redo Spot Heal")
                .shortcut_text(format!("{PRIMARY_MODIFIER}+Shift+Z")),
        )
        .clicked()
    {
        actions.push(UiAction::RedoEdit);
        ui.close();
    }
}

fn add_top_status(ui: &mut egui::Ui, status: &str, colors: ChromeColors) {
    let max_width = top_status_max_width(ui);
    ui.scope(|ui| {
        ui.set_max_width(max_width);
        let response = ui.add(
            egui::Label::new(RichText::new(status).size(12.5).color(colors.muted))
                .truncate()
                .show_tooltip_when_elided(true),
        );
        mark_as_polite_status(&response);
    });
}

fn top_status_max_width(ui: &egui::Ui) -> f32 {
    let responsive_limit = if ui.ctx().content_rect().width() < 720.0 {
        TOP_STATUS_COMPACT_MAX_WIDTH
    } else {
        TOP_STATUS_MAX_WIDTH
    };
    ui.available_width().min(responsive_limit)
}

fn mark_as_polite_status(response: &egui::Response) {
    response.ctx.accesskit_node_builder(response.id, |node| {
        node.set_live(egui::accesskit::Live::Polite);
    });
}

fn image_open_status(
    is_opening: bool,
    has_error: bool,
    selected_file_name: Option<&str>,
) -> Option<String> {
    let subject = selected_file_name.unwrap_or("image");
    if has_error {
        Some(format!("Could not open {subject}"))
    } else if is_opening {
        Some(format!("Opening {subject}"))
    } else {
        None
    }
}

fn retry_open_label(selected_file_name: Option<&str>) -> String {
    format!("Retry opening {}", selected_file_name.unwrap_or("image"))
}

fn add_retry_button(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    selected_file_name: Option<&str>,
) {
    let label = retry_open_label(selected_file_name);
    let response = ui
        .add(egui::Button::new("Retry").min_size(Vec2::new(58.0, 30.0)))
        .on_hover_text(&label);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), &label));
    if response.clicked() {
        actions.push(UiAction::RetryLoad);
    }
}

fn add_empty_retry_button(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    selected_file_name: Option<&str>,
    colors: ChromeColors,
) {
    let label = retry_open_label(selected_file_name);
    let response = ui
        .add(
            egui::Button::new(RichText::new("Retry").color(colors.accent_ink))
                .fill(colors.accent)
                .min_size(Vec2::new(92.0, 36.0)),
        )
        .on_hover_text(&label);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), &label));
    if response.clicked() {
        actions.push(UiAction::RetryLoad);
    }
}

fn view_menu(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    frame: &UiFrameOwned,
    chrome: ChromeViewModel,
) {
    let colors = chrome_colors(ui);
    ui.menu_button(RichText::new("View").size(13.5).color(colors.text), |ui| {
        ui.set_min_width(228.0);
        view_zoom_menu(ui, actions, chrome);
        view_sequence_menu(ui, actions, frame);
        ui.separator();
        let rating_filter_label = chrome.rating_filter_menu_label();
        ui.add_enabled_ui(chrome.is_enabled(ChromeControl::RatingFilterMenu), |ui| {
            ui.menu_button(rating_filter_label, |ui| {
                rating_filter_menu(ui, actions, chrome);
            });
        });
        ui.separator();
        view_fullscreen_menu(ui, actions, frame);
        ui.separator();
        ui.menu_button("Panels", |ui| panels_menu(ui, actions, chrome));
        ui.menu_button("Panel Position", |ui| {
            panel_position_menu(ui, actions, chrome);
        });
        ui.separator();
        ui.menu_button("Image Background", |ui| {
            background_menu(ui, actions, frame.background_override);
        });
        ui.menu_button(
            crate::chrome::appearance_menu_label(frame.theme_preference),
            |ui| {
                appearance_menu(ui, actions, frame.theme_preference, frame.theme_mode);
            },
        );
    });
}

fn view_zoom_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, chrome: ChromeViewModel) {
    let enabled = chrome.is_enabled(ChromeControl::ViewImage);
    if ui
        .add_enabled(
            enabled,
            egui::Button::new("Fit Image to View").shortcut_text(format!("{PRIMARY_MODIFIER}+0")),
        )
        .clicked()
    {
        actions.push(UiAction::FitToView);
        ui.close();
    }
    if ui
        .add_enabled(
            enabled,
            egui::Button::new("Actual Size").shortcut_text(format!("{PRIMARY_MODIFIER}+1")),
        )
        .clicked()
    {
        actions.push(UiAction::ActualSize);
        ui.close();
    }
    if ui
        .add_enabled(enabled, egui::Button::new("Zoom In").shortcut_text("+"))
        .clicked()
    {
        actions.push(UiAction::ZoomIn);
        ui.close();
    }
    if ui
        .add_enabled(enabled, egui::Button::new("Zoom Out").shortcut_text("-"))
        .clicked()
    {
        actions.push(UiAction::ZoomOut);
        ui.close();
    }
}

fn view_sequence_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    let Some((previous, next, can_previous, can_next)) = sequence_menu_items(frame) else {
        return;
    };
    ui.separator();
    if let Some(animation) = frame.animation {
        let label = if animation.is_playing {
            "Pause Animation"
        } else {
            "Play Animation"
        };
        if ui.add(egui::Button::new(label)).clicked() {
            actions.push(UiAction::ToggleAnimationPlayback);
            ui.close();
        }
    }
    if ui
        .add_enabled(can_previous, egui::Button::new(previous).shortcut_text("["))
        .clicked()
    {
        actions.push(UiAction::StepSequence(-1));
        ui.close();
    }
    if ui
        .add_enabled(can_next, egui::Button::new(next).shortcut_text("]"))
        .clicked()
    {
        actions.push(UiAction::StepSequence(1));
        ui.close();
    }
}

fn view_fullscreen_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    let fullscreen_label = if frame.dock.immersive {
        "Exit Fullscreen"
    } else {
        "Fullscreen"
    };
    if ui
        .add(
            egui::Button::new(fullscreen_label)
                .shortcut_text("F / F11")
                .selected(frame.dock.immersive),
        )
        .clicked()
    {
        actions.push(UiAction::ToggleFullscreen);
        ui.close();
    }
}

fn rating_filter_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, chrome: ChromeViewModel) {
    ui.set_min_width(220.0);
    for choice in chrome.rating_filter_choices() {
        let response = ui.radio(choice.selected, &choice.label);
        response.widget_info(|| {
            WidgetInfo::selected(
                WidgetType::RadioButton,
                ui.is_enabled(),
                choice.selected,
                &choice.accessibility_label,
            )
        });
        if response.clicked() {
            actions.push(UiAction::SetRatingFilter(choice.filter));
            ui.close();
        }
    }
}

fn panels_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, chrome: ChromeViewModel) {
    ui.set_min_width(224.0);
    for choice in chrome.dock.panel_toggles() {
        let response = ui
            .add_enabled(
                choice.enabled,
                egui::Button::new(choice.label)
                    .shortcut_text(choice.shortcut)
                    .selected(choice.selected)
                    .min_size(Vec2::new(ui.available_width(), 0.0)),
            )
            .on_hover_text(format!("Toggle {} ({})", choice.label, choice.shortcut));
        response.ctx.accesskit_node_builder(response.id, |node| {
            node.set_keyboard_shortcut(choice.shortcut);
        });
        if response.clicked() {
            actions.push(match choice.kind {
                PanelKind::Tools => UiAction::ToggleToolsPanelVisibility,
                PanelKind::Filmstrip => UiAction::ToggleFilmstripPanelVisibility,
                PanelKind::ImageInfo => UiAction::ToggleImageInfo,
            });
            ui.close();
        }
    }
}

fn panel_position_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, chrome: ChromeViewModel) {
    ui.set_min_width(224.0);
    render_dock_side_choices(
        ui,
        actions,
        "TOOLS",
        PositionedPanel::Tools,
        chrome.dock.tools.side,
        UiAction::SetToolsPanelSide,
    );
    ui.separator();
    render_dock_side_choices(
        ui,
        actions,
        "IMAGE INFORMATION",
        PositionedPanel::ImageInfo,
        chrome.dock.image_info_side,
        UiAction::SetImageInfoSide,
    );
}

fn render_dock_side_choices(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    heading: &str,
    panel: PositionedPanel,
    current: DockSide,
    action: fn(DockSide) -> UiAction,
) {
    let colors = chrome_colors(ui);
    ui.label(
        RichText::new(heading)
            .size(10.0)
            .color(colors.muted)
            .strong(),
    );
    for choice in crate::chrome::dock_side_choices(panel, current) {
        let response = ui.radio(choice.selected, choice.label);
        response.widget_info(|| {
            WidgetInfo::selected(
                WidgetType::RadioButton,
                ui.is_enabled(),
                choice.selected,
                &choice.accessibility_label,
            )
        });
        if response.clicked() {
            actions.push(action(choice.side));
            ui.close();
        }
    }
}

fn background_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, current: Option<[f64; 4]>) {
    ui.set_min_width(172.0);
    for choice in crate::chrome::background_choices(current) {
        if ui.radio(choice.selected, choice.label).clicked() {
            actions.push(UiAction::SetBackground(choice.value));
            ui.close();
        }
    }
}

fn appearance_menu(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    current: crate::theme::Preference,
    resolved: crate::theme::Mode,
) {
    const MENU_WIDTH: f32 = 320.0;
    const TEXT_WIDTH: f32 = 282.0;
    let colors = chrome_colors(ui);
    ui.set_width(MENU_WIDTH);
    ui.add(
        egui::Label::new(
            RichText::new(APPEARANCE_SCOPE_HELP)
                .size(11.5)
                .color(colors.muted),
        )
        .wrap(),
    );
    ui.separator();
    for choice in crate::chrome::appearance_choices(current, resolved) {
        let mut label = LayoutJob::default();
        label.wrap.max_width = TEXT_WIDTH;
        label.append(
            choice.label,
            0.0,
            TextFormat {
                font_id: FontId::proportional(13.0),
                color: colors.text,
                ..TextFormat::default()
            },
        );
        label.append(
            "\n",
            0.0,
            TextFormat {
                font_id: FontId::proportional(11.5),
                color: colors.muted,
                ..TextFormat::default()
            },
        );
        label.append(
            &choice.description,
            0.0,
            TextFormat {
                font_id: FontId::proportional(11.5),
                color: colors.muted,
                ..TextFormat::default()
            },
        );
        let response = ui.add(egui::RadioButton::new(choice.selected, label));
        response.widget_info(|| {
            WidgetInfo::selected(
                WidgetType::RadioButton,
                ui.is_enabled(),
                choice.selected,
                &choice.accessibility_label,
            )
        });
        if response.clicked() {
            actions.push(UiAction::SetTheme(choice.preference));
            ui.close();
        }
    }
}

fn help_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>) {
    let colors = chrome_colors(ui);
    ui.menu_button(RichText::new("Help").size(13.5).color(colors.text), |ui| {
        ui.set_min_width(180.0);
        if ui
            .button("Get latest release...")
            .on_hover_text("Open the latest official GitHub release. No background check.")
            .clicked()
        {
            actions.push(UiAction::ShowUpdate);
            ui.close();
        }
        ui.separator();
        if ui.button("About viewr").clicked() {
            actions.push(UiAction::ShowAbout);
            ui.close();
        }
    });
}

fn render_about(ui: &mut egui::Ui, actions: &mut Vec<UiAction>) {
    let colors = chrome_colors(ui);
    let mut close_clicked = false;
    let response = egui::Modal::new(egui::Id::new("about_viewr"))
        .backdrop_color(Color32::from_black_alpha(140))
        .frame(
            Frame::new()
                .fill(colors.panel)
                .stroke(Stroke::new(1.0, colors.border))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(egui::Margin::same(20)),
        )
        .show(ui.ctx(), |ui| {
            ui.set_max_width(520.0);
            let body_height = (ui.ctx().content_rect().height() - 80.0).clamp(180.0, 640.0);
            ScrollArea::vertical()
                .id_salt("about_body")
                .max_height(body_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.heading(RichText::new("About viewr").size(22.0).color(colors.text));
                        ui.label(
                            RichText::new("A private, local-first image viewer")
                                .size(13.0)
                                .color(colors.muted),
                        );
                    });
                    ui.add_space(8.0);
                    Frame::new()
                        .fill(colors.raised)
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("No network access")
                                    .color(colors.text)
                                    .strong(),
                            );
                            ui.label("No telemetry, accounts, cloud sync, or background indexing.");
                            ui.label(
                                "Photos and edits stay local unless you explicitly save a copy.",
                            );
                        });
                    ui.add_space(8.0);
                    egui::Grid::new("about_build_details")
                        .num_columns(2)
                        .spacing(Vec2::new(16.0, 4.0))
                        .show(ui, |ui| {
                            ui.label(RichText::new("Version").color(colors.muted));
                            ui.label(env!("CARGO_PKG_VERSION"));
                            ui.end_row();
                            ui.label(RichText::new("Platform").color(colors.muted));
                            ui.label(format!(
                                "{} / {}",
                                std::env::consts::OS,
                                std::env::consts::ARCH
                            ));
                            ui.end_row();
                            ui.label(RichText::new("License").color(colors.muted));
                            ui.label(env!("CARGO_PKG_LICENSE"));
                            ui.end_row();
                        });
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("Shortcuts")
                            .color(colors.muted)
                            .small()
                            .strong(),
                    );
                    ui.add_space(4.0);
                    render_about_shortcut_groups(ui, colors);
                });
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    close_clicked = true;
                }
            });
        });
    response.response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Window,
            true,
            "About viewr. Private local-first image viewer. No network access, telemetry, accounts, or background indexing.",
        )
    });
    if close_clicked || response.should_close() {
        actions.push(UiAction::CloseAbout);
    }
}

fn render_about_shortcut_groups(ui: &mut egui::Ui, colors: ChromeColors) {
    egui::Grid::new("about_shortcuts")
        .num_columns(2)
        .spacing(Vec2::new(20.0, 8.0))
        .show(ui, |ui| {
            for (index, group) in crate::shortcuts::ABOUT_SHORTCUT_GROUPS.iter().enumerate() {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(group.heading)
                            .size(12.0)
                            .color(colors.text)
                            .strong(),
                    );
                    for item in group.items {
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!(
                                    "{}  {}",
                                    crate::shortcuts::format_shortcut_keys(
                                        item.keys,
                                        PRIMARY_MODIFIER
                                    ),
                                    item.action
                                ))
                                .size(12.0)
                                .color(colors.muted),
                            )
                            .extend(),
                        );
                    }
                });
                if index % 2 == 1 {
                    ui.end_row();
                }
            }
        });
}

fn render_update(ui: &mut egui::Ui, actions: &mut Vec<UiAction>) {
    let colors = chrome_colors(ui);
    let mut close_clicked = false;
    let response = egui::Modal::new(egui::Id::new("update_viewr"))
        .backdrop_color(Color32::from_black_alpha(140))
        .frame(
            Frame::new()
                .fill(colors.panel)
                .stroke(Stroke::new(1.0, colors.border))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(egui::Margin::same(20)),
        )
        .show(ui.ctx(), |ui| {
            ui.set_max_width(560.0);
            ui.vertical(|ui| {
                let body_height = (ui.ctx().content_rect().height() - 80.0).clamp(180.0, 640.0);
                let mut handoff = false;
                ScrollArea::vertical()
                    .id_salt("update_body")
                    .max_height(body_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.heading(RichText::new("Update viewr").size(22.0).color(colors.text));
                        ui.label(
                            RichText::new(format!(
                                "Current version: {}",
                                env!("CARGO_PKG_VERSION")
                            ))
                            .size(13.0)
                            .color(colors.muted),
                        );
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new(
                                "viewr never checks for or downloads updates by itself.",
                            )
                            .size(13.0)
                            .color(colors.text),
                        );
                        ui.label(
                            RichText::new(
                                "Updates are explicit and come from the official GitHub release.",
                            )
                            .size(13.0)
                            .color(colors.muted),
                        );
                        ui.label(
                            RichText::new(
                                "Open the latest stable release in your browser, review its version and checksums, then close viewr before installing it.",
                            )
                            .size(13.0)
                            .color(colors.text),
                        );
                        ui.add_space(10.0);
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Get latest release")
                                        .strong()
                                        .color(colors.accent_ink),
                                )
                                .fill(colors.accent)
                                .min_size(Vec2::new(180.0, 36.0)),
                            )
                            .on_hover_text(crate::cli::OFFICIAL_LATEST_RELEASE_URL)
                            .clicked()
                        {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(
                                crate::cli::OFFICIAL_LATEST_RELEASE_URL,
                            ));
                            handoff = true;
                        }
                        ui.label(
                            RichText::new(
                                "This hands off only the release URL to your default browser. viewr itself does not connect to GitHub or run an updater.",
                            )
                            .size(12.0)
                            .color(colors.muted),
                        );
                    });
                if handoff {
                    close_clicked = true;
                }
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        close_clicked = true;
                    }
                });
            });
        });
    response.response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Window,
            true,
            "Update viewr. One explicit action opens the latest official GitHub release. No automatic network check or background updater.",
        )
    });
    if close_clicked || response.should_close() {
        actions.push(UiAction::CloseUpdate);
    }
}

fn render_save_overwrite_confirmation(ui: &mut egui::Ui, actions: &mut Vec<UiAction>) {
    let colors = chrome_colors(ui);
    let mut confirm_clicked = false;
    let mut cancel_clicked = false;
    let focus_state_id = egui::Id::new(SAVE_OVERWRITE_FOCUS_STATE);
    let focus_cancel = ui.ctx().data_mut(|data| {
        if data.get_temp::<bool>(focus_state_id).unwrap_or(false) {
            false
        } else {
            data.insert_temp(focus_state_id, true);
            true
        }
    });
    let response = egui::Modal::new(egui::Id::new("save_overwrite_confirmation"))
        .backdrop_color(Color32::from_black_alpha(140))
        .frame(
            Frame::new()
                .fill(colors.panel)
                .stroke(Stroke::new(1.0, colors.border))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(egui::Margin::same(20)),
        )
        .show(ui.ctx(), |ui| {
            ui.set_max_width(430.0);
            ui.heading(
                RichText::new("Replace existing file?")
                    .size(25.0)
                    .color(colors.text),
            );
            ui.add_space(10.0);
            ui.label(
                RichText::new(
                    "The selected Save As destination exists. Replace that exact file with this exported copy?",
                )
                .size(13.5)
                .color(colors.text),
            );
            ui.label(
                RichText::new(
                    "viewr rechecks this exact file immediately before replacement and stops if that check detects a change.",
                )
                .size(12.5)
                .color(colors.muted),
            );
            ui.add_space(14.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Replace file").clicked() {
                    confirm_clicked = true;
                }
                let cancel = ui.button("Cancel");
                if focus_cancel {
                    cancel.request_focus();
                }
                if cancel.clicked() {
                    cancel_clicked = true;
                }
            });
        });
    response.response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Window,
            true,
            "Replace existing file? The selected Save As destination exists. Confirm replacement or cancel without changing it.",
        )
    });
    if confirm_clicked {
        actions.push(UiAction::ConfirmSaveOverwrite);
    } else if cancel_clicked || response.should_close() {
        actions.push(UiAction::CancelSaveOverwrite);
    }
}

fn render_rating_disclosure(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    let Some(assignment) = frame.rating.pending_disclosure else {
        return;
    };
    let colors = chrome_colors(ui);
    let (title, confirm) = match assignment {
        crate::ratings::RatingAssignment::Clear => {
            ("Clear this rating?".to_owned(), "Clear rating")
        }
        crate::ratings::RatingAssignment::Set(rating) => {
            (format!("Save rating {} of 5?", rating.get()), "Save rating")
        }
    };
    let mut confirm_clicked = false;
    let mut cancel_clicked = false;
    let focus_state_id = egui::Id::new(RATING_DISCLOSURE_FOCUS_STATE);
    let focus_cancel = ui.ctx().data_mut(|data| {
        if data.get_temp::<bool>(focus_state_id).unwrap_or(false) {
            false
        } else {
            data.insert_temp(focus_state_id, true);
            true
        }
    });
    let response = egui::Modal::new(egui::Id::new("rating_write_disclosure"))
        .backdrop_color(Color32::from_black_alpha(140))
        .frame(
            Frame::new()
                .fill(colors.panel)
                .stroke(Stroke::new(1.0, colors.border))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(egui::Margin::same(20)),
        )
        .show(ui.ctx(), |ui| {
            ui.set_max_width(430.0);
            ui.heading(RichText::new(&title).size(25.0).color(colors.text));
            ui.add_space(10.0);
            ui.label(
                RichText::new(
                    "Ratings are written into this image file and may be visible to other apps.",
                )
                .size(13.5)
                .color(colors.text),
            );
            ui.label(
                RichText::new(
                    "viewr updates embedded metadata in the source JPEG. It does not create a database or sidecar.",
                )
                .size(12.5)
                .color(colors.muted),
            );
            ui.add_space(14.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(confirm).clicked() {
                    confirm_clicked = true;
                }
                let cancel = ui.button("Cancel");
                if focus_cancel {
                    cancel.request_focus();
                }
                if cancel.clicked() {
                    cancel_clicked = true;
                }
            });
        });
    response.response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Window,
            true,
            format!(
                "{title}. Ratings are written into this image file and may be visible to other apps."
            ),
        )
    });
    if confirm_clicked {
        actions.push(UiAction::ConfirmRatingDisclosure);
    } else if cancel_clicked || response.should_close() {
        actions.push(UiAction::CancelRatingDisclosure);
    }
}

fn rating_filter_is_empty(frame: &UiFrameOwned) -> bool {
    !frame.rating.discovery_busy
        && !frame.rating.outside_filter
        && !matches!(frame.rating.filter, crate::ratings::RatingFilter::All)
        && frame.rating.match_count == 0
}

fn render_filtered_empty_state(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    frame: &UiFrameOwned,
) {
    let crate::ratings::RatingFilter::AtLeast(minimum) = frame.rating.filter else {
        return;
    };
    let colors = chrome_colors(ui);
    let screen = ui.ctx().content_rect();
    let card_width = (screen.width() - 40.0).clamp(280.0, 430.0);
    Area::new("rating_filter_empty_state".into())
        .fixed_pos(screen.center())
        .pivot(Align2::CENTER_CENTER)
        .constrain_to(screen)
        .movable(false)
        .fade_in(false)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            ui.set_width(card_width);
            let card = Frame::new()
                .fill(colors.panel)
                .corner_radius(CornerRadius::same(12))
                .stroke(Stroke::new(1.0, colors.border))
                .inner_margin(egui::Margin::symmetric(28, 24))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        let heading = format!("No images are rated {} or higher.", minimum.get());
                        let heading_response =
                            ui.heading(RichText::new(&heading).size(20.0).color(colors.text));
                        heading_response
                            .widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &heading));
                        mark_as_polite_status(&heading_response);
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(format!(
                                "{} images remain loaded in this folder.",
                                frame.rating.folder_count
                            ))
                            .size(13.0)
                            .color(colors.muted),
                        );
                        ui.add_space(16.0);
                        let show_all = ui
                            .add(egui::Button::new("Show all images").shortcut_text("Esc"))
                            .on_hover_text("Esc or Left/Right also shows all images");
                        show_all.widget_info(|| {
                            WidgetInfo::labeled(WidgetType::Button, true, "Show all images")
                        });
                        show_all.ctx.accesskit_node_builder(show_all.id, |node| {
                            node.set_keyboard_shortcut("Esc");
                        });
                        if show_all.clicked() {
                            actions.push(UiAction::ShowAllRatings);
                        }
                    });
                });
            card.response.widget_info(|| {
                WidgetInfo::labeled(WidgetType::Panel, true, "No images match rating filter")
            });
        });
}

fn render_empty_state(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    frame: &UiFrameOwned,
    chrome: ChromeViewModel,
) {
    let is_opening = frame.is_opening;
    let load_error = frame.load_error.as_deref();
    let selected_file_name = frame.selected_file_name.as_deref();
    let colors = chrome_colors(ui);
    let screen = ui.ctx().content_rect();
    let card_width = (screen.width() - 40.0).clamp(280.0, 420.0);
    let minimum_card_top = screen.top() + 20.0;
    let maximum_card_top =
        (screen.bottom() - 20.0 - EMPTY_STATE_EXPECTED_HEIGHT).max(minimum_card_top);
    let card_position = Pos2::new(
        screen.center().x - card_width * 0.5,
        (screen.center().y - EMPTY_STATE_EXPECTED_HEIGHT * 0.5)
            .clamp(minimum_card_top, maximum_card_top),
    );
    let copy = crate::shortcuts::empty_state_copy(is_opening, load_error, selected_file_name);
    Area::new("empty_state".into())
        .fixed_pos(card_position)
        .constrain_to(screen)
        .movable(false)
        .fade_in(false)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            ui.set_width(card_width);
            let card = Frame::new()
                .fill(colors.panel)
                .corner_radius(CornerRadius::same(12))
                .stroke(Stroke::new(1.0, colors.border))
                .inner_margin(egui::Margin::symmetric(28, 24))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        if is_opening {
                            ui.add(egui::Spinner::new().size(28.0).color(colors.accent));
                        } else {
                            let (icon_rect, _) =
                                ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
                            paint_empty_image_icon(ui.painter(), icon_rect, colors);
                        }
                        ui.add_space(12.0);
                        let heading_response = ui.add(
                            egui::Label::new(
                                RichText::new(&copy.heading)
                                    .size(20.0)
                                    .color(colors.text)
                                    .strong(),
                            )
                            .truncate()
                            .show_tooltip_when_elided(true),
                        );
                        if is_opening || load_error.is_some() {
                            mark_as_polite_status(&heading_response);
                        }
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(&copy.description)
                                .size(13.0)
                                .color(colors.muted),
                        );
                        if !is_opening {
                            render_empty_state_actions(ui, actions, frame, chrome, colors);
                        }
                        ui.label(
                            RichText::new(LOCAL_PRIVACY_SUMMARY)
                                .size(12.0)
                                .color(colors.muted),
                        );
                    });
                });
            card.response.widget_info(|| {
                WidgetInfo::labeled(WidgetType::Panel, true, copy.heading.as_str())
            });
        });
}

fn render_empty_state_actions(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    frame: &UiFrameOwned,
    chrome: ChromeViewModel,
    colors: ChromeColors,
) {
    let load_error = frame.load_error.as_deref();
    let selected_file_name = frame.selected_file_name.as_deref();
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        if load_error.is_some() {
            ui.add_enabled_ui(
                chrome.is_enabled(ChromeControl::OpenSource)
                    && chrome.is_enabled(ChromeControl::RetryLoad),
                |ui| {
                    add_empty_retry_button(ui, actions, selected_file_name, colors);
                },
            );
        }
        let open_file = ui
            .add_enabled(
                chrome.is_enabled(ChromeControl::OpenSource),
                egui::Button::new("Open File").min_size(Vec2::new(116.0, 36.0)),
            )
            .on_hover_text(OPEN_FILE_SCOPE_HELP);
        if open_file.clicked() {
            actions.push(UiAction::Open);
        }
        let open_folder = ui
            .add_enabled(
                chrome.is_enabled(ChromeControl::OpenSource),
                egui::Button::new("Open Folder").min_size(Vec2::new(116.0, 36.0)),
            )
            .on_hover_text(OPEN_FOLDER_SCOPE_HELP);
        if open_folder.clicked() {
            actions.push(UiAction::OpenFolder);
        }
    });
    ui.add_space(12.0);
}

fn paint_empty_image_icon(painter: &egui::Painter, rect: Rect, colors: ChromeColors) {
    let frame = rect.shrink(3.0);
    painter.rect_filled(frame, CornerRadius::same(8), colors.raised);
    painter.rect_stroke(
        frame,
        CornerRadius::same(8),
        Stroke::new(1.5, colors.muted),
        egui::StrokeKind::Inside,
    );
    let mountain = [
        frame.left_bottom() + Vec2::new(7.0, -8.0),
        frame.center() + Vec2::new(-3.0, 2.0),
        frame.center() + Vec2::new(3.0, -3.0),
        frame.right_bottom() + Vec2::new(-6.0, -8.0),
    ];
    painter.line(mountain.to_vec(), Stroke::new(1.5, colors.muted));
    painter.circle_filled(frame.right_top() + Vec2::new(-9.0, 9.0), 3.0, colors.accent);
}

fn render_image_info_panel(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    frame: &UiFrameOwned,
    side: DockSide,
) {
    let colors = chrome_colors(ui);
    let panel = match side {
        DockSide::Left => Panel::left("image_info_panel"),
        DockSide::Right => Panel::right("image_info_panel"),
    };
    panel
        .exact_size(IMAGE_INFO_PANEL_WIDTH)
        .resizable(false)
        .frame(docked_frame(colors).inner_margin(16.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(RichText::new("Image Information").color(colors.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new("Close").min_size(Vec2::new(52.0, 36.0)))
                        .on_hover_text("Close panel (I)")
                        .clicked()
                    {
                        actions.push(UiAction::ToggleImageInfo);
                    }
                });
            });
            ScrollArea::vertical()
                .id_salt("image_info_body")
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    render_file_info(ui, actions, frame);
                    render_capture_info(ui, frame);
                    render_source_privacy(ui, frame);
                    render_export_privacy(ui, actions, frame);
                });
        });
}

fn render_file_info(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    let colors = chrome_colors(ui);
    ui.separator();
    ui.label(RichText::new("File").color(colors.muted).small().strong());
    ui.add_space(4.0);
    if let Some(path) = &frame.file_path {
        let name = crate::prefetch::privacy_safe_file_name(std::path::Path::new(path));
        ui.label(RichText::new(name).color(colors.text));
    }
    if let Some((width, height)) = frame.img_size {
        ui.label(RichText::new(format!("{width} × {height}")).color(colors.muted));
        let megapixels = f64::from(width) * f64::from(height) / 1_000_000.0;
        if let Some(aspect) = crate::image_info::aspect_ratio(width, height) {
            ui.label(RichText::new(format!("{megapixels:.1} MP · {aspect}")).color(colors.muted));
        }
    }
    if let Some(value) = frame.details.as_ref().and_then(format_and_size) {
        ui.label(RichText::new(value).color(colors.muted));
    }
    if let Some(color_profile) = frame.color_profile {
        ui.label(RichText::new(format!("Color · {}", color_profile.label())).color(colors.muted));
    }
    ui.label(
        RichText::new(format!("Display · {}", frame.display_output.label())).color(colors.muted),
    );
    render_animation_controls(ui, actions, frame, colors);
    render_page_controls(ui, actions, frame, colors);
}

fn render_animation_controls(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    frame: &UiFrameOwned,
    colors: ChromeColors,
) {
    let Some(animation) = frame.animation else {
        return;
    };
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let (label, tooltip) = if animation.is_playing {
            ("Pause", "Pause animation")
        } else {
            ("Play", "Play animation")
        };
        if ui
            .add(egui::Button::new(label).min_size(Vec2::new(64.0, 36.0)))
            .on_hover_text(tooltip)
            .clicked()
        {
            actions.push(UiAction::ToggleAnimationPlayback);
        }
        render_sequence_step_button(
            ui,
            actions,
            "Previous",
            "Previous frame",
            "[",
            animation.can_previous,
            -1,
        );
        render_sequence_step_button(
            ui,
            actions,
            "Next",
            "Next frame",
            "]",
            animation.can_next,
            1,
        );
        ui.label(
            RichText::new(format!(
                "Frame {} of {}",
                animation.frame_index.saturating_add(1),
                animation.frame_count
            ))
            .color(colors.muted),
        );
    });
}

fn render_page_controls(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    frame: &UiFrameOwned,
    colors: ChromeColors,
) {
    let Some(pages) = frame.pages.as_ref() else {
        return;
    };
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        render_sequence_step_button(
            ui,
            actions,
            "Previous",
            &format!("Previous {}", pages.noun.to_ascii_lowercase()),
            "[",
            pages.can_previous,
            -1,
        );
        render_sequence_step_button(
            ui,
            actions,
            "Next",
            &format!("Next {}", pages.noun.to_ascii_lowercase()),
            "]",
            pages.can_next,
            1,
        );
        let label = pages.visible_label.clone();
        let response = ui.label(RichText::new(&label).color(colors.muted));
        response.widget_info(|| {
            WidgetInfo::labeled(
                WidgetType::Label,
                ui.is_enabled(),
                &pages.accessibility_label,
            )
        });
    });
}

fn render_sequence_step_button(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    label: &str,
    accessible_name: &str,
    shortcut: &str,
    enabled: bool,
    delta: isize,
) {
    let response = ui
        .add_enabled(
            enabled,
            egui::Button::new(label).min_size(Vec2::new(72.0, 36.0)),
        )
        .on_hover_text(format!("{accessible_name} ({shortcut})"));
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, accessible_name));
    response.ctx.accesskit_node_builder(response.id, |node| {
        node.set_keyboard_shortcut(shortcut);
    });
    if response.clicked() {
        actions.push(UiAction::StepSequence(delta));
    }
}

fn sequence_menu_items(frame: &UiFrameOwned) -> Option<(&'static str, &'static str, bool, bool)> {
    if let Some(pages) = frame.pages.as_ref() {
        let (previous, next) = match pages.noun {
            "Icon" => ("Previous Icon", "Next Icon"),
            _ => ("Previous Page", "Next Page"),
        };
        return Some((previous, next, pages.can_previous, pages.can_next));
    }
    frame.animation.map(|animation| {
        (
            "Previous Frame",
            "Next Frame",
            animation.can_previous,
            animation.can_next,
        )
    })
}

fn format_and_size(details: &crate::image_info::ImageDetails) -> Option<String> {
    match (&details.format, details.file_bytes) {
        (Some(format), Some(bytes)) => Some(format!(
            "{format} · {}",
            crate::image_info::format_file_size(bytes)
        )),
        (Some(format), None) => Some(format.clone()),
        (None, Some(bytes)) => Some(crate::image_info::format_file_size(bytes)),
        (None, None) => None,
    }
}

fn render_capture_info(ui: &mut egui::Ui, frame: &UiFrameOwned) {
    let colors = chrome_colors(ui);
    let Some(details) = frame
        .details
        .as_ref()
        .filter(|details| has_camera_details(details))
    else {
        return;
    };
    ui.add_space(8.0);
    ui.separator();
    ui.label(
        RichText::new("Capture")
            .color(colors.muted)
            .small()
            .strong(),
    );
    ui.add_space(4.0);
    image_detail_value(ui, "Camera", details.camera.as_deref());
    image_detail_value(ui, "Lens", details.lens.as_deref());
    image_detail_value(ui, "Captured", details.captured_at.as_deref());
    let settings = [
        details.exposure.as_deref(),
        details.aperture.as_deref(),
        details.iso.as_deref(),
        details.focal_length.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    image_detail_value(
        ui,
        "Settings",
        (!settings.is_empty()).then_some(settings.as_str()),
    );
}

fn render_source_privacy(ui: &mut egui::Ui, frame: &UiFrameOwned) {
    let Some(details) = frame.details.as_ref() else {
        return;
    };
    let colors = chrome_colors(ui);
    ui.add_space(8.0);
    ui.separator();
    ui.label(RichText::new("Source Privacy").color(colors.text).strong());
    ui.add_space(4.0);
    if details.exif_tag_count == 0 {
        ui.label(RichText::new("No supported EXIF detected.").color(colors.muted));
    } else {
        let noun = if details.exif_tag_count == 1 {
            "tag"
        } else {
            "tags"
        };
        ui.label(
            RichText::new(format!(
                "{} supported EXIF {noun} detected.",
                details.exif_tag_count
            ))
            .color(colors.muted),
        );
        let categories = source_privacy_categories(details);
        if categories.is_empty() {
            ui.label(
                RichText::new("No common identity or location fields detected.")
                    .color(colors.muted),
            );
        } else {
            for category in categories {
                ui.label(RichText::new(format!("Present: {category}")).color(colors.text));
            }
            ui.label(
                RichText::new("Presence only. Sensitive values stay hidden on screen.")
                    .size(11.0)
                    .color(colors.muted),
            );
        }
    }
    ui.add_space(4.0);
    ui.label(
        RichText::new("Limited EXIF scan. Other metadata or hidden pixel data may still exist.")
            .size(11.0)
            .color(colors.muted),
    );
}

fn source_privacy_categories(details: &crate::image_info::ImageDetails) -> Vec<&'static str> {
    [
        (details.has_location, "location-related data"),
        (details.has_owner_or_author, "owner or author data"),
        (
            details.has_device_identifier,
            "camera, lens, or image identifiers",
        ),
        (
            details.has_description_or_comment,
            "description or comment data",
        ),
        (details.has_software_history, "software history"),
        (details.has_embedded_thumbnail, "embedded thumbnail"),
        (details.has_maker_specific_data, "maker-specific data"),
    ]
    .into_iter()
    .filter_map(|(present, label)| present.then_some(label))
    .collect()
}

fn render_export_privacy(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    let colors = chrome_colors(ui);
    ui.add_space(8.0);
    ui.separator();
    ui.label(RichText::new("Export Privacy").color(colors.text).strong());
    ui.add_space(4.0);
    let mut retain_exif = frame.retain_exif;
    if ui
        .checkbox(&mut retain_exif, "Keep camera metadata when saving")
        .on_hover_text("When enabled, Save As keeps supported EXIF tags, including GPS.")
        .changed()
    {
        actions.push(UiAction::ToggleRetainExif);
    }
    ui.add_space(6.0);
    ui.label(
        RichText::new(
            "Off by default. Save As removes supported EXIF metadata, including GPS and camera \
             identifiers. This choice lasts only for this session.",
        )
        .size(11.0)
        .color(colors.muted),
    );
}

fn has_camera_details(details: &crate::image_info::ImageDetails) -> bool {
    details.camera.is_some()
        || details.lens.is_some()
        || details.captured_at.is_some()
        || details.exposure.is_some()
        || details.aperture.is_some()
        || details.iso.is_some()
        || details.focal_length.is_some()
}

fn image_detail_value(ui: &mut egui::Ui, label: &str, value: Option<&str>) {
    let colors = chrome_colors(ui);
    if let Some(value) = value {
        ui.label(RichText::new(label).color(colors.muted).size(10.5));
        ui.label(RichText::new(value).color(colors.text));
    }
}

#[derive(Clone, Copy)]
enum ToolIcon {
    RotateCcw,
    RotateCw,
    FlipH,
    FlipV,
    Crop,
    Heal,
}

#[derive(Clone, Copy)]
enum ChevronDirection {
    Left,
    Right,
    Up,
    Down,
}

const fn map_disclosure_direction(direction: DisclosureDirection) -> ChevronDirection {
    match direction {
        DisclosureDirection::Left => ChevronDirection::Left,
        DisclosureDirection::Right => ChevronDirection::Right,
        DisclosureDirection::Up => ChevronDirection::Up,
        DisclosureDirection::Down => ChevronDirection::Down,
    }
}

fn dock_disclosure_button(ui: &mut egui::Ui, disclosure: DisclosureView) -> egui::Response {
    disclosure_button(
        ui,
        map_disclosure_direction(disclosure.direction),
        disclosure.label,
        disclosure.expanded,
    )
}

fn disclosure_button(
    ui: &mut egui::Ui,
    direction: ChevronDirection,
    label: &str,
    expanded: bool,
) -> egui::Response {
    let colors = chrome_colors(ui);
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::click());
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), expanded, label));
    let response = response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(label);
    let fill = if response.hovered() || response.has_focus() {
        colors.raised
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(6),
            Stroke::new(2.0, colors.accent),
            egui::StrokeKind::Inside,
        );
    }
    let center = rect.center();
    let points = match direction {
        ChevronDirection::Left => [
            center + Vec2::new(2.5, -5.0),
            center + Vec2::new(-2.5, 0.0),
            center + Vec2::new(2.5, 5.0),
        ],
        ChevronDirection::Right => [
            center + Vec2::new(-2.5, -5.0),
            center + Vec2::new(2.5, 0.0),
            center + Vec2::new(-2.5, 5.0),
        ],
        ChevronDirection::Up => [
            center + Vec2::new(-5.0, 2.5),
            center + Vec2::new(0.0, -2.5),
            center + Vec2::new(5.0, 2.5),
        ],
        ChevronDirection::Down => [
            center + Vec2::new(-5.0, -2.5),
            center + Vec2::new(0.0, 2.5),
            center + Vec2::new(5.0, -2.5),
        ],
    };
    ui.painter()
        .line(points.to_vec(), Stroke::new(1.75, colors.text));
    response
}

fn render_tools_panel(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    frame: &UiFrameOwned,
    chrome: ChromeViewModel,
) {
    let colors = chrome_colors(ui);
    let width = match chrome.dock.tools.state {
        DockState::Expanded => TOOLS_PANEL_WIDTH,
        DockState::Collapsed => TOOLS_RAIL_WIDTH,
        DockState::Hidden => return,
    };
    let panel = match chrome.dock.tools.side {
        DockSide::Left => Panel::left("tools_panel"),
        DockSide::Right => Panel::right("tools_panel"),
    };
    panel
        .exact_size(width)
        .resizable(false)
        .frame(docked_frame(colors))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                let disclosure = chrome
                    .dock
                    .tools
                    .disclosure()
                    .expect("visible tools dock has disclosure state");
                if dock_disclosure_button(ui, disclosure).clicked() {
                    actions.push(UiAction::ToggleToolsPanelExpansion);
                }

                if chrome.dock.tools.state == DockState::Expanded {
                    ui.label(
                        RichText::new("TOOLS")
                            .size(10.0)
                            .color(colors.muted)
                            .strong(),
                    );
                    ui.separator();
                    ui.set_width(44.0);
                    ui.vertical_centered(|ui| {
                        ui.spacing_mut().item_spacing.y = 5.0;
                        ui.add_enabled_ui(chrome.is_enabled(ChromeControl::EditTransform), |ui| {
                            icon_btn(
                                ui,
                                ToolIcon::RotateCcw,
                                "Rotate counterclockwise (L)",
                                false,
                                || {
                                    actions.push(UiAction::RotateCcw);
                                },
                            );
                            icon_btn(
                                ui,
                                ToolIcon::RotateCw,
                                "Rotate clockwise (R)",
                                false,
                                || {
                                    actions.push(UiAction::RotateCw);
                                },
                            );
                            icon_btn(ui, ToolIcon::FlipH, "Flip horizontally (H)", false, || {
                                actions.push(UiAction::FlipH);
                            });
                            icon_btn(ui, ToolIcon::FlipV, "Flip vertically (V)", false, || {
                                actions.push(UiAction::FlipV);
                            });
                        });
                        ui.add_space(4.0);
                        let crop = chrome.crop_control();
                        ui.add_enabled_ui(crop.enabled, |ui| {
                            icon_btn(ui, ToolIcon::Crop, "Crop (C)", crop.selected, || {
                                actions.push(UiAction::ToggleCrop);
                            });
                        });
                        let heal_tip = if frame.heal_supported {
                            "Spot heal (J)"
                        } else {
                            "Spot heal is unavailable for images larger than the GPU texture limit"
                        };
                        let heal = chrome.heal_control();
                        ui.add_enabled_ui(heal.enabled, |ui| {
                            icon_btn(ui, ToolIcon::Heal, heal_tip, heal.selected, || {
                                actions.push(UiAction::ToggleHeal);
                            });
                        });
                    });
                }
            });
        });
}

fn icon_btn(ui: &mut egui::Ui, icon: ToolIcon, tip: &str, active: bool, on_click: impl FnOnce()) {
    let colors = chrome_colors(ui);
    let size = Vec2::splat(38.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), active, tip));
    let response = response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(tip);
    let fill = if active {
        with_alpha(colors.accent, 40)
    } else if response.hovered() || response.has_focus() {
        with_alpha(colors.text, 18)
    } else {
        with_alpha(colors.text, 8)
    };
    let border = if active {
        Stroke::new(1.0, colors.accent)
    } else {
        Stroke::new(1.0, with_alpha(colors.text, 28))
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(8), fill);
    painter.rect_stroke(
        rect,
        CornerRadius::same(8),
        border,
        egui::StrokeKind::Inside,
    );
    if response.has_focus() {
        painter.rect_stroke(
            rect.shrink(1.0),
            CornerRadius::same(7),
            Stroke::new(2.0, colors.accent),
            egui::StrokeKind::Inside,
        );
    }
    let ink = if active { colors.accent } else { colors.text };
    paint_icon(painter, rect, icon, ink);
    if response.clicked() {
        on_click();
    }
}

#[allow(clippy::too_many_lines)] // one match arm per toolbar glyph
fn paint_icon(painter: &egui::Painter, rect: Rect, icon: ToolIcon, color: Color32) {
    let c = rect.center();
    let s = rect.width() * 0.28;
    let stroke = Stroke::new(1.75, color);
    match icon {
        ToolIcon::RotateCw => {
            painter.circle_stroke(c, s, stroke);
            painter.arrow(
                c + Vec2::new(s * 0.7, -s * 0.2),
                Vec2::new(s * 0.35, s * 0.45),
                stroke,
            );
        }
        ToolIcon::RotateCcw => {
            painter.circle_stroke(c, s, stroke);
            painter.arrow(
                c + Vec2::new(-s * 0.7, -s * 0.2),
                Vec2::new(-s * 0.35, s * 0.45),
                stroke,
            );
        }
        ToolIcon::FlipH => {
            painter.line_segment([c + Vec2::new(0.0, -s), c + Vec2::new(0.0, s)], stroke);
            painter.arrow(
                c + Vec2::new(-s * 0.2, 0.0),
                Vec2::new(-s * 0.7, 0.0),
                stroke,
            );
            painter.arrow(c + Vec2::new(s * 0.2, 0.0), Vec2::new(s * 0.7, 0.0), stroke);
        }
        ToolIcon::FlipV => {
            painter.line_segment([c + Vec2::new(-s, 0.0), c + Vec2::new(s, 0.0)], stroke);
            painter.arrow(
                c + Vec2::new(0.0, -s * 0.2),
                Vec2::new(0.0, -s * 0.7),
                stroke,
            );
            painter.arrow(c + Vec2::new(0.0, s * 0.2), Vec2::new(0.0, s * 0.7), stroke);
        }
        ToolIcon::Crop => {
            let r = Rect::from_center_size(c, Vec2::splat(s * 1.5));
            painter.rect_stroke(r, CornerRadius::ZERO, stroke, egui::StrokeKind::Outside);
            painter.line_segment(
                [
                    r.left_top() + Vec2::new(-3.0, 0.0),
                    r.left_top() + Vec2::new(6.0, 0.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    r.left_top() + Vec2::new(0.0, -3.0),
                    r.left_top() + Vec2::new(0.0, 6.0),
                ],
                stroke,
            );
        }
        ToolIcon::Heal => {
            painter.circle_stroke(c, s * 0.72, stroke);
            painter.line_segment(
                [c + Vec2::new(-s * 0.42, 0.0), c + Vec2::new(s * 0.42, 0.0)],
                stroke,
            );
            painter.line_segment(
                [c + Vec2::new(0.0, -s * 0.42), c + Vec2::new(0.0, s * 0.42)],
                stroke,
            );
            painter.circle_filled(c + Vec2::new(s * 0.82, -s * 0.82), 1.8, color);
        }
    }
}

fn render_heal_panel(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    frame: &UiFrameOwned,
    chrome: ChromeViewModel,
) {
    let colors = chrome_colors(ui);
    let panel = match chrome.dock.tools.side {
        DockSide::Left => Panel::left("spot_heal_panel"),
        DockSide::Right => Panel::right("spot_heal_panel"),
    };
    panel
        .exact_size(HEAL_PANEL_WIDTH)
        .resizable(false)
        .frame(docked_frame(colors).inner_margin(egui::Margin::same(14)))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("SPOT HEAL")
                        .size(11.0)
                        .color(colors.accent)
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let heal = chrome.heal_control();
                    if ui
                        .add_enabled(
                            heal.enabled,
                            egui::Button::new("Done")
                                .shortcut_text("Esc")
                                .selected(heal.selected),
                        )
                        .clicked()
                    {
                        actions.push(UiAction::ToggleHeal);
                    }
                });
            });
            ui.add_space(8.0);
            ui.add(
                egui::Label::new(
                    RichText::new("Paint over a small blemish, then release to repair it.")
                        .size(12.5)
                        .color(colors.text),
                )
                .wrap(),
            );
            ui.add_space(12.0);
            render_heal_controls(ui, actions, frame, chrome, colors);
            render_heal_guidance(ui, frame, colors);
        });
}

fn render_heal_controls(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    frame: &UiFrameOwned,
    chrome: ChromeViewModel,
    colors: ChromeColors,
) {
    let mut radius = frame.heal_brush_radius;
    ui.label(RichText::new("Brush radius").size(11.5).color(colors.muted));
    let slider = egui::Slider::new(
        &mut radius,
        crate::heal::MIN_BRUSH_RADIUS..=crate::heal::MAX_BRUSH_RADIUS,
    )
    .suffix(" px");
    let adjust_enabled = chrome.is_enabled(ChromeControl::HealAdjust);
    let response = ui.add_enabled(adjust_enabled, slider);
    response.widget_info(|| {
        WidgetInfo::slider(
            ui.is_enabled() && adjust_enabled,
            f64::from(radius),
            "Brush radius",
        )
    });
    if response.changed() {
        actions.push(UiAction::SetHealBrushRadius(radius));
    }

    ui.add_space(10.0);
    let mut feather = frame.heal_feather_percent;
    ui.label(RichText::new("Feather").size(11.5).color(colors.muted));
    let feather_slider =
        egui::Slider::new(&mut feather, 0..=crate::heal::MAX_FEATHER_PERCENT).suffix("%");
    let response = ui
        .add_enabled(adjust_enabled, feather_slider)
        .on_hover_text("Softens the repair edge outward from the painted area");
    response.widget_info(|| {
        WidgetInfo::slider(
            ui.is_enabled() && adjust_enabled,
            f64::from(feather),
            "Feather",
        )
    });
    if response.changed() {
        actions.push(UiAction::SetHealFeather(feather));
    }

    ui.add_space(12.0);
    let source_label = frame.heal_source.map_or_else(
        || "Refresh source".to_owned(),
        |(index, count)| format!("Source {} of {count}", index + 1),
    );
    if ui
        .add_enabled(
            chrome.is_enabled(ChromeControl::HealRefreshSource),
            egui::Button::new(source_label).shortcut_text("/"),
        )
        .on_hover_text("Try the next ranked clean source patch")
        .clicked()
    {
        actions.push(UiAction::RefreshHealSource);
    }

    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                chrome.is_enabled(ChromeControl::UndoEdit),
                egui::Button::new("Undo").shortcut_text(format!("{PRIMARY_MODIFIER}+Z")),
            )
            .clicked()
        {
            actions.push(UiAction::UndoEdit);
        }
        if ui
            .add_enabled(
                chrome.is_enabled(ChromeControl::RedoEdit),
                egui::Button::new("Redo").shortcut_text(format!("{PRIMARY_MODIFIER}+Shift+Z")),
            )
            .clicked()
        {
            actions.push(UiAction::RedoEdit);
        }
    });
}

fn render_heal_guidance(ui: &mut egui::Ui, frame: &UiFrameOwned, colors: ChromeColors) {
    ui.add_space(14.0);
    if frame.heal_busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(
                RichText::new("Repairing in memory...")
                    .size(12.0)
                    .color(colors.text),
            );
        });
    }
    ui.add_space(8.0);
    ui.add(
        egui::Label::new(
            RichText::new("The original file stays untouched. Use Save As to keep the edit.")
                .size(11.0)
                .color(colors.muted),
        )
        .wrap(),
    );
}

fn render_heal_overlay(ui: &mut egui::Ui, frame: &UiFrameOwned) {
    let colors = chrome_colors(ui);
    let image_viewport = image_viewport_rect(ui.ctx(), frame);
    let painter = ui
        .ctx()
        .layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("spot_heal_draw"),
        ))
        .with_clip_rect(image_viewport);
    let radius = frame.heal_brush_screen_radius.max(1.0);
    let mask = if frame.heal_busy {
        with_alpha(colors.accent, 72)
    } else {
        with_alpha(colors.accent, 92)
    };
    let outline = Stroke::new(1.25, with_alpha(colors.accent, 240));
    let outline_shadow = Stroke::new(3.25, with_alpha(colors.panel, 220));
    for point in &frame.heal_stroke_screen {
        painter.circle_filled(Pos2::new(point[0], point[1]), radius, mask);
    }
    for pair in frame.heal_stroke_screen.windows(2) {
        if let [a, b] = pair {
            painter.line_segment(
                [Pos2::new(a[0], a[1]), Pos2::new(b[0], b[1])],
                Stroke::new(radius * 2.0, mask),
            );
        }
    }
    if !frame.heal_busy
        && let Some(cursor) = frame.heal_cursor_screen
    {
        painter.circle_stroke(Pos2::new(cursor[0], cursor[1]), radius, outline_shadow);
        painter.circle_stroke(Pos2::new(cursor[0], cursor[1]), radius, outline);
    }
}

fn render_filmstrip(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    frame: &UiFrameOwned,
    chrome: ChromeViewModel,
) {
    let colors = chrome_colors(ui);
    let current = frame.rating.current_catalog_index;
    let height = match chrome.dock.filmstrip.state {
        DockState::Expanded => FILMSTRIP_PANEL_HEIGHT,
        DockState::Collapsed => FILMSTRIP_RAIL_HEIGHT,
        DockState::Hidden => return,
    };
    Panel::bottom("filmstrip_panel")
        .exact_size(height)
        .resizable(false)
        .frame(docked_frame(colors))
        .show(ui, |ui| {
            if chrome.dock.filmstrip.state == DockState::Expanded {
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        Vec2::new(112.0, ui.available_height()),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            let disclosure = chrome
                                .dock
                                .filmstrip
                                .disclosure()
                                .expect("visible filmstrip has disclosure state");
                            if dock_disclosure_button(ui, disclosure).clicked() {
                                actions.push(UiAction::ToggleFilmstripPanelExpansion);
                            }
                            ui.label(
                                RichText::new("FOLDER PREVIEWS")
                                    .size(10.0)
                                    .color(colors.muted)
                                    .strong(),
                            );
                            let position = match frame.rating.filter {
                                crate::ratings::RatingFilter::All => frame.playlist_pos,
                                crate::ratings::RatingFilter::AtLeast(_) => {
                                    frame.rating.visible_position
                                }
                            };
                            if let Some((index, total)) = position {
                                ui.label(
                                    RichText::new(format!("{index} of {total}"))
                                        .size(11.0)
                                        .color(colors.muted),
                                );
                            }
                        },
                    );
                    ui.separator();
                    ScrollArea::horizontal()
                        .id_salt("folder_preview_scroll")
                        .scroll_bar_visibility(ScrollBarVisibility::AlwaysHidden)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.add_enabled_ui(
                                chrome.is_enabled(ChromeControl::NavigateFilmstrip),
                                |ui| {
                                    ui.horizontal_centered(|ui| {
                                        ui.spacing_mut().item_spacing.x = 8.0;
                                        for item in &frame.filmstrip {
                                            render_filmstrip_item(ui, actions, item, current);
                                        }
                                    });
                                },
                            );
                        });
                });
            } else {
                ui.horizontal_centered(|ui| {
                    let disclosure = chrome
                        .dock
                        .filmstrip
                        .disclosure()
                        .expect("visible filmstrip has disclosure state");
                    if dock_disclosure_button(ui, disclosure).clicked() {
                        actions.push(UiAction::ToggleFilmstripPanelExpansion);
                    }
                    let position = match frame.rating.filter {
                        crate::ratings::RatingFilter::All => frame.playlist_pos,
                        crate::ratings::RatingFilter::AtLeast(_) => frame.rating.visible_position,
                    };
                    let label = position.map_or_else(
                        || "Folder previews".to_owned(),
                        |(index, total)| format!("Folder previews  {index} of {total}"),
                    );
                    ui.label(RichText::new(label).size(12.0).color(colors.muted));
                });
            }
        });
}

fn render_filmstrip_item(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    item: &FilmstripItem,
    current: Option<usize>,
) {
    let colors = chrome_colors(ui);
    let selected = current == Some(item.index);
    let border = if selected {
        Stroke::new(2.0, colors.accent)
    } else {
        Stroke::new(1.0, colors.border)
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(86.0, 78.0), Sense::click());
    let fill = if selected {
        with_alpha(colors.accent, 28)
    } else if response.hovered() {
        with_alpha(colors.text, 12)
    } else {
        colors.panel
    };
    let accessibility_label = filmstrip_accessibility_label(item);
    response.widget_info(|| {
        WidgetInfo::selected(
            WidgetType::Button,
            ui.is_enabled(),
            selected,
            &accessibility_label,
        )
    });
    let response = response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(&item.name);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(7), fill);
    painter.rect_stroke(
        rect,
        CornerRadius::same(7),
        border,
        egui::StrokeKind::Inside,
    );
    if response.has_focus() {
        painter.rect_stroke(
            rect.shrink(2.0),
            CornerRadius::same(5),
            Stroke::new(2.0, colors.accent),
            egui::StrokeKind::Inside,
        );
    }
    if let Some(texture) = &item.texture {
        let image_rect = rect.shrink(4.0);
        let size = texture.size_vec2();
        let scale = (image_rect.width() / size.x).min(image_rect.height() / size.y);
        let draw = Rect::from_center_size(image_rect.center(), size * scale);
        painter.image(
            texture.id(),
            draw,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            format!("{}", item.position),
            egui::FontId::proportional(14.0),
            colors.muted,
        );
    }
    if response.clicked() {
        actions.push(UiAction::NavigateTo(item.index));
    }
}

fn filmstrip_accessibility_label(item: &FilmstripItem) -> String {
    format!("image {}: {}", item.position, item.name)
}

fn render_toast(ui: &mut egui::Ui, msg: &str, frame: &UiFrameOwned) {
    let colors = chrome_colors(ui);
    let image_viewport = image_viewport_rect(ui.ctx(), frame);
    Area::new("toast".into())
        .fixed_pos(Pos2::new(
            image_viewport.center().x,
            image_viewport.bottom() - 12.0,
        ))
        .pivot(Align2::CENTER_BOTTOM)
        .constrain_to(image_viewport)
        .fade_in(false)
        .order(egui::Order::Tooltip)
        .show(ui.ctx(), |ui| {
            Frame::new()
                .fill(with_alpha(colors.panel, 242))
                .corner_radius(CornerRadius::same(8))
                .stroke(Stroke::new(1.0, colors.accent))
                .inner_margin(egui::Margin::symmetric(14, 8))
                .show(ui, |ui| {
                    let response = ui.label(RichText::new(msg).size(13.0).color(colors.text));
                    if !frame.rating.write_busy && rating_toast_is_status(msg) {
                        mark_as_polite_status(&response);
                    }
                });
        });
}

fn rating_toast_is_status(message: &str) -> bool {
    !matches!(
        message,
        "Saving rating..." | "Finishing the rating update before closing..."
    ) && (message.contains("rating")
        || message.contains("Rating")
        || message
            == "viewr could not verify this image's source safely. The file was not changed.")
}

fn render_crop_overlay(
    ui: &mut egui::Ui,
    frame: &UiFrameOwned,
    chrome: ChromeViewModel,
    actions: &mut Vec<UiAction>,
) {
    render_crop_toolbar(ui, frame, chrome, actions);
    render_crop_selection(ui, frame, actions);
}

fn render_crop_toolbar(
    ui: &mut egui::Ui,
    frame: &UiFrameOwned,
    chrome: ChromeViewModel,
    actions: &mut Vec<UiAction>,
) {
    let colors = chrome_colors(ui);
    let image_viewport = image_viewport_rect(ui.ctx(), frame);
    let toolbar_width = (image_viewport.width() - 24.0).clamp(240.0, 520.0);
    let toolbar_origin = Pos2::new(image_viewport.center().x, image_viewport.top() + 8.0);

    Area::new("crop_ratios".into())
        .fixed_pos(toolbar_origin)
        .pivot(Align2::CENTER_TOP)
        .constrain_to(image_viewport)
        .fade_in(false)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            docked_frame(colors)
                .corner_radius(CornerRadius::same(8))
                .show(ui, |ui| {
                    ui.set_width(toolbar_width);
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("Crop").color(colors.accent).strong());
                            ui.separator();
                            crop_ratio_picker(ui, frame, actions);
                            if ui
                                .add_enabled(
                                    frame.crop_ratio != crate::crop::CropRatio::Free,
                                    egui::Button::new("Swap").shortcut_text("X"),
                                )
                                .on_hover_text("Swap the crop between landscape and portrait")
                                .clicked()
                            {
                                actions.push(UiAction::SwapCropRatio);
                            }
                            ui.separator();
                            if ui
                                .add_enabled(
                                    chrome.is_enabled(ChromeControl::ApplyCrop),
                                    egui::Button::new(
                                        RichText::new("Apply").color(colors.accent_ink),
                                    )
                                    .fill(colors.accent),
                                )
                                .clicked()
                            {
                                actions.push(UiAction::ApplyCrop);
                            }
                            if ui.button("Cancel").clicked() {
                                actions.push(UiAction::CancelCrop);
                            }
                        });
                        ui.separator();
                        ui.label(
                            RichText::new(
                                "Arrows move  |  Shift+Arrows resize  |  Ctrl fine-tunes",
                            )
                            .size(11.0)
                            .color(colors.muted),
                        );
                        ui.label(
                            RichText::new(
                                "Drag to redraw  |  X swaps aspect  |  Enter applies  |  Esc cancels",
                            )
                                .size(11.0)
                                .color(colors.muted),
                        );
                    });
                });
        });
}

fn crop_ratio_picker(ui: &mut egui::Ui, frame: &UiFrameOwned, actions: &mut Vec<UiAction>) {
    let colors = chrome_colors(ui);
    let label = format!("Aspect: {}", frame.crop_ratio.label());
    ui.menu_button(label, |ui| {
        ui.set_min_width(292.0);
        let mut current = frame.crop_ratio;

        for (ratio, label) in [
            (crate::crop::CropRatio::Free, "Free"),
            (crate::crop::CropRatio::Original, "Original"),
            (crate::crop::CropRatio::SQUARE, "1:1  Square"),
        ] {
            if ui.selectable_value(&mut current, ratio, label).clicked() {
                actions.push(UiAction::SetCropRatio(current));
                ui.close();
            }
        }

        ui.separator();
        ui.label(RichText::new("Landscape").size(11.0).color(colors.muted));
        ui.horizontal(|ui| {
            for (ratio, label) in [
                (crate::crop::CropRatio::THREE_TWO, "3:2"),
                (crate::crop::CropRatio::FOUR_THREE, "4:3"),
                (crate::crop::CropRatio::FIVE_FOUR, "5:4"),
                (crate::crop::CropRatio::FIVE_THREE, "5:3"),
                (crate::crop::CropRatio::SIXTEEN_NINE, "16:9"),
            ] {
                if ui.selectable_value(&mut current, ratio, label).clicked() {
                    actions.push(UiAction::SetCropRatio(current));
                    ui.close();
                }
            }
        });

        ui.label(RichText::new("Portrait").size(11.0).color(colors.muted));
        ui.horizontal(|ui| {
            for (ratio, label) in [
                (crate::crop::CropRatio::TWO_THREE, "2:3"),
                (crate::crop::CropRatio::THREE_FOUR, "3:4"),
                (crate::crop::CropRatio::FOUR_FIVE, "4:5"),
                (crate::crop::CropRatio::THREE_FIVE, "3:5"),
                (crate::crop::CropRatio::NINE_SIXTEEN, "9:16"),
            ] {
                if ui.selectable_value(&mut current, ratio, label).clicked() {
                    actions.push(UiAction::SetCropRatio(current));
                    ui.close();
                }
            }
        });

        ui.separator();
        ui.label(RichText::new("Custom ratio").size(11.0).color(colors.muted));
        let (mut custom_width, mut custom_height) = frame.custom_crop_ratio;
        ui.horizontal(|ui| {
            ui.label("W");
            let width_changed = ui
                .add(
                    egui::DragValue::new(&mut custom_width)
                        .range(1..=999)
                        .speed(1),
                )
                .on_hover_text("Custom ratio width")
                .changed();
            ui.label(":  H");
            let height_changed = ui
                .add(
                    egui::DragValue::new(&mut custom_height)
                        .range(1..=999)
                        .speed(1),
                )
                .on_hover_text("Custom ratio height")
                .changed();
            if width_changed || height_changed {
                actions.push(UiAction::SetCustomCropRatio(custom_width, custom_height));
            }
            if ui.button("Use").clicked() {
                actions.push(UiAction::SetCustomCropRatio(custom_width, custom_height));
                actions.push(UiAction::SetCropRatio(crate::crop::CropRatio::fixed(
                    custom_width,
                    custom_height,
                )));
                ui.close();
            }
        });
    });
}

fn image_viewport_rect(ctx: &egui::Context, frame: &UiFrameOwned) -> Rect {
    frame.image_viewport.map_or_else(
        || ctx.content_rect(),
        |[left, top, right, bottom]| {
            Rect::from_min_max(Pos2::new(left, top), Pos2::new(right, bottom))
        },
    )
}

fn render_crop_selection(ui: &mut egui::Ui, frame: &UiFrameOwned, actions: &mut Vec<UiAction>) {
    let colors = chrome_colors(ui);
    if let Some([x0, y0, x1, y1]) = frame.crop_screen {
        let rect = Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1));
        let image_viewport = image_viewport_rect(ui.ctx(), frame);
        let painter = ui
            .ctx()
            .layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("crop_draw"),
            ))
            .with_clip_rect(image_viewport);
        painter.rect_stroke(
            rect,
            CornerRadius::ZERO,
            Stroke::new(1.5, colors.text),
            egui::StrokeKind::Outside,
        );
        for fraction in [1.0 / 3.0, 2.0 / 3.0] {
            let x = egui::lerp(rect.left()..=rect.right(), fraction);
            let y = egui::lerp(rect.top()..=rect.bottom(), fraction);
            let grid_stroke = Stroke::new(1.0, with_alpha(colors.text, 130));
            painter.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                grid_stroke,
            );
            painter.line_segment(
                [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                grid_stroke,
            );
        }

        render_crop_handles(ui, &painter, rect, actions);
        if let (Some((image_width, image_height)), Some(crop_uv)) = (frame.img_size, frame.crop_uv)
        {
            render_crop_dimensions_and_move(
                ui,
                &painter,
                CropMoveOverlay {
                    rect,
                    image_viewport,
                    image_size: (image_width, image_height),
                    crop_uv,
                    swap_axes: frame.crop_swaps_axes,
                    crop_ratio: frame.crop_ratio,
                },
                actions,
            );
        }
    }
}

#[derive(Clone, Copy)]
struct CropMoveOverlay {
    rect: Rect,
    image_viewport: Rect,
    image_size: (u32, u32),
    crop_uv: [f32; 4],
    swap_axes: bool,
    crop_ratio: crate::crop::CropRatio,
}

fn render_crop_dimensions_and_move(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    overlay: CropMoveOverlay,
    actions: &mut Vec<UiAction>,
) {
    let colors = chrome_colors(ui);
    let CropMoveOverlay {
        rect,
        image_viewport,
        image_size,
        crop_uv,
        swap_axes,
        crop_ratio,
    } = overlay;
    let Some((pixel_x, pixel_y, pixel_width, pixel_height)) =
        crop_pixel_bounds(image_size, crop_uv, swap_axes, crop_ratio)
    else {
        return;
    };
    let label = format!("{pixel_width} × {pixel_height} px");
    let galley = painter.layout_no_wrap(label, egui::FontId::proportional(12.0), colors.text);
    let label_size = galley.size() + Vec2::new(12.0, 8.0);
    let below = rect.center_bottom() + Vec2::new(0.0, 20.0);
    let above = rect.center_top() - Vec2::new(0.0, 20.0);
    let center = if below.y + label_size.y * 0.5 <= image_viewport.bottom() {
        below
    } else {
        above
    };
    let center_x = if image_viewport.width() > label_size.x {
        center.x.clamp(
            image_viewport.left() + label_size.x * 0.5,
            image_viewport.right() - label_size.x * 0.5,
        )
    } else {
        image_viewport.center().x
    };
    let center_y = if image_viewport.height() > label_size.y {
        center.y.clamp(
            image_viewport.top() + label_size.y * 0.5,
            image_viewport.bottom() - label_size.y * 0.5,
        )
    } else {
        image_viewport.center().y
    };
    let label_rect = Rect::from_center_size(Pos2::new(center_x, center_y), label_size);
    painter.rect_filled(
        label_rect,
        CornerRadius::same(5),
        with_alpha(colors.panel, 242),
    );
    painter.galley(
        label_rect.center() - galley.size() * 0.5,
        galley,
        colors.text,
    );

    let drag_rect = rect.shrink(10.0).intersect(image_viewport);
    let can_drag = drag_rect.is_positive();
    let semantic_rect = if can_drag {
        drag_rect
    } else {
        rect.intersect(image_viewport)
    };
    if !semantic_rect.is_positive() {
        return;
    }
    let response = ui.interact(
        semantic_rect,
        egui::Id::new("crop_rect_sense"),
        if can_drag {
            Sense::drag()
        } else {
            Sense::hover()
        },
    );
    let response = if can_drag {
        response.on_hover_cursor(CursorIcon::Grab)
    } else {
        response
    };
    if can_drag
        && response.dragged()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let delta = response.drag_delta();
        if delta != Vec2::ZERO {
            actions.push(UiAction::MoveCrop {
                pointer: [pointer.x, pointer.y],
                delta: [delta.x, delta.y],
            });
        }
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    }
    let accessibility_label =
        crop_accessibility_label((pixel_x, pixel_y, pixel_width, pixel_height), can_drag);
    response.widget_info(|| {
        WidgetInfo::labeled(WidgetType::Panel, ui.is_enabled(), &accessibility_label)
    });
}

fn crop_accessibility_label(bounds: (u32, u32, u32, u32), can_drag: bool) -> String {
    let (pixel_x, pixel_y, pixel_width, pixel_height) = bounds;
    let controls = if can_drag {
        "Drag inside to move. Arrow keys move; Shift plus Arrow keys resize."
    } else {
        "Arrow keys move; Shift plus Arrow keys resize."
    };
    format!(
        "Crop selection: {pixel_width} by {pixel_height} output pixels, source starts at x \
         {pixel_x}, y {pixel_y}. {controls}"
    )
}

fn crop_pixel_bounds(
    image_size: (u32, u32),
    crop_uv: [f32; 4],
    swap_axes: bool,
    crop_ratio: crate::crop::CropRatio,
) -> Option<(u32, u32, u32, u32)> {
    let source_ratio = crate::crop::crop_ratio_for_source(crop_ratio, i32::from(swap_axes));
    let pixel =
        crate::crop::quantized_crop_pixel_rect(crop_uv, image_size.0, image_size.1, source_ratio)?;
    let (output_width, output_height) = if swap_axes {
        (pixel.height, pixel.width)
    } else {
        (pixel.width, pixel.height)
    };
    Some((pixel.x, pixel.y, output_width, output_height))
}

fn render_crop_handles(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    rect: Rect,
    actions: &mut Vec<UiAction>,
) {
    let colors = chrome_colors(ui);
    let centers = [
        (rect.left_top(), CursorIcon::ResizeNwSe, "top left"),
        (rect.center_top(), CursorIcon::ResizeVertical, "top"),
        (rect.right_top(), CursorIcon::ResizeNeSw, "top right"),
        (rect.right_center(), CursorIcon::ResizeHorizontal, "right"),
        (rect.right_bottom(), CursorIcon::ResizeNwSe, "bottom right"),
        (rect.center_bottom(), CursorIcon::ResizeVertical, "bottom"),
        (rect.left_bottom(), CursorIcon::ResizeNeSw, "bottom left"),
        (rect.left_center(), CursorIcon::ResizeHorizontal, "left"),
    ];
    for (index, (center, cursor, name)) in centers.into_iter().enumerate() {
        let visual = Rect::from_center_size(center, Vec2::splat(8.0));
        painter.rect_filled(visual, CornerRadius::same(1), colors.text);
        painter.rect_stroke(
            visual,
            CornerRadius::same(1),
            Stroke::new(1.0, colors.accent_ink),
            egui::StrokeKind::Outside,
        );
        let hit_rect = Rect::from_center_size(center, Vec2::splat(20.0));
        let response = ui
            .interact(
                hit_rect,
                egui::Id::new(("crop_handle", index)),
                Sense::drag(),
            )
            .on_hover_cursor(cursor);
        response.widget_info(|| {
            WidgetInfo::labeled(
                WidgetType::Button,
                ui.is_enabled(),
                format!("Resize crop from {name}"),
            )
        });
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            actions.push(UiAction::ResizeCrop {
                handle_center: [center.x, center.y],
                pointer: [pointer.x, pointer.y],
            });
        }
    }
}

fn apply_cursor(ui: &mut egui::Ui, frame: &UiFrameOwned) {
    if ui.ctx().is_pointer_over_egui() {
        return;
    }
    if frame.is_cropping || frame.dock.heal_active {
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
    } else if frame.is_panning {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    } else if frame.space_held {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    } else {
        ui.ctx().set_cursor_icon(CursorIcon::Default);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        APPEARANCE_SCOPE_HELP, CROP_RECOVERY_STATUS, ChromeControl, DockInput, DockSide,
        EXTERNAL_EDIT_ACCESSIBLE_STATUS, EXTERNAL_EDIT_BADGE, FilmstripItem, LOCAL_PRIVACY_SUMMARY,
        OPEN_WITH_HELP, PREVIEW_RECOVERY_STATUS, PageUiInfo, SAVE_RECOVERY_STATUS, TOP_BAR_HEIGHT,
        TOP_STATUS_COMPACT_MAX_WIDTH, UiAction, UiFrameOwned, actions_owned_by_modal,
        add_top_status_with_external_edit, appearance_menu, chrome_colors_for, context_tool_button,
        crop_pixel_bounds, image_open_status, menu_tool_button, panels_menu, rating_filter_menu,
        rating_menu, rating_toast_is_status, render, retry_open_label, undo_trash_menu_item,
    };

    fn relative_luminance(color: egui::Color32) -> f64 {
        fn linear(channel: u8) -> f64 {
            let value = f64::from(channel) / 255.0;
            if value <= 0.040_45 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        }

        let [red, green, blue, _] = color.to_array();
        0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
    }

    fn contrast_ratio(first: egui::Color32, second: egui::Color32) -> f64 {
        let first = relative_luminance(first);
        let second = relative_luminance(second);
        let (lighter, darker) = if first >= second {
            (first, second)
        } else {
            (second, first)
        };
        (lighter + 0.05) / (darker + 0.05)
    }

    fn accessibility_test_frame() -> UiFrameOwned {
        UiFrameOwned {
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
            retain_exif: false,
            background_override: None,
            theme_preference: crate::theme::Preference::System,
            theme_mode: crate::theme::Mode::Dark,
            show_about: false,
            show_update: false,
            rating: super::RatingUiState {
                state: crate::ratings::RatingState::Unrated,
                capability: crate::ratings::RatingWriteCapability::WritableJpeg,
                filter: crate::ratings::RatingFilter::All,
                write_busy: false,
                recovery_unsettled: false,
                discovery_busy: false,
                outside_filter: false,
                visible_position: None,
                match_count: 2,
                current_catalog_index: Some(0),
                folder_count: 2,
                pending_disclosure: None,
            },
            external_edit_pending: false,
            source_gone: false,
            file_path: Some("C:/photos/current.png".to_owned()),
            selected_file_name: Some("current.png".to_owned()),
            img_size: Some((1920, 1080)),
            animation: None,
            pages: None,
            details: None,
            color_profile: Some(crate::decode::ColorProfileStatus::AssumedSrgb),
            display_output: crate::display_state::DisplayOutputStatus::SrgbFallback,
            is_cropping: false,
            crop_ratio: crate::crop::CropRatio::Free,
            custom_crop_ratio: (3, 5),
            heal_supported: true,
            heal_busy: false,
            heal_painting: false,
            heal_brush_radius: 18,
            heal_feather_percent: crate::heal::DEFAULT_FEATHER_PERCENT,
            heal_source: Some((0, 4)),
            has_undo_edit: false,
            has_redo_edit: false,
            has_undo_trash: true,
            restore_recovery_unsettled: false,
            is_panning: false,
            space_held: false,
            is_loading: false,
            is_opening: false,
            load_error: None,
            save_busy: false,
            save_overwrite_pending: false,
            save_recovery_unsettled: false,
            crop_busy: false,
            crop_recovery_unsettled: false,
            preview_recovery_unsettled: false,
            preview_load_retry_blocked: false,
            curation_status: None,
            curation_recovery_status: None,
            folder_scan_busy: false,
            playlist_pos: Some((1, 2)),
            pixel_scale: 1.0,
            toast: None,
            filmstrip: vec![
                FilmstripItem {
                    index: 0,
                    position: 1,
                    name: "current.png".to_owned(),
                    texture: None,
                },
                FilmstripItem {
                    index: 1,
                    position: 2,
                    name: "second.png".to_owned(),
                    texture: None,
                },
            ],
            crop_screen: None,
            crop_uv: None,
            crop_swaps_axes: false,
            image_viewport: Some([64.0, 40.0, 896.0, 688.0]),
            heal_stroke_screen: Vec::new(),
            heal_cursor_screen: None,
            heal_brush_screen_radius: 0.0,
            context_menu_pos: None,
        }
    }

    fn control_enabled(frame: &UiFrameOwned, control: ChromeControl) -> bool {
        frame.chrome_view_model().is_enabled(control)
    }

    #[test]
    fn filmstrip_labels_use_projection_position_but_navigation_uses_catalog_index() {
        let item = FilmstripItem {
            index: 8,
            position: 2,
            name: "rated.jpg".to_owned(),
            texture: None,
        };

        assert_eq!(
            super::filmstrip_accessibility_label(&item),
            "image 2: rated.jpg"
        );
        assert_eq!(item.index, 8);
    }

    #[test]
    fn trash_undo_copy_reports_availability_without_batch_counts() {
        let mut frame = accessibility_test_frame();
        frame.has_undo_trash = false;
        let unavailable = frame.chrome_view_model().undo_trash();
        assert_eq!(
            unavailable.help,
            "No safely recoverable Trash action is available."
        );
        assert_eq!(unavailable.accessibility_label, "Undo Trash");

        frame.has_undo_trash = true;
        let available = frame.chrome_view_model().undo_trash();
        assert_eq!(
            available.help,
            "Restores the latest safely recoverable Trash action. It may belong to another folder."
        );
        assert_eq!(
            available.accessibility_label,
            "Undo Trash. Restores the latest safely recoverable Trash action. It may belong to another folder."
        );

        frame.restore_recovery_unsettled = true;
        let unsettled = frame.chrome_view_model().undo_trash();
        assert_eq!(
            unsettled.help,
            "Trash restore state is not settled. Follow the current status or recovery guidance before using Undo Trash."
        );
        assert_eq!(
            unsettled.accessibility_label,
            "Undo Trash. Trash restore state is not settled. Follow the current status or recovery guidance before using Undo Trash."
        );
    }

    #[test]
    fn trash_undo_label_fits_its_menu_and_accessibility_node() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let frame = accessibility_test_frame();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(238.0, 80.0),
            )),
            ..egui::RawInput::default()
        };

        let output = context.run_ui(input, |ui| {
            let mut actions = Vec::new();
            undo_trash_menu_item(ui, &mut actions, frame.chrome_view_model());
            assert!(actions.is_empty());
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let node = update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| {
                node.label()
                    == Some(
                        "Undo Trash. Restores the latest safely recoverable Trash action. It may belong to another folder.",
                    )
            })
            .expect("Undo Trash action");

        assert_eq!(node.role(), egui::accesskit::Role::Button);
        assert!(!node.is_disabled());
        let bounds = node.bounds().expect("Undo Trash action bounds");
        assert!(bounds.x0 >= 0.0 && bounds.x1 <= 238.0);
        assert!(bounds.y0 >= 0.0 && bounds.y1 <= 80.0);
    }

    #[test]
    fn trash_and_restore_are_disabled_during_conflicting_visible_work() {
        let mut frame = accessibility_test_frame();
        assert!(control_enabled(&frame, ChromeControl::MoveToTrash));
        assert!(control_enabled(&frame, ChromeControl::UndoTrash));

        frame.restore_recovery_unsettled = true;
        assert!(control_enabled(&frame, ChromeControl::EditTransform));
        assert!(!control_enabled(&frame, ChromeControl::MoveToTrash));
        assert!(control_enabled(&frame, ChromeControl::UndoTrash));
        frame.restore_recovery_unsettled = false;

        frame.is_loading = true;
        assert!(!control_enabled(&frame, ChromeControl::MoveToTrash));
        assert!(!control_enabled(&frame, ChromeControl::UndoTrash));
        frame.is_loading = false;
        frame.crop_busy = true;
        assert!(!control_enabled(&frame, ChromeControl::MoveToTrash));
        assert!(!control_enabled(&frame, ChromeControl::UndoTrash));
        frame.crop_busy = false;
        frame.save_busy = true;
        assert!(!control_enabled(&frame, ChromeControl::MoveToTrash));
        assert!(!control_enabled(&frame, ChromeControl::UndoTrash));
        frame.save_busy = false;
        frame.heal_busy = true;
        assert!(!control_enabled(&frame, ChromeControl::MoveToTrash));
        assert!(!control_enabled(&frame, ChromeControl::UndoTrash));
        frame.heal_busy = false;
        frame.dock.heal_active = true;
        assert!(control_enabled(&frame, ChromeControl::EditTransform));
        assert!(!control_enabled(&frame, ChromeControl::UndoTrash));
        frame.dock.heal_active = false;
        frame.is_cropping = true;
        assert!(!control_enabled(&frame, ChromeControl::UndoTrash));
        frame.is_cropping = false;
        frame.folder_scan_busy = true;
        assert!(!control_enabled(&frame, ChromeControl::UndoTrash));
        frame.folder_scan_busy = false;
        frame.curation_status = Some("Restoring 1 file from Trash...".to_owned());
        assert!(!control_enabled(&frame, ChromeControl::UndoTrash));
        assert!(!control_enabled(&frame, ChromeControl::EditTransform));
    }

    #[test]
    fn active_curation_is_a_polite_status_and_disables_conflicting_surface_actions() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        let status = "Restoring 1 file from Trash...";
        frame.curation_status = Some(status.to_owned());
        frame.context_menu_pos = Some([100.0, 100.0]);
        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let nodes = update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .collect::<Vec<_>>();

        assert!(nodes.iter().any(|node| {
            node.role() == egui::accesskit::Role::Label
                && node.value() == Some(status)
                && node.live() == Some(egui::accesskit::Live::Polite)
        }));
        for label in [
            "image 1: current.png",
            "image 2: second.png",
            "Spot Heal (J)",
            "Crop (C)",
        ] {
            let node = nodes
                .iter()
                .find(|node| node.label() == Some(label))
                .unwrap_or_else(|| panic!("missing curation control: {label}"));
            assert!(
                node.is_disabled(),
                "curation control stayed enabled: {label}"
            );
        }
        let inspection_control = nodes
            .iter()
            .find(|node| node.label() == Some("Collapse folder previews"))
            .expect("missing non-mutating folder preview control");
        assert!(
            !inspection_control.is_disabled(),
            "non-mutating folder preview control was disabled"
        );

        let empty_context = egui::Context::default();
        empty_context.enable_accesskit();
        frame.dock.has_image = false;
        frame.file_path = None;
        frame.playlist_pos = None;
        frame.filmstrip.clear();
        let empty_output = empty_context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let empty_update = empty_output
            .platform_output
            .accesskit_update
            .expect("empty-state AccessKit update should be generated");
        for label in ["Open File", "Open Folder"] {
            let node = empty_update
                .nodes
                .iter()
                .map(|(_, node)| node)
                .find(|node| node.label() == Some(label))
                .unwrap_or_else(|| panic!("missing empty-state curation control: {label}"));
            assert!(
                node.is_disabled(),
                "curation control stayed enabled: {label}"
            );
        }
    }

    #[test]
    fn failed_save_is_persistent_and_blocks_only_another_save() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.save_recovery_unsettled = true;

        assert!(control_enabled(&frame, ChromeControl::EditTransform));
        assert!(!control_enabled(&frame, ChromeControl::SaveAs));

        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        assert!(update.nodes.iter().any(|(_, node)| {
            node.role() == egui::accesskit::Role::Label
                && node.value() == Some(SAVE_RECOVERY_STATUS)
                && node.live() == Some(egui::accesskit::Live::Polite)
        }));
    }

    #[test]
    fn lost_crop_is_persistent_and_allows_only_cancelling_the_restored_selection() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.crop_recovery_unsettled = true;

        assert!(control_enabled(&frame, ChromeControl::EditTransform));
        assert!(!control_enabled(&frame, ChromeControl::Crop));
        assert!(!control_enabled(&frame, ChromeControl::ApplyCrop));
        frame.is_cropping = true;
        assert!(control_enabled(&frame, ChromeControl::Crop));
        assert!(!control_enabled(&frame, ChromeControl::ApplyCrop));

        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        assert!(update.nodes.iter().any(|(_, node)| {
            node.role() == egui::accesskit::Role::Label
                && node.value() == Some(CROP_RECOVERY_STATUS)
                && node.live() == Some(egui::accesskit::Live::Polite)
        }));
    }

    #[test]
    fn lost_preview_executor_blocks_more_crop_work_but_keeps_other_actions_ready() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.preview_recovery_unsettled = true;

        assert!(control_enabled(&frame, ChromeControl::EditTransform));
        assert!(control_enabled(&frame, ChromeControl::SaveAs));
        assert!(!control_enabled(&frame, ChromeControl::Crop));
        assert!(!control_enabled(&frame, ChromeControl::ApplyCrop));
        frame.is_cropping = true;
        assert!(control_enabled(&frame, ChromeControl::Crop));
        assert!(!control_enabled(&frame, ChromeControl::ApplyCrop));

        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        assert!(update.nodes.iter().any(|(_, node)| {
            node.role() == egui::accesskit::Role::Label
                && node.value() == Some(PREVIEW_RECOVERY_STATUS)
                && node.live() == Some(egui::accesskit::Live::Polite)
        }));

        for has_image in [false, true] {
            let context = egui::Context::default();
            context.enable_accesskit();
            let mut frame = accessibility_test_frame();
            frame.dock.has_image = has_image;
            frame.load_error = Some(PREVIEW_RECOVERY_STATUS.to_owned());
            frame.preview_recovery_unsettled = true;
            frame.preview_load_retry_blocked = true;

            assert!(!control_enabled(&frame, ChromeControl::RetryLoad));
            let output = context.run_ui(accessibility_input(), |ui| {
                let _ = render(ui, &frame);
            });
            let update = output
                .platform_output
                .accesskit_update
                .expect("AccessKit update should be generated");
            let retry = update
                .nodes
                .iter()
                .map(|(_, node)| node)
                .find(|node| node.label() == Some("Retry opening current.png"))
                .expect("preview recovery Retry button");
            assert!(retry.is_disabled());
            assert!(update.nodes.iter().any(|(_, node)| {
                node.role() == egui::accesskit::Role::Label
                    && node.value() == Some(PREVIEW_RECOVERY_STATUS)
                    && node.live() == Some(egui::accesskit::Live::Polite)
            }));
        }

        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.load_error = Some("Could not decode: malformed image".to_owned());
        frame.preview_recovery_unsettled = true;

        assert!(control_enabled(&frame, ChromeControl::RetryLoad));
        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let retry = update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Retry opening current.png"))
            .expect("ordinary decode failure Retry button");
        assert!(!retry.is_disabled());
    }

    #[test]
    fn curation_recovery_is_polite_and_yields_to_current_load_failure() {
        let recovery_context = egui::Context::default();
        recovery_context.enable_accesskit();
        let mut recovery_frame = accessibility_test_frame();
        let recovery =
            "Trash restore stopped unexpectedly. Review the folder and system Trash, then retry U.";
        recovery_frame.curation_recovery_status = Some(recovery.to_owned());
        let recovery_output = recovery_context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &recovery_frame);
        });
        let recovery_update = recovery_output
            .platform_output
            .accesskit_update
            .expect("recovery AccessKit update should be generated");
        assert!(
            recovery_update
                .nodes
                .iter()
                .map(|(_, node)| node)
                .any(|node| {
                    node.value() == Some(recovery)
                        && node.live() == Some(egui::accesskit::Live::Polite)
                })
        );

        let load_failure_context = egui::Context::default();
        load_failure_context.enable_accesskit();
        recovery_frame.load_error = Some("controlled load failure".to_owned());
        let load_failure_output = load_failure_context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &recovery_frame);
        });
        let load_failure_update = load_failure_output
            .platform_output
            .accesskit_update
            .expect("load-failure AccessKit update should be generated");
        let load_failure_nodes = load_failure_update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .collect::<Vec<_>>();
        assert!(
            load_failure_nodes
                .iter()
                .any(|node| node.label() == Some("Retry opening current.png"))
        );
        assert!(load_failure_nodes.iter().any(|node| {
            node.value() == Some("Could not open current.png")
                && node.live() == Some(egui::accesskit::Live::Polite)
        }));
    }

    #[test]
    fn rating_recovery_is_persistent_polite_and_disables_rating_mutation() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.rating.recovery_unsettled = true;
        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("rating recovery AccessKit update should be generated");
        assert!(update.nodes.iter().any(|(_, node)| {
            node.value() == Some(super::RATING_RECOVERY_STATUS)
                && node.live() == Some(egui::accesskit::Live::Polite)
        }));

        let menu_context = egui::Context::default();
        menu_context.enable_accesskit();
        let menu_output = menu_context.run_ui(accessibility_input(), |ui| {
            let mut actions = Vec::new();
            rating_menu(ui, &mut actions, frame.chrome_view_model());
            assert!(actions.is_empty());
        });
        let menu_update = menu_output
            .platform_output
            .accesskit_update
            .expect("rating recovery menu AccessKit update should be generated");
        let radios = menu_update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .filter(|node| node.role() == egui::accesskit::Role::RadioButton)
            .collect::<Vec<_>>();
        assert_eq!(radios.len(), 6);
        assert!(radios.iter().all(|node| node.is_disabled()));
        assert_eq!(
            frame.chrome_view_model().rating_unavailable_text(),
            super::RATING_RECOVERY_STATUS
        );
    }

    #[test]
    fn failed_rating_observation_has_truthful_restart_guidance() {
        let mut frame = accessibility_test_frame();
        frame.rating.state = crate::ratings::RatingState::Unreadable;
        frame.rating.capability = crate::ratings::RatingWriteCapability::ObservationFailed;

        assert_eq!(
            frame.chrome_view_model().rating_unavailable_text(),
            "Rating could not be read. Close and reopen viewr before changing it."
        );
        assert_eq!(
            frame.chrome_view_model().rating_menu_label(),
            "Rating: Unreadable"
        );
    }

    fn accessibility_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(1200.0, 800.0),
            )),
            ..egui::RawInput::default()
        }
    }

    fn assert_accesskit_node_within(
        name: &str,
        node: &egui::accesskit::Node,
        container: &egui::accesskit::Node,
    ) {
        let bounds = node.bounds().expect("accessible node bounds");
        let container_bounds = container.bounds().expect("accessible container bounds");
        assert!(
            bounds.x0 >= container_bounds.x0
                && bounds.x1 <= container_bounds.x1
                && bounds.y0 >= container_bounds.y0
                && bounds.y1 <= container_bounds.y1,
            "{name} escaped its container: {bounds:?} outside {container_bounds:?}"
        );
    }

    #[test]
    fn ui_action_variants_exist_for_toolbar() {
        let _ = UiAction::OpenFolder;
        let _ = UiAction::Reload;
        let _ = UiAction::ConfirmSaveOverwrite;
        let _ = UiAction::CancelSaveOverwrite;
        let _ = UiAction::ToggleCrop;
        let _ = UiAction::ApplyCrop;
        let _ = UiAction::CancelCrop;
        let _ = UiAction::SetCropRatio(crate::crop::CropRatio::SQUARE);
        let _ = UiAction::SetCustomCropRatio(3, 5);
        let _ = UiAction::SwapCropRatio;
        let _ = UiAction::MoveCrop {
            pointer: [10.0, 20.0],
            delta: [1.0, 2.0],
        };
        let _ = UiAction::ResizeCrop {
            handle_center: [10.0, 20.0],
            pointer: [12.0, 22.0],
        };
        let _ = UiAction::ToggleHeal;
        let _ = UiAction::SetHealBrushRadius(18);
        let _ = UiAction::SetHealFeather(35);
        let _ = UiAction::RefreshHealSource;
        let _ = UiAction::UndoEdit;
        let _ = UiAction::RedoEdit;
        let _ = UiAction::Navigate(1);
        let _ = UiAction::NavigateTo(0);
        let _ = UiAction::PermanentDelete;
        let _ = UiAction::ToggleImageInfo;
        let _ = UiAction::ToggleAnimationPlayback;
        let _ = UiAction::StepSequence(1);
        let _ = UiAction::RetryLoad;
        let _ = UiAction::ToggleToolsPanelVisibility;
        let _ = UiAction::ToggleToolsPanelExpansion;
        let _ = UiAction::ToggleFilmstripPanelVisibility;
        let _ = UiAction::ToggleFilmstripPanelExpansion;
        let _ = UiAction::SetToolsPanelSide(DockSide::Right);
        let _ = UiAction::SetImageInfoSide(DockSide::Left);
        let _ = UiAction::FitToView;
        let _ = UiAction::ActualSize;
        let _ = UiAction::ZoomIn;
        let _ = UiAction::ZoomOut;
        let _ = UiAction::SetTheme(crate::theme::Preference::Console);
        let _ = UiAction::ShowAbout;
        let _ = UiAction::CloseAbout;
        let _ = UiAction::ShowUpdate;
        let _ = UiAction::CloseUpdate;
    }

    #[test]
    fn chrome_text_and_controls_meet_wcag_aa_contrast() {
        for mode in [
            crate::theme::Mode::Light,
            crate::theme::Mode::Dark,
            crate::theme::Mode::Console,
        ] {
            let colors = chrome_colors_for(mode);
            assert!(contrast_ratio(colors.text, colors.panel) >= 4.5);
            assert!(contrast_ratio(colors.muted, colors.panel) >= 4.5);
            assert!(contrast_ratio(colors.accent, colors.panel) >= 4.5);
            assert!(contrast_ratio(colors.accent_ink, colors.accent) >= 4.5);
        }
    }

    #[test]
    fn open_status_copy_names_only_an_active_or_failed_target() {
        assert_eq!(
            image_open_status(true, false, Some("target.png")).as_deref(),
            Some("Opening target.png")
        );
        assert_eq!(
            image_open_status(false, true, Some("target.png")).as_deref(),
            Some("Could not open target.png")
        );
        assert_eq!(
            image_open_status(true, false, None).as_deref(),
            Some("Opening image")
        );
        assert_eq!(image_open_status(false, false, Some("target.png")), None);
        assert_eq!(
            retry_open_label(Some("target.png")),
            "Retry opening target.png"
        );
        assert_eq!(retry_open_label(None), "Retry opening image");
    }

    #[test]
    fn appearance_parent_label_exposes_the_current_preference() {
        for (preference, expected) in [
            (crate::theme::Preference::System, "Appearance: System"),
            (crate::theme::Preference::Light, "Appearance: Light"),
            (crate::theme::Preference::Dark, "Appearance: Dark"),
            (crate::theme::Preference::Console, "Appearance: Console"),
        ] {
            assert_eq!(crate::chrome::appearance_menu_label(preference), expected);
        }
    }

    #[test]
    fn replacement_status_names_selected_target_and_keeps_presented_identity() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.file_path = Some("C:/photos/presented.png".to_owned());
        frame.selected_file_name = Some("target.png".to_owned());
        frame.is_loading = true;
        frame.is_opening = true;
        frame.playlist_pos = Some((2, 2));

        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let labels = update
            .nodes
            .iter()
            .filter_map(|(_, node)| node.label())
            .collect::<Vec<_>>();
        let values = update
            .nodes
            .iter()
            .filter_map(|(_, node)| node.value())
            .collect::<Vec<_>>();

        assert!(
            values.contains(&"Opening target.png"),
            "status should name the selected target: labels={labels:?}, values={values:?}"
        );
        assert!(update.nodes.iter().any(|(_, node)| {
            node.value() == Some("Opening target.png")
                && node.live() == Some(egui::accesskit::Live::Polite)
        }));
        assert!(values.contains(&"presented.png"));
        assert!(!values.contains(&"Opening..."));
    }

    #[test]
    fn failed_and_preview_states_publish_specific_semantics() {
        let failed_context = egui::Context::default();
        failed_context.enable_accesskit();
        let mut failed = accessibility_test_frame();
        failed.selected_file_name = Some("target.png".to_owned());
        failed.load_error = Some("Could not decode this image".to_owned());
        failed.toast = Some("Could not display image: adapter rejected upload".to_owned());
        let failed_output = failed_context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &failed);
        });
        let failed_update = failed_output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let failed_labels = failed_update
            .nodes
            .iter()
            .filter_map(|(_, node)| node.label())
            .collect::<Vec<_>>();
        assert!(failed_update.nodes.iter().any(|(_, node)| {
            node.value() == Some("Could not open target.png")
                && node.live() == Some(egui::accesskit::Live::Polite)
        }));
        assert!(failed_labels.contains(&"Retry opening target.png"));
        assert!(failed_update.nodes.iter().any(|(_, node)| {
            node.value() == Some("Could not display image: adapter rejected upload")
        }));
        assert!(
            failed_update
                .nodes
                .iter()
                .filter(|(_, node)| {
                    node.value() == Some("Could not display image: adapter rejected upload")
                })
                .all(|(_, node)| node.live() != Some(egui::accesskit::Live::Polite))
        );

        let preview_context = egui::Context::default();
        preview_context.enable_accesskit();
        let mut preview = accessibility_test_frame();
        preview.selected_file_name = Some("target.png".to_owned());
        preview.is_loading = true;
        preview.is_opening = false;
        let preview_output = preview_context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &preview);
        });
        let preview_update = preview_output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let preview_labels = preview_update
            .nodes
            .iter()
            .filter_map(|(_, node)| node.label())
            .collect::<Vec<_>>();
        let preview_values = preview_update
            .nodes
            .iter()
            .filter_map(|(_, node)| node.value())
            .collect::<Vec<_>>();
        assert!(preview_update.nodes.iter().any(|(_, node)| {
            node.value() == Some("Preparing preview...")
                && node.live() == Some(egui::accesskit::Live::Polite)
        }));
        assert!(!preview_values.contains(&"Opening target.png"));
        assert!(!preview_labels.contains(&"Opening target.png"));

        let empty_context = egui::Context::default();
        empty_context.enable_accesskit();
        let mut empty = accessibility_test_frame();
        empty.dock.has_image = false;
        empty.file_path = None;
        empty.is_loading = true;
        empty.is_opening = true;
        empty.playlist_pos = None;
        empty.filmstrip.clear();
        let empty_output = empty_context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &empty);
        });
        let empty_update = empty_output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let empty_labels = empty_update
            .nodes
            .iter()
            .filter_map(|(_, node)| node.label())
            .collect::<Vec<_>>();
        let empty_values = empty_update
            .nodes
            .iter()
            .filter_map(|(_, node)| node.value())
            .collect::<Vec<_>>();
        assert!(
            empty_values.contains(&"Opening current.png"),
            "the empty-state card should name the opening target: labels={empty_labels:?}, values={empty_values:?}"
        );
        assert!(empty_update.nodes.iter().any(|(_, node)| {
            node.value() == Some("Opening current.png")
                && node.live() == Some(egui::accesskit::Live::Polite)
        }));
    }

    #[test]
    fn long_open_target_status_keeps_the_minimum_top_bar_bounded() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        let target_name = format!("{}.png", "x".repeat(92));
        let expected_status = format!("Opening {target_name}");
        frame.selected_file_name = Some(target_name);
        frame.is_loading = true;
        frame.is_opening = true;
        frame.playlist_pos = Some((2, 2));
        frame.dock.show_tools = false;
        frame.dock.show_filmstrip = false;
        frame.dock.show_image_info = false;
        let mut input = accessibility_input();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::Vec2::new(640.0, 480.0),
        ));

        let output = context.run_ui(input, |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let status_nodes = update
            .nodes
            .iter()
            .filter_map(|(_, node)| (node.value() == Some(&expected_status)).then_some(node))
            .collect::<Vec<_>>();
        assert!(!status_nodes.is_empty());
        for node in status_nodes {
            let bounds = node.bounds().expect("status bounds");
            assert!(bounds.x0 >= 0.0 && bounds.x1 <= 640.0);
            assert!(bounds.y0 >= 0.0 && bounds.y1 <= f64::from(TOP_BAR_HEIGHT));
            assert!(bounds.x1 - bounds.x0 <= f64::from(TOP_STATUS_COMPACT_MAX_WIDTH) + 1.0);
        }
        assert!(update.nodes.iter().any(|(_, node)| {
            node.value() == Some("2 / 2")
                && node.bounds().is_some_and(|bounds| {
                    bounds.x0 >= 0.0
                        && bounds.x1 <= 640.0
                        && bounds.y0 >= 0.0
                        && bounds.y1 <= f64::from(TOP_BAR_HEIGHT)
                })
        }));
        for (_, node) in &update.nodes {
            let Some(label) = node.label() else {
                continue;
            };
            if !["File", "Edit", "View", "Tools", "Help"].contains(&label) {
                continue;
            }
            let menu_bounds = node.bounds().expect("menu bounds");
            assert!(
                menu_bounds.x0 >= 0.0
                    && menu_bounds.x1 <= 640.0
                    && menu_bounds.y0 >= 0.0
                    && menu_bounds.y1 <= f64::from(TOP_BAR_HEIGHT),
                "menu {label} escaped the minimum top bar: {menu_bounds:?}"
            );
        }
    }

    #[test]
    fn custom_controls_publish_descriptive_accessibility_nodes() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let frame = accessibility_test_frame();
        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let nodes = update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .collect::<Vec<_>>();
        for expected in [
            "Collapse tools panel",
            "Rotate clockwise (R)",
            "Spot heal (J)",
            "Collapse folder previews",
            "image 1: current.png",
            "image 2: second.png",
            "Keep camera metadata when saving",
        ] {
            assert!(
                nodes.iter().any(|node| node.label() == Some(expected)),
                "missing accessibility node: {expected}"
            );
        }
        let current_thumbnail = nodes
            .iter()
            .find(|node| node.label() == Some("image 1: current.png"))
            .expect("current thumbnail node");
        assert_eq!(current_thumbnail.role(), egui::accesskit::Role::Button);
        assert_eq!(
            current_thumbnail.toggled(),
            Some(egui::accesskit::Toggled::True)
        );
    }

    #[test]
    fn panel_menu_exposes_visible_shortcuts_as_accessible_accelerators() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let frame = accessibility_test_frame();
        let output = context.run_ui(accessibility_input(), |ui| {
            let mut actions = Vec::new();
            panels_menu(ui, &mut actions, frame.chrome_view_model());
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        for (label, shortcut) in [
            ("Tools", "T"),
            ("Folder Previews", "G"),
            ("Image Information", "I"),
        ] {
            let accessible_label = format!("{label} {shortcut}");
            let node = update
                .nodes
                .iter()
                .map(|(_, node)| node)
                .find(|node| node.label() == Some(accessible_label.as_str()))
                .unwrap_or_else(|| panic!("missing panel menu item: {label}"));
            assert_eq!(node.keyboard_shortcut(), Some(shortcut));
        }
    }

    #[test]
    fn image_information_reports_display_output_status() {
        fn exposed_text(frame: &UiFrameOwned) -> Vec<String> {
            let context = egui::Context::default();
            context.enable_accesskit();
            let output = context.run_ui(accessibility_input(), |ui| {
                let _ = render(ui, frame);
            });
            output
                .platform_output
                .accesskit_update
                .expect("display-status AccessKit update should be generated")
                .nodes
                .iter()
                .flat_map(|(_, node)| [node.label(), node.value()])
                .flatten()
                .map(str::to_owned)
                .collect()
        }

        let mut frame = accessibility_test_frame();
        frame.display_output = crate::display_state::DisplayOutputStatus::SrgbFallback;
        let fallback = exposed_text(&frame);
        assert!(
            fallback
                .iter()
                .any(|text| text.contains("Display · sRGB fallback")),
            "missing fallback display status; exposed: {fallback:?}"
        );

        frame.display_output = crate::display_state::DisplayOutputStatus::SrgbOperatingSystem;
        let managed = exposed_text(&frame);
        assert!(
            managed
                .iter()
                .any(|text| text.contains("Display · sRGB, operating-system managed")),
            "missing OS-managed display status; exposed: {managed:?}"
        );

        frame.display_output = crate::display_state::DisplayOutputStatus::SrgbDisplayProfileApplied;
        let applied = exposed_text(&frame);
        assert!(
            applied
                .iter()
                .any(|text| text.contains("Display · sRGB, display profile applied")),
            "missing applied display-profile status; exposed: {applied:?}"
        );
    }

    #[test]
    fn rating_menu_exposes_all_assignments_as_descriptive_radio_buttons() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.rating.state = crate::ratings::RatingState::Rated(
            crate::ratings::Rating::new(4).expect("valid test rating"),
        );
        let output = context.run_ui(accessibility_input(), |ui| {
            let mut actions = Vec::new();
            rating_menu(ui, &mut actions, frame.chrome_view_model());
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("rating-menu AccessKit update should be generated");
        let radios = update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .filter(|node| node.role() == egui::accesskit::Role::RadioButton)
            .collect::<Vec<_>>();
        assert_eq!(radios.len(), 6);
        for (expected, selected) in [
            ("Rating Unrated, shortcut 0", false),
            ("Rating 1 of 5, shortcut 1", false),
            ("Rating 2 of 5, shortcut 2", false),
            ("Rating 3 of 5, shortcut 3", false),
            ("Rating 4 of 5, shortcut 4", true),
            ("Rating 5 of 5, shortcut 5", false),
        ] {
            let node = radios
                .iter()
                .find(|node| node.label() == Some(expected))
                .unwrap_or_else(|| panic!("missing rating choice: {expected}"));
            assert_eq!(
                node.toggled(),
                Some(if selected {
                    egui::accesskit::Toggled::True
                } else {
                    egui::accesskit::Toggled::False
                })
            );
        }
    }

    #[test]
    fn no_image_rating_surfaces_name_the_required_next_step() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.dock.has_image = false;
        frame.file_path = None;
        frame.playlist_pos = None;
        frame.rating.folder_count = 0;
        frame.rating.current_catalog_index = None;

        assert_eq!(
            frame.chrome_view_model().rating_menu_label(),
            "Rating: Open an image"
        );
        assert_eq!(
            frame.chrome_view_model().rating_filter_menu_label(),
            "Rating Filter: Open a folder"
        );

        frame.is_opening = true;
        frame.folder_scan_busy = true;
        assert_eq!(
            frame.chrome_view_model().rating_menu_label(),
            "Rating: Loading image"
        );
        assert_eq!(
            frame.chrome_view_model().rating_filter_menu_label(),
            "Rating Filter: Reading folder..."
        );
        frame.is_opening = false;
        frame.folder_scan_busy = false;

        let output = context.run_ui(accessibility_input(), |ui| {
            let mut actions = Vec::new();
            rating_menu(ui, &mut actions, frame.chrome_view_model());
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("unavailable-rating AccessKit update should be generated");
        assert!(update.nodes.iter().any(|(_, node)| {
            node.value() == Some("Open an image to assign a rating.")
                || node.label() == Some("Open an image to assign a rating.")
        }));
        let radios = update
            .nodes
            .iter()
            .filter(|(_, node)| node.role() == egui::accesskit::Role::RadioButton)
            .collect::<Vec<_>>();
        assert_eq!(radios.len(), 6);
        assert!(radios.iter().all(|(_, node)| node.is_disabled()));
    }

    #[test]
    fn rating_filter_menu_exposes_all_thresholds_as_radio_buttons() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.rating.filter = crate::ratings::RatingFilter::AtLeast(
            crate::ratings::Rating::new(3).expect("valid test rating"),
        );
        let output = context.run_ui(accessibility_input(), |ui| {
            let mut actions = Vec::new();
            rating_filter_menu(ui, &mut actions, frame.chrome_view_model());
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("rating-filter AccessKit update should be generated");
        let radios = update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .filter(|node| node.role() == egui::accesskit::Role::RadioButton)
            .collect::<Vec<_>>();
        assert_eq!(radios.len(), 6);
        assert!(radios.iter().any(|node| {
            node.label() == Some("All images")
                && node.toggled() == Some(egui::accesskit::Toggled::False)
        }));
        for minimum in 1..=5 {
            let expected = format!("Rating filter: At least {minimum}");
            let node = radios
                .iter()
                .find(|node| node.label() == Some(expected.as_str()))
                .unwrap_or_else(|| panic!("missing rating filter: {expected}"));
            assert_eq!(
                node.toggled(),
                Some(if minimum == 3 {
                    egui::accesskit::Toggled::True
                } else {
                    egui::accesskit::Toggled::False
                })
            );
        }
    }

    #[test]
    fn save_overwrite_confirmation_is_explicit_and_focuses_cancel() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.save_overwrite_pending = true;

        let _ = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("Save As overwrite AccessKit update should be generated");
        let cancel_id = update
            .nodes
            .iter()
            .find_map(|(id, node)| {
                (node.role() == egui::accesskit::Role::Button && node.label() == Some("Cancel"))
                    .then_some(*id)
            })
            .expect("Save As overwrite Cancel button");
        assert_eq!(
            update.focus, cancel_id,
            "the non-destructive overwrite action should receive initial keyboard focus"
        );
        let mut enabled_buttons = update
            .nodes
            .iter()
            .filter_map(|(_, node)| {
                (node.role() == egui::accesskit::Role::Button && !node.is_disabled())
                    .then(|| node.label().map(str::to_owned))
                    .flatten()
            })
            .collect::<Vec<_>>();
        enabled_buttons.sort_unstable();
        assert_eq!(
            enabled_buttons,
            ["Cancel", "Replace file"],
            "only the overwrite dialog may expose enabled accessibility actions"
        );
        let exposed = update
            .nodes
            .iter()
            .flat_map(|(_, node)| [node.label(), node.value()])
            .flatten()
            .collect::<Vec<_>>();
        for expected in [
            "Replace existing file?",
            "The selected Save As destination exists.",
            "viewr rechecks this exact file immediately before replacement",
            "Replace file",
            "Cancel",
        ] {
            assert!(
                exposed.iter().any(|text| text.contains(expected)),
                "missing Save As overwrite content: {expected}; exposed: {exposed:?}"
            );
        }
    }

    #[test]
    fn save_overwrite_modal_discards_every_background_action() {
        let mut frame = accessibility_test_frame();
        frame.save_overwrite_pending = true;
        let actions = actions_owned_by_modal(
            vec![
                UiAction::Open,
                UiAction::Trash,
                UiAction::ConfirmSaveOverwrite,
                UiAction::CancelSaveOverwrite,
                UiAction::ToggleHeal,
            ],
            &frame,
        );

        assert_eq!(actions.len(), 2);
        assert!(matches!(actions[0], UiAction::ConfirmSaveOverwrite));
        assert!(matches!(actions[1], UiAction::CancelSaveOverwrite));
    }

    #[test]
    fn first_rating_write_discloses_file_metadata_effects_and_actions() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.rating.pending_disclosure = Some(crate::ratings::RatingAssignment::Set(
            crate::ratings::Rating::new(4).expect("valid test rating"),
        ));
        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("rating-disclosure AccessKit update should be generated");
        let cancel_id = update
            .nodes
            .iter()
            .find_map(|(id, node)| {
                (node.role() == egui::accesskit::Role::Button && node.label() == Some("Cancel"))
                    .then_some(*id)
            })
            .expect("rating disclosure Cancel button");
        assert_eq!(
            update.focus, cancel_id,
            "the safe disclosure action should receive initial keyboard focus"
        );
        let exposed = update
            .nodes
            .iter()
            .flat_map(|(_, node)| [node.label(), node.value()])
            .flatten()
            .collect::<Vec<_>>();
        for expected in [
            "Save rating 4 of 5?",
            "Ratings are written into this image file and may be visible to other apps.",
            "viewr updates embedded metadata in the source JPEG. It does not create a database or sidecar.",
            "Save rating",
            "Cancel",
        ] {
            assert!(
                exposed.iter().any(|text| text.contains(expected)),
                "missing rating disclosure content: {expected}; exposed: {exposed:?}"
            );
        }

        let mut tab_input = accessibility_input();
        tab_input.events.push(egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let tab_output = context.run_ui(tab_input, |ui| {
            let _ = render(ui, &frame);
        });
        let tab_update = tab_output
            .platform_output
            .accesskit_update
            .expect("tabbed rating-disclosure AccessKit update should be generated");
        assert_ne!(
            tab_update.focus, cancel_id,
            "the modal should not steal focus back to Cancel after its opening frame"
        );
    }

    #[test]
    fn empty_rating_filter_has_a_specific_recovery_state() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.rating.filter = crate::ratings::RatingFilter::AtLeast(
            crate::ratings::Rating::new(4).expect("valid test rating"),
        );
        frame.rating.match_count = 0;
        frame.rating.visible_position = None;
        frame.rating.folder_count = 12;
        frame.dock.has_image = false;
        frame.file_path = None;
        frame.playlist_pos = None;
        frame.filmstrip.clear();
        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("rating-empty-state AccessKit update should be generated");
        let nodes = update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .collect::<Vec<_>>();
        assert!(nodes.iter().any(|node| {
            node.role() == egui::accesskit::Role::Pane
                && node.label() == Some("No images match rating filter")
        }));
        assert!(nodes.iter().any(|node| {
            node.value() == Some("No images are rated 4 or higher.")
                && node.live() == Some(egui::accesskit::Live::Polite)
        }));
        for expected in [
            "No images are rated 4 or higher.",
            "12 images remain loaded in this folder.",
        ] {
            assert!(nodes.iter().any(|node| node.value() == Some(expected)));
        }
        assert!(nodes.iter().any(|node| {
            node.role() == egui::accesskit::Role::Button && node.label() == Some("Show all images")
        }));
        assert!(nodes.iter().any(|node| {
            node.role() == egui::accesskit::Role::Button
                && node.label() == Some("Show all images")
                && node.keyboard_shortcut() == Some("Esc")
        }));
        assert!(
            nodes
                .iter()
                .all(|node| { !matches!(node.label(), Some("Open File" | "Open Folder")) })
        );
    }

    #[test]
    fn first_image_names_a_busy_folder_scan() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.folder_scan_busy = true;
        frame.playlist_pos = None;
        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("folder-scan status AccessKit update should be generated");
        assert!(update.nodes.iter().any(|(_, node)| {
            node.value() == Some("Reading folder...") || node.label() == Some("Reading folder...")
        }));
    }

    #[test]
    fn rating_outcome_toast_is_polite_while_ordinary_toast_stays_non_live() {
        assert!(rating_toast_is_status("Rating 4 of 5 saved."));
        assert!(rating_toast_is_status(
            "Could not save the rating safely. The previous rating is unchanged."
        ));
        assert!(!rating_toast_is_status("Saving rating..."));
        assert!(!rating_toast_is_status("Saved copy · EXIF retained"));

        let rating_context = egui::Context::default();
        rating_context.enable_accesskit();
        let mut rating_frame = accessibility_test_frame();
        rating_frame.toast = Some("Rating 4 of 5 saved.".to_owned());
        let rating_output = rating_context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &rating_frame);
        });
        let rating_update = rating_output
            .platform_output
            .accesskit_update
            .expect("rating-toast AccessKit update should be generated");
        assert!(rating_update.nodes.iter().any(|(_, node)| {
            node.value() == Some("Rating 4 of 5 saved.")
                && node.live() == Some(egui::accesskit::Live::Polite)
        }));

        let ordinary_context = egui::Context::default();
        ordinary_context.enable_accesskit();
        let mut ordinary_frame = accessibility_test_frame();
        ordinary_frame.toast = Some("Saved copy · EXIF retained".to_owned());
        let ordinary_output = ordinary_context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &ordinary_frame);
        });
        let ordinary_update = ordinary_output
            .platform_output
            .accesskit_update
            .expect("ordinary-toast AccessKit update should be generated");
        assert!(ordinary_update.nodes.iter().any(|(_, node)| {
            node.value() == Some("Saved copy · EXIF retained")
                && node.live() != Some(egui::accesskit::Live::Polite)
        }));
    }

    #[test]
    fn source_privacy_reports_categories_and_an_honest_scan_limit() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.details = Some(crate::image_info::ImageDetails {
            exif_tag_count: 7,
            has_location: true,
            has_owner_or_author: true,
            has_device_identifier: true,
            has_description_or_comment: true,
            has_software_history: true,
            has_embedded_thumbnail: true,
            has_maker_specific_data: true,
            ..Default::default()
        });
        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let values = update
            .nodes
            .iter()
            .filter_map(|(_, node)| node.value())
            .collect::<Vec<_>>();
        for expected in [
            "Source Privacy",
            "7 supported EXIF tags detected.",
            "Present: location-related data",
            "Present: owner or author data",
            "Present: camera, lens, or image identifiers",
            "Present: description or comment data",
            "Present: software history",
            "Present: embedded thumbnail",
            "Present: maker-specific data",
            "Presence only. Sensitive values stay hidden on screen.",
            "Limited EXIF scan. Other metadata or hidden pixel data may still exist.",
        ] {
            assert!(
                values.contains(&expected),
                "missing privacy text: {expected}"
            );
        }
    }

    #[test]
    fn absent_supported_exif_is_not_presented_as_a_complete_privacy_verdict() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.details = Some(crate::image_info::ImageDetails::default());
        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let values = update
            .nodes
            .iter()
            .filter_map(|(_, node)| node.value())
            .collect::<Vec<_>>();
        assert!(values.contains(&"No supported EXIF detected."));
        assert!(
            values.contains(
                &"Limited EXIF scan. Other metadata or hidden pixel data may still exist."
            )
        );
        assert!(!values.iter().any(|value| value.contains("metadata-free")));
    }

    #[test]
    fn page_navigator_publishes_identifiable_accessible_controls() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.pages = Some(PageUiInfo {
            index: 1,
            count: 3,
            noun: "Page",
            can_previous: true,
            can_next: true,
            accessibility_label: "Page 2 of 3, 800 by 600".into(),
            visible_label: "Page 2 of 3".into(),
        });
        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let nodes = update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .collect::<Vec<_>>();
        assert!(
            nodes.iter().any(|node| {
                node.role() == egui::accesskit::Role::Button
                    && node.label() == Some("Previous page")
            }),
            "missing accessible previous-page action"
        );
        assert!(
            nodes
                .iter()
                .any(|node| node.role() == egui::accesskit::Role::Button
                    && node.label() == Some("Next page")),
            "missing accessible next-page action"
        );
        assert!(
            nodes
                .iter()
                .any(|node| node.label() == Some("Page 2 of 3, 800 by 600")
                    || node.value() == Some("Page 2 of 3, 800 by 600")),
            "missing identifiable page position"
        );
    }

    #[test]
    fn animation_controls_publish_playback_and_step_semantics() {
        for (is_playing, playback_action, hidden_action) in
            [(false, "Play", "Pause"), (true, "Pause", "Play")]
        {
            let context = egui::Context::default();
            context.enable_accesskit();
            let mut frame = accessibility_test_frame();
            frame.animation = Some(super::AnimationUiInfo {
                frame_index: 1,
                frame_count: 3,
                is_playing,
                can_previous: true,
                can_next: true,
            });
            let output = context.run_ui(accessibility_input(), |ui| {
                let _ = render(ui, &frame);
            });
            let update = output
                .platform_output
                .accesskit_update
                .expect("AccessKit update should be generated");
            let nodes = update
                .nodes
                .iter()
                .map(|(_, node)| node)
                .collect::<Vec<_>>();

            for expected in [playback_action, "Previous frame", "Next frame"] {
                assert!(
                    nodes.iter().any(|node| {
                        node.role() == egui::accesskit::Role::Button
                            && node.label() == Some(expected)
                    }),
                    "missing accessible animation action {expected}"
                );
            }
            assert!(
                nodes.iter().all(|node| {
                    node.role() != egui::accesskit::Role::Button
                        || node.label() != Some(hidden_action)
                }),
                "animation exposed both playback actions"
            );
            assert!(
                nodes.iter().any(|node| {
                    node.label() == Some("Frame 2 of 3") || node.value() == Some("Frame 2 of 3")
                }),
                "missing identifiable animation position"
            );
        }
    }

    #[test]
    fn open_with_context_action_explains_source_and_reload_boundaries() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.context_menu_pos = Some([100.0, 100.0]);
        assert!(control_enabled(&frame, ChromeControl::OpenWith));
        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let nodes = update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .collect::<Vec<_>>();
        assert!(
            nodes.iter().any(|node| {
                node.role() == egui::accesskit::Role::Button && node.label() == Some("Open With...")
            }),
            "missing accessible Open With action"
        );
        assert!(
            nodes
                .iter()
                .any(|node| node.value() == Some(OPEN_WITH_HELP))
        );
    }

    #[test]
    fn context_spot_heal_uses_shared_dynamic_state_and_accessibility() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.context_menu_pos = Some([100.0, 100.0]);
        frame.heal_supported = false;
        frame.heal_busy = true;

        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let node = update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| {
                node.role() == egui::accesskit::Role::Button
                    && node.label() == Some("Finishing Spot Heal... (J)")
            })
            .expect("dynamic Spot Heal context action");

        assert!(node.is_disabled());
    }

    #[test]
    fn active_tools_publish_selected_state_across_shared_adapters() {
        let adapter_context = egui::Context::default();
        adapter_context.enable_accesskit();
        let mut crop_frame = accessibility_test_frame();
        crop_frame.is_cropping = true;
        let crop = crop_frame.chrome_view_model().crop_control();
        let mut heal_frame = accessibility_test_frame();
        heal_frame.dock.heal_active = true;
        heal_frame.heal_busy = true;
        let heal = heal_frame.chrome_view_model().heal_control();
        let adapter_output = adapter_context.run_ui(accessibility_input(), |ui| {
            let _ = context_tool_button(ui, crop);
            let _ = menu_tool_button(ui, heal);
        });
        let adapter_update = adapter_output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        for label in ["Cancel Crop (Esc)", "Finish Spot Heal"] {
            let node = adapter_update
                .nodes
                .iter()
                .map(|(_, node)| node)
                .find(|node| node.label() == Some(label))
                .unwrap_or_else(|| panic!("missing active tool adapter: {label}"));
            assert_eq!(node.toggled(), Some(egui::accesskit::Toggled::True));
            assert_eq!(node.keyboard_shortcut(), Some("Esc"));
            assert!(!node.is_disabled());
        }

        let dock_context = egui::Context::default();
        dock_context.enable_accesskit();
        let dock_output = dock_context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &heal_frame);
        });
        let dock_update = dock_output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let dock_heal = dock_update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Spot heal (J)"))
            .expect("active dock Spot Heal action");
        assert_eq!(dock_heal.toggled(), Some(egui::accesskit::Toggled::True));
        assert!(!dock_heal.is_disabled());
    }

    #[test]
    fn a_missing_source_is_persistent_polite_status() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.source_gone = true;
        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        assert!(update.nodes.iter().any(|(_, node)| {
            node.role() == egui::accesskit::Role::Label
                && node.value() == Some(crate::file_coherence::current_gone_copy())
                && node.live() == Some(egui::accesskit::Live::Polite)
        }));
    }

    #[test]
    fn external_handoff_reminder_is_persistent_polite_status() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.external_edit_pending = true;
        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        assert!(update.nodes.iter().any(|(_, node)| {
            node.role() == egui::accesskit::Role::Label
                && node.value() == Some(EXTERNAL_EDIT_ACCESSIBLE_STATUS)
                && node.live() == Some(egui::accesskit::Live::Polite)
        }));
    }

    #[test]
    fn failed_reload_keeps_the_external_handoff_reminder_visible() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.external_edit_pending = true;
        frame.selected_file_name = Some("target.png".to_owned());
        frame.load_error = Some("Could not decode this image".to_owned());
        let mut input = accessibility_input();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::Vec2::new(640.0, 480.0),
        ));
        let mut badge_text_width = 0.0;
        let mut primary_token_width = 0.0;
        let output = context.run_ui(input, |ui| {
            badge_text_width = ui
                .painter()
                .layout_no_wrap(
                    EXTERNAL_EDIT_BADGE.to_owned(),
                    egui::FontId::proportional(11.5),
                    egui::Color32::WHITE,
                )
                .size()
                .x;
            primary_token_width = ui
                .painter()
                .layout_no_wrap(
                    "Could not".to_owned(),
                    egui::FontId::proportional(12.5),
                    egui::Color32::WHITE,
                )
                .size()
                .x;
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        assert!(
            !update
                .nodes
                .iter()
                .any(|(_, node)| node.label() == Some(EXTERNAL_EDIT_BADGE)),
            "the visual badge must not duplicate speech"
        );
        let status = update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| {
                node.role() == egui::accesskit::Role::Label
                    && node.value()
                        == Some(
                            "Source may have changed. Press F5 when it is safe to reload. Could not open target.png",
                        )
                    && node.live() == Some(egui::accesskit::Live::Polite)
            })
            .expect("combined external-edit and failed-reload status");
        let status_bounds = status.bounds().expect("top status bounds");
        assert!(
            f64::from(primary_token_width) <= status_bounds.x1 - status_bounds.x0,
            "the compact primary status token must fit beside the external badge"
        );

        let layout_context = egui::Context::default();
        let mut layout_input = accessibility_input();
        layout_input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::Vec2::new(TOP_STATUS_COMPACT_MAX_WIDTH, 48.0),
        ));
        let mut responses = None;
        let _ = layout_context.run_ui(layout_input, |ui| {
            responses = add_top_status_with_external_edit(
                ui,
                "Could not open target.png",
                true,
                chrome_colors_for(crate::theme::Mode::Dark),
            );
        });
        let (badge, primary) = responses.expect("external badge and primary status responses");
        assert!(
            badge_text_width <= badge.rect.width(),
            "the complete external badge must be visible"
        );
        assert!(
            primary_token_width <= primary.rect.width(),
            "the recognizable primary status token must be visible"
        );
        assert!(
            badge.rect.width() + primary.rect.width() + 4.0 <= TOP_STATUS_COMPACT_MAX_WIDTH + 1.0,
            "both visible status signals must stay inside the compact allocation"
        );
    }

    #[test]
    fn empty_state_publishes_file_access_scope_and_actions() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.dock.has_image = false;
        frame.file_path = None;
        frame.playlist_pos = None;
        frame.filmstrip.clear();
        let mut input = accessibility_input();
        let screen_rect =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(640.0, 480.0));
        input.screen_rect = Some(screen_rect);

        let output = context.run_ui(input, |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let nodes = update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .collect::<Vec<_>>();
        let labels = nodes
            .iter()
            .filter_map(|node| node.label())
            .collect::<Vec<_>>();
        let values = nodes
            .iter()
            .filter_map(|node| node.value())
            .collect::<Vec<_>>();

        for expected in [crate::shortcuts::FIRST_RUN_SCOPE, LOCAL_PRIVACY_SUMMARY] {
            assert!(
                nodes.iter().any(|node| {
                    node.role() == egui::accesskit::Role::Label && node.value() == Some(expected)
                }),
                "missing empty-state accessible text: {expected}; values: {values:?}"
            );
        }
        for expected in ["Open File", "Open Folder"] {
            assert!(
                nodes.iter().any(|node| {
                    node.role() == egui::accesskit::Role::Button && node.label() == Some(expected)
                }),
                "missing empty-state action: {expected}; labels: {labels:?}"
            );
        }

        let card = nodes
            .iter()
            .find(|node| {
                node.role() == egui::accesskit::Role::Pane && node.label() == Some("Open an image")
            })
            .expect("empty-state card node");
        let card_bounds = card.bounds().expect("empty-state card bounds");
        assert!(
            card_bounds.x0 >= f64::from(screen_rect.left())
                && card_bounds.x1 <= f64::from(screen_rect.right())
                && card_bounds.y0 >= f64::from(screen_rect.top())
                && card_bounds.y1 <= f64::from(screen_rect.bottom()),
            "empty-state card escaped the minimum window: {card_bounds:?}"
        );
        let scope = nodes
            .iter()
            .find(|node| node.value() == Some(crate::shortcuts::FIRST_RUN_SCOPE))
            .expect("scope text node");
        let open_file = nodes
            .iter()
            .find(|node| node.label() == Some("Open File"))
            .expect("Open File node");
        let open_folder = nodes
            .iter()
            .find(|node| node.label() == Some("Open Folder"))
            .expect("Open Folder node");
        let privacy = nodes
            .iter()
            .find(|node| node.value() == Some(LOCAL_PRIVACY_SUMMARY))
            .expect("privacy text node");
        for (name, node) in [
            ("scope", scope),
            ("Open File", open_file),
            ("Open Folder", open_folder),
            ("privacy", privacy),
        ] {
            assert_accesskit_node_within(name, node, card);
        }
        assert!(
            scope.bounds().expect("scope bounds").y1
                <= open_file.bounds().expect("Open File bounds").y0
        );
        assert!(
            open_file.bounds().expect("Open File bounds").y1
                <= privacy.bounds().expect("privacy bounds").y0
        );
    }

    fn empty_state_pane_label(frame: &UiFrameOwned) -> String {
        crate::shortcuts::empty_state_copy(
            frame.is_opening,
            frame.load_error.as_deref(),
            frame.selected_file_name.as_deref(),
        )
        .heading
    }

    fn assert_empty_state_geometry_stays_stable(frame: &UiFrameOwned, state: &str) {
        let context = egui::Context::default();
        context.enable_accesskit();
        let screen_rect =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(1_000.0, 720.0));
        let mut observed = Vec::new();
        let pane_label = empty_state_pane_label(frame);

        for _ in 0..8 {
            let mut input = accessibility_input();
            input.screen_rect = Some(screen_rect);
            let output = context.run_ui(input, |ui| {
                let _ = render(ui, frame);
            });
            let update = output
                .platform_output
                .accesskit_update
                .expect("empty-state AccessKit update should be generated");
            observed.push(
                update
                    .nodes
                    .iter()
                    .find_map(|(_, node)| {
                        (node.role() == egui::accesskit::Role::Pane
                            && node.label() == Some(pane_label.as_str()))
                        .then(|| node.bounds())
                        .flatten()
                    })
                    .expect("empty-state card bounds"),
            );
        }

        for pair in observed.windows(2) {
            let previous = pair[0];
            let current = pair[1];
            assert!(
                (previous.y0 - current.y0).abs() <= 1.0
                    && (previous.height() - current.height()).abs() <= 1.0,
                "unchanged {state} state moved between frames: {observed:?}"
            );
        }
    }

    #[test]
    fn empty_opening_and_error_geometry_stays_stable_across_unchanged_frames() {
        let mut frame = accessibility_test_frame();
        frame.dock.has_image = false;
        frame.file_path = None;
        frame.playlist_pos = None;
        frame.filmstrip.clear();
        assert_empty_state_geometry_stays_stable(&frame, "empty");

        frame.is_opening = true;
        frame.selected_file_name = Some("opening.png".to_owned());
        assert_empty_state_geometry_stays_stable(&frame, "opening");

        frame.is_opening = false;
        frame.load_error = Some("The selected image could not be decoded".to_owned());
        assert_empty_state_geometry_stays_stable(&frame, "error");
    }

    #[test]
    fn top_image_facts_have_distinct_reading_space() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let frame = accessibility_test_frame();
        let mut input = accessibility_input();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::Vec2::new(1_200.0, 800.0),
        ));

        let output = context.run_ui(input, |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("top-bar AccessKit update");
        let bounds_for = |value: &str| {
            update
                .nodes
                .iter()
                .map(|(_, node)| node)
                .find(|node| {
                    node.value() == Some(value)
                        && node.bounds().is_some_and(|bounds| {
                            bounds.y0 >= 0.0 && bounds.y1 <= f64::from(TOP_BAR_HEIGHT)
                        })
                })
                .unwrap_or_else(|| panic!("missing top-bar value: {value}"))
                .bounds()
                .unwrap_or_else(|| panic!("missing top-bar bounds: {value}"))
        };
        let name = bounds_for("current.png");
        let dimensions = bounds_for("1920 × 1080");
        let zoom = bounds_for("100%");
        let rating = bounds_for("Rating: Unrated");
        let position = bounds_for("1 / 2");

        assert!(
            dimensions.x0 - name.x1 >= 8.0,
            "filename and dimensions are crowded: {name:?}, {dimensions:?}"
        );
        assert!(
            zoom.x0 - dimensions.x1 >= 8.0,
            "dimensions and zoom are crowded: {dimensions:?}, {zoom:?}"
        );
        assert!(
            rating.x0 - zoom.x1 >= 8.0,
            "zoom and rating are crowded: {zoom:?}, {rating:?}"
        );
        assert!(
            position.x0 - rating.x1 >= 8.0,
            "rating and folder position are crowded: {rating:?}, {position:?}"
        );
    }

    #[test]
    fn top_status_names_the_embedded_rating_and_filtered_position() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.rating.state = crate::ratings::RatingState::Rated(
            crate::ratings::Rating::new(4).expect("valid test rating"),
        );
        frame.rating.filter = crate::ratings::RatingFilter::AtLeast(
            crate::ratings::Rating::new(4).expect("valid test rating"),
        );
        frame.rating.visible_position = Some((3, 3));
        frame.rating.match_count = 3;
        frame.rating.folder_count = 12;
        let mut input = accessibility_input();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::Vec2::new(1_200.0, 800.0),
        ));
        let output = context.run_ui(input, |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("top-status AccessKit update should be generated");
        let values = update
            .nodes
            .iter()
            .filter_map(|(_, node)| node.value())
            .collect::<Vec<_>>();
        for expected in ["3 / 3 rated 4+ · 12 total", "Rating: 4 of 5"] {
            assert!(
                values.contains(&expected),
                "missing rating status: {expected}; values: {values:?}"
            );
        }
    }

    #[test]
    fn about_surface_is_present_in_the_accessibility_tree() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.show_about = true;
        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let exposed = update
            .nodes
            .iter()
            .flat_map(|(_, node)| [node.label(), node.value()])
            .flatten()
            .collect::<Vec<_>>();
        for expected in [
            "About viewr",
            "A private, local-first image viewer",
            "No network access",
            "No telemetry, accounts, cloud sync, or background indexing.",
            "Photos and edits stay local unless you explicitly save a copy.",
            env!("CARGO_PKG_VERSION"),
            env!("CARGO_PKG_LICENSE"),
            "Close",
            "[ / ]  Previous / next page or frame",
            "F5  Reload file",
            "T G I  Panels",
            "Space  Fit; hold to pan",
            "U  Undo Trash",
        ] {
            assert!(
                exposed.iter().any(|text| text.contains(expected)),
                "missing About node: {expected}; exposed: {exposed:?}"
            );
        }
        let platform = format!("{} / {}", std::env::consts::OS, std::env::consts::ARCH);
        assert!(
            exposed.iter().any(|text| text.contains(&platform)),
            "missing About platform: {platform}; exposed: {exposed:?}"
        );
        assert!(
            exposed
                .iter()
                .all(|text| !text.contains("A / D") && !text.contains("`A`/`D`")),
            "About listed a navigation shortcut the event loop does not own: {exposed:?}"
        );

        let filtered = actions_owned_by_modal(
            vec![UiAction::Open, UiAction::CloseAbout, UiAction::Trash],
            &frame,
        );
        assert_eq!(filtered.len(), 1);
        assert!(matches!(filtered[0], UiAction::CloseAbout));

        let mut escape_input = accessibility_input();
        escape_input.events.push(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let mut escape_actions = Vec::new();
        let _ = context.run_ui(escape_input, |ui| {
            escape_actions = render(ui, &frame);
        });
        assert!(
            escape_actions
                .iter()
                .any(|action| matches!(action, UiAction::CloseAbout)),
            "Escape did not close About"
        );
    }

    #[test]
    fn about_close_stays_inside_the_minimum_window() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.show_about = true;
        let screen_rect =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(640.0, 480.0));
        let mut input = accessibility_input();
        input.screen_rect = Some(screen_rect);
        let output = context.run_ui(input, |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let close = update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Close"))
            .expect("About Close button");
        let bounds = close.bounds().expect("About Close bounds");
        assert!(
            bounds.x0 >= f64::from(screen_rect.left())
                && bounds.x1 <= f64::from(screen_rect.right())
                && bounds.y0 >= f64::from(screen_rect.top())
                && bounds.y1 <= f64::from(screen_rect.bottom()),
            "About Close escaped the minimum window: {bounds:?}"
        );
    }

    #[test]
    fn empty_state_bounds_long_decoder_errors() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.dock.has_image = false;
        frame.file_path = None;
        frame.playlist_pos = None;
        frame.filmstrip.clear();
        frame.selected_file_name = Some("night.png".to_owned());
        let long_error = format!("{}\nsecond line", "é".repeat(200));
        frame.load_error = Some(long_error.clone());
        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("empty-state AccessKit update should be generated");
        let values = update
            .nodes
            .iter()
            .filter_map(|(_, node)| node.value())
            .collect::<Vec<_>>();
        let bounded = crate::shortcuts::bound_user_error(&long_error);
        assert!(
            values.contains(&bounded.as_str()),
            "missing bounded empty-state error {bounded}; values: {values:?}"
        );
        assert!(
            values
                .iter()
                .all(|value| *value != long_error && !value.contains('\n')),
            "empty-state published an overflowing decoder error: {values:?}"
        );
    }

    #[test]
    fn update_surface_is_present_and_truthful_in_the_accessibility_tree() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.show_update = true;
        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let exposed = update
            .nodes
            .iter()
            .flat_map(|(_, node)| [node.label(), node.value()])
            .flatten()
            .collect::<Vec<_>>();
        for expected in [
            "Update viewr",
            concat!("Current version: ", env!("CARGO_PKG_VERSION")),
            "viewr never checks for or downloads updates by itself.",
            "Get latest release",
            "review its version and checksums",
            "hands off only the release URL",
            "Close",
        ] {
            assert!(
                exposed.iter().any(|text| text.contains(expected)),
                "missing update guidance: {expected}; exposed text: {exposed:?}"
            );
        }
        for forbidden in ["git pull", "latest version", "Download now"] {
            assert!(
                exposed.iter().all(|text| !text.contains(forbidden)),
                "update surface made an unsupported claim: {forbidden}; exposed text: {exposed:?}"
            );
        }
    }

    #[test]
    fn update_close_stays_inside_the_minimum_window() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.show_update = true;
        let screen_rect =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(640.0, 480.0));
        let mut input = accessibility_input();
        input.screen_rect = Some(screen_rect);
        let output = context.run_ui(input, |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let close = update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Close"))
            .expect("Update Close button");
        let bounds = close.bounds().expect("Update Close bounds");
        assert!(
            bounds.x0 >= f64::from(screen_rect.left())
                && bounds.x1 <= f64::from(screen_rect.right())
                && bounds.y0 >= f64::from(screen_rect.top())
                && bounds.y1 <= f64::from(screen_rect.bottom()),
            "Update Close escaped the minimum window: {bounds:?}"
        );
    }

    #[test]
    fn spot_heal_refinements_publish_named_controls() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.dock.heal_active = true;
        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("AccessKit update should be generated");
        let labels = update
            .nodes
            .iter()
            .filter_map(|(_, node)| node.label())
            .collect::<Vec<_>>();
        for expected in ["Brush radius", "Feather", "Source 1 of 4", "Done"] {
            assert!(
                labels.iter().any(|label| label.starts_with(expected)),
                "missing Spot Heal node: {expected}; labels: {labels:?}"
            );
        }
    }

    #[test]
    fn every_appearance_mode_renders_the_complete_chrome() {
        for mode in [
            crate::theme::Mode::Light,
            crate::theme::Mode::Dark,
            crate::theme::Mode::Console,
        ] {
            let context = egui::Context::default();
            let mut frame = accessibility_test_frame();
            frame.theme_mode = mode;
            let output = context.run_ui(accessibility_input(), |ui| {
                let _ = render(ui, &frame);
            });
            assert!(!output.shapes.is_empty());
        }
    }

    #[test]
    fn appearance_menu_exposes_scope_and_descriptive_radio_names() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let output = context.run_ui(accessibility_input(), |ui| {
            let mut actions = Vec::new();
            appearance_menu(
                ui,
                &mut actions,
                crate::theme::Preference::System,
                crate::theme::Mode::Dark,
            );
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("Appearance AccessKit update should be generated");
        let nodes = update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .collect::<Vec<_>>();
        assert!(nodes.iter().any(|node| {
            node.role() == egui::accesskit::Role::Label
                && node.value() == Some(APPEARANCE_SCOPE_HELP)
        }));
        for (expected, selected) in [
            (
                "System: Follows your operating system. Currently Dark.",
                true,
            ),
            (
                "Light: Bright neutral chrome, light window frame, soft-white canvas.",
                false,
            ),
            (
                "Dark: Low-glare charcoal chrome, dark window frame, deep-ink canvas.",
                false,
            ),
            (
                "Console: Green-screen look, near-black canvas, phosphor-green chrome, monospaced type.",
                false,
            ),
        ] {
            let node = nodes
                .iter()
                .find(|node| node.label() == Some(expected))
                .unwrap_or_else(|| panic!("missing Appearance radio: {expected}"));
            assert_eq!(node.role(), egui::accesskit::Role::RadioButton);
            assert_eq!(
                node.toggled(),
                Some(if selected {
                    egui::accesskit::Toggled::True
                } else {
                    egui::accesskit::Toggled::False
                })
            );
        }
    }

    #[test]
    fn appearance_recovery_toast_is_exposed_as_semantic_status_text() {
        const NOTICE: &str = "Could not restore saved appearance. Using System.";
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.toast = Some(NOTICE.to_owned());
        let output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let update = output
            .platform_output
            .accesskit_update
            .expect("toast AccessKit update should be generated");
        assert!(update.nodes.iter().any(|(_, node)| {
            node.role() == egui::accesskit::Role::Label
                && (node.label() == Some(NOTICE) || node.value() == Some(NOTICE))
        }));
    }

    #[test]
    fn crop_selection_publishes_exact_accessible_bounds() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.is_cropping = true;
        frame.crop_screen = Some([160.0, 120.0, 720.0, 560.0]);
        frame.crop_uv = Some([0.1, 0.2, 0.7, 0.8]);
        let crop_output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let crop_update = crop_output
            .platform_output
            .accesskit_update
            .expect("crop AccessKit update should be generated");
        assert!(crop_update.nodes.iter().any(|(_, node)| {
            node.label()
                == Some(
                    "Crop selection: 1152 by 649 output pixels, source starts at x 192, y 216. \
                     Drag inside to move. Arrow keys move; Shift plus Arrow keys resize.",
                )
        }));
    }

    #[test]
    fn tiny_crop_selection_keeps_exact_accessible_bounds_without_an_inner_drag_area() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.is_cropping = true;
        frame.crop_screen = Some([300.0, 240.0, 312.0, 252.0]);
        frame.crop_uv = Some([0.5, 0.5, 0.505, 0.505]);
        let (pixel_x, pixel_y, pixel_width, pixel_height) = crop_pixel_bounds(
            frame.img_size.unwrap(),
            frame.crop_uv.unwrap(),
            false,
            crate::crop::CropRatio::Free,
        )
        .unwrap();
        let expected = format!(
            "Crop selection: {pixel_width} by {pixel_height} output pixels, source starts at x \
             {pixel_x}, y {pixel_y}. Arrow keys move; Shift plus Arrow keys resize."
        );
        let crop_output = context.run_ui(accessibility_input(), |ui| {
            let _ = render(ui, &frame);
        });
        let crop_update = crop_output
            .platform_output
            .accesskit_update
            .expect("tiny crop AccessKit update should be generated");
        assert!(
            crop_update
                .nodes
                .iter()
                .any(|(_, node)| node.label() == Some(expected.as_str()))
        );
    }

    #[test]
    fn crop_dimensions_follow_the_visible_export_orientation() {
        assert_eq!(
            crop_pixel_bounds(
                (1_920, 1_080),
                [0.1, 0.2, 0.7, 0.8],
                false,
                crate::crop::CropRatio::Free,
            ),
            Some((192, 216, 1_152, 649))
        );
        assert_eq!(
            crop_pixel_bounds(
                (1_920, 1_080),
                [0.1, 0.2, 0.7, 0.8],
                true,
                crate::crop::CropRatio::Free,
            ),
            Some((192, 216, 649, 1_152))
        );
        assert_eq!(
            crop_pixel_bounds(
                (101, 101),
                [0.8, 0.8, 1.0, 1.0],
                false,
                crate::crop::CropRatio::Free,
            ),
            Some((80, 80, 21, 21))
        );
        assert_eq!(
            crop_pixel_bounds(
                (101, 101),
                [0.0, 0.0, 1.0, 1.0],
                false,
                crate::crop::CropRatio::SIXTEEN_NINE,
            ),
            Some((3, 24, 96, 54))
        );
    }
}
