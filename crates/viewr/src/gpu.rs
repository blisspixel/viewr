//! The wgpu rendering pipeline: clears the window to the theme background and,
//! when an image is loaded, draws it as a textured quad scaled to fit. Later
//! phases add the neighbor texture cache and zoom/pan on top of this.

use std::sync::Arc;
use std::time::Duration;

use winit::window::Window;

use crate::decode::DecodedImage;
use crate::error::Error;
use crate::theme::{self, Mode, Palette};
use crate::view;

/// Owns the GPU surface and everything needed to draw a frame.
pub struct Renderer {
    /// A reference-counted handle to the application window.
    pub window: Arc<winit::window::Window>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    clear: wgpu::Color,
    max_dim: u32,
    max_base_pixels: u64,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    mipmap_blitter: wgpu::util::TextureBlitter,
    pipeline: wgpu::RenderPipeline,
    image: Option<Image>,
    placement: wgpu::Buffer,
    /// The egui context for immediate mode UI.
    pub egui_ctx: egui::Context,
    /// The winit state integration for egui.
    pub egui_state: egui_winit::State,
    /// Native accessibility bridge through the target platform's local IPC.
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    accesskit: Option<accesskit_winit::Adapter>,
    /// Accessibility actions queued by the native bridge for the next frame.
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    pending_accesskit_actions: Vec<egui::accesskit::ActionRequest>,
    /// The wgpu renderer integration for egui.
    pub egui_renderer: egui_wgpu::Renderer,
}

/// The currently displayed image: its GPU binding and pixel dimensions.
struct Image {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    source_size: (u32, u32),
    texture_size: (u32, u32),
    mip_level_count: u32,
}

/// Maximum number of RGBA pixels retained in the base GPU image texture.
///
/// A complete mip chain adds at most one third again, so this caps the image
/// allocation at roughly 341 MiB while preserving full-resolution CPU pixels for
/// export. Typical 60-megapixel camera images still fit without a proxy.
pub(crate) const MAX_GPU_BASE_PIXELS: u64 = 64 * 1024 * 1024;

/// The explicit GUI probe uses a lower limit so ordinary CI hardware exercises
/// the asynchronous preview path without allocating a hostile-size fixture.
pub(crate) const PERFORMANCE_PROBE_GPU_BASE_PIXELS: u64 = 1024 * 1024;

/// Dimensions selected for one bounded GPU preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PreviewSpec {
    width: u32,
    height: u32,
    source_size: (u32, u32),
}

/// A complete, validated preview prepared away from the event thread.
pub(crate) struct ImagePreview {
    rgba: Vec<u8>,
    spec: PreviewSpec,
}

/// Largest aspect-preserving dimensions that fit texture and pixel limits.
#[must_use]
fn texture_dimensions(source: (u32, u32), max_dim: u32, max_pixels: u64) -> (u32, u32) {
    let (width, height) = source;
    let source_pixels = u64::from(width).saturating_mul(u64::from(height));
    if width <= max_dim && height <= max_dim && source_pixels <= max_pixels {
        return source;
    }
    let longest = u64::from(width.max(height));
    let dimensions_at = |long_edge: u64| {
        let scaled = |edge: u32| {
            u32::try_from(u64::from(edge).saturating_mul(long_edge) / longest)
                .unwrap_or(max_dim)
                .max(1)
        };
        (scaled(width), scaled(height))
    };

    let mut low = 1_u64;
    let mut high = longest.min(u64::from(max_dim.max(1)));
    let mut best = dimensions_at(1);
    while low <= high {
        let middle = low + (high - low) / 2;
        let candidate = dimensions_at(middle);
        let pixels = u64::from(candidate.0).saturating_mul(u64::from(candidate.1));
        if pixels <= max_pixels {
            best = candidate;
            low = middle.saturating_add(1);
        } else {
            high = middle.saturating_sub(1);
        }
    }
    best
}

