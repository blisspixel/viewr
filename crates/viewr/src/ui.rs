//! Immediate-mode chrome: hybrid menus, progressive floating toolbar, status,
//! empty state, crop overlay, and toasts.
//!
//! Design intent (see `docs/DESIGN.md`): the photo is the hero; chrome auto-hides;
//! amber accent marks active tools only. Keyboard remains primary.

use egui::{
    Align2, Area, Color32, CornerRadius, CursorIcon, Frame, Panel, Pos2, Rect, RichText, Sense,
    Stroke, Vec2,
};

/// Accent amber for active tools (DESIGN.md).
const ACCENT: Color32 = Color32::from_rgb(0xF7, 0xA8, 0x45);
const INK: Color32 = Color32::from_rgb(0x0B, 0x0E, 0x14);
const TEXT: Color32 = Color32::from_rgb(0xE8, 0xED, 0xF3);
const MUTED: Color32 = Color32::from_rgb(0xB8, 0xC0, 0xCC);

/// Actions dispatched from the UI to be handled by the main application logic.
pub enum UiAction {
    /// Open a new file dialog.
    Open,
    /// Open a save as dialog.
    SaveAs,
    /// Move the current file to the trash.
    Trash,
    /// Undo the last trash operation.
    UndoTrash,
    /// Set the background color.
    SetBackground(Option<[f64; 4]>),
    /// Toggle the EXIF metadata panel.
    ToggleExif,
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
    /// Navigate to a relative file index.
    Navigate(isize),
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
    /// Whether the metadata side panel is open.
    pub show_exif: bool,
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
    /// 1-based index and total in the folder playlist, if known.
    pub playlist_pos: Option<(usize, usize)>,
    /// Current zoom multiplier (1.0 = fit).
    pub zoom: f32,
    /// Transient toast message (trash undo hint, etc.).
    pub toast: Option<String>,
    /// Show floating chrome (mouse recently moved / near edge / crop mode).
    pub chrome_visible: bool,
    /// Mouse is near the left edge (force toolbar).
    pub mouse_near_left: bool,
    /// Crop rectangle in screen pixels `[x0, y0, x1, y1]` when previewing.
    pub crop_screen: Option<[f32; 4]>,
}

/// Render the UI overlays and return a list of actions triggered by the user.
pub fn render(ui: &mut egui::Ui, frame: &UiFrameOwned) -> Vec<UiAction> {
    let mut actions = Vec::new();

    // Menus stay as discovery/accessibility fallback; keep them quiet.
    render_top_menu(ui, &mut actions, frame);

    if !frame.has_image {
        render_empty_state(ui);
        return actions;
    }

    if frame.show_exif {
        render_exif_panel(ui, frame);
    }

    if frame.chrome_visible || frame.mouse_near_left || frame.is_cropping {
        render_left_toolbar(ui, &mut actions, frame);
    }

    render_status_chip(ui, frame);

    if let Some(msg) = &frame.toast {
        render_toast(ui, msg);
    }

    if frame.is_cropping {
        render_crop_overlay(ui, frame, &mut actions);
    }

    apply_cursor(ui, frame);
    actions
}

fn menu_frame() -> Frame {
    Frame::new()
        .fill(Color32::from_rgba_unmultiplied(11, 14, 20, 210))
        .inner_margin(6.0)
}

fn glass_frame() -> Frame {
    Frame::new()
        .fill(Color32::from_rgba_unmultiplied(11, 14, 20, 220))
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(
            1.0,
            Color32::from_rgba_unmultiplied(255, 255, 255, 20),
        ))
        .inner_margin(10.0)
        .shadow(egui::Shadow {
            offset: [0, 4],
            blur: 16,
            spread: 0,
            color: Color32::from_black_alpha(80),
        })
}

fn render_top_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    // Thin, low-contrast bar: discovery only, not the main control surface.
    Panel::top("top_panel")
        .frame(menu_frame())
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                file_menu(ui, actions, frame.flag_count);
                edit_menu(ui, actions, frame.is_cropping);
                view_menu(ui, actions);
                canvas_menu(ui, actions);
                info_menu(ui, actions);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some((i, n)) = frame.playlist_pos {
                        ui.label(RichText::new(format!("{i} / {n}")).size(12.5).color(MUTED));
                    }
                });
            });
        });
}

