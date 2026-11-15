use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use ash::vk;
use dpi::PhysicalSize;
use euclid::default::Size2D;
use gleam::gl::{self, Gl};
use servo::{DeviceIntRect, RenderingContext, RgbaImage};
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
    #[error("Slint's WGPU renderer is not using Vulkan")]
    WgpuNotVulkan,
    #[error("the offscreen surface has no framebuffer")]
    NoFramebuffer,
    #[error("the offscreen surface disappeared while importing a frame")]
    NoSurface,
    #[error("no Vulkan memory type can back the shared frame")]
    NoCompatibleMemoryType,
    #[error("Vulkan texture import failed: {0}")]
    Vulkan(#[from] vk::Result),
    #[error("OpenGL texture import failed: {0}")]
    OpenGl(String),
    #[error("Slint rejected the imported WGPU texture: {0}")]
    SlintTexture(String),
}

impl From<surfman::Error> for RenderingBridgeError {
    fn from(error: surfman::Error) -> Self {
        Self::Surfman(error)
    }
}

pub(super) struct SurfmanContext {
    gleam_gl: Rc<dyn Gl>,
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

    fn make_current(&self) -> Result<(), surfman::Error> {
        self.device
            .borrow()
            .make_context_current(&self.context.borrow())
    }

    fn framebuffer(&self) -> Option<glow::NativeFramebuffer> {
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
}

impl BrowserRenderingContext {
    pub fn new(
        size: ViewportSize,
        wgpu_device: &wgpu::Device,
    ) -> Result<Self, RenderingBridgeError> {
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

        Ok(Self {
            size: Cell::new(size),
            swap_chain,
            surfman,
        })
    }

    /// Imports the completed Servo framebuffer into the WGPU device owned by Slint.
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

    fn read_to_image(&self, _source_rectangle: DeviceIntRect) -> Option<RgbaImage> {
        None
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
        let _ = self.swap_chain.resize(
            &self.surfman.device.borrow(),
            &mut self.surfman.context.borrow_mut(),
            Size2D::new(size.width() as i32, size.height() as i32),
        );
        self.size.set(size);
    }

    fn present(&self) {
        let _ = self.swap_chain.swap_buffers(
            &self.surfman.device.borrow(),
            &mut self.surfman.context.borrow_mut(),
            PreserveBuffer::No,
        );
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