/// Number of levels needed to reduce the longest edge to one pixel.
#[must_use]
fn mip_level_count(size: (u32, u32)) -> u32 {
    size.0.max(size.1).max(1).ilog2() + 1
}

fn preview_spec(source: (u32, u32), max_dim: u32, max_pixels: u64) -> Option<PreviewSpec> {
    let target = texture_dimensions(source, max_dim, max_pixels);
    (target != source).then_some(PreviewSpec {
        width: target.0,
        height: target.1,
        source_size: source,
    })
}

/// Build a linear-light, alpha-correct area preview with bounded allocation.
/// Returning `Ok(None)` means the generation was canceled between output rows.
pub(crate) fn prepare_image_preview(
    image: &DecodedImage,
    spec: PreviewSpec,
    is_cancelled: impl Fn() -> bool,
) -> Result<Option<ImagePreview>, Error> {
    if spec.source_size != (image.width, image.height) || spec.width == 0 || spec.height == 0 {
        return Err(Error::Gpu(
            "image preview dimensions are inconsistent".into(),
        ));
    }
    let source_len = viewr_protocol::checked_rgba_len(image.width, image.height)
        .map_err(|error| Error::Gpu(error.to_string()))?;
    if image.rgba.len() != source_len {
        return Err(Error::Gpu(
            "image preview source does not match its dimensions".into(),
        ));
    }
    let output_len = viewr_protocol::checked_rgba_len(spec.width, spec.height)
        .map_err(|error| Error::Gpu(error.to_string()))?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(output_len)
        .map_err(|error| Error::Gpu(format!("could not allocate image preview: {error}")))?;
    rgba.resize(output_len, 0);

    let source_width = f64::from(image.width);
    let source_height = f64::from(image.height);
    let target_width = f64::from(spec.width);
    let target_height = f64::from(spec.height);
    let linear = srgb_decode_table();

    for target_y in 0..spec.height {
        if is_cancelled() {
            return Ok(None);
        }
        let source_top = f64::from(target_y) * source_height / target_height;
        let source_bottom = f64::from(target_y + 1) * source_height / target_height;
        let first_y = nonnegative_floor_to_u32(source_top);
        let last_y = nonnegative_ceil_to_u32(source_bottom).min(image.height);
        for target_x in 0..spec.width {
            let source_left = f64::from(target_x) * source_width / target_width;
            let source_right = f64::from(target_x + 1) * source_width / target_width;
            let first_x = nonnegative_floor_to_u32(source_left);
            let last_x = nonnegative_ceil_to_u32(source_right).min(image.width);
            let mut total_weight = 0.0_f32;
            let mut alpha_sum = 0.0_f32;
            let mut red_sum = 0.0_f32;
            let mut green_sum = 0.0_f32;
            let mut blue_sum = 0.0_f32;

            for source_y in first_y..last_y {
                let vertical = (source_bottom.min(f64::from(source_y + 1))
                    - source_top.max(f64::from(source_y))) as f32;
                for source_x in first_x..last_x {
                    let horizontal = (source_right.min(f64::from(source_x + 1))
                        - source_left.max(f64::from(source_x)))
                        as f32;
                    let weight = horizontal * vertical;
                    let offset = usize::try_from(
                        (u64::from(source_y) * u64::from(image.width) + u64::from(source_x)) * 4,
                    )
                    .map_err(|_| Error::Gpu("image preview offset overflowed".into()))?;
                    let pixel = &image.rgba[offset..offset + 4];
                    let alpha = f32::from(pixel[3]) / 255.0;
                    let premultiplied_weight = weight * alpha;
                    total_weight += weight;
                    alpha_sum += premultiplied_weight;
                    red_sum += linear[usize::from(pixel[0])] * premultiplied_weight;
                    green_sum += linear[usize::from(pixel[1])] * premultiplied_weight;
                    blue_sum += linear[usize::from(pixel[2])] * premultiplied_weight;
                }
            }

            let output_offset = usize::try_from(
                (u64::from(target_y) * u64::from(spec.width) + u64::from(target_x)) * 4,
            )
            .map_err(|_| Error::Gpu("image preview output offset overflowed".into()))?;
            let output = &mut rgba[output_offset..output_offset + 4];
            if alpha_sum > 0.0 {
                output[0] = linear_to_srgb_byte(red_sum / alpha_sum);
                output[1] = linear_to_srgb_byte(green_sum / alpha_sum);
                output[2] = linear_to_srgb_byte(blue_sum / alpha_sum);
            }
            output[3] = unit_to_byte(alpha_sum / total_weight.max(f32::EPSILON));
        }
    }

    Ok(Some(ImagePreview { rgba, spec }))
}