fn file_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, flag_count: usize) {
    ui.menu_button("File", |ui| {
        if ui.button("Open…          Ctrl+O").clicked() {
            actions.push(UiAction::Open);
            ui.close();
        }
        if ui.button("Save As…       W").clicked() {
            actions.push(UiAction::SaveAs);
            ui.close();
        }
        ui.separator();
        if ui.button("Flag / Unflag  X").clicked() {
            actions.push(UiAction::ToggleFlag);
            ui.close();
        }
        if ui
            .add_enabled(
                flag_count > 0,
                egui::Button::new(format!("Trash Flagged ({flag_count})  B")),
            )
            .clicked()
        {
            actions.push(UiAction::TrashFlagged);
            ui.close();
        }
        if ui.button("Move to Trash  Del").clicked() {
            actions.push(UiAction::Trash);
            ui.close();
        }
        if ui.button("Permanently Delete  Shift+Del").clicked() {
            actions.push(UiAction::PermanentDelete);
            ui.close();
        }
        if ui.button("Undo Trash     U").clicked() {
            actions.push(UiAction::UndoTrash);
            ui.close();
        }
    });
}

fn edit_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, is_cropping: bool) {
    ui.menu_button("Edit", |ui| {
        let crop_label = if is_cropping {
            "Cancel Crop     Esc"
        } else {
            "Crop            C"
        };
        if ui.button(crop_label).clicked() {
            actions.push(UiAction::ToggleCrop);
            ui.close();
        }
        if is_cropping && ui.button("Apply Crop      Enter").clicked() {
            actions.push(UiAction::ApplyCrop);
            ui.close();
        }
    });
}

fn view_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>) {
    ui.menu_button("View", |ui| {
        if ui.button("Fullscreen     F").clicked() {
            actions.push(UiAction::ToggleFullscreen);
            ui.close();
        }
        ui.separator();
        if ui.button("Rotate CW      R").clicked() {
            actions.push(UiAction::RotateCw);
        }
        if ui.button("Rotate CCW     L").clicked() {
            actions.push(UiAction::RotateCcw);
        }
        if ui.button("Flip H         H").clicked() {
            actions.push(UiAction::FlipH);
        }
        if ui.button("Flip V         V").clicked() {
            actions.push(UiAction::FlipV);
        }
    });
}

fn canvas_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>) {
    ui.menu_button("Canvas", |ui| {
        if ui.button("Auto (Theme)").clicked() {
            actions.push(UiAction::SetBackground(None));
            ui.close();
        }
        if ui.button("Pure Black").clicked() {
            actions.push(UiAction::SetBackground(Some([0.0, 0.0, 0.0, 1.0])));
            ui.close();
        }
        if ui.button("Dark Grey").clicked() {
            actions.push(UiAction::SetBackground(Some([0.2, 0.2, 0.2, 1.0])));
            ui.close();
        }
        if ui.button("Pure White").clicked() {
            actions.push(UiAction::SetBackground(Some([1.0, 1.0, 1.0, 1.0])));
            ui.close();
        }
    });
}

fn info_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>) {
    ui.menu_button("Info", |ui| {
        if ui.button("Toggle Metadata").clicked() {
            actions.push(UiAction::ToggleExif);
            ui.close();
        }
    });
}

fn render_empty_state(ui: &mut egui::Ui) {
    let screen = ui.ctx().content_rect();
    Area::new("empty_state".into())
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .order(egui::Order::Middle)
        .show(ui.ctx(), |ui| {
            ui.set_min_width(screen.width().min(420.0));
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Drop an image or folder here")
                        .size(18.0)
                        .color(TEXT)
                        .strong(),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new("or press Ctrl+O  ·  arrows to browse  ·  Del to trash")
                        .size(13.0)
                        .color(MUTED),
                );
                ui.add_space(14.0);
                ui.label(
                    RichText::new("viewr never phones home")
                        .size(12.0)
                        .color(Color32::from_rgba_unmultiplied(184, 192, 204, 160)),
                );
            });
        });
}

