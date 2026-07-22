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
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
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
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
}

impl Renderer {
    /// Create a renderer for `window`, clearing to the palette for `mode`.
    ///
    /// # Errors
    /// Returns an error if the GPU adapter, device, or surface cannot be created.
    pub async fn new(window: Arc<winit::window::Window>, mode: Mode) -> Result<Self, Error> {
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
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let (pipeline, bind_layout) = build_pipeline(&device, format);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("viewr sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
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
            bind_layout,
            sampler,
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
        self.image.as_ref().map(|img| img.size)
    }

    /// Clear the currently displayed image.
    pub fn clear_image(&mut self) {
        self.image = None;
    }

    /// Upload `image` as the currently displayed image, replacing any previous
    /// one. The image is clamped to the GPU's maximum texture size.
    pub fn set_image(&mut self, image: &DecodedImage) {
        let width = image.width.min(self.max_dim);
        let height = image.height.min(self.max_dim);

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewr image"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            &image.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.width * 4),
                rows_per_image: Some(image.height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

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
            bind_group,
            size: (width, height),
        });
        self.window.request_redraw();
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
    use super::pack_placement;
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
}
