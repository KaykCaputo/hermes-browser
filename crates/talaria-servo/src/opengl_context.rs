use std::cell::Cell;
use std::ffi::{CStr, CString, c_void};
use std::ptr;
use std::rc::Rc;
use std::sync::Arc;

use dpi::{LogicalSize, PhysicalSize};
use gleam::gl::{self, Gl};
use glow::HasContext as _;
use servo::{DeviceIntRect, RenderingContext, RgbaImage};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ViewportSize {
    width: u32,
    height: u32,
}

impl ViewportSize {
    pub fn new(width: u32, height: u32) -> Option<Self> {
        (width > 0 && height > 0).then_some(Self { width, height })
    }

    pub fn from_logical(width: f32, height: f32, scale_factor: f32) -> Option<Self> {
        if !width.is_finite()
            || !height.is_finite()
            || !scale_factor.is_finite()
            || width <= 0.0
            || height <= 0.0
            || scale_factor <= 0.0
        {
            return None;
        }

        let logical_size = LogicalSize::new(f64::from(width), f64::from(height));

        let physical_size = logical_size.to_physical::<u32>(f64::from(scale_factor));

        Self::new(physical_size.width, physical_size.height)
    }

    pub fn width(self) -> u32 {
        self.width
    }

    pub fn height(self) -> u32 {
        self.height
    }

    fn physical_size(self) -> PhysicalSize<u32> {
        PhysicalSize::new(self.width, self.height)
    }
}

pub struct SharedOpenGlContext {
    size: Cell<ViewportSize>,
    gleam_gl: Rc<dyn Gl>,
    glow_gl: Arc<glow::Context>,
}

impl SharedOpenGlContext {
    /// Creates a rendering context from the OpenGL context currently owned by the embedder.
    /// # Safety
    /// `get_proc_address` must return function pointers for the OpenGL context that is current on
    /// this thread. That context must remain alive while this `SharedOpenGlContext` exists, and
    /// every Servo rendering operation must run while the same context is current.
    pub unsafe fn new(
        size: ViewportSize,
        get_proc_address: &dyn Fn(&CStr) -> *const c_void,
    ) -> Self {
        let glow_gl = Arc::new(unsafe {
            glow::Context::from_loader_function_cstr(|symbol| get_proc_address(symbol))
        });

        let gleam_gl: Rc<dyn Gl> = if glow_gl.version().is_embedded {
            unsafe { gl::GlesFns::load_with(|symbol| load_symbol(get_proc_address, symbol)) }
        } else {
            unsafe { gl::GlFns::load_with(|symbol| load_symbol(get_proc_address, symbol)) }
        };

        Self {
            size: Cell::new(size),
            gleam_gl,
            glow_gl,
        }
    }

    pub fn resize(&self, size: ViewportSize) {
        self.size.set(size);
    }
}

impl RenderingContext for SharedOpenGlContext {
    fn read_to_image(&self, _source_rectangle: DeviceIntRect) -> Option<RgbaImage> {
        None
    }

    fn size(&self) -> PhysicalSize<u32> {
        self.size.get().physical_size()
    }

    fn resize(&self, size: PhysicalSize<u32>) {
        if let Some(size) = ViewportSize::new(size.width, size.height) {
            SharedOpenGlContext::resize(self, size);
        }
    }

    fn present(&self) {}

    fn make_current(&self) -> Result<(), surfman::Error> {
        Ok(())
    }

    fn gleam_gl_api(&self) -> Rc<dyn Gl> {
        Rc::clone(&self.gleam_gl)
    }

    fn glow_gl_api(&self) -> Arc<glow::Context> {
        Arc::clone(&self.glow_gl)
    }
}

fn load_symbol(get_proc_address: &dyn Fn(&CStr) -> *const c_void, symbol: &str) -> *const c_void {
    let Ok(symbol) = CString::new(symbol) else {
        return ptr::null();
    };

    get_proc_address(&symbol)
}
