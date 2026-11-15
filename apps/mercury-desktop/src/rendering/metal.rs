/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// Portions of this file are adapted from the Slint Servo example
// (https://github.com/slint-ui/slint, tag v1.17.1,
// examples/servo/src/webview/rendering_context/metal.rs):
//
//     Copyright © SixtyFPS GmbH <info@slint.dev>
//     SPDX-License-Identifier: MIT
//
// The MIT licence text for those portions is in LICENSES/MIT-slint-examples.txt.

//! macOS frame bridge. Servo renders with OpenGL into an IOSurface; that IOSurface is wrapped
//! as a Metal texture on Slint's own WGPU device and converted by one render pass into the
//! `Rgba8Unorm` texture Slint draws. Reading the frame back with `glReadPixels` is the
//! fallback, latched after the first failed import.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::pin::pin;
use std::task::{Context as TaskContext, Poll, Waker};
use std::time::{Duration, Instant};

use euclid::default::{Point2D, Size2D};
use gleam::gl::{self, Gl};
use glow::HasContext;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLDevice, MTLPixelFormat, MTLTexture, MTLTextureDescriptor, MTLTextureType, MTLTextureUsage,
};
use servo::{DeviceIntRect, RenderingContext, RgbaImage};
use slint::wgpu_29::wgpu::{
    self, BindGroupEntry, BindGroupLayoutEntry, BindingResource, BindingType, FilterMode,
    ShaderStages, TextureFormat, TextureUsages,
};
use slint::{Image, SharedPixelBuffer};
use surfman::cgl::surface::NativeSurface;

use super::surfman::{BrowserRenderingContext, RenderingBridgeError};

/// Set to `1` to read every frame back through the CPU instead of importing it through Metal.
const FORCE_READBACK_VARIABLE: &str = "MERCURY_FORCE_FRAME_READBACK";
/// Frame import times are summarised at debug level after this many frames.
const TIMING_SUMMARY_INTERVAL: usize = 240;

#[derive(Debug, thiserror::Error)]
pub enum MetalImportError {
    #[error("Slint's WGPU renderer is not using Metal")]
    WgpuNotMetal,
    #[error("Servo's surface has an invalid size: {width}x{height}")]
    InvalidSize { width: i32, height: i32 },
    #[error("Metal could not create a texture for Servo's IOSurface")]
    TextureCreation,
    #[error("WGPU rejected the frame conversion: {0}")]
    Wgpu(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FramePath {
    Metal,
    ReadBack,
}

impl fmt::Display for FramePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Metal => "IOSurface import through Metal",
            Self::ReadBack => "OpenGL read-back",
        })
    }
}

/// The macOS-only state of a [`BrowserRenderingContext`].
pub(super) struct MacFrameBridge {
    wgpu_queue: wgpu::Queue,
    /// `None` when every frame is read back.
    importer: Option<MetalFrameImporter>,
    import_failed: Cell<bool>,
    /// A precaution: an extra reference to the last imported IOSurface, held until the next
    /// import. The `MTLTexture` created from it probably keeps it alive for the flip pass, but
    /// that ownership is not documented. Released when the read-back latch engages.
    last_imported_surface: RefCell<Option<NativeSurface>>,
    timings: RefCell<ImportTimings>,
}

impl MacFrameBridge {
    /// `gleam_gl` must belong to the rendering context, which must be current.
    pub(super) fn new(
        gleam_gl: &dyn Gl,
        wgpu_device: &wgpu::Device,
        wgpu_queue: &wgpu::Queue,
    ) -> Self {
        let force_readback = std::env::var(FORCE_READBACK_VARIABLE).ok();
        let importer = if force_readback_requested(force_readback.as_deref()) {
            log::info!("{FORCE_READBACK_VARIABLE} is set: Servo frames will be read back");
            None
        } else {
            match MetalFrameImporter::new(wgpu_device) {
                Ok(importer) => Some(importer),
                Err(error) => {
                    log::warn!(
                        "Servo frames cannot be imported through Metal ({error}); \
                         reading them back through the CPU instead"
                    );
                    None
                }
            }
        };
        let path = if importer.is_some() {
            FramePath::Metal
        } else {
            FramePath::ReadBack
        };
        log::info!(
            "Servo frame path: {path}; GL_VERSION {:?}, GL_RENDERER {:?}",
            gleam_gl.get_string(gl::VERSION),
            gleam_gl.get_string(gl::RENDERER),
        );

        Self {
            wgpu_queue: wgpu_queue.clone(),
            importer,
            import_failed: Cell::new(false),
            last_imported_surface: RefCell::new(None),
            timings: RefCell::new(ImportTimings::default()),
        }
    }

