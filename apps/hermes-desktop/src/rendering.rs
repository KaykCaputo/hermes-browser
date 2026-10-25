#[cfg(target_os = "linux")]
mod platform {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::Arc;

    use ash::vk;
    use dpi::PhysicalSize;
    use euclid::default::Size2D;
    use gleam::gl::{self, Gl};
    use glow::HasContext;
    use servo::{DeviceIntRect, RenderingContext, RgbaImage};
    use slint::Image;
    use slint::wgpu_29::wgpu::{self, hal};
    use surfman::chains::{PreserveBuffer, SwapChain};
    use surfman::{
        Adapter, Connection, Context, ContextAttributeFlags, ContextAttributes, Device, GLApi,
        Surface, SurfaceAccess, SurfaceInfo, SurfaceTexture, SurfaceType,
    };

    use talaria_servo::ViewportSize;

    mod gl_bindings {
        #![allow(clippy::all)]
        #![allow(unsafe_op_in_unsafe_fn)]
        include!(concat!(env!("OUT_DIR"), "/gl_bindings.rs"));
    }

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

    struct SurfmanContext {
        gleam_gl: Rc<dyn Gl>,
        glow_gl: Arc<glow::Context>,
        device: RefCell<Device>,
        context: RefCell<Context>,
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
            let descriptor =
                device.create_context_descriptor(&ContextAttributes { flags, version })?;
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
        size: Cell<ViewportSize>,
        swap_chain: SwapChain<Device>,
        surfman: SurfmanContext,
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

        fn import_frame_into_wgpu(
            &self,
            wgpu_device: &wgpu::Device,
        ) -> Result<wgpu::Texture, RenderingBridgeError> {
            let device = self.surfman.device.borrow();
            let mut context = self.surfman.context.borrow_mut();
            let surface = device
                .unbind_surface_from_context(&mut context)?
                .ok_or(RenderingBridgeError::NoSurface)?;

            let import_result = (|| unsafe {
                device.make_context_current(&context)?;
                let surface_info = device.surface_info(&surface);
                let read_framebuffer = surface_info
                    .framebuffer_object
                    .ok_or(RenderingBridgeError::NoFramebuffer)?;
                let size = self.size.get();

                let hal_device = wgpu_device
                    .as_hal::<wgpu::wgc::api::Vulkan>()
                    .ok_or(RenderingBridgeError::WgpuNotVulkan)?;
                let vulkan_device = hal_device.raw_device().clone();
                let vulkan_instance = hal_device.shared_instance().raw_instance();

                let mut external_image = vk::ExternalMemoryImageCreateInfo::default()
                    .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
                let image = vulkan_device.create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(vk::Format::R8G8B8A8_UNORM)
                        .extent(vk::Extent3D {
                            width: size.width(),
                            height: size.height(),
                            depth: 1,
                        })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::COLOR_ATTACHMENT)
                        .sharing_mode(vk::SharingMode::EXCLUSIVE)
                        .initial_layout(vk::ImageLayout::UNDEFINED)
                        .push_next(&mut external_image),
                    None,
                )?;
                let requirements = vulkan_device.get_image_memory_requirements(image);
                let memory_properties = vulkan_instance
                    .get_physical_device_memory_properties(hal_device.raw_physical_device());
                let memory_type_index = (0..memory_properties.memory_type_count)
                    .find(|index| requirements.memory_type_bits & (1 << index) != 0)
                    .ok_or(RenderingBridgeError::NoCompatibleMemoryType)?;

                let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
                let mut export = vk::ExportMemoryAllocateInfo::default()
                    .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
                let memory = vulkan_device.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(requirements.size)
                        .memory_type_index(memory_type_index)
                        .push_next(&mut dedicated)
                        .push_next(&mut export),
                    None,
                )?;
                if let Err(error) = vulkan_device.bind_image_memory(image, memory, 0) {
                    vulkan_device.free_memory(memory, None);
                    vulkan_device.destroy_image(image, None);
                    return Err(error.into());
                }

