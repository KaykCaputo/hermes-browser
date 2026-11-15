use std::rc::Rc;
use std::sync::Arc;

use dpi::PhysicalSize;
use euclid::default::{Point2D, Size2D};
use gleam::gl::Gl;
use servo::{DeviceIntRect, RenderingContext, RgbaImage, SoftwareRenderingContext};
use slint::wgpu_29::wgpu;
use slint::{Image, SharedPixelBuffer};
use surfman::{Connection, Surface, SurfaceTexture};

use talaria_servo::ViewportSize;

#[derive(Debug, thiserror::Error)]
pub enum RenderingBridgeError {
    #[error("failed to create Servo's fallback rendering context: {0:?}")]
    Surfman(surfman::Error),
    #[error("Servo did not produce an image for the current frame")]
    MissingFrame,
}

impl From<surfman::Error> for RenderingBridgeError {
    fn from(error: surfman::Error) -> Self {
        Self::Surfman(error)
    }
}

pub struct BrowserRenderingContext {
    inner: SoftwareRenderingContext,
}

impl BrowserRenderingContext {
    pub fn new(
        size: ViewportSize,
        _wgpu_device: &wgpu::Device,
        _wgpu_queue: &wgpu::Queue,
    ) -> Result<Self, RenderingBridgeError> {
        Ok(Self {
            inner: SoftwareRenderingContext::new(PhysicalSize::new(size.width(), size.height()))?,
        })
    }

    pub fn current_frame_as_image(
        &self,
        _wgpu_device: &wgpu::Device,
    ) -> Result<Image, RenderingBridgeError> {
        let size = self.inner.size2d().to_i32();
        let pixels = self
            .inner
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
}

impl RenderingContext for BrowserRenderingContext {
    fn prepare_for_rendering(&self) {
        self.inner.prepare_for_rendering();
    }
    fn read_to_image(&self, rect: DeviceIntRect) -> Option<RgbaImage> {
        self.inner.read_to_image(rect)
    }
    fn size(&self) -> PhysicalSize<u32> {
        self.inner.size()
    }
    fn resize(&self, size: PhysicalSize<u32>) {
        self.inner.resize(size);
    }
    fn present(&self) {
        self.inner.present();
    }
    fn make_current(&self) -> Result<(), surfman::Error> {
        self.inner.make_current()
    }
    fn gleam_gl_api(&self) -> Rc<dyn Gl> {
        self.inner.gleam_gl_api()
    }
    fn glow_gl_api(&self) -> Arc<glow::Context> {
        self.inner.glow_gl_api()
    }
    fn create_texture(&self, surface: Surface) -> Option<(SurfaceTexture, u32, Size2D<i32>)> {
        self.inner.create_texture(surface)
    }
    fn destroy_texture(&self, texture: SurfaceTexture) -> Option<Surface> {
        self.inner.destroy_texture(texture)
    }
    fn connection(&self) -> Option<Connection> {
        self.inner.connection()
    }
}