    fn active_importer(&self) -> Option<&MetalFrameImporter> {
        self.importer.as_ref().filter(|_| !self.import_failed.get())
    }
}

impl Drop for MacFrameBridge {
    fn drop(&mut self) {
        if let Some(summary) = self.timings.get_mut().take_summary() {
            log::debug!("{summary}");
        }
    }
}

impl BrowserRenderingContext {
    /// Hands the completed Servo frame to Slint: imported through Metal, or read back once an
    /// import has failed.
    pub fn current_frame_as_image(
        &self,
        wgpu_device: &wgpu::Device,
    ) -> Result<Image, RenderingBridgeError> {
        let started = log::log_enabled!(log::Level::Debug).then(Instant::now);
        let bridge = &self.frame_bridge;
        let (path, result) = match bridge.active_importer() {
            Some(importer) => match self.import_frame_through_metal(importer, wgpu_device) {
                Err(
                    error @ (RenderingBridgeError::MetalImport(_)
                    | RenderingBridgeError::SlintTexture(_)),
                ) => {
                    log::warn!(
                        "importing a Servo frame through Metal failed ({error}); \
                         reading frames back through the CPU from now on"
                    );
                    bridge.import_failed.set(true);
                    bridge.last_imported_surface.replace(None);
                    (FramePath::ReadBack, self.read_back_frame())
                }
                result => (FramePath::Metal, result),
            },
            None => (FramePath::ReadBack, self.read_back_frame()),
        };

        if let Some(started) = started
            && result.is_ok()
            && let Some(summary) = bridge.timings.borrow_mut().record(path, started.elapsed())
        {
            log::debug!("{summary}");
        }
        result
    }

    fn import_frame_through_metal(
        &self,
        importer: &MetalFrameImporter,
        wgpu_device: &wgpu::Device,
    ) -> Result<Image, RenderingBridgeError> {
        let device = self.surfman.device.borrow();
        let mut context = self.surfman.context.borrow_mut();

        // OpenGL and Metal share the IOSurface but not a command queue. Waiting here
        // guarantees that Metal never samples a partially drawn frame.
        device.make_context_current(&context)?;
        // SAFETY: `glow_gl` was loaded from this context, which is current on this thread.
        unsafe { self.surfman.glow_gl.finish() };

        let surface = device
            .unbind_surface_from_context(&mut context)?
            .ok_or(RenderingBridgeError::NoSurface)?;
        // No early return until the surface is bound again: dropping a surfman `Surface` panics.
        let imported = importer.import(
            wgpu_device,
            &self.frame_bridge.wgpu_queue,
            &device,
            &surface,
        );
        device
            .bind_surface_to_context(&mut context, surface)
            .map_err(|(error, mut surface)| {
                let _ = device.destroy_surface(&mut context, &mut surface);
                error
            })?;

        let (texture, io_surface) = imported?;
        self.frame_bridge
            .last_imported_surface
            .replace(Some(io_surface));
        Image::try_from(texture)
            .map_err(|error| RenderingBridgeError::SlintTexture(error.to_string()))
    }

    fn read_back_frame(&self) -> Result<Image, RenderingBridgeError> {
        let size = self.size2d().to_i32();
        let pixels = self
            .read_to_image(DeviceIntRect::from_origin_and_size(
                Point2D::origin().cast_unit(),
                size.cast_unit(),
            ))
            .ok_or(RenderingBridgeError::MissingFrame)?;
        let (width, height) = pixels.dimensions();
        Ok(Image::from_rgba8(SharedPixelBuffer::clone_from_slice(
            pixels.as_raw(),
            width,
            height,
        )))
    }

