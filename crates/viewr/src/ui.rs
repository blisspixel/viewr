//! Immediate-mode chrome: menus, docked collapsible panels, empty state, crop
//! overlay, and toasts.
//!
//! Design intent (see `docs/DESIGN.md`): persistent controls reserve their own
//! space and never cover the photo. Amber marks active tools only.

use egui::containers::scroll_area::ScrollBarVisibility;
use egui::{
    Align2, Area, Color32, CornerRadius, CursorIcon, Frame, Panel, Pos2, Rect, RichText,
    ScrollArea, Sense, Stroke, Vec2, WidgetInfo, WidgetType,
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
/// Logical width of the Image Info panel.
pub const IMAGE_INFO_PANEL_WIDTH: f32 = 304.0;

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
        top: TOP_BAR_HEIGHT * scale,
        bottom: match layout.filmstrip {
            DockState::Hidden => 0.0,
            DockState::Collapsed => FILMSTRIP_RAIL_HEIGHT,
            DockState::Expanded => FILMSTRIP_PANEL_HEIGHT,
        } * scale,
    }
}

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
    /// Toggle flag on the current image for batch cull.
    ToggleFlag,
    /// Move all flagged images to the system trash.
    TrashFlagged,
    /// Permanently delete the current image (UI will confirm).
    PermanentDelete,
}

/// Owned frame inputs for drawing chrome.
#[allow(clippy::struct_excessive_bools)] // independent UI mode bits for one frame
pub struct UiFrameOwned {
    /// Whether the Image Info side panel is open.
    pub show_image_info: bool,
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
    /// Whether the docked tools panel is visible at all.
    pub show_tools_panel: bool,
    /// Whether the visible tools panel is expanded.
    pub tools_panel_open: bool,
    /// Horizontal edge used by the tools panel.
    pub tools_panel_side: DockSide,
    /// Whether the docked folder-preview panel is visible at all.
    pub show_filmstrip_panel: bool,
    /// Whether the visible folder-preview panel is expanded.
    pub filmstrip_panel_open: bool,
    /// Horizontal edge used by Image Information.
    pub image_info_side: DockSide,
    /// Path of the current image (display only).
    pub file_path: Option<String>,
    /// Pixel dimensions of the current image, if any.
    pub img_size: Option<(u32, u32)>,
    /// Playback state for an animated image.
    pub animation: Option<AnimationUiInfo>,
    /// Best-effort local file and camera metadata.
    pub details: Option<crate::image_info::ImageDetails>,
    /// How the displayed pixels were normalized for the sRGB render pipeline.
    pub color_profile: Option<crate::decode::ColorProfileStatus>,
    /// Crop tool active.
    pub is_cropping: bool,
    /// Active crop aspect lock.
    pub crop_ratio: crate::crop::CropRatio,
    /// Session-local custom ratio fields shown by the crop picker.
    pub custom_crop_ratio: (u16, u16),
    /// Focused spot-heal mode is active.
    pub is_healing: bool,
    /// The displayed texture represents every source pixel and can be edited.
    pub can_heal: bool,
    /// A spot-heal worker is processing the current stroke.
    pub heal_busy: bool,
    /// Spot-heal radius in source-image pixels.
    pub heal_brush_radius: u32,
    /// Spot-heal feather as a percentage of brush radius.
    pub heal_feather_percent: u8,
    /// Selected and total ranked source patches for the latest repair.
    pub heal_source: Option<(usize, usize)>,
    /// Whether an in-memory pixel edit can be undone.
    pub can_undo_edit: bool,
    /// Whether an undone in-memory pixel edit can be reapplied.
    pub can_redo_edit: bool,
    /// Hand tool is currently dragging.
    pub is_panning: bool,
    /// Current image is in the flag set.
    pub is_flagged: bool,
    /// Number of flagged paths in the folder session.
    pub flag_count: usize,
    /// An image texture is loaded.
    pub has_image: bool,
    /// A requested image is currently decoding.
    pub is_loading: bool,
    /// Most recent decode failure for the selected path.
    pub load_error: Option<String>,
    /// An explicit Save As encode is running.
    pub save_busy: bool,
    /// A full-resolution crop is being applied off the UI thread.
    pub crop_busy: bool,
    /// 1-based index and total in the folder playlist, if known.
    pub playlist_pos: Option<(usize, usize)>,
    /// Physical display pixels per source-image pixel (`1.0` = actual size).
    pub pixel_scale: f32,
    /// Transient toast message (trash undo hint, etc.).
    pub toast: Option<String>,
    /// Neighbor filmstrip entries (index, name, flagged, optional texture).
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

/// Animation state shown in Image Information.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnimationUiInfo {
    /// Zero-based displayed frame index.
    pub frame_index: usize,
    /// Total decoded frames.
    pub frame_count: usize,
    /// Whether timed playback is active.
    pub is_playing: bool,
}

impl UiFrameOwned {
    fn current_selection_ready(&self) -> bool {
        self.has_image
            && !self.is_loading
            && self.load_error.is_none()
            && !self.crop_busy
            && !self.save_busy
    }
}

/// One cell in the progressive bottom filmstrip.
#[derive(Clone)]
pub struct FilmstripItem {
    /// Playlist index.
    pub index: usize,
    /// File basename for tooltip / fallback label.
    pub name: String,
    /// Whether the path is flagged for batch cull.
    pub flagged: bool,
    /// Thumbnail texture when ready.
    pub texture: Option<egui::TextureHandle>,
}

