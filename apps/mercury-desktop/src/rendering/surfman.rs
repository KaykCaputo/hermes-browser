/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// This file follows the Slint Servo example step for step
// (https://github.com/slint-ui/slint, tag v1.17.1,
// examples/servo/src/webview/rendering_context/{surfman_context,gpu_rendering_context}.rs;
// Copyright © SixtyFPS GmbH <info@slint.dev>, SPDX-License-Identifier: MIT; licence text in
// LICENSES/MIT-slint-examples.txt). `SurfmanContext` follows Servo's `SurfmanRenderingContext`
// (servo-paint-api, components/shared/paint/rendering_context.rs, MPL-2.0).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

#[cfg(target_os = "linux")]
use ash::vk;
use dpi::PhysicalSize;
use euclid::default::Size2D;
use gleam::gl::{self, Gl};
use servo::{DeviceIntRect, RenderingContext, RgbaImage};
#[cfg(target_os = "linux")]
use slint::Image;
use slint::wgpu_29::wgpu;
use surfman::chains::{PreserveBuffer, SwapChain};
use surfman::{
    Adapter, Connection, Context, ContextAttributeFlags, ContextAttributes, Device, GLApi, Surface,
    SurfaceAccess, SurfaceInfo, SurfaceTexture, SurfaceType,
};

use talaria_servo::ViewportSize;