    /// Adapted from Servo's `Framebuffer::read_framebuffer_to_image` (servo-paint-api 0.8.0,
    /// `components/shared/paint/rendering_context.rs` in https://github.com/servo/servo,
    /// MPL-2.0).
    pub(super) fn read_framebuffer_to_image(
        &self,
        source_rectangle: DeviceIntRect,
    ) -> Option<RgbaImage> {
        self.surfman.make_current().ok()?;
        let framebuffer_id = self
            .surfman
            .framebuffer()
            .map_or(0, |framebuffer| framebuffer.0.get());
        let gleam_gl = &self.surfman.gleam_gl;
        gleam_gl.bind_framebuffer(gl::FRAMEBUFFER, framebuffer_id);
        gleam_gl.bind_vertex_array(0);

        let mut pixels = gleam_gl.read_pixels(
            source_rectangle.min.x,
            source_rectangle.min.y,
            source_rectangle.width(),
            source_rectangle.height(),
            gl::RGBA,
            gl::UNSIGNED_BYTE,
        );
        let gl_error = gleam_gl.get_error();
        if gl_error != gl::NO_ERROR {
            log::warn!("GL error code 0x{gl_error:x} set after read_pixels");
        }

        // OpenGL's first row is the bottom one.
        let source_rectangle = source_rectangle.to_usize();
        flip_rows(&mut pixels, source_rectangle.width() * 4);
        RgbaImage::from_raw(
            source_rectangle.width() as u32,
            source_rectangle.height() as u32,
            pixels,
        )
    }
}

/// The resources of the flip pass, created once per rendering context.
struct MetalFrameImporter {
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
}

impl MetalFrameImporter {
    fn new(wgpu_device: &wgpu::Device) -> Result<Self, MetalImportError> {
        // SAFETY: the guard is only tested for presence and dropped at once; nothing is
        // created or destroyed through it.
        if unsafe { wgpu_device.as_hal::<wgpu::wgc::api::Metal>() }.is_none() {
            return Err(MetalImportError::WgpuNotMetal);
        }
        let (bind_group_layout, pipeline, sampler) = create_flip_pipeline_resources(wgpu_device);
        Ok(Self {
            bind_group_layout,
            pipeline,
            sampler,
        })
    }

    /// `surface` must be unbound from its context, and OpenGL must have finished drawing into
    /// it. Returns the converted frame together with the IOSurface it was read from.
    fn import(
        &self,
        wgpu_device: &wgpu::Device,
        wgpu_queue: &wgpu::Queue,
        surfman_device: &surfman::Device,
        surface: &surfman::Surface,
    ) -> Result<(wgpu::Texture, NativeSurface), MetalImportError> {
        // The size of the surface being imported, not a cached one: they differ after a
        // failed resize.
        let size = frame_extent(surfman_device.surface_info(surface).size)?;
        let io_surface = surfman_device.native_surface(surface);

        // WGPU's default error handler panics; any WGPU error here must select the read-back
        // path instead.
        let error_scopes = ErrorScopes::push(wgpu_device);
        let converted = objc2_metal_texture_from_iosurface(size, wgpu_device, &io_surface).map(
            |metal_texture| {
                let source = wgpu_hal_texture(size, wgpu_device, metal_texture);
                self.create_flipped_texture_render(size, wgpu_device, wgpu_queue, &source)
            },
        );
        let wgpu_error = error_scopes.pop();

        match (converted, wgpu_error) {
            (Err(error), _) => Err(error),
            (Ok(_), Some(error)) => Err(MetalImportError::Wgpu(error.to_string())),
            (Ok(texture), None) => Ok((texture, io_surface)),
        }
    }

    /// Draws `source_texture` upside down into a new `Rgba8Unorm` texture.
    fn create_flipped_texture_render(
        &self,
        size: wgpu::Extent3d,
        wgpu_device: &wgpu::Device,
        wgpu_queue: &wgpu::Queue,
        source_texture: &wgpu::Texture,
    ) -> wgpu::Texture {
        let descriptor = create_wgpu_texture_descriptor(
            size,
            "Flipped Metal IOSurface Texture",
            TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT,
            TextureFormat::Rgba8Unorm,
        );

        let flipped_texture = wgpu_device.create_texture(&descriptor);

        let source_view = source_texture.create_view(&Default::default());

        let bind_group = wgpu_device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Metal Texture Flip Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&source_view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        let target_view = &flipped_texture.create_view(&Default::default());

        let mut encoder = wgpu_device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Metal Texture Flip Command Encoder"),
        });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Metal Texture Flip Render Pass"),
                timestamp_writes: None,
                occlusion_query_set: None,
                depth_stencil_attachment: None,
                multiview_mask: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
            });

            render_pass.set_pipeline(&self.pipeline);
            render_pass.set_bind_group(0, &bind_group, &[]);
            render_pass.draw(0..3, 0..1); // Draw a fullscreen triangle
        }

        wgpu_queue.submit(std::iter::once(encoder.finish()));

        flipped_texture
    }
}