/// Render the UI overlays and return a list of actions triggered by the user.
pub fn render(ui: &mut egui::Ui, frame: &UiFrameOwned) -> Vec<UiAction> {
    let mut actions = Vec::new();
    apply_chrome_theme(ui.ctx(), frame.theme_mode);
    let colors = chrome_colors(ui);

    render_top_menu(ui, &mut actions, frame);
    if frame.show_about {
        render_about(ui, &mut actions);
    }

    if !frame.has_image {
        render_empty_state(
            ui,
            &mut actions,
            frame.is_loading,
            frame.load_error.as_deref(),
        );
        if let Some(msg) = &frame.toast {
            render_toast(ui, msg, frame);
        }
        return actions;
    }

    if let Some(pos) = frame.context_menu_pos {
        let mut close = false;
        egui::Window::new("Quick Tools")
            .fixed_pos(Pos2::new(pos[0], pos[1]))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Spot Heal (J)").clicked() {
                        actions.push(UiAction::ToggleHeal);
                        close = true;
                    }
                    if ui.button("Crop (C)").clicked() {
                        actions.push(UiAction::ToggleCrop);
                        close = true;
                    }
                });
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
                let response = ui.add(slider);
                response.widget_info(|| {
                    WidgetInfo::slider(ui.is_enabled(), f64::from(radius), "Heal brush radius")
                });
                if response.changed() {
                    actions.push(UiAction::SetHealBrushRadius(radius));
                }
                let mut feather = frame.heal_feather_percent;
                ui.label(RichText::new("Heal Feather").size(11.5).color(colors.muted));
                let response = ui.add(
                    egui::Slider::new(&mut feather, 0..=crate::heal::MAX_FEATHER_PERCENT)
                        .suffix("%"),
                );
                response.widget_info(|| {
                    WidgetInfo::slider(ui.is_enabled(), f64::from(feather), "Heal feather")
                });
                if response.changed() {
                    actions.push(UiAction::SetHealFeather(feather));
                }
            });

        if close
            || (ui.ctx().input(|i| i.pointer.any_pressed()) && !ui.ctx().is_pointer_over_egui())
        {
            actions.push(UiAction::CloseContextMenu);
        }
    }

    if frame.show_image_info {
        render_image_info_panel(ui, &mut actions, frame);
    }

    if frame.show_filmstrip_panel && frame.filmstrip.len() > 1 {
        render_filmstrip(ui, &mut actions, frame);
    }

    if frame.show_tools_panel {
        render_tools_panel(ui, &mut actions, frame);
    }

    if frame.is_healing {
        render_heal_panel(ui, &mut actions, frame);
        render_heal_overlay(ui, frame);
    }

    if let Some(msg) = &frame.toast {
        render_toast(ui, msg, frame);
    }

    if frame.is_cropping {
        render_crop_overlay(ui, frame, &mut actions);
    }

    apply_cursor(ui, frame);
    actions
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

fn render_top_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    let colors = chrome_colors(ui);
    Panel::top("top_panel")
        .exact_size(TOP_BAR_HEIGHT)
        .resizable(false)
        .frame(menu_frame(colors))
        .show(ui, |ui| {
            ui.spacing_mut().button_padding = Vec2::new(10.0, 4.0);
            ui.visuals_mut().widgets.inactive.bg_fill = Color32::TRANSPARENT;
            ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
            ui.visuals_mut().widgets.hovered.bg_fill = colors.raised;
            ui.visuals_mut().widgets.hovered.weak_bg_fill = colors.raised;
            ui.visuals_mut().widgets.active.bg_fill = colors.active;
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                file_menu(ui, actions, frame);
                edit_menu(ui, actions, frame);
                view_menu(ui, actions, frame);
                tools_menu(ui, actions, frame);
                help_menu(ui, actions);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if frame.load_error.is_some() {
                        if ui
                            .add(egui::Button::new("Retry").min_size(Vec2::new(58.0, 30.0)))
                            .on_hover_text("Retry opening the selected image")
                            .clicked()
                        {
                            actions.push(UiAction::RetryLoad);
                        }
                        ui.label(RichText::new("Open failed").size(12.5).color(colors.muted));
                    } else if frame.is_loading {
                        ui.add(egui::Spinner::new().size(14.0).color(colors.accent));
                        ui.label(RichText::new("Opening...").size(12.5).color(colors.muted));
                    } else if frame.save_busy {
                        ui.add(egui::Spinner::new().size(14.0).color(colors.accent));
                        ui.label(RichText::new("Saving...").size(12.5).color(colors.muted));
                    } else if frame.crop_busy {
                        ui.add(egui::Spinner::new().size(14.0).color(colors.accent));
                        ui.label(
                            RichText::new("Applying crop...")
                                .size(12.5)
                                .color(colors.muted),
                        );
                    }
                    if let Some((i, n)) = frame.playlist_pos {
                        Frame::new()
                            .fill(colors.raised)
                            .corner_radius(CornerRadius::same(6))
                            .inner_margin(egui::Margin::symmetric(8, 3))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(format!("{i} / {n}"))
                                        .size(12.5)
                                        .color(colors.muted),
                                );
                            });
                    }
                    let show_details = ui.ctx().content_rect().width() >= 720.0;
                    if show_details && frame.has_image {
                        ui.label(
                            RichText::new(format!("{:.0}%", frame.pixel_scale * 100.0))
                                .size(12.5)
                                .color(colors.muted),
                        );
                    }
                    if show_details && let Some((w, h)) = frame.img_size {
                        ui.label(
                            RichText::new(format!("{w} × {h}"))
                                .size(12.5)
                                .color(colors.muted),
                        );
                    }
                    if show_details
                        && let Some(name) = frame.file_path.as_ref().and_then(|path| {
                            std::path::Path::new(path)
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                        })
                    {
                        let response = ui.add(
                            egui::Label::new(RichText::new(&name).size(12.5).color(colors.text))
                                .truncate(),
                        );
                        let _ = response.on_hover_text(name);
                    }
                });
            });
        });
}

