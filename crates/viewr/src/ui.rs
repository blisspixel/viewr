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

/// Accent amber for active tools (DESIGN.md).
const ACCENT: Color32 = Color32::from_rgb(0xF7, 0xA8, 0x45);
const INK: Color32 = Color32::from_rgb(0x0B, 0x0E, 0x14);
const TEXT: Color32 = Color32::from_rgb(0xE8, 0xED, 0xF3);
const MUTED: Color32 = Color32::from_rgb(0xB8, 0xC0, 0xCC);
const PANEL: Color32 = Color32::from_rgb(0x0F, 0x13, 0x1A);
const PANEL_RAISED: Color32 = Color32::from_rgb(0x1A, 0x20, 0x2A);
const PANEL_BORDER: Color32 = Color32::from_rgb(0x2B, 0x33, 0x40);

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
/// Logical height of the collapsed folder-preview rail.
pub const FILMSTRIP_RAIL_HEIGHT: f32 = 44.0;
/// Logical height of the expanded folder-preview panel.
pub const FILMSTRIP_PANEL_HEIGHT: f32 = 112.0;
/// Logical width of the Image Info panel.
pub const IMAGE_INFO_PANEL_WIDTH: f32 = 304.0;

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
    /// Folder-preview rail or expanded preview panel.
    pub filmstrip: DockState,
    /// Whether Image Info is visible.
    pub image_info_visible: bool,
    /// Physical pixels per logical UI point.
    pub scale_factor: f64,
}

/// Convert persistent panel state into physical-pixel image insets.
#[must_use]
pub fn viewport_insets(layout: ChromeLayout) -> crate::view::ViewportInsets {
    let scale = layout.scale_factor.max(0.0) as f32;
    crate::view::ViewportInsets {
        left: match layout.tools {
            DockState::Hidden => 0.0,
            DockState::Collapsed => TOOLS_RAIL_WIDTH,
            DockState::Expanded => TOOLS_PANEL_WIDTH,
        } * scale,
        right: if layout.image_info_visible {
            IMAGE_INFO_PANEL_WIDTH * scale
        } else {
            0.0
        },
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
    /// Open a save as dialog.
    SaveAs,
    /// Move the current file to the trash.
    Trash,
    /// Undo the last trash operation.
    UndoTrash,
    /// Set the background color.
    SetBackground(Option<[f64; 4]>),
    /// Toggle the Image Info panel.
    ToggleImageInfo,
    /// Expand or collapse the docked tools panel.
    ToggleToolsPanel,
    /// Expand or collapse the docked folder-preview panel.
    ToggleFilmstripPanel,
    /// Toggle whether Save As retains EXIF (default off = strip).
    ToggleRetainExif,
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
    SetCropRatio(crate::app::CropRatio),
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
    /// Whether the docked tools panel is expanded.
    pub tools_panel_open: bool,
    /// Whether the docked folder-preview panel is expanded.
    pub filmstrip_panel_open: bool,
    /// Path of the current image (display only).
    pub file_path: Option<String>,
    /// Pixel dimensions of the current image, if any.
    pub img_size: Option<(u32, u32)>,
    /// Crop tool active.
    pub is_cropping: bool,
    /// Active crop aspect lock.
    pub crop_ratio: crate::app::CropRatio,
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
    /// Image-safe viewport in logical UI coordinates `[x0, y0, x1, y1]`.
    pub image_viewport: Option<[f32; 4]>,
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
    apply_chrome_theme(ui.ctx());

    render_top_menu(ui, &mut actions, frame);

    if !frame.has_image {
        render_empty_state(ui, &mut actions, frame.is_loading);
        if let Some(msg) = &frame.toast {
            render_toast(ui, msg, frame);
        }
        return actions;
    }

    if frame.show_image_info {
        render_image_info_panel(ui, &mut actions, frame);
    }

    if frame.filmstrip.len() > 1 {
        render_filmstrip(ui, &mut actions, frame);
    }

    render_left_toolbar(ui, &mut actions, frame);

    if let Some(msg) = &frame.toast {
        render_toast(ui, msg, frame);
    }

    if frame.is_cropping {
        render_crop_overlay(ui, frame, &mut actions);
    }

    apply_cursor(ui, frame);
    actions
}

fn apply_chrome_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = PANEL;
    visuals.window_fill = PANEL;
    visuals.window_stroke = Stroke::new(1.0, PANEL_BORDER);
    visuals.selection.bg_fill = Color32::from_rgba_unmultiplied(247, 168, 69, 48);
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.hyperlink_color = ACCENT;
    if ctx.style_of(ctx.theme()).visuals != visuals {
        ctx.set_visuals(visuals);
    }
}