/// Wraps the IOSurface as a `BGRA8Unorm` Metal texture on WGPU's own Metal device.
fn objc2_metal_texture_from_iosurface(
    size: wgpu::Extent3d,
    wgpu_device: &wgpu::Device,
    io_surface: &NativeSurface,
) -> Result<Retained<ProtocolObject<dyn MTLTexture>>, MetalImportError> {
    // SAFETY: the guard only lends out WGPU's `MTLDevice` to create a new texture; nothing
    // owned by WGPU is destroyed or modified, and the guard is dropped on return.
    let metal_device = unsafe { wgpu_device.as_hal::<wgpu::wgc::api::Metal>() }
        .ok_or(MetalImportError::WgpuNotMetal)?;
    let metal_device_raw: &Retained<ProtocolObject<dyn MTLDevice>> = metal_device.raw_device();

    let descriptor = MTLTextureDescriptor::new();
    descriptor.setTextureType(MTLTextureType::Type2D);
    // surfman's IOSurfaces are always 32BGRA. Never use an sRGB format: bytes pass unchanged.
    descriptor.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
    descriptor.setUsage(MTLTextureUsage::ShaderRead);
    // SAFETY: plain property setters. The values describe a 2D, single-sample, single-level
    // texture of the IOSurface's own non-zero size; Metal validates them on creation.
    unsafe {
        descriptor.setWidth(size.width as usize);
        descriptor.setHeight(size.height as usize);
        descriptor.setDepth(1);
        descriptor.setMipmapLevelCount(1);
        descriptor.setSampleCount(1);
    }

    metal_device_raw
        .newTextureWithDescriptor_iosurface_plane(&descriptor, &io_surface.0, 0)
        .ok_or(MetalImportError::TextureCreation)
}

/// Hands the IOSurface-backed Metal texture to WGPU, which then owns it.
fn wgpu_hal_texture(
    size: wgpu::Extent3d,
    wgpu_device: &wgpu::Device,
    metal_texture: Retained<ProtocolObject<dyn MTLTexture>>,
) -> wgpu::Texture {
    // SAFETY: `metal_texture` was created on this WGPU device's own `MTLDevice`
    // (`objc2_metal_texture_from_iosurface`) as a 2D `BGRA8Unorm` texture of `size` with one
    // layer, one mip level and one sample, exactly as declared to `texture_from_raw` and in the
    // descriptor below; it is only sampled, which matches its `ShaderRead` usage and
    // `TEXTURE_BINDING`. OpenGL finished drawing into the IOSurface (`glFinish`) before it was
    // unbound, so the contents are initialised.
    unsafe {
        let hal_texture = wgpu::hal::metal::Device::texture_from_raw(
            metal_texture,
            TextureFormat::Bgra8Unorm,
            MTLTextureType::Type2D,
            1,
            1,
            wgpu::hal::CopyExtent {
                width: size.width,
                height: size.height,
                depth: 1,
            },
        );

        let descriptor = create_wgpu_texture_descriptor(
            size,
            "Metal IOSurface Texture",
            TextureUsages::TEXTURE_BINDING,
            TextureFormat::Bgra8Unorm,
        );

        wgpu_device.create_texture_from_hal::<wgpu::wgc::api::Metal>(hal_texture, &descriptor)
    }
}