fn file_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    let colors = chrome_colors(ui);
    ui.menu_button(RichText::new("File").size(13.5).color(colors.text), |ui| {
        ui.set_min_width(238.0);
        if ui
            .add(egui::Button::new("Open File...").shortcut_text(format!("{PRIMARY_MODIFIER}+O")))
            .clicked()
        {
            actions.push(UiAction::Open);
            ui.close();
        }
        if ui
            .add(
                egui::Button::new("Open Folder...")
                    .shortcut_text(format!("{PRIMARY_MODIFIER}+Shift+O")),
            )
            .clicked()
        {
            actions.push(UiAction::OpenFolder);
            ui.close();
        }
        if ui
            .add_enabled(
                frame.current_selection_ready() && !frame.heal_busy,
                egui::Button::new("Reload File").shortcut_text("F5"),
            )
            .clicked()
        {
            actions.push(UiAction::Reload);
            ui.close();
        }
        if ui
            .add_enabled(
                frame.current_selection_ready() && !frame.heal_busy && !frame.save_busy,
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
                frame.current_selection_ready(),
                egui::Button::new("Flag for review").shortcut_text("X"),
            )
            .clicked()
        {
            actions.push(UiAction::ToggleFlag);
            ui.close();
        }
        if ui
            .add_enabled(
                frame.flag_count > 0 && !frame.save_busy && !frame.crop_busy && !frame.heal_busy,
                egui::Button::new(format!("Move {} flagged to Trash", frame.flag_count))
                    .shortcut_text("B"),
            )
            .clicked()
        {
            actions.push(UiAction::TrashFlagged);
            ui.close();
        }
        if ui
            .add_enabled(
                frame.current_selection_ready(),
                egui::Button::new("Move to Trash").shortcut_text("Delete"),
            )
            .clicked()
        {
            actions.push(UiAction::Trash);
            ui.close();
        }
        if ui
            .add_enabled(
                frame.current_selection_ready(),
                egui::Button::new("Permanently Delete...").shortcut_text("Shift+Delete"),
            )
            .clicked()
        {
            actions.push(UiAction::PermanentDelete);
            ui.close();
        }
        if ui
            .add(egui::Button::new("Undo Trash").shortcut_text("U"))
            .clicked()
        {
            actions.push(UiAction::UndoTrash);
            ui.close();
        }
    });
}

fn edit_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    let colors = chrome_colors(ui);
    ui.menu_button(RichText::new("Edit").size(13.5).color(colors.text), |ui| {
        ui.set_min_width(210.0);
        let crop_label = if frame.is_cropping {
            "Cancel Crop"
        } else {
            "Crop"
        };
        let crop_shortcut = if frame.is_cropping { "Esc" } else { "C" };
        if ui
            .add_enabled(
                frame.current_selection_ready(),
                egui::Button::new(crop_label).shortcut_text(crop_shortcut),
            )
            .clicked()
        {
            actions.push(UiAction::ToggleCrop);
            ui.close();
        }
        if frame.is_cropping
            && ui
                .add(egui::Button::new("Apply Crop").shortcut_text("Enter"))
                .clicked()
        {
            actions.push(UiAction::ApplyCrop);
            ui.close();
        }
        spot_heal_menu_items(ui, actions, frame);
        ui.separator();
        if ui
            .add_enabled(
                frame.current_selection_ready(),
                egui::Button::new("Rotate Clockwise").shortcut_text("R"),
            )
            .clicked()
        {
            actions.push(UiAction::RotateCw);
            ui.close();
        }
        if ui
            .add_enabled(
                frame.current_selection_ready(),
                egui::Button::new("Rotate Counterclockwise").shortcut_text("L"),
            )
            .clicked()
        {
            actions.push(UiAction::RotateCcw);
            ui.close();
        }
        if ui
            .add_enabled(
                frame.current_selection_ready(),
                egui::Button::new("Flip Horizontally").shortcut_text("H"),
            )
            .clicked()
        {
            actions.push(UiAction::FlipH);
            ui.close();
        }
        if ui
            .add_enabled(
                frame.current_selection_ready(),
                egui::Button::new("Flip Vertically").shortcut_text("V"),
            )
            .clicked()
        {
            actions.push(UiAction::FlipV);
            ui.close();
        }
    });
}

fn tools_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    let colors = chrome_colors(ui);
    ui.menu_button(RichText::new("Tools").size(13.5).color(colors.text), |ui| {
        ui.set_min_width(210.0);
        let crop_label = if frame.is_cropping {
            "Cancel Crop"
        } else {
            "Crop"
        };
        let crop_shortcut = if frame.is_cropping { "Esc" } else { "C" };
        if ui
            .add_enabled(
                frame.current_selection_ready(),
                egui::Button::new(crop_label).shortcut_text(crop_shortcut),
            )
            .clicked()
        {
            actions.push(UiAction::ToggleCrop);
            ui.close();
        }

        let heal_label = if frame.is_healing {
            "Finish Spot Heal"
        } else if frame.heal_busy {
            "Finishing Spot Heal..."
        } else {
            "Spot Heal"
        };
        let heal_shortcut = if frame.is_healing { "Esc" } else { "J" };
        if ui
            .add_enabled(
                frame.can_heal && (!frame.heal_busy || frame.is_healing),
                egui::Button::new(heal_label).shortcut_text(heal_shortcut),
            )
            .clicked()
        {
            actions.push(UiAction::ToggleHeal);
            ui.close();
        }
    });
}

