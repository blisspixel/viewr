//! The application: a message loop of our own on winit's event loop. For Phase 0
//! it opens a window, sets up the GPU renderer, and clears each frame to the
//! theme background. The Elm-style shape (one state, messages, update, render)
//! is borrowed without depending on a UI framework.
#![allow(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::decode::DecodedImage;
use crate::error::Error;
use crate::gpu::{FrameResult, Renderer};
use crate::theme::Mode;

/// Start viewr: create the event loop and run the application to completion. The
/// first command-line argument, if present, is the image to open.
///
/// # Errors
/// Returns [`Error`] if the event loop cannot be created or fails while running.
pub fn run() -> Result<(), Error> {
    let event_loop = EventLoop::new()?;
    // A viewer is idle most of the time; wait for events rather than spin.
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App {
        image_path: std::env::args_os().nth(1).map(PathBuf::from),
        renderer: None,
        playlist: None,
        scanner_rx: None,
        transform: Transform::default(),
        is_fullscreen: false,
        last_trashed: None,
        current_image: None,
        show_exif: false,
        bg_override: None,
        image_loader_rx: None,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct Playlist {
    files: Vec<PathBuf>,
    index: usize,
}

struct TrashedFile {
    original_path: PathBuf,
    trashed_path: PathBuf,
    playlist_index: usize,
}

/// The aspect ratio to enforce when cropping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CropRatio {
    /// Free-form cropping with no locked ratio.
    Free,
    /// 1:1 square crop.
    Square,
    /// 4:3 crop.
    FourThree,
    /// 16:9 crop.
    SixteenNine,
}

#[derive(Clone, Copy)]
#[allow(clippy::struct_excessive_bools)] // independent mode flags for view/crop tools
struct Transform {
    zoom: f32,
    offset_x: f32,
    offset_y: f32,
    rotation_steps: i32,
    flip_h: bool,
    flip_v: bool,
    is_panning: bool,
    last_cursor: Option<(f64, f64)>,
    crop_rect: Option<[f32; 4]>,
    is_cropping: bool,
    crop_ratio: CropRatio,
    crop_start: Option<(f32, f32)>,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
            rotation_steps: 0,
            flip_h: false,
            flip_v: false,
            is_panning: false,
            last_cursor: None,
            crop_rect: None,
            is_cropping: false,
            crop_ratio: CropRatio::Free,
            crop_start: None,
        }
    }
}

/// The whole application state. Deliberately small.
#[derive(Default)]
struct App {
    renderer: Option<Renderer>,
    image_path: Option<PathBuf>,
    playlist: Option<Playlist>,
    scanner_rx: Option<std::sync::mpsc::Receiver<Vec<PathBuf>>>,
    transform: Transform,
    is_fullscreen: bool,
    last_trashed: Option<TrashedFile>,
    current_image: Option<DecodedImage>,
    show_exif: bool,
    bg_override: Option<[f64; 4]>,
    image_loader_rx: Option<
        std::sync::mpsc::Receiver<(
            std::path::PathBuf,
            Result<crate::decode::DecodedImage, String>,
        )>,
    >,
}