fn create_flip_pipeline_resources(
    wgpu_device: &wgpu::Device,
) -> (wgpu::BindGroupLayout, wgpu::RenderPipeline, wgpu::Sampler) {
    // Vertex and fragment in one module. The vertex shader emits a fullscreen triangle;
    // the fragment shader samples the source texture with a vertical flip.
    const FLIP_SHADER: &str = r#"
        @vertex
        fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
            let uv = vec2<f32>(f32(i >> 1u), f32(i & 1u)) * 2.0;
            return vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
        }

        @group(0) @binding(0) var src: texture_2d<f32>;
        @group(0) @binding(1) var smp: sampler;

        @fragment
        fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
            let sz = textureDimensions(src);
            let uv = vec2<f32>(pos.x / f32(sz.x), 1.0 - pos.y / f32(sz.y));
            return textureSample(src, smp, uv);
        }
    "#;

    let shader = wgpu_device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Metal Flip Shader"),
        source: wgpu::ShaderSource::Wgsl(FLIP_SHADER.into()),
    });

    let bind_group_layout =
        wgpu_device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Metal Texture Flip Bind Group Layout"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

    let layout = wgpu_device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Metal Texture Flip Pipeline Layout"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    let pipeline = wgpu_device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Metal Texture Flip Render Pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });

    let sampler = wgpu_device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("Metal Texture Sampler"),
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..Default::default()
    });

    (bind_group_layout, pipeline, sampler)
}

fn create_wgpu_texture_descriptor(
    size: wgpu::Extent3d,
    label: &str,
    usage: TextureUsages,
    format: TextureFormat,
) -> wgpu::TextureDescriptor<'_> {
    wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    }
}

fn force_readback_requested(value: Option<&str>) -> bool {
    value == Some("1")
}

fn frame_extent(size: Size2D<i32>) -> Result<wgpu::Extent3d, MetalImportError> {
    match (u32::try_from(size.width), u32::try_from(size.height)) {
        (Ok(width @ 1..), Ok(height @ 1..)) => Ok(wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        }),
        _ => Err(MetalImportError::InvalidSize {
            width: size.width,
            height: size.height,
        }),
    }
}

/// Captures every kind of WGPU error raised while it exists, so that none reaches WGPU's
/// default handler, which panics. Fields are declared innermost first: struct fields drop in
/// declaration order, so even an un-popped `ErrorScopes` pops its scopes in strict LIFO order.
struct ErrorScopes {
    validation: wgpu::ErrorScopeGuard,
    internal: wgpu::ErrorScopeGuard,
    out_of_memory: wgpu::ErrorScopeGuard,
}

impl ErrorScopes {
    fn push(wgpu_device: &wgpu::Device) -> Self {
        let out_of_memory = wgpu_device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let internal = wgpu_device.push_error_scope(wgpu::ErrorFilter::Internal);
        let validation = wgpu_device.push_error_scope(wgpu::ErrorFilter::Validation);
        Self {
            validation,
            internal,
            out_of_memory,
        }
    }

    /// Pops all three scopes and returns the first error caught, if any.
    fn pop(self) -> Option<wgpu::Error> {
        let validation = ready_value(self.validation.pop()).flatten();
        let internal = ready_value(self.internal.pop()).flatten();
        let out_of_memory = ready_value(self.out_of_memory.pop()).flatten();
        validation.or(internal).or(out_of_memory)
    }
}

/// Polls `future` once. WGPU's native backend resolves error-scope futures immediately.
fn ready_value<F: Future>(future: F) -> Option<F::Output> {
    let mut future = pin!(future);
    match future
        .as_mut()
        .poll(&mut TaskContext::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => Some(value),
        Poll::Pending => None,
    }
}

/// Reverses the order of the rows of `row_length` bytes in `pixels`, in place.
fn flip_rows(pixels: &mut [u8], row_length: usize) {
    if row_length == 0 {
        return;
    }
    let height = pixels.len() / row_length;
    for y in 0..height / 2 {
        let (top, bottom) = pixels.split_at_mut((height - y - 1) * row_length);
        top[y * row_length..(y + 1) * row_length].swap_with_slice(&mut bottom[..row_length]);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct TimingSummary {
    path: FramePath,
    count: usize,
    median: Duration,
    p95: Duration,
}

impl TimingSummary {
    fn of(path: FramePath, samples: &[Duration]) -> Option<Self> {
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        Some(Self {
            path,
            count: sorted.len(),
            median: nearest_rank(&sorted, 50)?,
            p95: nearest_rank(&sorted, 95)?,
        })
    }
}

impl fmt::Display for TimingSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Servo frame import ({}): {} frames, median {:.3} ms, 95th percentile {:.3} ms",
            self.path,
            self.count,
            self.median.as_secs_f64() * 1000.0,
            self.p95.as_secs_f64() * 1000.0,
        )
    }
}