                let external_memory =
                    ash::khr::external_memory_fd::Device::new(vulkan_instance, &vulkan_device);
                let memory_fd = external_memory.get_memory_fd(
                    &vk::MemoryGetFdInfoKHR::default()
                        .memory(memory)
                        .handle_type(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD),
                )?;

                let gl = &self.surfman.glow_gl;
                let gl_ext =
                    gl_bindings::Gles2::load_with(|name| device.get_proc_address(&context, name));
                let mut memory_object = 0;
                gl_ext.CreateMemoryObjectsEXT(1, &mut memory_object);
                gl_ext.MemoryObjectParameterivEXT(
                    memory_object,
                    gl_bindings::DEDICATED_MEMORY_OBJECT_EXT,
                    &1,
                );
                gl_ext.ImportMemoryFdEXT(
                    memory_object,
                    requirements.size,
                    gl_bindings::HANDLE_TYPE_OPAQUE_FD_EXT,
                    memory_fd,
                );

                let texture = gl.create_texture().map_err(RenderingBridgeError::OpenGl)?;
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl_ext.TexStorageMem2DEXT(
                    glow::TEXTURE_2D,
                    1,
                    glow::RGBA8,
                    size.width() as i32,
                    size.height() as i32,
                    memory_object,
                    0,
                );
                let draw_framebuffer = gl
                    .create_framebuffer()
                    .map_err(RenderingBridgeError::OpenGl)?;
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(draw_framebuffer));
                gl.framebuffer_texture_2d(
                    glow::DRAW_FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(texture),
                    0,
                );
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(read_framebuffer));
                gl.blit_framebuffer(
                    0,
                    0,
                    size.width() as i32,
                    size.height() as i32,
                    0,
                    size.height() as i32,
                    size.width() as i32,
                    0,
                    glow::COLOR_BUFFER_BIT,
                    glow::NEAREST,
                );

                // The two APIs share memory but not command queues. Waiting here guarantees
                // that Slint never samples a partially copied frame during move or resize.
                gl.finish();
                gl.delete_framebuffer(draw_framebuffer);
                gl.delete_texture(texture);
                gl_ext.DeleteMemoryObjectsEXT(1, &memory_object);

                let texture = wgpu_device.create_texture_from_hal::<wgpu::wgc::api::Vulkan>(
                    hal_device.texture_from_raw(
                        image,
                        &hal::TextureDescriptor {
                            label: Some("Hermes Servo frame"),
                            size: wgpu::Extent3d {
                                width: size.width(),
                                height: size.height(),
                                depth_or_array_layers: 1,
                            },
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            dimension: wgpu::TextureDimension::D2,
                            mip_level_count: 1,
                            sample_count: 1,
                            usage: wgpu::TextureUses::RESOURCE | wgpu::TextureUses::COLOR_TARGET,
                            view_formats: Vec::new(),
                            memory_flags: hal::MemoryFlags::empty(),
                        },
                        Some(Box::new(move || {
                            vulkan_device.destroy_image(image, None);
                            vulkan_device.free_memory(memory, None);
                        })),
                        hal::vulkan::TextureMemory::External,
                    ),
                    &wgpu::TextureDescriptor {
                        label: Some("Hermes Servo frame"),
                        size: wgpu::Extent3d {
                            width: size.width(),
                            height: size.height(),
                            depth_or_array_layers: 1,
                        },
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        dimension: wgpu::TextureDimension::D2,
                        mip_level_count: 1,
                        sample_count: 1,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING
                            | wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    },
                );

                Ok(texture)
            })();

            device
                .bind_surface_to_context(&mut context, surface)
                .map_err(|(error, mut surface)| {
                    let _ = device.destroy_surface(&mut context, &mut surface);
                    error
                })?;

            import_result
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
}

#[cfg(not(target_os = "linux"))]
mod platform {
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
        ) -> Result<Self, RenderingBridgeError> {
            Ok(Self {
                inner: SoftwareRenderingContext::new(PhysicalSize::new(
                    size.width(),
                    size.height(),
                ))?,
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
                    Point2D::origin(),
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
}

pub use platform::BrowserRenderingContext;