#[derive(Debug, thiserror::Error)]
pub enum RenderingBridgeError {
    #[error("Surfman failed to create the offscreen rendering context: {0:?}")]
    Surfman(surfman::Error),
    #[cfg(target_os = "linux")]
    #[error("Slint's WGPU renderer is not using Vulkan")]
    WgpuNotVulkan,
    #[cfg(target_os = "linux")]
    #[error("the offscreen surface has no framebuffer")]
    NoFramebuffer,
    #[error("the offscreen surface disappeared while importing a frame")]
    NoSurface,
    #[cfg(target_os = "linux")]
    #[error("no Vulkan memory type can back the shared frame")]
    NoCompatibleMemoryType,
    #[cfg(target_os = "linux")]
    #[error("Vulkan texture import failed: {0}")]
    Vulkan(#[from] vk::Result),
    #[cfg(target_os = "linux")]
    #[error("OpenGL texture import failed: {0}")]
    OpenGl(String),
    #[error("Slint rejected the imported WGPU texture: {0}")]
    SlintTexture(String),
    #[cfg(target_os = "macos")]
    #[error("Metal frame import failed: {0}")]
    MetalImport(#[from] super::metal::MetalImportError),
    #[cfg(target_os = "macos")]
    #[error("Servo did not produce an image for the current frame")]
    MissingFrame,
}

impl From<surfman::Error> for RenderingBridgeError {
    fn from(error: surfman::Error) -> Self {
        Self::Surfman(error)
    }
}

pub(super) struct SurfmanContext {
    pub(super) gleam_gl: Rc<dyn Gl>,
    pub(super) glow_gl: Arc<glow::Context>,
    pub(super) device: RefCell<Device>,
    pub(super) context: RefCell<Context>,
}

impl SurfmanContext {
    fn new(connection: &Connection, adapter: &Adapter) -> Result<Self, surfman::Error> {
        let device = connection.create_device(adapter)?;
        let flags = ContextAttributeFlags::ALPHA
            | ContextAttributeFlags::DEPTH
            | ContextAttributeFlags::STENCIL;
        let gl_api = connection.gl_api();
        let version = match gl_api {
            GLApi::GLES => surfman::GLVersion { major: 3, minor: 0 },
            GLApi::GL => surfman::GLVersion { major: 4, minor: 5 },
        };
        let descriptor = device.create_context_descriptor(&ContextAttributes { flags, version })?;
        let context = device.create_context(&descriptor, None)?;

        let gleam_gl: Rc<dyn Gl> = match gl_api {
            GLApi::GL => unsafe {
                gl::GlFns::load_with(|name| device.get_proc_address(&context, name))
            },
            GLApi::GLES => unsafe {
                gl::GlesFns::load_with(|name| device.get_proc_address(&context, name))
            },
        };
        let glow_gl = unsafe {
            glow::Context::from_loader_function(|name| device.get_proc_address(&context, name))
        };

        Ok(Self {
            gleam_gl,
            glow_gl: Arc::new(glow_gl),
            device: RefCell::new(device),
            context: RefCell::new(context),
        })
    }

    pub(super) fn make_current(&self) -> Result<(), surfman::Error> {
        self.device
            .borrow()
            .make_context_current(&self.context.borrow())
    }

    pub(super) fn framebuffer(&self) -> Option<glow::NativeFramebuffer> {
        self.device
            .borrow()
            .context_surface_info(&self.context.borrow())
            .unwrap_or(None)
            .and_then(|info| info.framebuffer_object)
    }
}

impl Drop for SurfmanContext {
    fn drop(&mut self) {
        let _ = self
            .device
            .borrow_mut()
            .destroy_context(&mut self.context.borrow_mut());
    }
}

/// A dedicated Servo surface that never shares OpenGL state or a framebuffer with Slint.
pub struct BrowserRenderingContext {
    pub(super) size: Cell<ViewportSize>,
    swap_chain: SwapChain<Device>,
    pub(super) surfman: SurfmanContext,
    resize_failure: ReportOnce,
    present_failure: ReportOnce,
    #[cfg(target_os = "macos")]
    pub(super) frame_bridge: super::metal::MacFrameBridge,
}

impl BrowserRenderingContext {
    pub fn new(
        size: ViewportSize,
        wgpu_device: &wgpu::Device,
        wgpu_queue: &wgpu::Queue,
    ) -> Result<Self, RenderingBridgeError> {
        #[cfg(target_os = "linux")]
        let _ = wgpu_queue;
        #[cfg(target_os = "linux")]
        if unsafe { wgpu_device.as_hal::<wgpu::wgc::api::Vulkan>() }.is_none() {
            return Err(RenderingBridgeError::WgpuNotVulkan);
        }

        let connection = Connection::new()?;
        let adapter = connection.create_adapter()?;
        let surfman = SurfmanContext::new(&connection, &adapter)?;
        let surface = surfman.device.borrow_mut().create_surface(
            &surfman.context.borrow(),
            SurfaceAccess::GPUOnly,
            SurfaceType::Generic {
                size: Size2D::new(size.width() as i32, size.height() as i32),
            },
        )?;
        surfman
            .device
            .borrow()
            .bind_surface_to_context(&mut surfman.context.borrow_mut(), surface)
            .map_err(|(error, mut surface)| {
                let _ = surfman
                    .device
                    .borrow()
                    .destroy_surface(&mut surfman.context.borrow_mut(), &mut surface);
                error
            })?;
        surfman.make_current()?;
        let swap_chain = {
            let device = surfman.device.borrow();
            let mut context = surfman.context.borrow_mut();
            SwapChain::create_attached(&*device, &mut *context, SurfaceAccess::GPUOnly)?
        };
        #[cfg(target_os = "macos")]
        let frame_bridge =
            super::metal::MacFrameBridge::new(&*surfman.gleam_gl, wgpu_device, wgpu_queue);

        Ok(Self {
            size: Cell::new(size),
            swap_chain,
            surfman,
            resize_failure: ReportOnce::default(),
            present_failure: ReportOnce::default(),
            #[cfg(target_os = "macos")]
            frame_bridge,
        })
    }

    /// Imports the completed Servo framebuffer into the WGPU device owned by Slint.
    #[cfg(target_os = "linux")]
    pub fn current_frame_as_image(
        &self,
        wgpu_device: &wgpu::Device,
    ) -> Result<Image, RenderingBridgeError> {
        let texture = self.import_frame_into_wgpu(wgpu_device)?;
        Image::try_from(texture)
            .map_err(|error| RenderingBridgeError::SlintTexture(error.to_string()))
    }
}

impl Drop for BrowserRenderingContext {
    fn drop(&mut self) {
        let _ = self.swap_chain.destroy(
            &self.surfman.device.borrow(),
            &mut self.surfman.context.borrow_mut(),
        );
    }
}

impl RenderingContext for BrowserRenderingContext {
    fn prepare_for_rendering(&self) {
        let framebuffer = self
            .surfman
            .framebuffer()
            .map_or(0, |framebuffer| framebuffer.0.get());
        self.surfman
            .gleam_gl
            .bind_framebuffer(gl::FRAMEBUFFER, framebuffer);
    }

    #[cfg(target_os = "linux")]
    fn read_to_image(&self, _source_rectangle: DeviceIntRect) -> Option<RgbaImage> {
        None
    }

    #[cfg(target_os = "macos")]
    fn read_to_image(&self, source_rectangle: DeviceIntRect) -> Option<RgbaImage> {
        self.read_framebuffer_to_image(source_rectangle)
    }

    fn size(&self) -> PhysicalSize<u32> {
        let size = self.size.get();
        PhysicalSize::new(size.width(), size.height())
    }

    fn resize(&self, size: PhysicalSize<u32>) {
        let Some(size) = ViewportSize::new(size.width, size.height) else {
            return;
        };
        if size == self.size.get() {
            return;
        }
        if let Err(error) = self.swap_chain.resize(
            &self.surfman.device.borrow(),
            &mut self.surfman.context.borrow_mut(),
            Size2D::new(size.width() as i32, size.height() as i32),
        ) {
            // Keep the old size: it still describes the surface Servo draws into.
            if self.resize_failure.first() {
                log::warn!(
                    "failed to resize Servo's offscreen surface to {}x{}: {error:?}",
                    size.width(),
                    size.height()
                );
            }
            return;
        }
        self.size.set(size);
    }

    fn present(&self) {
        if let Err(error) = self.swap_chain.swap_buffers(
            &self.surfman.device.borrow(),
            &mut self.surfman.context.borrow_mut(),
            PreserveBuffer::No,
        ) && self.present_failure.first()
        {
            log::warn!("failed to present Servo's offscreen surface: {error:?}");
        }
    }

    fn make_current(&self) -> Result<(), surfman::Error> {
        self.surfman.make_current()
    }

    fn gleam_gl_api(&self) -> Rc<dyn Gl> {
        Rc::clone(&self.surfman.gleam_gl)
    }

    fn glow_gl_api(&self) -> Arc<glow::Context> {
        Arc::clone(&self.surfman.glow_gl)
    }

    fn create_texture(&self, surface: Surface) -> Option<(SurfaceTexture, u32, Size2D<i32>)> {
        let device = self.surfman.device.borrow();
        let mut context = self.surfman.context.borrow_mut();
        let SurfaceInfo { size, .. } = device.surface_info(&surface);
        let surface_texture = device.create_surface_texture(&mut context, surface).ok()?;
        let texture = device
            .surface_texture_object(&surface_texture)
            .map(|texture| texture.0.get())
            .unwrap_or(0);
        Some((surface_texture, texture, size))
    }

    fn destroy_texture(&self, surface_texture: SurfaceTexture) -> Option<Surface> {
        self.surfman
            .device
            .borrow()
            .destroy_surface_texture(&mut self.surfman.context.borrow_mut(), surface_texture)
            .map_err(|(error, _)| error)
            .ok()
    }

    fn connection(&self) -> Option<Connection> {
        Some(self.surfman.device.borrow().connection())
    }
}

/// Lets a failure that recurs every frame be logged once.
#[derive(Default)]
struct ReportOnce(Cell<bool>);

impl ReportOnce {
    /// `true` the first time only.
    fn first(&self) -> bool {
        !self.0.replace(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_is_reported_only_the_first_time() {
        let report = ReportOnce::default();
        assert!(report.first());
        assert!(!report.first());
        assert!(!report.first());
    }
}
