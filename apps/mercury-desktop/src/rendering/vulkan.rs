use ash::vk;
use glow::HasContext;
use slint::wgpu_29::wgpu::{self, hal};

use super::surfman::{BrowserRenderingContext, RenderingBridgeError};

mod gl_bindings {
    #![allow(clippy::all)]
    #![allow(unsafe_op_in_unsafe_fn)]
    include!(concat!(env!("OUT_DIR"), "/gl_bindings.rs"));
}

impl BrowserRenderingContext {
    pub(super) fn import_frame_into_wgpu(
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
                        label: Some("Mercury Servo frame"),
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
                    label: Some("Mercury Servo frame"),
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