fn menu_frame() -> Frame {
    Frame::new()
        .fill(PANEL)
        .inner_margin(egui::Margin::symmetric(10, 4))
        .stroke(Stroke::new(1.0, PANEL_BORDER))
}

fn docked_frame() -> Frame {
    Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, PANEL_BORDER))
        .inner_margin(4.0)
}

fn render_top_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    Panel::top("top_panel")
        .exact_size(TOP_BAR_HEIGHT)
        .resizable(false)
        .frame(menu_frame())
        .show(ui, |ui| {
            ui.spacing_mut().button_padding = Vec2::new(10.0, 4.0);
            ui.visuals_mut().widgets.inactive.bg_fill = Color32::TRANSPARENT;
            ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
            ui.visuals_mut().widgets.hovered.bg_fill = PANEL_RAISED;
            ui.visuals_mut().widgets.hovered.weak_bg_fill = PANEL_RAISED;
            ui.visuals_mut().widgets.active.bg_fill = Color32::from_rgb(0x25, 0x2D, 0x39);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                file_menu(ui, actions, frame);
                edit_menu(ui, actions, frame);
                view_menu(ui, actions, frame);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some((i, n)) = frame.playlist_pos {
                        Frame::new()
                            .fill(PANEL_RAISED)
                            .corner_radius(CornerRadius::same(6))
                            .inner_margin(egui::Margin::symmetric(8, 3))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(format!("{i} / {n}")).size(12.5).color(MUTED),
                                );
                            });
                    }
                    let show_details = ui.ctx().content_rect().width() >= 720.0;
                    if show_details && frame.has_image {
                        ui.label(
                            RichText::new(format!("{:.0}%", frame.pixel_scale * 100.0))
                                .size(12.5)
                                .color(MUTED),
                        );
                    }
                    if show_details && let Some((w, h)) = frame.img_size {
                        ui.label(RichText::new(format!("{w} × {h}")).size(12.5).color(MUTED));
                    }
                    if show_details
                        && let Some(name) = frame.file_path.as_ref().and_then(|path| {
                            std::path::Path::new(path)
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                        })
                    {
                        let response = ui.add(
                            egui::Label::new(RichText::new(&name).size(12.5).color(TEXT))
                                .truncate(),
                        );
                        let _ = response.on_hover_text(name);
                    }
                });
            });
        });
}

