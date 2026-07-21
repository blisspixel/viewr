//! Immediate-mode chrome: menus and the left floating toolbar.
//!
//! Pure UI construction only. Side effects are returned as [`UiAction`] values
//! for the application message loop to apply.

use egui::{Color32, Frame, Panel};

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
    /// Set the aspect ratio for the crop tool.
    SetCropRatio(crate::app::CropRatio),
}

/// Render the UI overlays and return a list of actions triggered by the user.
pub fn render(
    ui: &mut egui::Ui,
    show_exif: bool,
    file_path: Option<&str>,
    img_size: Option<(u32, u32)>,
    is_cropping: bool,
    crop_ratio: crate::app::CropRatio,
    is_panning: bool,
) -> Vec<UiAction> {
    let mut actions = Vec::new();
    render_top_menu(ui, &mut actions, is_cropping);
    if show_exif {
        render_exif_panel(ui, file_path, img_size);
    }
    render_left_toolbar(ui, &mut actions, is_cropping, crop_ratio);
    apply_cursor(ui, is_cropping, is_panning);
    actions
}

fn menu_frame() -> Frame {
    Frame::new()
        .fill(Color32::from_rgba_unmultiplied(20, 20, 20, 200))
        .inner_margin(8.0)
}

fn panel_frame() -> Frame {
    Frame::new()
        .fill(Color32::from_rgba_unmultiplied(30, 30, 30, 240))
        .inner_margin(16.0)
}

fn toolbar_frame() -> Frame {
    Frame::new()
        .fill(Color32::from_rgba_unmultiplied(30, 30, 30, 240))
        .inner_margin(12.0)
}

fn render_top_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, is_cropping: bool) {
    Panel::top("top_panel").frame(menu_frame()).show(ui, |ui| {
        ui.horizontal(|ui| {
            file_menu(ui, actions);
            edit_menu(ui, actions, is_cropping);
            view_menu(ui, actions);
            canvas_menu(ui, actions);
            info_menu(ui, actions);
        });
    });
}

fn file_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>) {
    ui.menu_button("File", |ui| {
        if ui.button("Open...").clicked() {
            actions.push(UiAction::Open);
            ui.close();
        }
        if ui.button("Save As...").clicked() {
            actions.push(UiAction::SaveAs);
            ui.close();
        }
        ui.separator();
        if ui.button("Move to Trash").clicked() {
            actions.push(UiAction::Trash);
            ui.close();
        }
        if ui.button("Undo Trash").clicked() {
            actions.push(UiAction::UndoTrash);
            ui.close();
        }
    });
}

fn edit_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, is_cropping: bool) {
    ui.menu_button("Edit", |ui| {
        let crop_label = if is_cropping {
            "Cancel Crop"
        } else {
            "Crop Image"
        };
        if ui.button(crop_label).clicked() {
            actions.push(UiAction::ToggleCrop);
            ui.close();
        }
        if is_cropping && ui.button("Apply Crop (Enter)").clicked() {
            actions.push(UiAction::ApplyCrop);
            ui.close();
        }
    });
}

fn view_menu(ui: &mut egui::Ui, actions: &mut Vec<UiAction>) {
    ui.menu_button("View", |ui| {
        if ui.button("Fullscreen").clicked() {
            actions.push(UiAction::ToggleFullscreen);
            ui.close();
        }
        ui.separator();
        if ui.button("Rotate CW").clicked() {
            actions.push(UiAction::RotateCw);
        }
        if ui.button("Rotate CCW").clicked() {
            actions.push(UiAction::RotateCcw);
        }
        if ui.button("Flip H").clicked() {
            actions.push(UiAction::FlipH);
        }
        if ui.button("Flip V").clicked() {
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
        if ui.button("Pure White").clicked() {
            actions.push(UiAction::SetBackground(Some([1.0, 1.0, 1.0, 1.0])));
            ui.close();
        }
        if ui.button("Light Grey").clicked() {
            actions.push(UiAction::SetBackground(Some([0.8, 0.8, 0.8, 1.0])));
            ui.close();
        }
        if ui.button("Neutral Grey").clicked() {
            actions.push(UiAction::SetBackground(Some([0.5, 0.5, 0.5, 1.0])));
            ui.close();
        }
        if ui.button("Dark Grey").clicked() {
            actions.push(UiAction::SetBackground(Some([0.2, 0.2, 0.2, 1.0])));
            ui.close();
        }
        if ui.button("Pure Black").clicked() {
            actions.push(UiAction::SetBackground(Some([0.0, 0.0, 0.0, 1.0])));
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

fn render_exif_panel(ui: &mut egui::Ui, file_path: Option<&str>, img_size: Option<(u32, u32)>) {
    Panel::right("exif_panel")
        .frame(panel_frame())
        .show(ui, |ui| {
            ui.heading("Image Info");
            ui.separator();
            if let Some(path) = file_path {
                ui.label(format!("File: {path}"));
            }
            if let Some((w, h)) = img_size {
                ui.label(format!("Dimensions: {w}x{h}"));
            } else {
                ui.label("No image loaded.");
            }
            ui.separator();
            ui.label("EXIF metadata parsing coming soon.");
        });
}

fn render_left_toolbar(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    is_cropping: bool,
    crop_ratio: crate::app::CropRatio,
) {
    egui::Area::new("left_toolbar".into())
        .anchor(egui::Align2::LEFT_CENTER, [16.0, 0.0])
        .show(ui.ctx(), |ui| {
            toolbar_frame().show(ui, |ui| {
                ui.vertical(|ui| {
                    tool_toggles(ui, actions, is_cropping);
                    if is_cropping {
                        crop_ratio_controls(ui, actions, crop_ratio);
                    }
                });
            });
        });
}

fn tool_toggles(ui: &mut egui::Ui, actions: &mut Vec<UiAction>, is_cropping: bool) {
    let mut hand_selected = !is_cropping;
    if ui
        .toggle_value(&mut hand_selected, "✋ Hand Tool")
        .clicked()
        && is_cropping
    {
        actions.push(UiAction::ToggleCrop);
    }

    ui.add_space(4.0);

    let mut crop_selected = is_cropping;
    if ui.toggle_value(&mut crop_selected, "◩ Crop Tool").clicked() && !is_cropping {
        actions.push(UiAction::ToggleCrop);
    }
}

fn crop_ratio_controls(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    crop_ratio: crate::app::CropRatio,
) {
    ui.separator();
    ui.label("Ratio:");
    let mut current = crop_ratio;
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
    if ui.button("Apply Crop").clicked() {
        actions.push(UiAction::ApplyCrop);
    }
}

fn apply_cursor(ui: &mut egui::Ui, is_cropping: bool, is_panning: bool) {
    if ui.ctx().is_pointer_over_egui() {
        return;
    }
    if is_cropping {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    } else if is_panning {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
}

#[cfg(test)]
mod tests {
    use super::UiAction;

    #[test]
    fn ui_action_variants_exist_for_toolbar() {
        // Smoke: enum stays usable from pure tests without a GPU.
        let _ = UiAction::ToggleCrop;
        let _ = UiAction::ApplyCrop;
        let _ = UiAction::SetCropRatio(crate::app::CropRatio::Square);
        let _ = UiAction::Navigate(1);
    }
}