fn spot_heal_menu_items(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    let heal_label = if frame.is_healing {
        "Finish Spot Heal"
    } else if frame.heal_busy {
        "Finishing Spot Heal..."
    } else {
        "Spot Heal"
    };
    let shortcut = if frame.is_healing { "Esc" } else { "J" };
    if ui
        .add_enabled(
            frame.can_heal && (!frame.heal_busy || frame.is_healing),
            egui::Button::new(heal_label).shortcut_text(shortcut),
        )
        .clicked()
    {
        actions.push(UiAction::ToggleHeal);
        ui.close();
    }
    ui.separator();
    if ui
        .add_enabled(
            frame.can_undo_edit,
            egui::Button::new("Undo Spot Heal").shortcut_text(format!("{PRIMARY_MODIFIER}+Z")),
        )
        .clicked()
    {
        actions.push(UiAction::UndoEdit);
        ui.close();
    }
    if ui
        .add_enabled(
            frame.can_redo_edit,
            egui::Button::new("Redo Spot Heal")
                .shortcut_text(format!("{PRIMARY_MODIFIER}+Shift+Z")),
        )
        .clicked()
    {
        actions.push(UiAction::RedoEdit);
        ui.close();
    }
}

fn view_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    let colors = chrome_colors(ui);
    ui.menu_button(RichText::new("View").size(13.5).color(colors.text), |ui| {
        ui.set_min_width(228.0);
        if ui
            .add_enabled(
                frame.has_image,
                egui::Button::new("Fit Image to View").shortcut_text("0"),
            )
            .clicked()
        {
            actions.push(UiAction::FitToView);
            ui.close();
        }
        if ui
            .add_enabled(
                frame.has_image,
                egui::Button::new("Actual Size").shortcut_text("1"),
            )
            .clicked()
        {
            actions.push(UiAction::ActualSize);
            ui.close();
        }
        if ui
            .add_enabled(
                frame.has_image,
                egui::Button::new("Zoom In").shortcut_text("+"),
            )
            .clicked()
        {
            actions.push(UiAction::ZoomIn);
            ui.close();
        }
        if ui
            .add_enabled(
                frame.has_image,
                egui::Button::new("Zoom Out").shortcut_text("-"),
            )
            .clicked()
        {
            actions.push(UiAction::ZoomOut);
            ui.close();
        }
        ui.separator();
        if ui
            .add(egui::Button::new("Fullscreen").shortcut_text("F"))
            .clicked()
        {
            actions.push(UiAction::ToggleFullscreen);
            ui.close();
        }
        ui.separator();
        ui.menu_button("Panels", |ui| panels_menu(ui, actions, frame));
        ui.menu_button("Panel Position", |ui| {
            panel_position_menu(ui, actions, frame);
        });
        ui.separator();
        ui.menu_button("Image Background", |ui| {
            background_menu(ui, actions, frame.background_override);
        });
        ui.menu_button("Appearance", |ui| {
            appearance_menu(ui, actions, frame.theme_preference);
        });
    });
}

fn panels_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    ui.set_min_width(224.0);
    let choices = [
        (
            "Tools",
            "T",
            frame.has_image,
            frame.show_tools_panel,
            UiAction::ToggleToolsPanelVisibility,
        ),
        (
            "Folder Previews",
            "G",
            frame.filmstrip.len() > 1,
            frame.show_filmstrip_panel,
            UiAction::ToggleFilmstripPanelVisibility,
        ),
        (
            "Image Information",
            "I",
            frame.has_image,
            frame.show_image_info,
            UiAction::ToggleImageInfo,
        ),
    ];
    for (label, shortcut, enabled, selected, action) in choices {
        let mut checked = selected;
        let response = ui
            .add_enabled(enabled, egui::Checkbox::new(&mut checked, label))
            .on_hover_text(format!("{label} ({shortcut})"));
        if response.changed() {
            actions.push(action);
            ui.close();
        }
    }
}

fn panel_position_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    ui.set_min_width(224.0);
    dock_side_choices(
        ui,
        actions,
        "TOOLS",
        "Tools",
        frame.tools_panel_side,
        UiAction::SetToolsPanelSide,
    );
    ui.separator();
    dock_side_choices(
        ui,
        actions,
        "IMAGE INFORMATION",
        "Image Information",
        frame.image_info_side,
        UiAction::SetImageInfoSide,
    );
}

fn dock_side_choices(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    heading: &str,
    accessibility_heading: &str,
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
    for (side, label) in [(DockSide::Left, "Left"), (DockSide::Right, "Right")] {
        let response = ui.radio(current == side, label);
        response.widget_info(|| {
            WidgetInfo::selected(
                WidgetType::RadioButton,
                ui.is_enabled(),
                current == side,
                format!("{accessibility_heading}: {label}"),
            )
        });
        if response.clicked() {
            actions.push(action(side));
            ui.close();
        }
    }
}

fn background_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, current: Option<[f64; 4]>) {
    ui.set_min_width(172.0);
    let choices = [
        ("Theme Default", None),
        ("Black", Some([0.0, 0.0, 0.0, 1.0])),
        ("Neutral Gray", Some([0.2, 0.2, 0.2, 1.0])),
        ("White", Some([1.0, 1.0, 1.0, 1.0])),
    ];
    for (label, value) in choices {
        if ui.radio(current == value, label).clicked() {
            actions.push(UiAction::SetBackground(value));
            ui.close();
        }
    }
}