fn render_exif_panel(ui: &mut egui::Ui, frame: &UiFrameOwned) {
    Panel::right("exif_panel")
        .frame(
            Frame::new()
                .fill(Color32::from_rgba_unmultiplied(11, 14, 20, 245))
                .inner_margin(16.0)
                .stroke(Stroke::new(
                    1.0,
                    Color32::from_rgba_unmultiplied(255, 255, 255, 16),
                )),
        )
        .show(ui, |ui| {
            ui.heading(RichText::new("Image Info").color(TEXT));
            ui.separator();
            if let Some(path) = &frame.file_path {
                let name = std::path::Path::new(path).file_name().map_or_else(
                    || path.clone(),
                    |s| s.to_string_lossy().into_owned(),
                );
                ui.label(RichText::new(name).color(TEXT));
            }
            if let Some((w, h)) = frame.img_size {
                ui.label(RichText::new(format!("{w} × {h}")).color(MUTED));
            }
            ui.separator();
            ui.label(
                RichText::new(if frame.is_flagged {
                    "Flagged for batch cull"
                } else {
                    "Not flagged"
                })
                .color(if frame.is_flagged { ACCENT } else { MUTED }),
            );
            ui.label(RichText::new(format!("Flagged: {}", frame.flag_count)).color(MUTED));
            ui.separator();
            ui.label(
                RichText::new("Export re-encodes pixels and strips metadata.")
                    .size(12.0)
                    .color(MUTED),
            );
        });
}

fn render_left_toolbar(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, frame: &UiFrameOwned) {
    Area::new("left_toolbar".into())
        .anchor(Align2::LEFT_CENTER, [14.0, 0.0])
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            glass_frame().show(ui, |ui| {
                ui.set_width(44.0);
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    tool_btn(ui, "↺", "Rotate CCW (L)", || {
                        actions.push(UiAction::RotateCcw);
                    });
                    tool_btn(ui, "↻", "Rotate CW (R)", || {
                        actions.push(UiAction::RotateCw);
                    });
                    tool_btn(ui, "↔", "Flip H (H)", || actions.push(UiAction::FlipH));
                    tool_btn(ui, "↕", "Flip V (V)", || actions.push(UiAction::FlipV));
                    ui.add_space(4.0);
                    let crop_active = frame.is_cropping;
                    tool_btn_active(ui, "✂", "Crop (C)", crop_active, || {
                        actions.push(UiAction::ToggleCrop);
                    });
                    tool_btn_active(ui, "★", "Flag (X)", frame.is_flagged, || {
                        actions.push(UiAction::ToggleFlag);
                    });
                    tool_btn(ui, "🗑", "Trash (Del)", || actions.push(UiAction::Trash));
                    tool_btn(ui, "💾", "Save As (W)", || actions.push(UiAction::SaveAs));
                    if frame.flag_count > 0 {
                        ui.add_space(4.0);
                        tool_btn(
                            ui,
                            &format!("{}", frame.flag_count),
                            "Trash flagged (B)",
                            || actions.push(UiAction::TrashFlagged),
                        );
                    }
                });
            });
        });
}

fn tool_btn(ui: &mut egui::Ui, label: &str, tip: &str, on_click: impl FnOnce()) {
    tool_btn_active(ui, label, tip, false, on_click);
}

fn tool_btn_active(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    active: bool,
    on_click: impl FnOnce(),
) {
    let fill = if active {
        Color32::from_rgba_unmultiplied(247, 168, 69, 40)
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 8)
    };
    let stroke = if active {
        Stroke::new(1.0, ACCENT)
    } else {
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 18))
    };
    let btn = egui::Button::new(RichText::new(label).size(16.0).color(TEXT))
        .fill(fill)
        .stroke(stroke)
        .min_size(Vec2::splat(36.0));
    let response = ui.add(btn).on_hover_text(tip);
    if response.clicked() {
        on_click();
    }
}