fn file_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    ui.menu_button(RichText::new("File").size(13.5).color(TEXT), |ui| {
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
                frame.has_image,
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
                frame.has_image,
                egui::Button::new("Flag for review").shortcut_text("X"),
            )
            .clicked()
        {
            actions.push(UiAction::ToggleFlag);
            ui.close();
        }
        if ui
            .add_enabled(
                frame.flag_count > 0,
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
                frame.has_image,
                egui::Button::new("Move to Trash").shortcut_text("Delete"),
            )
            .clicked()
        {
            actions.push(UiAction::Trash);
            ui.close();
        }
        if ui
            .add_enabled(
                frame.has_image,
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
    ui.menu_button(RichText::new("Edit").size(13.5).color(TEXT), |ui| {
        ui.set_min_width(210.0);
        let crop_label = if frame.is_cropping {
            "Cancel Crop"
        } else {
            "Crop"
        };
        let crop_shortcut = if frame.is_cropping { "Esc" } else { "C" };
        if ui
            .add_enabled(
                frame.has_image,
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
        ui.separator();
        if ui
            .add_enabled(
                frame.has_image,
                egui::Button::new("Rotate Clockwise").shortcut_text("R"),
            )
            .clicked()
        {
            actions.push(UiAction::RotateCw);
            ui.close();
        }
        if ui
            .add_enabled(
                frame.has_image,
                egui::Button::new("Rotate Counterclockwise").shortcut_text("L"),
            )
            .clicked()
        {
            actions.push(UiAction::RotateCcw);
            ui.close();
        }
        if ui
            .add_enabled(
                frame.has_image,
                egui::Button::new("Flip Horizontally").shortcut_text("H"),
            )
            .clicked()
        {
            actions.push(UiAction::FlipH);
            ui.close();
        }
        if ui
            .add_enabled(
                frame.has_image,
                egui::Button::new("Flip Vertically").shortcut_text("V"),
            )
            .clicked()
        {
            actions.push(UiAction::FlipV);
            ui.close();
        }
    });
}

fn view_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    ui.menu_button(RichText::new("View").size(13.5).color(TEXT), |ui| {
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
        if ui
            .add_enabled(
                frame.has_image,
                egui::Button::new("Image Information")
                    .selected(frame.show_image_info)
                    .shortcut_text("I"),
            )
            .clicked()
        {
            actions.push(UiAction::ToggleImageInfo);
            ui.close();
        }
        if ui
            .add_enabled(
                frame.has_image,
                egui::Button::new("Tools Panel")
                    .selected(frame.tools_panel_open)
                    .shortcut_text("T"),
            )
            .clicked()
        {
            actions.push(UiAction::ToggleToolsPanel);
            ui.close();
        }
        if ui
            .add_enabled(
                frame.filmstrip.len() > 1,
                egui::Button::new("Folder Previews")
                    .selected(frame.filmstrip_panel_open)
                    .shortcut_text("G"),
            )
            .clicked()
        {
            actions.push(UiAction::ToggleFilmstripPanel);
            ui.close();
        }
        ui.separator();
        ui.menu_button("Image Background", |ui| {
            background_menu(ui, actions, frame.background_override);
        });
    });
}

fn background_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, current: Option<[f64; 4]>) {
    ui.set_min_width(172.0);
    let choices = [
        ("Follow System Theme", None),
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

fn render_empty_state(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, is_loading: bool) {
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
                .fill(PANEL)
                .corner_radius(CornerRadius::same(12))
                .stroke(Stroke::new(1.0, PANEL_BORDER))
                .inner_margin(egui::Margin::symmetric(28, 24))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        if is_loading {
                            ui.add(egui::Spinner::new().size(28.0).color(ACCENT));
                        } else {
                            let (icon_rect, _) =
                                ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
                            paint_empty_image_icon(ui.painter(), icon_rect);
                        }
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new(if is_loading {
                                "Opening image"
                            } else {
                                "Open an image"
                            })
                            .size(20.0)
                            .color(TEXT)
                            .strong(),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(if is_loading {
                                "Decoding locally while the window stays responsive."
                            } else {
                                "Drop a file or folder here, or choose where to start."
                            })
                            .size(13.0)
                            .color(MUTED),
                        );
                        if !is_loading {
                            ui.add_space(16.0);
                            ui.horizontal_centered(|ui| {
                                if ui
                                    .add(
                                        egui::Button::new(RichText::new("Open File").color(INK))
                                            .fill(ACCENT)
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
                                .color(MUTED),
                        );
                    });
                });
        },
    );
}

fn paint_empty_image_icon(painter: &egui::Painter, rect: Rect) {
    let frame = rect.shrink(3.0);
    painter.rect_filled(frame, CornerRadius::same(8), PANEL_RAISED);
    painter.rect_stroke(
        frame,
        CornerRadius::same(8),
        Stroke::new(1.5, MUTED),
        egui::StrokeKind::Inside,
    );
    let mountain = [
        frame.left_bottom() + Vec2::new(7.0, -8.0),
        frame.center() + Vec2::new(-3.0, 2.0),
        frame.center() + Vec2::new(3.0, -3.0),
        frame.right_bottom() + Vec2::new(-6.0, -8.0),
    ];
    painter.line(mountain.to_vec(), Stroke::new(1.5, MUTED));
    painter.circle_filled(frame.right_top() + Vec2::new(-9.0, 9.0), 3.0, ACCENT);
}