fn appearance_menu(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    current: crate::theme::Preference,
) {
    ui.set_min_width(172.0);
    let choices = [
        (crate::theme::Preference::System, "System"),
        (crate::theme::Preference::Light, "Light"),
        (crate::theme::Preference::Dark, "Dark"),
        (crate::theme::Preference::Console, "Console"),
    ];
    for (preference, label) in choices {
        if ui.radio(current == preference, label).clicked() {
            actions.push(UiAction::SetTheme(preference));
            ui.close();
        }
    }
}

fn help_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>) {
    let colors = chrome_colors(ui);
    ui.menu_button(RichText::new("Help").size(13.5).color(colors.text), |ui| {
        ui.set_min_width(180.0);
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
            ui.set_max_width(420.0);
            ui.vertical_centered(|ui| {
                ui.heading(RichText::new("About viewr").size(28.0).color(colors.text));
                ui.label(
                    RichText::new("A private, local-first image viewer")
                        .size(14.0)
                        .color(colors.muted),
                );
            });
            ui.add_space(12.0);
            Frame::new()
                .fill(colors.raised)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("No network access")
                            .color(colors.text)
                            .strong(),
                    );
                    ui.label("No telemetry, accounts, cloud sync, or background indexing.");
                    ui.label("Photos and edits stay local unless you explicitly save a copy.");
                });
            ui.add_space(12.0);
            egui::Grid::new("about_build_details")
                .num_columns(2)
                .spacing(Vec2::new(16.0, 6.0))
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
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Shortcuts: O open, arrows browse, 0 fit, 1 actual size")
                        .size(11.5)
                        .color(colors.muted),
                );
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
            "About viewr. Private local-first image viewer. No network access, telemetry, accounts, or background indexing.",
        )
    });
    if close_clicked || response.should_close() {
        actions.push(UiAction::CloseAbout);
    }
}

fn render_empty_state(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    is_loading: bool,
    load_error: Option<&str>,
) {
    let colors = chrome_colors(ui);
    let screen = ui.ctx().content_rect();
    let card_width = (screen.width() - 40.0).clamp(280.0, 420.0);
    let card_height = if is_loading { 188.0 } else { 250.0 };
    let card_rect = Rect::from_center_size(screen.center(), Vec2::new(card_width, card_height));
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt("empty_state")
            .max_rect(card_rect)
            .layout(egui::Layout::top_down(egui::Align::Center)),
        |ui| {
            ui.set_min_width(card_width);
            Frame::new()
                .fill(colors.panel)
                .corner_radius(CornerRadius::same(12))
                .stroke(Stroke::new(1.0, colors.border))
                .inner_margin(egui::Margin::symmetric(28, 24))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        if is_loading {
                            ui.add(egui::Spinner::new().size(28.0).color(colors.accent));
                        } else {
                            let (icon_rect, _) =
                                ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
                            paint_empty_image_icon(ui.painter(), icon_rect, colors);
                        }
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new(if is_loading {
                                "Opening image"
                            } else if load_error.is_some() {
                                "Could not open image"
                            } else {
                                "Open an image"
                            })
                            .size(20.0)
                            .color(colors.text)
                            .strong(),
                        );
                        ui.add_space(8.0);
                        let description = if is_loading {
                            "Decoding locally while the window stays responsive."
                        } else {
                            load_error
                                .unwrap_or("Drop a file or folder here, or choose where to start.")
                        };
                        ui.label(RichText::new(description).size(13.0).color(colors.muted));
                        if !is_loading {
                            ui.add_space(16.0);
                            ui.horizontal_centered(|ui| {
                                if load_error.is_some()
                                    && ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new("Retry").color(colors.accent_ink),
                                            )
                                            .fill(colors.accent)
                                            .min_size(Vec2::new(92.0, 36.0)),
                                        )
                                        .clicked()
                                {
                                    actions.push(UiAction::RetryLoad);
                                }
                                if ui
                                    .add(
                                        egui::Button::new("Open File")
                                            .min_size(Vec2::new(116.0, 36.0)),
                                    )
                                    .clicked()
                                {
                                    actions.push(UiAction::Open);
                                }
                                if ui
                                    .add(
                                        egui::Button::new("Open Folder")
                                            .min_size(Vec2::new(116.0, 36.0)),
                                    )
                                    .clicked()
                                {
                                    actions.push(UiAction::OpenFolder);
                                }
                            });
                            ui.add_space(12.0);
                        }
                        ui.label(
                            RichText::new("Maximum privacy. It just works.")
                                .size(12.0)
                                .color(colors.muted),
                        );
                    });
                });
        },
    );
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

fn render_image_info_panel(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    let colors = chrome_colors(ui);
    let panel = match frame.image_info_side {
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
            render_file_info(ui, actions, frame);
            render_capture_info(ui, frame);
            render_review_and_privacy(ui, actions, frame);
        });
}