impl App {
    fn load_and_scan(&mut self, path: PathBuf) {
        if let Some(renderer) = self.renderer.as_mut() {
            match DecodedImage::load(&path) {
                Ok(image) => {
                    renderer.set_image(&image);
                    self.current_image = Some(image);

                    // Resize window to fit aspect ratio
                    if let Some(monitor) = renderer.window().current_monitor() {
                        let scale = monitor.scale_factor();
                        let max_w = f64::from(monitor.size().width) / scale;
                        let max_h = f64::from(monitor.size().height) / scale;

                        let img_w = f64::from(renderer.image_size().unwrap().0);
                        let img_h = f64::from(renderer.image_size().unwrap().1);

                        // Pick a reasonable max size (80% of monitor)
                        let target_w = (max_w * 0.8).min(img_w);
                        let target_h = (max_h * 0.8).min(img_h);

                        let aspect = img_w / img_h;

                        let (new_w, new_h) = if target_w / aspect <= target_h {
                            (target_w, target_w / aspect)
                        } else {
                            (target_h * aspect, target_h)
                        };

                        if let Some(size) = winit::dpi::LogicalSize::new(new_w, new_h).into() {
                            let _ = renderer.window().request_inner_size(size);
                        }
                    }
                }
                Err(e) => {
                    log::error!("Failed to load {}: {}", path.display(), e);
                    return;
                }
            }
        }
        self.image_path = Some(path.clone());
        self.playlist = None;
        self.transform = Transform::default();

        let (tx, rx) = std::sync::mpsc::channel();
        self.scanner_rx = Some(rx);

        std::thread::spawn(move || {
            let mut files = Vec::new();
            if let Some(parent) = path.parent()
                && let Ok(entries) = std::fs::read_dir(parent)
            {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_file() && crate::fs::is_supported_image(&p) {
                        files.push(p);
                    }
                }
            }
            files.sort_by(|a, b| {
                let a_str = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let b_str = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
                crate::fs::natural_cmp(a_str, b_str)
            });
            let _ = tx.send(files);
        });
    }

    fn screen_to_uv(&self, x: f64, y: f64) -> Option<(f32, f32)> {
        let renderer = self.renderer.as_ref()?;
        let win_size = renderer.window().inner_size();
        if win_size.width == 0 || win_size.height == 0 {
            return None;
        }
        let image_size = renderer.image_size()?;

        let ndc_x = (x as f32 / win_size.width as f32) * 2.0 - 1.0;
        let ndc_y = 1.0 - (y as f32 / win_size.height as f32) * 2.0;

        let rotated90 = self.transform.rotation_steps.rem_euclid(2) != 0;
        let mut p =
            crate::view::fit_to_window((win_size.width, win_size.height), image_size, rotated90);

        p.scale[0] *= self.transform.zoom;
        p.scale[1] *= self.transform.zoom;
        p.offset[0] = self.transform.offset_x;
        p.offset[1] = self.transform.offset_y;

        let corner_x = (ndc_x - p.offset[0]) / p.scale[0];
        let corner_y = (ndc_y - p.offset[1]) / p.scale[1];

        let base_uv_x = (corner_x + 1.0) * 0.5;
        let base_uv_y = (1.0 - corner_y) * 0.5;

        let cx = base_uv_x - 0.5;
        let cy = base_uv_y - 0.5;

        let rot = self.transform.rotation_steps.rem_euclid(4);
        let mut uv_matrix = match rot {
            1 => [0.0, -1.0, 1.0, 0.0],
            2 => [-1.0, 0.0, 0.0, -1.0],
            3 => [0.0, 1.0, -1.0, 0.0],
            _ => [1.0, 0.0, 0.0, 1.0],
        };

        if self.transform.flip_h {
            uv_matrix[0] = -uv_matrix[0];
            uv_matrix[2] = -uv_matrix[2];
        }
        if self.transform.flip_v {
            uv_matrix[1] = -uv_matrix[1];
            uv_matrix[3] = -uv_matrix[3];
        }

        let uv_x = uv_matrix[0] * cx + uv_matrix[2] * cy + 0.5;
        let uv_y = uv_matrix[1] * cx + uv_matrix[3] * cy + 0.5;

        Some((uv_x, uv_y))
    }

    fn navigate(&mut self, delta: isize) {
        if let Some(playlist) = &mut self.playlist {
            if playlist.files.is_empty() {
                return;
            }
            let max_idx = playlist.files.len().saturating_sub(1).cast_signed();
            let new_index = (playlist.index.cast_signed() + delta)
                .clamp(0, max_idx)
                .cast_unsigned();

            if new_index != playlist.index {
                playlist.index = new_index;
                let next_path = playlist.files[new_index].clone();
                self.image_path = Some(next_path.clone());
                self.transform = Transform::default();

                let (tx, rx) = std::sync::mpsc::channel();
                self.image_loader_rx = Some(rx);
                let window = self.renderer.as_ref().map(|r| r.window.clone());

                std::thread::spawn(move || {
                    let res = DecodedImage::load(&next_path).map_err(|e| e.to_string());
                    let _ = tx.send((next_path, res));
                    if let Some(w) = window {
                        w.request_redraw();
                    }
                });
            }
        }
    }

    fn trash_current(&mut self) {
        if let Some(path) = &self.image_path
            && let Some(parent) = path.parent()
        {
            let trash_dir = parent.join("_trash");
            if let Err(e) = std::fs::create_dir_all(&trash_dir) {
                log::error!("Failed to create _trash dir: {e}");
                return;
            }

            if let Some(file_name) = path.file_name() {
                let trashed_path = trash_dir.join(file_name);

                if let Err(e) = std::fs::rename(path, &trashed_path) {
                    log::error!("Failed to move file to trash: {e}");
                    return;
                }

                if let Some(playlist) = &mut self.playlist {
                    self.last_trashed = Some(TrashedFile {
                        original_path: path.clone(),
                        trashed_path,
                        playlist_index: playlist.index,
                    });

                    playlist.files.remove(playlist.index);
                    if playlist.files.is_empty() {
                        self.image_path = None;
                        self.current_image = None;
                        if let Some(renderer) = self.renderer.as_mut() {
                            renderer.clear_image();
                        }
                    } else {
                        playlist.index = playlist.index.min(playlist.files.len().saturating_sub(1));
                        let next_path = playlist.files[playlist.index].clone();
                        self.image_path = Some(next_path.clone());
                        self.transform = Transform::default();

                        let (tx, rx) = std::sync::mpsc::channel();
                        self.image_loader_rx = Some(rx);
                        let window = self.renderer.as_ref().map(|r| r.window.clone());

                        std::thread::spawn(move || {
                            let res = crate::decode::DecodedImage::load(&next_path)
                                .map_err(|e| e.to_string());
                            let _ = tx.send((next_path, res));
                            if let Some(w) = window {
                                w.request_redraw();
                            }
                        });
                    }
                }
            }
        }
    }

    fn undo_trash(&mut self) {
        if let Some(trashed) = self.last_trashed.take() {
            if let Err(e) = std::fs::rename(&trashed.trashed_path, &trashed.original_path) {
                log::error!("Failed to restore file from trash: {e}");
                self.last_trashed = Some(trashed);
                return;
            }

            if let Some(playlist) = &mut self.playlist {
                let index = trashed.playlist_index.min(playlist.files.len());
                playlist.files.insert(index, trashed.original_path.clone());
                playlist.index = index;

                self.image_path = Some(trashed.original_path.clone());
                self.transform = Transform::default();

                let (tx, rx) = std::sync::mpsc::channel();
                self.image_loader_rx = Some(rx);
                let window = self.renderer.as_ref().map(|r| r.window.clone());
                let path_clone = trashed.original_path.clone();

                std::thread::spawn(move || {
                    let res =
                        crate::decode::DecodedImage::load(&path_clone).map_err(|e| e.to_string());
                    let _ = tx.send((path_clone, res));
                    if let Some(w) = window {
                        w.request_redraw();
                    }
                });
            }
        }
    }

    fn star_current(&self) {
        use std::io::Write;
        if let Some(path) = &self.image_path
            && let Some(parent) = path.parent()
            && let Some(file_name) = path.file_name()
        {
            let picks_file = parent.join("_picks.txt");
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(picks_file)
            {
                let name = file_name.to_string_lossy();
                let _ = writeln!(file, "{name}");
            }
        }
    }

    fn save_as(&self) {
        if let Some(path) = &self.image_path
            && let Some(image) = &self.current_image
        {
            let default_name = path.with_extension("jpg");
            if let Some(file_name) = default_name.file_name()
                && let Some(save_path) = rfd::FileDialog::new()
                    .set_file_name(file_name.to_string_lossy())
                    .add_filter("JPEG", &["jpg", "jpeg"])
                    .add_filter("PNG", &["png"])
                    .add_filter("WebP", &["webp"])
                    .add_filter("BMP", &["bmp"])
                    .save_file()
                && let Err(e) = crate::edit::save(image, &save_path)
            {
                log::error!("Failed to save image: {e}");
            }
        }
    }

    /// Convert a UV crop rect (0..1) into pixel coordinates for [`crate::edit::crop`].
    fn crop_pixel_rect(rect: [f32; 4], width: u32, height: u32) -> Option<crate::edit::Rect> {
        let cw = width as f32;
        let ch = height as f32;
        let x = nonneg_round_u32(rect[0] * cw);
        let y = nonneg_round_u32(rect[1] * ch);
        let w = nonneg_round_u32((rect[2] - rect[0]) * cw);
        let h = nonneg_round_u32((rect[3] - rect[1]) * ch);
        if w == 0 || h == 0 {
            return None;
        }
        Some(crate::edit::Rect {
            x,
            y,
            width: w,
            height: h,
        })
    }

    fn apply_crop_rect(&mut self) {
        let Some(rect) = self.transform.crop_rect else {
            return;
        };
        let Some(image) = &self.current_image else {
            return;
        };
        let Some(pixel_rect) = Self::crop_pixel_rect(rect, image.width, image.height) else {
            return;
        };
        let cropped = crate::edit::crop(image, pixel_rect);
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_image(&cropped);
        }
        self.current_image = Some(cropped);
        self.transform = Transform::default();
        if let Some(r) = self.renderer.as_mut() {
            r.window().request_redraw();
        }
    }
}