fn srgb_decode_table() -> &'static [f32; 256] {
    static TABLE: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        std::array::from_fn(|index| {
            let encoded = index as f32 / 255.0;
            if encoded <= 0.040_45 {
                encoded / 12.92
            } else {
                ((encoded + 0.055) / 1.055).powf(2.4)
            }
        })
    })
}

fn linear_to_srgb_byte(linear: f32) -> u8 {
    const STEPS: usize = 4096;
    static TABLE: std::sync::OnceLock<[u8; STEPS + 1]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        std::array::from_fn(|index| {
            let linear = index as f32 / STEPS as f32;
            let encoded = if linear <= 0.003_130_8 {
                linear * 12.92
            } else {
                1.055 * linear.powf(1.0 / 2.4) - 0.055
            };
            unit_to_byte(encoded)
        })
    });
    let index = (linear.clamp(0.0, 1.0) * STEPS as f32).round();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        table[index as usize]
    }
}

fn unit_to_byte(value: f32) -> u8 {
    let rounded = (value.clamp(0.0, 1.0) * 255.0).round();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        rounded as u8
    }
}

fn nonnegative_ceil_to_u32(value: f64) -> u32 {
    let value = value.ceil().clamp(0.0, f64::from(u32::MAX));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        value as u32
    }
}

fn nonnegative_floor_to_u32(value: f64) -> u32 {
    let value = value.floor().clamp(0.0, f64::from(u32::MAX));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        value as u32
    }
}

fn build_image_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("viewr sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    })
}

fn build_mipmap_blitter(device: &wgpu::Device) -> wgpu::util::TextureBlitter {
    wgpu::util::TextureBlitterBuilder::new(device, wgpu::TextureFormat::Rgba8UnormSrgb)
        .sample_type(wgpu::FilterMode::Linear)
        .build()
}