/// The nearest-rank percentile of samples sorted in ascending order.
fn nearest_rank(sorted: &[Duration], percentile: usize) -> Option<Duration> {
    let rank = (sorted.len() * percentile).div_ceil(100).max(1);
    sorted.get(rank - 1).copied()
}

#[derive(Default)]
struct ImportTimings {
    path: Option<FramePath>,
    samples: Vec<Duration>,
}

impl ImportTimings {
    /// Returns a summary when the path changed or a full interval has been collected.
    fn record(&mut self, path: FramePath, sample: Duration) -> Option<TimingSummary> {
        let previous_path = if self.path == Some(path) {
            None
        } else {
            self.take_summary()
        };
        self.path = Some(path);
        self.samples.push(sample);
        if previous_path.is_some() {
            return previous_path;
        }
        if self.samples.len() >= TIMING_SUMMARY_INTERVAL {
            return self.take_summary();
        }
        None
    }

    fn take_summary(&mut self) -> Option<TimingSummary> {
        let summary = TimingSummary::of(self.path?, &self.samples);
        self.samples.clear();
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn milliseconds(values: impl IntoIterator<Item = u64>) -> Vec<Duration> {
        values.into_iter().map(Duration::from_millis).collect()
    }

    #[test]
    fn only_the_value_1_forces_the_read_back() {
        assert!(force_readback_requested(Some("1")));
        assert!(!force_readback_requested(None));
        assert!(!force_readback_requested(Some("")));
        assert!(!force_readback_requested(Some("0")));
    }

    #[test]
    fn a_surface_size_becomes_a_single_layer_extent() {
        let extent = frame_extent(Size2D::new(901, 701)).expect("the size is valid");
        assert_eq!(
            extent,
            wgpu::Extent3d {
                width: 901,
                height: 701,
                depth_or_array_layers: 1,
            }
        );
    }

    #[test]
    fn empty_or_negative_surface_sizes_are_rejected() {
        for (width, height) in [(0, 10), (10, 0), (-1, 10), (10, -5)] {
            assert!(matches!(
                frame_extent(Size2D::new(width, height)),
                Err(MetalImportError::InvalidSize { .. })
            ));
        }
    }

    #[test]
    fn a_ready_future_yields_its_value_and_a_pending_one_nothing() {
        assert_eq!(ready_value(std::future::ready(7)), Some(7));
        assert_eq!(ready_value(std::future::pending::<u8>()), None);
    }

    /// A Metal device of its own, or `None` on a machine without one.
    fn test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::METAL;
        let instance = wgpu::Instance::new(descriptor);
        let adapter = ready_value(instance.request_adapter(&Default::default()))?.ok()?;
        ready_value(adapter.request_device(&Default::default()))?.ok()
    }

    fn texture(
        wgpu_device: &wgpu::Device,
        usage: TextureUsages,
        size: wgpu::Extent3d,
    ) -> wgpu::Texture {
        wgpu_device.create_texture(&create_wgpu_texture_descriptor(
            size,
            "test",
            usage,
            TextureFormat::Rgba8Unorm,
        ))
    }

    const SMALL: wgpu::Extent3d = wgpu::Extent3d {
        width: 4,
        height: 4,
        depth_or_array_layers: 1,
    };

