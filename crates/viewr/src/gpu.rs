//! The wgpu rendering pipeline: clears the window to the theme background and,
//! when an image is loaded, draws it as a textured quad scaled to fit. Later
//! phases add the neighbor texture cache and zoom/pan on top of this.

use std::sync::Arc;
use std::time::Duration;

use winit::window::Window;

use crate::color::{OutputColorTransform, WorkingColorEncoding};
use crate::decode::DecodedImage;
use crate::display_output::DisplayOutputNormalizer;
use crate::error::Error;
pub(crate) use crate::gpu_image::{
    ImagePreview, MAX_GPU_BASE_PIXELS, PERFORMANCE_PROBE_GPU_BASE_PIXELS, PreviewSpec,
    prepare_image_preview,
};
use crate::gpu_image::{mip_level_count, preview_spec, select_image_upload};
use crate::gpu_policy::{
    PLACEMENT_BYTES, pack_placement, palette_to_color, select_srgb_surface_format,
    validate_patch_upload,
};
use crate::performance::GpuAdapterReport;
use crate::theme::{self, Mode};

/// Owns the GPU surface and everything needed to draw a frame.
pub struct Renderer {
    /// A reference-counted handle to the application window.
    pub window: Arc<winit::window::Window>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    clear: wgpu::Color,
    adapter_report: GpuAdapterReport,
    max_dim: u32,
    max_base_pixels: u64,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    mipmap_blitter: wgpu::util::TextureBlitter,
    pipeline: wgpu::RenderPipeline,
    output_color_transform: OutputColorTransform,
    display_output: DisplayOutputNormalizer,
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
    working_color: WorkingColorEncoding,
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
        display: winit::event_loop::OwnedDisplayHandle,
        mode: Mode,
        max_base_pixels: u64,
    ) -> Result<Self, Error> {
        let size = window.inner_size();

        // The display connection must reach the instance. Without it the GL
        // backend selects a surfaceless platform, then reports every window
        // surface as incompatible, so a session whose only working renderer is
        // software Mesa can never present. Honors WGPU_BACKEND and related env
        // vars, else picks sensible defaults.
        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(display)),
        );

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
        // Keep only stable, path-free identity from the adapter that created the
        // device. PCI identity and extended driver details never enter reports.
        let adapter_report = GpuAdapterReport::from_wgpu(&adapter.get_info());

        let width = size.width.clamp(1, max_dim);
        let height = size.height.clamp(1, max_dim);

        let caps = surface.get_capabilities(&adapter);
        let (format, output_color_transform) = select_srgb_surface_format(&caps.formats)?;

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
            adapter_report,
            max_dim,
            max_base_pixels: max_base_pixels.max(1),
            bind_layout,
            sampler,
            mipmap_blitter,
            pipeline,
            output_color_transform,
            display_output: DisplayOutputNormalizer::identity(),
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

    pub(crate) fn performance_adapter(&self) -> &GpuAdapterReport {
        &self.adapter_report
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

    /// Install the CPU display transform used at the next image or patch upload.
    pub(crate) fn set_display_output(&mut self, output: DisplayOutputNormalizer) {
        self.display_output = output;
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
        let upload = select_image_upload(
            image,
            prepared,
            self.required_preview(image),
            self.output_color_transform,
        )?;
        let display_pixels = self.display_output.apply(upload.rgba)?;
        let (width, height) = upload.size;
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
            display_pixels.as_ref(),
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
            working_color: image.working_color,
        });
        self.window.request_redraw();
        Ok(upload.full_resolution)
    }

    /// Upload one tightly packed RGBA8 patch into the current image texture.
    ///
    /// Returns `false` without writing when the displayed texture is a reduced
    /// preview or the patch does not exactly fit the full-resolution texture.
    /// Normal Spot Heal operations use this path so committing a small edit never
    /// reallocates or uploads the full image.
    #[must_use]
    pub fn update_image_patch(&self, patch: &crate::heal::ImagePatch) -> bool {
        let Some(image) = self.image.as_ref() else {
            return false;
        };
        if !self.output_color_transform.accepts(image.working_color) {
            return false;
        }
        let Some(upload) = validate_patch_upload(image.source_size, image.texture_size, patch)
        else {
            return false;
        };
        let Ok(display_pixels) = self.display_output.apply(&patch.rgba) else {
            return false;
        };

        let mut destination = image.texture.as_image_copy();
        destination.origin = upload.origin;
        self.queue.write_texture(
            destination,
            display_pixels.as_ref(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(upload.bytes_per_row),
                rows_per_image: Some(upload.extent.height),
            },
            upload.extent,
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