impl Renderer {
    /// Create a renderer for `window`, clearing to the palette for `mode`.
    ///
    /// # Errors
    /// Returns an error if the GPU adapter, device, or surface cannot be created.
    pub async fn new(
        window: Arc<winit::window::Window>,
        mode: Mode,
        max_base_pixels: u64,
    ) -> Result<Self, Error> {
        let size = window.inner_size();

        // Honors WGPU_BACKEND and related env vars, else picks sensible defaults.
        let instance = wgpu::Instance::default();

        let surface = instance
            .create_surface(Arc::clone(&window))
            .map_err(|e| Error::Gpu(format!("create_surface: {e}")))?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            })
            .await
            .map_err(|e| Error::Gpu(format!("request_adapter: {e}")))?;

        // Request exactly what this adapter supports. A desktop viewer must
        // handle hi-DPI windows and large images, so the downlevel/webgl caps
        // (max texture 2048) are far too small; the adapter's own limits are
        // always satisfiable and give us the real hardware maximum.
        let limits = adapter.limits();
        let max_dim = limits.max_texture_dimension_2d;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("viewr_device"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                memory_hints: wgpu::MemoryHints::Performance,
                ..Default::default()
            })
            .await
            .map_err(|e| Error::Gpu(format!("request_device: {e}")))?;

        let width = size.width.clamp(1, max_dim);
        let height = size.height.clamp(1, max_dim);

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo, // Standard VSync to guarantee no flickering
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let (pipeline, bind_layout) = build_pipeline(&device, format);
        let sampler = build_image_sampler(&device);
        let mipmap_blitter = build_mipmap_blitter(&device);
        let placement = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewr placement uniform"),
            size: PLACEMENT_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let egui_ctx = egui::Context::default();
        let viewport_id = egui_ctx.viewport_id();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            viewport_id,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            &device,
            config.format,
            egui_wgpu::RendererOptions::default(),
        );

        Ok(Self {
            window,
            device,
            queue,
            surface,
            config,
            clear: palette_to_color(theme::palette_for(mode)),
            max_dim,
            max_base_pixels: max_base_pixels.max(1),
            bind_layout,
            sampler,
            mipmap_blitter,
            pipeline,
            image: None,
            placement,
            egui_ctx,
            egui_state,
            #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
            accesskit: None,
            #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
            pending_accesskit_actions: Vec::new(),
            egui_renderer,
        })
    }

    /// The window this renderer draws into.
    #[must_use]
    pub fn window(&self) -> &Arc<Window> {
        &self.window
    }

    /// Initialize native accessibility before the initially hidden window is shown.
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    pub fn init_accessibility<T>(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        proxy: winit::event_loop::EventLoopProxy<T>,
    ) where
        T: From<accesskit_winit::Event> + Send + 'static,
    {
        self.accesskit = Some(accesskit_winit::Adapter::with_event_loop_proxy(
            event_loop,
            self.window.as_ref(),
            proxy,
        ));
    }

    /// Forward a native window event to the accessibility adapter.
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    pub fn process_accessibility_window_event(
        &mut self,
        window: &Window,
        event: &winit::event::WindowEvent,
    ) {
        if let Some(adapter) = self.accesskit.as_mut() {
            adapter.process_event(window, event);
        }
    }

    /// Queue an assistive-technology action for egui's next input frame.
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    pub fn queue_accessibility_action(&mut self, request: egui::accesskit::ActionRequest) {
        self.pending_accesskit_actions.push(request);
    }

    fn append_accessibility_actions(&mut self, input: &mut egui::RawInput) {
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        input.events.extend(
            self.pending_accesskit_actions
                .drain(..)
                .map(egui::Event::AccessKitActionRequest),
        );
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            let _ = self;
            let _ = input;
        }
    }

    fn publish_accessibility_update(&mut self, output: &mut egui::PlatformOutput) {
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        if let (Some(adapter), Some(update)) =
            (self.accesskit.as_mut(), output.accesskit_update.take())
        {
            adapter.update_if_active(|| update);
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            let _ = self;
            let _ = output;
        }
    }

    /// The current image size, if any.
    #[must_use]
    pub fn image_size(&self) -> Option<(u32, u32)> {
        self.image.as_ref().map(|img| img.source_size)
    }

    /// Pixel dimensions uploaded to the GPU. These can be smaller than the
    /// source dimensions on adapters with a lower texture limit.
    #[must_use]
    pub fn image_texture_size(&self) -> Option<(u32, u32)> {
        self.image.as_ref().map(|img| img.texture_size)
    }

    /// Clear the currently displayed image.
    pub fn clear_image(&mut self) {
        self.image = None;
    }

    /// Return the preview dimensions required by this adapter, if any.
    #[must_use]
    pub(crate) fn required_preview(&self, image: &DecodedImage) -> Option<PreviewSpec> {
        preview_spec(
            (image.width, image.height),
            self.max_dim,
            self.max_base_pixels,
        )
    }

    /// Upload `image` as the currently displayed image, replacing any previous
    /// one. A required over-limit preview must already have been prepared away
    /// from the event thread.
    ///
    /// Returns `true` when the source was uploaded at full resolution.
    ///
    /// # Errors
    /// Returns [`Error::Gpu`] when the source or preview buffer is malformed,
    /// missing, stale, or inconsistent with this adapter's limits.
    pub(crate) fn set_image(
        &mut self,
        image: &DecodedImage,
        prepared: Option<&ImagePreview>,
    ) -> Result<bool, Error> {
        let source_len = viewr_protocol::checked_rgba_len(image.width, image.height)
            .map_err(|error| Error::Gpu(error.to_string()))?;
        if image.rgba.len() != source_len {
            return Err(Error::Gpu(
                "image pixels do not match their declared dimensions".into(),
            ));
        }
        let required = self.required_preview(image);
        let (width, height, rgba, full_resolution) = match (required, prepared) {
            (None, None) => (image.width, image.height, image.rgba.as_slice(), true),
            (Some(required), Some(preview)) if preview.spec == required => {
                let expected = viewr_protocol::checked_rgba_len(required.width, required.height)
                    .map_err(|error| Error::Gpu(error.to_string()))?;
                if preview.rgba.len() != expected {
                    return Err(Error::Gpu(
                        "prepared preview does not match its dimensions".into(),
                    ));
                }
                (
                    required.width,
                    required.height,
                    preview.rgba.as_slice(),
                    false,
                )
            }
            (Some(_), None) => {
                return Err(Error::Gpu(
                    "image requires a background-prepared GPU preview".into(),
                ));
            }
            (None | Some(_), Some(_)) => {
                return Err(Error::Gpu("prepared image preview is stale".into()));
            }
        };
        let mip_level_count = mip_level_count((width, height));

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewr image"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.regenerate_mipmaps(&texture, mip_level_count);

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewr image bind group"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.placement.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        self.image = Some(Image {
            texture,
            bind_group,
            source_size: (image.width, image.height),
            texture_size: (width, height),
            mip_level_count,
        });
        self.window.request_redraw();
        Ok(full_resolution)
    }

    /// Upload one tightly packed RGBA8 patch into the current image texture.
    ///
    /// Returns `false` without writing when the patch shape does not exactly fit
    /// inside the displayed texture. Normal Spot Heal operations use this path
    /// so committing a small edit never reallocates or uploads the full image.
    #[must_use]
    pub fn update_image_patch(&self, patch: &crate::heal::ImagePatch) -> bool {
        let Some(image) = self.image.as_ref() else {
            return false;
        };
        let bounds = patch.bounds;
        let Some(right) = bounds.x.checked_add(bounds.width) else {
            return false;
        };
        let Some(bottom) = bounds.y.checked_add(bounds.height) else {
            return false;
        };
        let Some(expected_bytes) = usize::try_from(bounds.width)
            .ok()
            .and_then(|width| {
                usize::try_from(bounds.height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(4))
        else {
            return false;
        };
        if bounds.width == 0
            || bounds.height == 0
            || right > image.texture_size.0
            || bottom > image.texture_size.1
            || patch.rgba.len() != expected_bytes
        {
            return false;
        }

        let mut destination = image.texture.as_image_copy();
        destination.origin = wgpu::Origin3d {
            x: bounds.x,
            y: bounds.y,
            z: 0,
        };
        self.queue.write_texture(
            destination,
            &patch.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bounds.width * 4),
                rows_per_image: Some(bounds.height),
            },
            wgpu::Extent3d {
                width: bounds.width,
                height: bounds.height,
                depth_or_array_layers: 1,
            },
        );
        self.regenerate_mipmaps(&image.texture, image.mip_level_count);
        self.window.request_redraw();
        true
    }

    /// Rebuild every derived level from the preceding sRGB texture level.
    /// Sampling decodes sRGB and rendering re-encodes it, so minification is
    /// averaged in linear light rather than directly averaging encoded bytes.
    fn regenerate_mipmaps(&self, texture: &wgpu::Texture, mip_level_count: u32) {
        if mip_level_count <= 1 {
            return;
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("viewr mipmap encoder"),
            });
        for target_level in 1..mip_level_count {
            let source = texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("viewr mipmap source"),
                usage: Some(wgpu::TextureUsages::TEXTURE_BINDING),
                base_mip_level: target_level - 1,
                mip_level_count: Some(1),
                ..Default::default()
            });
            let target = texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("viewr mipmap target"),
                usage: Some(wgpu::TextureUsages::RENDER_ATTACHMENT),
                base_mip_level: target_level,
                mip_level_count: Some(1),
                ..Default::default()
            });
            self.mipmap_blitter
                .copy(&self.device, &mut encoder, &source, &target);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Set the clear color.
    pub fn set_clear_color(&mut self, color: [f64; 4]) {
        self.clear = wgpu::Color {
            r: color[0],
            g: color[1],
            b: color[2],
            a: color[3],
        };
    }

    /// Change the clear color to match `mode`.
    pub fn set_mode(&mut self, mode: Mode) {
        self.clear = palette_to_color(theme::palette_for(mode));
    }

    /// Resize the surface. Dimensions are clamped to `1..=max_dim` so
    /// configuration never fails when the window is minimized or larger than the
    /// GPU's maximum texture size.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.clamp(1, self.max_dim);
        self.config.height = height.clamp(1, self.max_dim);
        self.surface.configure(&self.device, &self.config);
    }

    /// Reconfigure the surface after it is lost or outdated.
    pub fn reconfigure(&mut self) {
        self.surface.configure(&self.device, &self.config);
    }

    /// Draw one frame: clear to the background and draw the image if present.
    /// Also draws the egui user interface overlay.
    #[allow(clippy::too_many_lines)] // wgpu + egui frame path is one pipeline sequence
    pub fn render(
        &mut self,
        placement: Option<crate::view::Placement>,
        image_viewport: Option<crate::view::PhysicalViewport>,
        mut app_ui: impl FnMut(&mut egui::Ui),
    ) -> FrameOutput {
        let (frame, suboptimal) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => (f, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => (f, true),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                return FrameOutput::without_egui(FrameResult::NeedsReconfigure);
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => {
                return FrameOutput::without_egui(FrameResult::Skipped);
            }
        };

        if let Some(placement_matrix) = placement {
            self.queue
                .write_buffer(&self.placement, 0, &pack_placement(&placement_matrix));
        }

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut raw_input = self.egui_state.take_egui_input(self.window.as_ref());
        self.append_accessibility_actions(&mut raw_input);
        let full_output = self.egui_ctx.run_ui(raw_input, |ui| {
            app_ui(ui);
        });
        let repaint_after = full_output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(Duration::MAX, |output| output.repaint_delay);
        let mut platform_output = full_output.platform_output;
        self.publish_accessibility_update(&mut platform_output);
        self.egui_state
            .handle_platform_output(self.window.as_ref(), platform_output);

        let tris = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        for (id, image_delta) in &full_output.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *id, image_delta);
        }

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: self.window.scale_factor() as f32,
        };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("viewr encoder"),
            });

        self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &tris,
            &screen_descriptor,
        );

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewr render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if let (Some(image), Some(viewport), Some(_)) = (
                &self.image,
                image_viewport.and_then(|viewport| {
                    viewport.intersect((self.config.width, self.config.height))
                }),
                placement,
            ) {
                rpass.set_scissor_rect(viewport.x, viewport.y, viewport.width, viewport.height);
                rpass.set_pipeline(&self.pipeline);
                rpass.set_bind_group(0, &image.bind_group, &[]);
                rpass.draw(0..6, 0..1);
            }
        }

        {
            let rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            let mut rpass = rpass.forget_lifetime();
            self.egui_renderer
                .render(&mut rpass, &tris, &screen_descriptor);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();

        for id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }

        let result = if suboptimal {
            FrameResult::NeedsReconfigure
        } else {
            FrameResult::Presented
        };
        FrameOutput {
            result,
            repaint_after: Some(repaint_after),
        }
    }
}