    #[test]
    fn error_scopes_catch_an_error_instead_of_panicking() {
        let Some((wgpu_device, wgpu_queue)) = test_device() else {
            eprintln!("no Metal device; skipped");
            return;
        };
        let scopes = ErrorScopes::push(&wgpu_device);
        // Rendering into a texture without RENDER_ATTACHMENT is a validation error.
        let target = texture(&wgpu_device, TextureUsages::TEXTURE_BINDING, SMALL);
        let view = target.create_view(&Default::default());
        let mut encoder = wgpu_device.create_command_encoder(&Default::default());
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            depth_stencil_attachment: None,
            multiview_mask: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
        }));
        wgpu_queue.submit(std::iter::once(encoder.finish()));
        assert!(matches!(scopes.pop(), Some(wgpu::Error::Validation { .. })));
    }

    #[test]
    fn error_scopes_report_nothing_for_valid_work() {
        let Some((wgpu_device, _queue)) = test_device() else {
            eprintln!("no Metal device; skipped");
            return;
        };
        let scopes = ErrorScopes::push(&wgpu_device);
        let _texture = texture(
            &wgpu_device,
            TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT,
            SMALL,
        );
        assert!(scopes.pop().is_none());
    }

    #[test]
    fn unpopped_error_scopes_unwind_in_order() {
        let Some((wgpu_device, _queue)) = test_device() else {
            eprintln!("no Metal device; skipped");
            return;
        };
        // Dropping the guards in the wrong order would panic.
        drop(ErrorScopes::push(&wgpu_device));
        // The device has no scopes left: a new set pushes and pops cleanly.
        assert!(ErrorScopes::push(&wgpu_device).pop().is_none());
    }

    #[test]
    fn rows_are_flipped_in_place() {
        let mut odd = vec![1, 1, 2, 2, 3, 3];
        flip_rows(&mut odd, 2);
        assert_eq!(odd, vec![3, 3, 2, 2, 1, 1]);

        let mut even = vec![1, 2, 3, 4, 5, 6, 7, 8];
        flip_rows(&mut even, 4);
        assert_eq!(even, vec![5, 6, 7, 8, 1, 2, 3, 4]);

        let mut single = vec![1, 2, 3];
        flip_rows(&mut single, 3);
        assert_eq!(single, vec![1, 2, 3]);

        let mut empty: Vec<u8> = Vec::new();
        flip_rows(&mut empty, 0);
        assert!(empty.is_empty());
    }

    #[test]
    fn nearest_rank_percentiles() {
        let ten = milliseconds(1..=10);
        assert_eq!(nearest_rank(&ten, 50), Some(Duration::from_millis(5)));
        assert_eq!(nearest_rank(&ten, 95), Some(Duration::from_millis(10)));

        let hundred = milliseconds(1..=100);
        assert_eq!(nearest_rank(&hundred, 50), Some(Duration::from_millis(50)));
        assert_eq!(nearest_rank(&hundred, 95), Some(Duration::from_millis(95)));

        let one = milliseconds([4]);
        assert_eq!(nearest_rank(&one, 50), Some(Duration::from_millis(4)));
        assert_eq!(nearest_rank(&one, 95), Some(Duration::from_millis(4)));

        assert_eq!(nearest_rank(&[], 50), None);
    }

    #[test]
    fn a_summary_sorts_its_samples() {
        let samples = milliseconds([9, 1, 5, 3, 7]);
        assert_eq!(
            TimingSummary::of(FramePath::Metal, &samples),
            Some(TimingSummary {
                path: FramePath::Metal,
                count: 5,
                median: Duration::from_millis(5),
                p95: Duration::from_millis(9),
            })
        );
        assert_eq!(TimingSummary::of(FramePath::Metal, &[]), None);
    }

    #[test]
    fn timings_are_summarised_once_per_interval() {
        let mut timings = ImportTimings::default();
        for _ in 1..TIMING_SUMMARY_INTERVAL {
            assert_eq!(
                timings.record(FramePath::Metal, Duration::from_millis(2)),
                None
            );
        }
        let summary = timings
            .record(FramePath::Metal, Duration::from_millis(2))
            .expect("a full interval is summarised");
        assert_eq!(summary.count, TIMING_SUMMARY_INTERVAL);
        assert_eq!(summary.median, Duration::from_millis(2));
        assert_eq!(timings.take_summary(), None);
    }

    #[test]
    fn a_path_change_summarises_the_previous_path() {
        let mut timings = ImportTimings::default();
        assert_eq!(
            timings.record(FramePath::Metal, Duration::from_millis(1)),
            None
        );
        assert_eq!(
            timings.record(FramePath::Metal, Duration::from_millis(3)),
            None
        );
        let summary = timings
            .record(FramePath::ReadBack, Duration::from_millis(8))
            .expect("the Metal samples are summarised");
        assert_eq!(summary.path, FramePath::Metal);
        assert_eq!(summary.count, 2);

        let remainder = timings.take_summary().expect("one read-back sample");
        assert_eq!(remainder.path, FramePath::ReadBack);
        assert_eq!(remainder.count, 1);
        assert_eq!(remainder.median, Duration::from_millis(8));
    }
}