fn render_status_chip(ui: &mut egui::Ui, frame: &UiFrameOwned) {
    if !frame.chrome_visible && !frame.is_cropping {
        return;
    }
    let name = frame
        .file_path
        .as_ref()
        .and_then(|p| {
            std::path::Path::new(p)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    let dims = frame
        .img_size
        .map(|(w, h)| format!("{w} × {h}"))
        .unwrap_or_default();
    let pos = frame
        .playlist_pos
        .map(|(i, n)| format!("{i} / {n}"))
        .unwrap_or_default();
    let zoom = if (frame.zoom - 1.0).abs() < 0.02 {
        String::new()
    } else {
        format!("  ·  {:.0}%", frame.zoom * 100.0)
    };
    let mut parts = Vec::new();
    if !name.is_empty() {
        parts.push(name);
    }
    if !dims.is_empty() {
        parts.push(dims);
    }
    if !pos.is_empty() {
        parts.push(pos);
    }
    let mut text = parts.join("  ·  ");
    text.push_str(&zoom);
    if text.is_empty() {
        return;
    }

    Area::new("status_chip".into())
        .anchor(Align2::LEFT_BOTTOM, [16.0, -16.0])
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            Frame::new()
                .fill(Color32::from_rgba_unmultiplied(11, 14, 20, 180))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.label(RichText::new(text).size(12.5).color(MUTED));
                });
        });
}

fn render_toast(ui: &mut egui::Ui, msg: &str) {
    Area::new("toast".into())
        .anchor(Align2::CENTER_BOTTOM, [0.0, -28.0])
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
    // Ratio strip — only in crop mode.
    Area::new("crop_ratios".into())
        .anchor(Align2::CENTER_TOP, [0.0, 48.0])
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            glass_frame().show(ui, |ui| {
                ui.horizontal(|ui| {
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
                        .add(egui::Button::new(RichText::new("Apply").color(INK)).fill(ACCENT))
                        .clicked()
                    {
                        actions.push(UiAction::ApplyCrop);
                    }
                    if ui.button("Cancel").clicked() {
                        actions.push(UiAction::CancelCrop);
                    }
                });
            });
        });

    // Handles + dimensions when a rect is being drawn.
    if let Some([x0, y0, x1, y1]) = frame.crop_screen {
        let rect = Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1));
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("crop_draw"),
        ));
        painter.rect_stroke(
            rect,
            CornerRadius::ZERO,
            Stroke::new(1.5, Color32::from_rgb(0xE8, 0xED, 0xF3)),
            egui::StrokeKind::Outside,
        );
        // Corner handles.
        let handle = 7.0;
        for p in [
            rect.left_top(),
            rect.right_top(),
            rect.left_bottom(),
            rect.right_bottom(),
            rect.center_top(),
            rect.center_bottom(),
            rect.left_center(),
            rect.right_center(),
        ] {
            let hr = Rect::from_center_size(p, Vec2::splat(handle));
            painter.rect_filled(hr, CornerRadius::same(1), ACCENT);
            painter.rect_stroke(
                hr,
                CornerRadius::same(1),
                Stroke::new(1.0, Color32::WHITE),
                egui::StrokeKind::Outside,
            );
        }
        if let Some((iw, ih)) = frame.img_size {
            let screen = ui.ctx().content_rect();
            let pw = ((x1 - x0).abs() / screen.width() * iw as f32).round() as i32;
            let ph = ((y1 - y0).abs() / screen.height() * ih as f32).round() as i32;
            // Approximate pixel size from UV crop if available — better label from UV in app.
            let label = format!("{pw} × {ph} px (approx)");
            painter.text(
                rect.center_bottom() + Vec2::new(0.0, 18.0),
                Align2::CENTER_TOP,
                label,
                egui::FontId::proportional(12.0),
                MUTED,
            );
        }
        // Invisible sense so hover over crop doesn't force grab cursor from below.
        ui.interact(rect, egui::Id::new("crop_rect_sense"), Sense::hover());
    } else {
        Area::new("crop_hint".into())
            .anchor(Align2::CENTER_CENTER, [0.0, 80.0])
            .show(ui.ctx(), |ui| {
                ui.label(
                    RichText::new(
                        "Drag on the image to set the crop  ·  Enter apply  ·  Esc cancel",
                    )
                    .size(13.0)
                    .color(MUTED),
                );
            });
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
    use super::UiAction;

    #[test]
    fn ui_action_variants_exist_for_toolbar() {
        let _ = UiAction::ToggleCrop;
        let _ = UiAction::ApplyCrop;
        let _ = UiAction::CancelCrop;
        let _ = UiAction::SetCropRatio(crate::app::CropRatio::Square);
        let _ = UiAction::Navigate(1);
        let _ = UiAction::ToggleFlag;
        let _ = UiAction::TrashFlagged;
        let _ = UiAction::PermanentDelete;
    }
}