/// Rendering result plus egui's next requested root-viewport repaint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameOutput {
    /// Surface presentation outcome.
    pub result: FrameResult,
    /// Delay until the next egui repaint, or `None` when egui did not run.
    pub repaint_after: Option<Duration>,
}

impl FrameOutput {
    const fn without_egui(result: FrameResult) -> Self {
        Self {
            result,
            repaint_after: None,
        }
    }
}

/// The outcome of a call to [`Renderer::render`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameResult {
    /// The frame was drawn and presented.
    Presented,
    /// A transient condition (timeout or occlusion); skip this frame.
    Skipped,
    /// The surface is outdated, lost, or suboptimal; reconfigure before retrying.
    NeedsReconfigure,
}

/// The uniform buffer size (16 for `scale`/`offset`, 16 for `uv_matrix`, 16 for `crop_rect`).
const PLACEMENT_BYTES: u64 = 48;

/// Pack `Placement` into 48 bytes (vec4, vec4, vec4).
fn pack_placement(p: &view::Placement) -> [u8; 48] {
    let mut bytes = [0; 48];
    bytes[0..4].copy_from_slice(&p.scale[0].to_ne_bytes());
    bytes[4..8].copy_from_slice(&p.scale[1].to_ne_bytes());
    bytes[8..12].copy_from_slice(&p.offset[0].to_ne_bytes());
    bytes[12..16].copy_from_slice(&p.offset[1].to_ne_bytes());
    bytes[16..20].copy_from_slice(&p.uv_matrix[0].to_ne_bytes());
    bytes[20..24].copy_from_slice(&p.uv_matrix[1].to_ne_bytes());
    bytes[24..28].copy_from_slice(&p.uv_matrix[2].to_ne_bytes());
    bytes[28..32].copy_from_slice(&p.uv_matrix[3].to_ne_bytes());
    bytes[32..36].copy_from_slice(&p.crop_rect[0].to_ne_bytes());
    bytes[36..40].copy_from_slice(&p.crop_rect[1].to_ne_bytes());
    bytes[40..44].copy_from_slice(&p.crop_rect[2].to_ne_bytes());
    bytes[44..48].copy_from_slice(&p.crop_rect[3].to_ne_bytes());
    bytes
}