fn render_image_info_panel(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    Panel::right("image_info_panel")
        .exact_size(IMAGE_INFO_PANEL_WIDTH)
        .resizable(false)
        .frame(docked_frame().inner_margin(16.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(RichText::new("Image Information").color(TEXT));
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
            ui.separator();
            ui.label(RichText::new("File").color(MUTED).small().strong());
            ui.add_space(4.0);
            if let Some(path) = &frame.file_path {
                let name = std::path::Path::new(path)
                    .file_name()
                    .map_or_else(|| path.clone(), |s| s.to_string_lossy().into_owned());
                ui.label(RichText::new(name).color(TEXT));
            }
            if let Some((w, h)) = frame.img_size {
                ui.label(RichText::new(format!("{w} × {h}")).color(MUTED));
            }
            ui.add_space(8.0);
            ui.separator();
            ui.label(RichText::new("Review").color(MUTED).small().strong());
            ui.add_space(4.0);
            ui.label(
                RichText::new(if frame.is_flagged {
                    "Flagged for review"
                } else {
                    "Not flagged"
                })
                .color(if frame.is_flagged { ACCENT } else { MUTED }),
            );
            ui.label(
                RichText::new(format!("{} flagged in this folder", frame.flag_count)).color(MUTED),
            );
            ui.add_space(8.0);
            ui.separator();
            ui.label(RichText::new("Export Privacy").color(TEXT).strong());
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
                    "Off by default. Save As removes supported EXIF metadata, including GPS and \
                     camera identifiers. This choice lasts only for this session.",
                )
                .size(11.0)
                .color(MUTED),
            );
        });
}

#[derive(Clone, Copy)]
enum ToolIcon {
    RotateCcw,
    RotateCw,
    FlipH,
    FlipV,
    Crop,
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
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::click());
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), expanded, label));
    let response = response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(label);
    let fill = if response.hovered() || response.has_focus() {
        PANEL_RAISED
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(6),
            Stroke::new(2.0, ACCENT),
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
    ui.painter().line(points.to_vec(), Stroke::new(1.75, TEXT));
    response
}

fn render_left_toolbar(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    let width = if frame.tools_panel_open {
        TOOLS_PANEL_WIDTH
    } else {
        TOOLS_RAIL_WIDTH
    };
    Panel::left("tools_panel")
        .exact_size(width)
        .resizable(false)
        .frame(docked_frame())
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                let (direction, label) = if frame.tools_panel_open {
                    (ChevronDirection::Left, "Collapse tools panel (T)")
                } else {
                    (ChevronDirection::Right, "Expand tools panel (T)")
                };
                if disclosure_button(ui, direction, label, frame.tools_panel_open).clicked() {
                    actions.push(UiAction::ToggleToolsPanel);
                }

                if frame.tools_panel_open {
                    ui.label(RichText::new("TOOLS").size(10.0).color(MUTED).strong());
                    ui.separator();
                    ui.set_width(44.0);
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
                }
            });
        });
}

fn icon_btn(ui: &mut egui::Ui, icon: ToolIcon, tip: &str, active: bool, on_click: impl FnOnce()) {
    let size = Vec2::splat(38.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), active, tip));
    let response = response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(tip);
    let fill = if active {
        Color32::from_rgba_unmultiplied(247, 168, 69, 40)
    } else if response.hovered() || response.has_focus() {
        Color32::from_rgba_unmultiplied(255, 255, 255, 18)
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 8)
    };
    let border = if active {
        Stroke::new(1.0, ACCENT)
    } else {
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 18))
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
            Stroke::new(2.0, ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    let ink = if active { ACCENT } else { TEXT };
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

fn render_filmstrip(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    let current = frame.playlist_pos.map(|(i, _)| i.saturating_sub(1));
    let height = if frame.filmstrip_panel_open {
        FILMSTRIP_PANEL_HEIGHT
    } else {
        FILMSTRIP_RAIL_HEIGHT
    };
    Panel::bottom("filmstrip_panel")
        .exact_size(height)
        .resizable(false)
        .frame(docked_frame())
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
                                "Collapse folder previews (G)",
                                true,
                            )
                            .clicked()
                            {
                                actions.push(UiAction::ToggleFilmstripPanel);
                            }
                            ui.label(
                                RichText::new("FOLDER PREVIEWS")
                                    .size(10.0)
                                    .color(MUTED)
                                    .strong(),
                            );
                            if let Some((index, total)) = frame.playlist_pos {
                                ui.label(
                                    RichText::new(format!("{index} of {total}"))
                                        .size(11.0)
                                        .color(MUTED),
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
                    if disclosure_button(
                        ui,
                        ChevronDirection::Up,
                        "Expand folder previews (G)",
                        false,
                    )
                    .clicked()
                    {
                        actions.push(UiAction::ToggleFilmstripPanel);
                    }
                    let label = frame.playlist_pos.map_or_else(
                        || "Folder previews".to_owned(),
                        |(index, total)| format!("Folder previews  {index} of {total}"),
                    );
                    ui.label(RichText::new(label).size(12.0).color(MUTED));
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
    let selected = current == Some(item.index);
    let border = if selected {
        Stroke::new(2.0, ACCENT)
    } else if item.flagged {
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(247, 168, 69, 180))
    } else {
        Stroke::new(1.0, PANEL_BORDER)
    };
    let fill = if selected {
        Color32::from_rgba_unmultiplied(247, 168, 69, 28)
    } else {
        INK
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(86.0, 78.0), Sense::click());
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
            Stroke::new(2.0, ACCENT),
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
            MUTED,
        );
    }
    if item.flagged {
        painter.circle_filled(rect.right_top() + Vec2::new(-9.0, 9.0), 4.0, ACCENT);
    }
    if response.clicked() {
        actions.push(UiAction::NavigateTo(item.index));
    }
}