fn render_file_info(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    let colors = chrome_colors(ui);
    ui.separator();
    ui.label(RichText::new("File").color(colors.muted).small().strong());
    ui.add_space(4.0);
    if let Some(path) = &frame.file_path {
        let name = std::path::Path::new(path).file_name().map_or_else(
            || path.clone(),
            |value| value.to_string_lossy().into_owned(),
        );
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
    if let Some(animation) = frame.animation {
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
    if details.has_location {
        ui.label(RichText::new("GPS location is present in the source").color(colors.muted));
    }
}

fn render_review_and_privacy(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    let colors = chrome_colors(ui);
    ui.add_space(8.0);
    ui.separator();
    ui.label(RichText::new("Review").color(colors.muted).small().strong());
    ui.add_space(4.0);
    ui.label(
        RichText::new(if frame.is_flagged {
            "Flagged for review"
        } else {
            "Not flagged"
        })
        .color(if frame.is_flagged {
            colors.accent
        } else {
            colors.muted
        }),
    );
    ui.label(
        RichText::new(format!("{} flagged in this folder", frame.flag_count)).color(colors.muted),
    );
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
        || details.has_location
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
    Flag,
}

#[derive(Clone, Copy)]
enum ChevronDirection {
    Left,
    Right,
    Up,
    Down,
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

fn render_tools_panel(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    let colors = chrome_colors(ui);
    let width = if frame.tools_panel_open {
        TOOLS_PANEL_WIDTH
    } else {
        TOOLS_RAIL_WIDTH
    };
    let panel = match frame.tools_panel_side {
        DockSide::Left => Panel::left("tools_panel"),
        DockSide::Right => Panel::right("tools_panel"),
    };
    panel
        .exact_size(width)
        .resizable(false)
        .frame(docked_frame(colors))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                let (direction, label) = match (frame.tools_panel_side, frame.tools_panel_open) {
                    (DockSide::Left, true) => (ChevronDirection::Left, "Collapse tools panel"),
                    (DockSide::Left, false) => (ChevronDirection::Right, "Expand tools panel"),
                    (DockSide::Right, true) => (ChevronDirection::Right, "Collapse tools panel"),
                    (DockSide::Right, false) => (ChevronDirection::Left, "Expand tools panel"),
                };
                if disclosure_button(ui, direction, label, frame.tools_panel_open).clicked() {
                    actions.push(UiAction::ToggleToolsPanelExpansion);
                }

                if frame.tools_panel_open {
                    ui.label(
                        RichText::new("TOOLS")
                            .size(10.0)
                            .color(colors.muted)
                            .strong(),
                    );
                    ui.separator();
                    ui.set_width(44.0);
                    ui.add_enabled_ui(frame.current_selection_ready(), |ui| {
                        ui.vertical_centered(|ui| {
                        ui.spacing_mut().item_spacing.y = 5.0;
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
                        ui.add_space(4.0);
                        icon_btn(ui, ToolIcon::Crop, "Crop (C)", frame.is_cropping, || {
                            actions.push(UiAction::ToggleCrop);
                        });
                        let heal_tip = if frame.can_heal {
                            "Spot heal (J)"
                        } else {
                            "Spot heal is unavailable for images larger than the GPU texture limit"
                        };
                        ui.add_enabled_ui(frame.can_heal || frame.is_healing, |ui| {
                            icon_btn(ui, ToolIcon::Heal, heal_tip, frame.is_healing, || {
                                actions.push(UiAction::ToggleHeal);
                            });
                        });
                        icon_btn(
                            ui,
                            ToolIcon::Flag,
                            "Flag for review (X)",
                            frame.is_flagged,
                            || {
                                actions.push(UiAction::ToggleFlag);
                            },
                        );
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
        ToolIcon::Flag => {
            let points = [
                c + Vec2::new(-s * 0.5, s),
                c + Vec2::new(-s * 0.5, -s),
                c + Vec2::new(s * 0.6, -s * 0.55),
                c + Vec2::new(-s * 0.5, -s * 0.1),
            ];
            painter.line_segment([points[0], points[1]], stroke);
            painter.line_segment([points[1], points[2]], stroke);
            painter.line_segment([points[2], points[3]], stroke);
        }
    }
}

fn render_heal_panel(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    let colors = chrome_colors(ui);
    let panel = match frame.tools_panel_side {
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
                    if ui.button("Done").clicked() {
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
            render_heal_controls(ui, actions, frame, colors);
            render_heal_guidance(ui, frame, colors);
        });
}

fn render_heal_controls(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    frame: &UiFrameOwned,
    colors: ChromeColors,
) {
    let mut radius = frame.heal_brush_radius;
    ui.label(RichText::new("Brush radius").size(11.5).color(colors.muted));
    let slider = egui::Slider::new(
        &mut radius,
        crate::heal::MIN_BRUSH_RADIUS..=crate::heal::MAX_BRUSH_RADIUS,
    )
    .suffix(" px");
    let response = ui.add_enabled(!frame.heal_busy, slider);
    response.widget_info(|| WidgetInfo::slider(ui.is_enabled(), f64::from(radius), "Brush radius"));
    if response.changed() {
        actions.push(UiAction::SetHealBrushRadius(radius));
    }

    ui.add_space(10.0);
    let mut feather = frame.heal_feather_percent;
    ui.label(RichText::new("Feather").size(11.5).color(colors.muted));
    let feather_slider =
        egui::Slider::new(&mut feather, 0..=crate::heal::MAX_FEATHER_PERCENT).suffix("%");
    let response = ui
        .add_enabled(!frame.heal_busy, feather_slider)
        .on_hover_text("Softens the repair edge outward from the painted area");
    response.widget_info(|| WidgetInfo::slider(ui.is_enabled(), f64::from(feather), "Feather"));
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
            !frame.heal_busy && frame.heal_source.is_some(),
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
            .add_enabled(frame.can_undo_edit, egui::Button::new("Undo"))
            .clicked()
        {
            actions.push(UiAction::UndoEdit);
        }
        if ui
            .add_enabled(frame.can_redo_edit, egui::Button::new("Redo"))
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
    } else {
        ui.add(
            egui::Label::new(
                RichText::new(format!(
                    "Drag to paint  |  / next source  |  {PRIMARY_MODIFIER}+Z undo  |  Esc finish"
                ))
                .size(11.0)
                .color(colors.muted),
            )
            .wrap(),
        );
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
        painter.line_segment(
            [
                Pos2::new(pair[0][0], pair[0][1]),
                Pos2::new(pair[1][0], pair[1][1]),
            ],
            Stroke::new(radius * 2.0, mask),
        );
    }
    if !frame.heal_busy
        && let Some(cursor) = frame.heal_cursor_screen
    {
        painter.circle_stroke(Pos2::new(cursor[0], cursor[1]), radius, outline_shadow);
        painter.circle_stroke(Pos2::new(cursor[0], cursor[1]), radius, outline);
    }
}

fn render_filmstrip(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    let colors = chrome_colors(ui);
    let current = frame.playlist_pos.map(|(i, _)| i.saturating_sub(1));
    let height = if frame.filmstrip_panel_open {
        FILMSTRIP_PANEL_HEIGHT
    } else {
        FILMSTRIP_RAIL_HEIGHT
    };
    Panel::bottom("filmstrip_panel")
        .exact_size(height)
        .resizable(false)
        .frame(docked_frame(colors))
        .show(ui, |ui| {
            if frame.filmstrip_panel_open {
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        Vec2::new(112.0, ui.available_height()),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            if disclosure_button(
                                ui,
                                ChevronDirection::Down,
                                "Collapse folder previews",
                                true,
                            )
                            .clicked()
                            {
                                actions.push(UiAction::ToggleFilmstripPanelExpansion);
                            }
                            ui.label(
                                RichText::new("FOLDER PREVIEWS")
                                    .size(10.0)
                                    .color(colors.muted)
                                    .strong(),
                            );
                            if let Some((index, total)) = frame.playlist_pos {
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
                            ui.horizontal_centered(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                for item in &frame.filmstrip {
                                    render_filmstrip_item(ui, actions, item, current);
                                }
                            });
                        });
                });
            } else {
                ui.horizontal_centered(|ui| {
                    if disclosure_button(ui, ChevronDirection::Up, "Expand folder previews", false)
                        .clicked()
                    {
                        actions.push(UiAction::ToggleFilmstripPanelExpansion);
                    }
                    let label = frame.playlist_pos.map_or_else(
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
    } else if item.flagged {
        Stroke::new(1.0, with_alpha(colors.accent, 180))
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
    let accessibility_label = format!(
        "{}image {}: {}",
        if item.flagged { "Flagged " } else { "" },
        item.index + 1,
        item.name
    );
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
            format!("{}", item.index + 1),
            egui::FontId::proportional(14.0),
            colors.muted,
        );
    }
    if item.flagged {
        painter.circle_filled(rect.right_top() + Vec2::new(-9.0, 9.0), 4.0, colors.accent);
    }
    if response.clicked() {
        actions.push(UiAction::NavigateTo(item.index));
    }
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
                    ui.label(RichText::new(msg).size(13.0).color(colors.text));
                });
        });
}

fn render_crop_overlay(ui: &mut egui::Ui, frame: &UiFrameOwned, actions: &mut Vec<UiAction>) {
    render_crop_toolbar(ui, frame, actions);
    render_crop_selection(ui, frame, actions);
}

fn render_crop_toolbar(ui: &mut egui::Ui, frame: &UiFrameOwned, actions: &mut Vec<UiAction>) {
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
                                .add(
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

    let visible_rect = rect.shrink(10.0).intersect(image_viewport);
    if !visible_rect.is_positive() {
        return;
    }
    let response = ui
        .interact(
            visible_rect,
            egui::Id::new("crop_rect_sense"),
            Sense::drag(),
        )
        .on_hover_cursor(CursorIcon::Grab);
    if response.dragged()
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
    let accessibility_label = format!(
        "Crop selection: {pixel_width} by {pixel_height} output pixels, source starts at x \
         {pixel_x}, y {pixel_y}. Drag inside to move. Arrow keys move; Shift plus Arrow \
         keys resize."
    );
    response.widget_info(|| {
        WidgetInfo::labeled(WidgetType::Panel, ui.is_enabled(), &accessibility_label)
    });
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
    if frame.is_cropping || frame.is_healing {
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
    } else if frame.is_panning {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    } else {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChromeLayout, DockSide, DockState, FILMSTRIP_PANEL_HEIGHT, FILMSTRIP_RAIL_HEIGHT,
        FilmstripItem, HEAL_PANEL_WIDTH, IMAGE_INFO_PANEL_WIDTH, TOOLS_PANEL_WIDTH,
        TOOLS_RAIL_WIDTH, TOP_BAR_HEIGHT, UiAction, UiFrameOwned, chrome_colors_for,
        crop_pixel_bounds, render, viewport_insets,
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
            show_image_info: true,
            retain_exif: false,
            background_override: None,
            theme_preference: crate::theme::Preference::System,
            theme_mode: crate::theme::Mode::Dark,
            show_about: false,
            show_tools_panel: true,
            tools_panel_open: true,
            tools_panel_side: DockSide::Left,
            show_filmstrip_panel: true,
            filmstrip_panel_open: true,
            image_info_side: DockSide::Right,
            file_path: Some("C:/photos/current.png".to_owned()),
            img_size: Some((1920, 1080)),
            animation: None,
            details: None,
            color_profile: Some(crate::decode::ColorProfileStatus::AssumedSrgb),
            is_cropping: false,
            crop_ratio: crate::crop::CropRatio::Free,
            custom_crop_ratio: (3, 5),
            is_healing: false,
            can_heal: true,
            heal_busy: false,
            heal_brush_radius: 18,
            heal_feather_percent: crate::heal::DEFAULT_FEATHER_PERCENT,
            heal_source: Some((0, 4)),
            can_undo_edit: false,
            can_redo_edit: false,
            is_panning: false,
            is_flagged: false,
            flag_count: 1,
            has_image: true,
            is_loading: false,
            load_error: None,
            save_busy: false,
            crop_busy: false,
            playlist_pos: Some((1, 2)),
            pixel_scale: 1.0,
            toast: None,
            filmstrip: vec![
                FilmstripItem {
                    index: 0,
                    name: "current.png".to_owned(),
                    flagged: false,
                    texture: None,
                },
                FilmstripItem {
                    index: 1,
                    name: "flagged.png".to_owned(),
                    flagged: true,
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

    fn accessibility_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(1200.0, 800.0),
            )),
            ..egui::RawInput::default()
        }
    }

    #[test]
    fn ui_action_variants_exist_for_toolbar() {
        let _ = UiAction::OpenFolder;
        let _ = UiAction::Reload;
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
        let _ = UiAction::ToggleFlag;
        let _ = UiAction::TrashFlagged;
        let _ = UiAction::PermanentDelete;
        let _ = UiAction::ToggleImageInfo;
        let _ = UiAction::ToggleAnimationPlayback;
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
    }

    #[test]
    fn expanded_panels_reserve_scaled_physical_pixels() {
        let insets = viewport_insets(ChromeLayout {
            tools: DockState::Expanded,
            tools_side: DockSide::Left,
            heal: false,
            filmstrip: DockState::Expanded,
            image_info: Some(DockSide::Right),
            scale_factor: 1.5,
        });
        assert!((insets.left - TOOLS_PANEL_WIDTH * 1.5).abs() < f32::EPSILON);
        assert!((insets.right - IMAGE_INFO_PANEL_WIDTH * 1.5).abs() < f32::EPSILON);
        assert!((insets.top - TOP_BAR_HEIGHT * 1.5).abs() < f32::EPSILON);
        assert!((insets.bottom - FILMSTRIP_PANEL_HEIGHT * 1.5).abs() < f32::EPSILON);
    }

    #[test]
    fn collapsed_disclosure_rails_reserve_their_exact_size() {
        let insets = viewport_insets(ChromeLayout {
            tools: DockState::Collapsed,
            tools_side: DockSide::Left,
            heal: false,
            filmstrip: DockState::Collapsed,
            image_info: None,
            scale_factor: 1.0,
        });
        assert!((insets.left - TOOLS_RAIL_WIDTH).abs() < f32::EPSILON);
        assert!((insets.bottom - FILMSTRIP_RAIL_HEIGHT).abs() < f32::EPSILON);
        assert!(insets.right.abs() < f32::EPSILON);
    }

    #[test]
    fn side_panels_reserve_their_selected_edges_and_accumulate() {
        let insets = viewport_insets(ChromeLayout {
            tools: DockState::Expanded,
            tools_side: DockSide::Right,
            heal: false,
            filmstrip: DockState::Hidden,
            image_info: Some(DockSide::Right),
            scale_factor: 1.0,
        });
        assert!(insets.left.abs() < f32::EPSILON);
        assert!((insets.right - TOOLS_PANEL_WIDTH - IMAGE_INFO_PANEL_WIDTH).abs() < f32::EPSILON);
        assert!((insets.top - TOP_BAR_HEIGHT).abs() < f32::EPSILON);
        assert!(insets.bottom.abs() < f32::EPSILON);
    }

    #[test]
    fn fully_hidden_panels_reserve_no_image_space_beyond_the_menu() {
        let insets = viewport_insets(ChromeLayout {
            tools: DockState::Hidden,
            tools_side: DockSide::Right,
            heal: false,
            filmstrip: DockState::Hidden,
            image_info: None,
            scale_factor: 1.0,
        });
        assert!(insets.left.abs() < f32::EPSILON);
        assert!(insets.right.abs() < f32::EPSILON);
        assert!((insets.top - TOP_BAR_HEIGHT).abs() < f32::EPSILON);
        assert!(insets.bottom.abs() < f32::EPSILON);
    }

    #[test]
    fn spot_heal_inspector_reserves_space_on_the_tools_edge() {
        let insets = viewport_insets(ChromeLayout {
            tools: DockState::Expanded,
            tools_side: DockSide::Right,
            heal: true,
            filmstrip: DockState::Hidden,
            image_info: None,
            scale_factor: 1.25,
        });
        assert!(insets.left.abs() < f32::EPSILON);
        assert!(
            (insets.right - (TOOLS_PANEL_WIDTH + HEAL_PANEL_WIDTH) * 1.25).abs() < f32::EPSILON
        );
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
            "Flagged image 2: flagged.png",
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
        let labels = update
            .nodes
            .iter()
            .filter_map(|(_, node)| node.label())
            .collect::<Vec<_>>();
        for expected in ["About viewr", "Close"] {
            assert!(
                labels.iter().any(|label| label.contains(expected)),
                "missing About node: {expected}; labels: {labels:?}"
            );
        }
    }

    #[test]
    fn spot_heal_refinements_publish_named_controls() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut frame = accessibility_test_frame();
        frame.is_healing = true;
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