/// Build the textured-quad pipeline and its bind group layout.
fn build_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("viewr shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
    });

    let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("viewr bind layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });

    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("viewr pipeline layout"),
        bind_group_layouts: &[Some(&bind_layout)],
        immediate_size: 0,
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("viewr pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    (pipeline, bind_layout)
}

/// Convert a [`Palette`] background into a wgpu clear color.
// r, g, b, a are the standard, unambiguous color-channel names.
#[allow(clippy::many_single_char_names)]
fn palette_to_color(palette: Palette) -> wgpu::Color {
    let [r, g, b, a] = palette.background;
    wgpu::Color { r, g, b, a }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_GPU_BASE_PIXELS, mip_level_count, pack_placement, prepare_image_preview, preview_spec,
        texture_dimensions,
    };
    use crate::decode::{ColorProfileStatus, DecodedImage};
    use crate::view::Placement;

    #[test]
    fn packs_placement_in_field_order() {
        let p = Placement {
            scale: [0.5, 0.25],
            offset: [-0.1, 0.2],
            uv_matrix: [1.0, 0.0, 0.0, 1.0],
            crop_rect: [0.0, 0.0, 1.0, 1.0],
        };
        let bytes = pack_placement(&p);
        let unpack = |i: usize| f32::from_ne_bytes(bytes[i..i + 4].try_into().unwrap());
        assert!((unpack(0) - 0.5).abs() < f32::EPSILON);
        assert!((unpack(4) - 0.25).abs() < f32::EPSILON);
        assert!((unpack(8) - -0.1).abs() < f32::EPSILON);
        assert!((unpack(12) - 0.2).abs() < f32::EPSILON);
        assert!((unpack(16) - 1.0).abs() < f32::EPSILON);
        assert!((unpack(28) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn oversized_texture_dimensions_preserve_the_complete_aspect() {
        assert_eq!(
            texture_dimensions((20_000, 10_000), 8_192, MAX_GPU_BASE_PIXELS),
            (8_192, 4_096)
        );
        assert_eq!(
            texture_dimensions((10_000, 20_000), 8_192, MAX_GPU_BASE_PIXELS),
            (4_096, 8_192)
        );
        assert_eq!(
            texture_dimensions((4_000, 3_000), 8_192, MAX_GPU_BASE_PIXELS),
            (4_000, 3_000)
        );
        assert_eq!(
            texture_dimensions((1, 65_535), 8_192, MAX_GPU_BASE_PIXELS),
            (1, 8_192)
        );
        assert_eq!(
            texture_dimensions((12_000, 12_000), 16_384, MAX_GPU_BASE_PIXELS),
            (8_192, 8_192)
        );
        let bounded = texture_dimensions((16_000, 8_000), 16_384, MAX_GPU_BASE_PIXELS);
        assert!(bounded.0.abs_diff(bounded.1.saturating_mul(2)) <= 1);
        assert!(u64::from(bounded.0) * u64::from(bounded.1) <= MAX_GPU_BASE_PIXELS);
    }

    #[test]
    fn preview_area_filter_is_linear_light_and_alpha_correct() {
        let image = DecodedImage {
            rgba: vec![
                0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255,
            ],
            width: 2,
            height: 2,
            color_profile: ColorProfileStatus::AssumedSrgb,
        };
        let spec = preview_spec((2, 2), 2, 1).unwrap();
        let preview = prepare_image_preview(&image, spec, || false)
            .unwrap()
            .unwrap();
        assert_eq!(preview.rgba, [188, 188, 188, 255]);

        let alpha = DecodedImage {
            rgba: vec![255, 0, 0, 0, 0, 0, 255, 255],
            width: 2,
            height: 1,
            color_profile: ColorProfileStatus::AssumedSrgb,
        };
        let spec = preview_spec((2, 1), 2, 1).unwrap();
        let preview = prepare_image_preview(&alpha, spec, || false)
            .unwrap()
            .unwrap();
        assert_eq!(preview.rgba, [0, 0, 255, 128]);
    }

    #[test]
    fn preview_preparation_is_generation_cancellable_and_validates_source() {
        let image = DecodedImage {
            rgba: vec![0; 4 * 4 * 4],
            width: 4,
            height: 4,
            color_profile: ColorProfileStatus::AssumedSrgb,
        };
        let spec = preview_spec((4, 4), 4, 4).unwrap();
        assert!(
            prepare_image_preview(&image, spec, || true)
                .unwrap()
                .is_none()
        );

        let malformed = DecodedImage {
            rgba: vec![0; 3],
            width: 4,
            height: 4,
            color_profile: ColorProfileStatus::AssumedSrgb,
        };
        assert!(prepare_image_preview(&malformed, spec, || false).is_err());
    }

    #[test]
    fn mip_chain_reaches_one_pixel_on_the_longest_edge() {
        assert_eq!(mip_level_count((1, 1)), 1);
        assert_eq!(mip_level_count((2, 1)), 2);
        assert_eq!(mip_level_count((3, 2)), 2);
        assert_eq!(mip_level_count((4_000, 3_000)), 12);
        assert_eq!(mip_level_count((8_192, 4_096)), 14);
        assert_eq!(mip_level_count((0, 0)), 1);
    }
}