fn render_toast(ui: &mut egui::Ui, msg: &str, frame: &UiFrameOwned) {
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
                .fill(Color32::from_rgba_unmultiplied(11, 14, 20, 230))
                .corner_radius(CornerRadius::same(8))
                .stroke(Stroke::new(1.0, ACCENT))
                .inner_margin(egui::Margin::symmetric(14, 8))
                .show(ui, |ui| {
                    ui.label(RichText::new(msg).size(13.0).color(TEXT));
                });
        });
}

fn render_crop_overlay(ui: &mut egui::Ui, frame: &UiFrameOwned, actions: &mut Vec<UiAction>) {
    render_crop_toolbar(ui, frame, actions);
    render_crop_selection(ui, frame);
}

fn render_crop_toolbar(ui: &mut egui::Ui, frame: &UiFrameOwned, actions: &mut Vec<UiAction>) {
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
            docked_frame()
                .corner_radius(CornerRadius::same(8))
                .show(ui, |ui| {
                    ui.set_width(toolbar_width);
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("Crop").color(ACCENT).strong());
                            ui.separator();
                            let mut current = frame.crop_ratio;
                            for (ratio, label) in [
                                (crate::app::CropRatio::Free, "Free"),
                                (crate::app::CropRatio::Square, "1:1"),
                                (crate::app::CropRatio::FourThree, "4:3"),
                                (crate::app::CropRatio::SixteenNine, "16:9"),
                            ] {
                                if ui.selectable_value(&mut current, ratio, label).clicked() {
                                    actions.push(UiAction::SetCropRatio(current));
                                }
                            }
                            ui.separator();
                            if ui
                                .add(
                                    egui::Button::new(RichText::new("Apply").color(INK))
                                        .fill(ACCENT),
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
                            .color(MUTED),
                        );
                        ui.label(
                            RichText::new("Drag to redraw  |  Enter applies  |  Esc cancels")
                                .size(11.0)
                                .color(MUTED),
                        );
                    });
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

fn render_crop_selection(ui: &mut egui::Ui, frame: &UiFrameOwned) {
    // Border + dimensions when a rect is being drawn. There are deliberately no
    // resize handles until pointer-based handle dragging is implemented.
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
            Stroke::new(1.5, Color32::from_rgb(0xE8, 0xED, 0xF3)),
            egui::StrokeKind::Outside,
        );
        if let (Some((image_width, image_height)), Some(crop_uv)) = (frame.img_size, frame.crop_uv)
        {
            let pixel_x = crop_uv[0].min(crop_uv[2]) * image_width as f32;
            let pixel_y = crop_uv[1].min(crop_uv[3]) * image_height as f32;
            let pixel_width = ((crop_uv[2] - crop_uv[0]).abs() * image_width as f32).round();
            let pixel_height = ((crop_uv[3] - crop_uv[1]).abs() * image_height as f32).round();
            let label = format!("{pixel_width:.0} × {pixel_height:.0} px");
            let font = egui::FontId::proportional(12.0);
            let galley = painter.layout_no_wrap(label, font, TEXT);
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
            let center = Pos2::new(center_x, center_y);
            let label_rect = Rect::from_center_size(center, label_size);
            painter.rect_filled(
                label_rect,
                CornerRadius::same(5),
                Color32::from_rgba_unmultiplied(11, 14, 20, 230),
            );
            painter.galley(label_rect.center() - galley.size() * 0.5, galley, TEXT);

            let visible_rect = rect.intersect(image_viewport);
            if visible_rect.is_positive() {
                let response = ui.interact(
                    visible_rect,
                    egui::Id::new("crop_rect_sense"),
                    Sense::hover(),
                );
                let accessibility_label = format!(
                    "Crop selection: {pixel_width:.0} by {pixel_height:.0} pixels, starting at x \
                     {pixel_x:.0}, y {pixel_y:.0}. Arrow keys move; Shift plus Arrow keys resize."
                );
                response.widget_info(|| {
                    WidgetInfo::labeled(WidgetType::Panel, ui.is_enabled(), &accessibility_label)
                });
            }
        }
    }
}