/// Round a non-negative f32 to u32 without triggering sign-loss noise.
fn nonneg_round_u32(v: f32) -> u32 {
    let v = v.round().max(0.0);
    if v >= f32::from(u16::MAX) {
        u32::from(u16::MAX)
    } else {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            v as u32
        }
    }
}

fn load_icon() -> Option<winit::window::Icon> {
    let bytes = include_bytes!("../../../assets/icon.ico");
    if let Ok(image) = image::load_from_memory(bytes) {
        let rgba = image.into_rgba8();
        let (width, height) = rgba.dimensions();
        winit::window::Icon::from_rgba(rgba.into_raw(), width, height).ok()
    } else {
        None
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title("viewr")
            .with_inner_size(LogicalSize::new(1100.0, 750.0))
            .with_visible(false);

        if let Some(icon) = load_icon() {
            attrs = attrs.with_window_icon(Some(icon));
        }

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };

        let mode = Mode::from_winit_or_dark(window.theme());
        match pollster::block_on(Renderer::new(window, mode)) {
            Ok(renderer) => {
                let initial_path = self.image_path.clone();
                self.renderer = Some(renderer);

                if let Some(path) = initial_path {
                    self.load_and_scan(path);
                }

                let _ = self.renderer.as_mut().unwrap().render(None, |_| {});
                self.renderer.as_ref().unwrap().window().set_visible(true);
            }
            Err(e) => {
                log::error!("failed to initialize gpu: {e}");
                event_loop.exit();
            }
        }
    }

    #[allow(clippy::too_many_lines)] // single key/mouse dispatch table; splitting scatters bindings
    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.renderer.is_none() {
            return;
        }

        if let Some(renderer) = &mut self.renderer
            && renderer.window().id() == window_id
        {
            let window = renderer.window.clone();
            let response = renderer.egui_state.on_window_event(window.as_ref(), &event);
            if response.consumed {
                return;
            }
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::DroppedFile(path) => {
                self.load_and_scan(path);
            }
            WindowEvent::MouseWheel {
                delta: winit::event::MouseScrollDelta::LineDelta(_, y),
                ..
            } => {
                let zoom_factor = if y > 0.0 { 1.1 } else { 0.9 };
                self.transform.zoom *= zoom_factor;
                self.renderer.as_mut().unwrap().window().request_redraw();
            }
            WindowEvent::MouseInput {
                state,
                button: winit::event::MouseButton::Left,
                ..
            } => {
                if self.transform.is_cropping {
                    if state == winit::event::ElementState::Pressed {
                        if let Some((x, y)) = self.transform.last_cursor {
                            self.transform.crop_start = self.screen_to_uv(x, y);
                            self.transform.crop_rect = None;
                            self.renderer.as_mut().unwrap().window().request_redraw();
                        }
                    } else {
                        self.transform.crop_start = None;
                    }
                } else {
                    self.transform.is_panning = state == winit::event::ElementState::Pressed;
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if self.transform.is_cropping
                    && let Some(start) = self.transform.crop_start
                    && let Some(end) = self.screen_to_uv(position.x, position.y)
                {
                    let mut u_min = start.0.min(end.0).clamp(0.0, 1.0);
                    let mut v_min = start.1.min(end.1).clamp(0.0, 1.0);
                    let mut u_max = start.0.max(end.0).clamp(0.0, 1.0);
                    let mut v_max = start.1.max(end.1).clamp(0.0, 1.0);

                    if self.transform.crop_ratio != CropRatio::Free
                        && let Some(renderer) = self.renderer.as_ref()
                        && let Some((img_w, img_h)) = renderer.image_size()
                    {
                        // Free is excluded by the outer guard; grouped with Square for exhaustiveness.
                        let target_ratio = match self.transform.crop_ratio {
                            CropRatio::FourThree => 4.0 / 3.0,
                            CropRatio::SixteenNine => 16.0 / 9.0,
                            CropRatio::Square | CropRatio::Free => 1.0,
                        };

                        let width = (u_max - u_min) * (img_w as f32);
                        let height = (v_max - v_min) * (img_h as f32);

                        if height > 0.0 {
                            let current_ratio = width / height;
                            if current_ratio > target_ratio {
                                let new_width = height * target_ratio;
                                let u_diff = new_width / (img_w as f32);
                                if end.0 > start.0 {
                                    u_max = u_min + u_diff;
                                } else {
                                    u_min = u_max - u_diff;
                                }
                            } else {
                                let new_height = width / target_ratio;
                                let v_diff = new_height / (img_h as f32);
                                if end.1 > start.1 {
                                    v_max = v_min + v_diff;
                                } else {
                                    v_min = v_max - v_diff;
                                }
                            }
                        }
                    }
                    self.transform.crop_rect = Some([
                        u_min.clamp(0.0, 1.0),
                        v_min.clamp(0.0, 1.0),
                        u_max.clamp(0.0, 1.0),
                        v_max.clamp(0.0, 1.0),
                    ]);
                    self.renderer.as_mut().unwrap().window().request_redraw();
                } else if self.transform.is_panning
                    && let Some((last_x, last_y)) = self.transform.last_cursor
                {
                    let dx = position.x - last_x;
                    let dy = position.y - last_y;
                    let win_size = self.renderer.as_mut().unwrap().window().inner_size();

                    self.transform.offset_x += (dx as f32) / (win_size.width as f32 / 2.0);
                    self.transform.offset_y -= (dy as f32) / (win_size.height as f32 / 2.0);

                    self.renderer.as_mut().unwrap().window().request_redraw();
                }
                self.transform.last_cursor = Some((position.x, position.y));
            }
            WindowEvent::KeyboardInput {
                event:
                    winit::event::KeyEvent {
                        state: winit::event::ElementState::Pressed,
                        logical_key,
                        ..
                    },
                ..
            } => {
                use winit::keyboard::{Key, NamedKey};
                match logical_key {
                    Key::Character(c) if c == "o" || c == "O" => {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter(
                                "Images",
                                &[
                                    "jpg", "jpeg", "png", "gif", "webp", "bmp", "ico", "tiff",
                                    "tga", "hdr", "avif", "heic", "heif", "cr2", "nef", "arw",
                                    "dng",
                                ],
                            )
                            .pick_file()
                        {
                            self.load_and_scan(path);
                        }
                    }
                    Key::Character(c) if c == " " => {
                        self.transform = Transform::default();
                        self.renderer.as_mut().unwrap().window().request_redraw();
                    }
                    Key::Character(c) if c == "r" || c == "R" => {
                        self.transform.rotation_steps += 1;
                        self.renderer.as_mut().unwrap().window().request_redraw();
                    }
                    Key::Character(c) if c == "l" || c == "L" => {
                        self.transform.rotation_steps -= 1;
                        self.renderer.as_mut().unwrap().window().request_redraw();
                    }
                    Key::Character(c) if c == "h" || c == "H" => {
                        self.transform.flip_h = !self.transform.flip_h;
                        self.renderer.as_mut().unwrap().window().request_redraw();
                    }
                    Key::Character(c) if c == "v" || c == "V" => {
                        self.transform.flip_v = !self.transform.flip_v;
                        self.renderer.as_mut().unwrap().window().request_redraw();
                    }
                    Key::Character(c) if c == "s" || c == "S" => {
                        self.star_current();
                    }
                    Key::Character(c) if c == "w" || c == "W" => {
                        self.save_as();
                    }
                    Key::Character(c) if c == "c" || c == "C" => {
                        self.transform.is_cropping = !self.transform.is_cropping;
                        if !self.transform.is_cropping {
                            self.transform.crop_rect = None;
                            self.transform.crop_start = None;
                        }
                        self.renderer.as_mut().unwrap().window().request_redraw();
                    }
                    Key::Named(NamedKey::Enter) => {
                        self.apply_crop_rect();
                    }
                    Key::Character(c) if c == "u" || c == "U" => {
                        self.undo_trash();
                        self.renderer.as_mut().unwrap().window().request_redraw();
                    }
                    Key::Character(c) if c == "f" || c == "F" => {
                        self.is_fullscreen = !self.is_fullscreen;
                        let renderer = self.renderer.as_mut().unwrap();
                        if self.is_fullscreen {
                            renderer
                                .window()
                                .set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
                        } else {
                            renderer.window().set_fullscreen(None);
                        }
                    }
                    Key::Named(NamedKey::ArrowRight) => self.navigate(1),
                    Key::Named(NamedKey::ArrowLeft) => self.navigate(-1),
                    Key::Named(NamedKey::Home) => self.navigate(-999_999),
                    Key::Named(NamedKey::End) => self.navigate(999_999),
                    Key::Named(NamedKey::Delete | NamedKey::Backspace) => {
                        self.trash_current();
                        if let Some(r) = self.renderer.as_mut() {
                            r.window().request_redraw();
                        }
                    }
                    Key::Named(NamedKey::F11) => {
                        self.is_fullscreen = !self.is_fullscreen;
                        let renderer = self.renderer.as_mut().unwrap();
                        if self.is_fullscreen {
                            renderer
                                .window()
                                .set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
                        } else {
                            renderer.window().set_fullscreen(None);
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::Resized(size) => {
                let renderer = self.renderer.as_mut().unwrap();
                renderer.resize(size.width, size.height);
                renderer.window().request_redraw();
            }
            WindowEvent::ThemeChanged(theme) => {
                let renderer = self.renderer.as_mut().unwrap();
                renderer.set_mode(Mode::from_winit(theme));
                renderer.window().request_redraw();
            }
            WindowEvent::RedrawRequested => {
                let mut ui_actions = Vec::new();
                let renderer = self.renderer.as_mut().unwrap();
                renderer.window().set_visible(true);

                if let Some(bg) = self.bg_override {
                    renderer.set_clear_color(bg);
                } else {
                    renderer.set_mode(crate::theme::Mode::from_winit_or_dark(
                        renderer.window().theme(),
                    ));
                }

                let placement = if let Some(size) = renderer.image_size() {
                    let win_size = renderer.window().inner_size();
                    let rotated90 = self.transform.rotation_steps.rem_euclid(2) != 0;
                    let mut p = crate::view::fit_to_window(
                        (win_size.width, win_size.height),
                        size,
                        rotated90,
                    );

                    p.scale[0] *= self.transform.zoom;
                    p.scale[1] *= self.transform.zoom;
                    p.offset[0] = self.transform.offset_x;
                    p.offset[1] = self.transform.offset_y;

                    let rot = self.transform.rotation_steps.rem_euclid(4);
                    let mut uv_matrix = match rot {
                        1 => [0.0, -1.0, 1.0, 0.0],
                        2 => [-1.0, 0.0, 0.0, -1.0],
                        3 => [0.0, 1.0, -1.0, 0.0],
                        _ => [1.0, 0.0, 0.0, 1.0],
                    };

                    if self.transform.flip_h {
                        uv_matrix[0] = -uv_matrix[0];
                        uv_matrix[2] = -uv_matrix[2];
                    }
                    if self.transform.flip_v {
                        uv_matrix[1] = -uv_matrix[1];
                        uv_matrix[3] = -uv_matrix[3];
                    }
                    p.uv_matrix = uv_matrix;

                    if let Some(cr) = self.transform.crop_rect {
                        p.crop_rect = cr;
                    }

                    Some(p)
                } else {
                    None
                };

                let path_str = self
                    .image_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned());
                let img_size = renderer.image_size();
                let show_exif = self.show_exif;
                let is_cropping = self.transform.is_cropping;
                let crop_ratio = self.transform.crop_ratio;
                let is_panning = self.transform.is_panning;

                match renderer.render(placement, |ui| {
                    ui_actions = crate::ui::render(
                        ui,
                        show_exif,
                        path_str.as_deref(),
                        img_size,
                        is_cropping,
                        crop_ratio,
                        is_panning,
                    );
                }) {
                    FrameResult::Presented | FrameResult::Skipped => {}
                    FrameResult::NeedsReconfigure => renderer.reconfigure(),
                }

                for action in ui_actions {
                    match action {
                        crate::ui::UiAction::Open => {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter(
                                    "Images",
                                    &[
                                        "jpg", "jpeg", "png", "gif", "webp", "bmp", "ico", "tiff",
                                        "tga", "hdr", "avif", "heic", "heif", "cr2", "nef", "arw",
                                        "dng",
                                    ],
                                )
                                .pick_file()
                            {
                                self.load_and_scan(path);
                            }
                        }
                        crate::ui::UiAction::SaveAs => self.save_as(),
                        crate::ui::UiAction::Trash => {
                            self.trash_current();
                            if let Some(r) = self.renderer.as_mut() {
                                r.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::UndoTrash => {
                            self.undo_trash();
                            if let Some(r) = self.renderer.as_mut() {
                                r.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::SetBackground(bg) => {
                            self.bg_override = bg;
                            if let Some(r) = self.renderer.as_mut() {
                                r.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::ToggleExif => {
                            self.show_exif = !self.show_exif;
                        }
                        crate::ui::UiAction::RotateCw => {
                            self.transform.rotation_steps += 1;
                            if let Some(r) = self.renderer.as_mut() {
                                r.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::RotateCcw => {
                            self.transform.rotation_steps -= 1;
                            if let Some(r) = self.renderer.as_mut() {
                                r.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::FlipH => {
                            self.transform.flip_h = !self.transform.flip_h;
                            if let Some(r) = self.renderer.as_mut() {
                                r.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::FlipV => {
                            self.transform.flip_v = !self.transform.flip_v;
                            if let Some(r) = self.renderer.as_mut() {
                                r.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::ToggleFullscreen => {
                            self.is_fullscreen = !self.is_fullscreen;
                            let renderer = self.renderer.as_mut().unwrap();
                            if self.is_fullscreen {
                                renderer.window().set_fullscreen(Some(
                                    winit::window::Fullscreen::Borderless(None),
                                ));
                            } else {
                                renderer.window().set_fullscreen(None);
                            }
                        }
                        crate::ui::UiAction::Navigate(d) => self.navigate(d),
                        crate::ui::UiAction::ToggleCrop => {
                            self.transform.is_cropping = !self.transform.is_cropping;
                            if !self.transform.is_cropping {
                                self.transform.crop_rect = None;
                                self.transform.crop_start = None;
                            }
                            if let Some(r) = self.renderer.as_mut() {
                                r.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::ApplyCrop => {
                            self.apply_crop_rect();
                        }
                        crate::ui::UiAction::SetCropRatio(r) => {
                            self.transform.crop_ratio = r;
                            if let Some(renderer) = self.renderer.as_mut() {
                                renderer.window().request_redraw();
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(rx) = &self.image_loader_rx
            && let Ok((path, result)) = rx.try_recv()
        {
            self.image_loader_rx = None;
            if Some(&path) == self.image_path.as_ref() {
                match result {
                    Ok(image) => {
                        if let Some(renderer) = self.renderer.as_mut() {
                            renderer.set_image(&image);
                        }
                        self.current_image = Some(image);
                    }
                    Err(e) => log::error!("{e}"),
                }
                if let Some(r) = self.renderer.as_mut() {
                    r.window().request_redraw();
                }
            }
        }

        if let Some(rx) = &self.scanner_rx
            && let Ok(files) = rx.try_recv()
        {
            self.scanner_rx = None;
            if let Some(current) = &self.image_path {
                let index = files.iter().position(|p| p == current).unwrap_or(0);
                self.playlist = Some(Playlist { files, index });
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_load_icon() {
        assert!(load_icon().is_some(), "load_icon returned None!");
    }
}