fn apply_cursor(ui: &mut egui::Ui, frame: &UiFrameOwned) {
    if ui.ctx().is_pointer_over_egui() {
        return;
    }
    if frame.is_cropping {
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
        ACCENT, ChromeLayout, DockState, FILMSTRIP_PANEL_HEIGHT, FILMSTRIP_RAIL_HEIGHT,
        FilmstripItem, IMAGE_INFO_PANEL_WIDTH, INK, MUTED, PANEL, TEXT, TOOLS_PANEL_WIDTH,
        TOOLS_RAIL_WIDTH, TOP_BAR_HEIGHT, UiAction, UiFrameOwned, render, viewport_insets,
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
            tools_panel_open: true,
            filmstrip_panel_open: true,
            file_path: Some("C:/photos/current.png".to_owned()),
            img_size: Some((1920, 1080)),
            is_cropping: false,
            crop_ratio: crate::app::CropRatio::Free,
            is_panning: false,
            is_flagged: false,
            flag_count: 1,
            has_image: true,
            is_loading: false,
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
            image_viewport: Some([64.0, 40.0, 896.0, 688.0]),
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
        let _ = UiAction::ToggleCrop;
        let _ = UiAction::ApplyCrop;
        let _ = UiAction::CancelCrop;
        let _ = UiAction::SetCropRatio(crate::app::CropRatio::Square);
        let _ = UiAction::Navigate(1);
        let _ = UiAction::NavigateTo(0);
        let _ = UiAction::ToggleFlag;
        let _ = UiAction::TrashFlagged;
        let _ = UiAction::PermanentDelete;
        let _ = UiAction::ToggleImageInfo;
        let _ = UiAction::ToggleToolsPanel;
        let _ = UiAction::ToggleFilmstripPanel;
        let _ = UiAction::FitToView;
        let _ = UiAction::ActualSize;
        let _ = UiAction::ZoomIn;
        let _ = UiAction::ZoomOut;
    }

    #[test]
    fn expanded_panels_reserve_scaled_physical_pixels() {
        let insets = viewport_insets(ChromeLayout {
            tools: DockState::Expanded,
            filmstrip: DockState::Expanded,
            image_info_visible: true,
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
            filmstrip: DockState::Collapsed,
            image_info_visible: false,
            scale_factor: 1.0,
        });
        assert!((insets.left - TOOLS_RAIL_WIDTH).abs() < f32::EPSILON);
        assert!((insets.bottom - FILMSTRIP_RAIL_HEIGHT).abs() < f32::EPSILON);
        assert!(insets.right.abs() < f32::EPSILON);
    }

    #[test]
    fn chrome_text_and_controls_meet_wcag_aa_contrast() {
        assert!(contrast_ratio(TEXT, PANEL) >= 4.5);
        assert!(contrast_ratio(MUTED, PANEL) >= 4.5);
        assert!(contrast_ratio(ACCENT, PANEL) >= 4.5);
        assert!(contrast_ratio(INK, ACCENT) >= 4.5);
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
            "Collapse tools panel (T)",
            "Rotate clockwise (R)",
            "Collapse folder previews (G)",
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
                    "Crop selection: 1152 by 648 pixels, starting at x 192, y 216. Arrow keys \
                     move; Shift plus Arrow keys resize.",
                )
        }));
    }
}
