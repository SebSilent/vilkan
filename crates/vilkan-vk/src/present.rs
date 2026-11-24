//! Native swapchain presentation: a real window with a first native frame.
//! No Proton, no D3D12. Renders offscreen and blits to the swapchain image.
//! Enabled by the `present` feature (winit + ash-window).

use anyhow::{Context as _, Result};
use ash::vk;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::Context;
use crate::graphics::RenderTarget;

pub struct Swapchain<'a> {
    ctx: &'a Context,
    surface: vk::SurfaceKHR,
    surface_loader: ash::khr::surface::Instance,
    pub loader: ash::khr::swapchain::Device,
    pub handle: vk::SwapchainKHR,
    images: Vec<vk::Image>,
    views: Vec<vk::ImageView>,
    pub format: vk::Format,
    pub extent: vk::Extent2D,
    acquire: vk::Semaphore,
    present_done: vk::Semaphore,
    fence: vk::Fence,
}

impl<'a> Swapchain<'a> {
    pub fn new<W: HasDisplayHandle + HasWindowHandle>(ctx: &'a Context, window: &W, w: u32, h: u32) -> Result<Self> {
        let dev = &ctx.device;
        // SAFETY: valid entry/instance + window handles live for the call.
        let surface = unsafe { ash_window::create_surface(ctx.entry(), ctx.instance(),
            window.display_handle()?.as_raw(), window.window_handle()?.as_raw(), None) }
            .context("create Vulkan surface")?;
        let surface_loader = ash::khr::surface::Instance::new(ctx.entry(), ctx.instance());
        // SAFETY: valid physical device/surface.
        let caps = unsafe { surface_loader.get_physical_device_surface_capabilities(ctx.physical_device(), surface) }?;
        // SAFETY: valid physical device/surface.
        let formats = unsafe { surface_loader.get_physical_device_surface_formats(ctx.physical_device(), surface) }?;
        let sf = formats.iter().find(|f| f.format == vk::Format::B8G8R8A8_UNORM || f.format == vk::Format::R8G8B8A8_UNORM)
            .copied().unwrap_or(formats[0]);
        let extent = vk::Extent2D {
            width: w.clamp(caps.min_image_extent.width, caps.max_image_extent.width),
            height: h.clamp(caps.min_image_extent.height, caps.max_image_extent.height),
        };
        let image_count = (caps.min_image_count + 1).min(if caps.max_image_count > 0 { caps.max_image_count } else { u32::MAX });
        let sc_info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(image_count)
            .image_format(sf.format)
            .image_color_space(sf.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::TRANSFER_DST)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(caps.current_transform)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(vk::PresentModeKHR::FIFO)
            .clipped(true);
        let loader = ash::khr::swapchain::Device::new(ctx.instance(), dev);
        // SAFETY: valid device/create info.
        let handle = unsafe { loader.create_swapchain(&sc_info, None) }?;
        // SAFETY: valid swapchain.
        let images = unsafe { loader.get_swapchain_images(handle) }?;
        let mut views = Vec::new();
        for &img in &images {
            let vi = vk::ImageViewCreateInfo::default().image(img)
                .view_type(vk::ImageViewType::TYPE_2D).format(sf.format)
                .subresource_range(vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 });
            // SAFETY: valid image/create info.
            views.push(unsafe { dev.create_image_view(&vi, None) }?);
        }
        let sem_info = vk::SemaphoreCreateInfo::default();
        // SAFETY: valid device/create info.
        let acquire = unsafe { dev.create_semaphore(&sem_info, None) }?;
        // SAFETY: valid device/create info.
        let present_done = unsafe { dev.create_semaphore(&sem_info, None) }?;
        let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
        // SAFETY: valid device/create info.
        let fence = unsafe { dev.create_fence(&fence_info, None) }?;
        Ok(Swapchain { ctx, surface, surface_loader, loader, handle, images, views, format: sf.format, extent, acquire, present_done, fence })
    }

    /// Blit the offscreen render target to the next swapchain image and present.
    pub fn present_blit(&self, rt: &RenderTarget) -> Result<()> {
        let dev = &self.ctx.device;
        // SAFETY: valid fence.
        unsafe { dev.wait_for_fences(&[self.fence], true, u64::MAX) }?;
        // SAFETY: valid fence.
        unsafe { dev.reset_fences(&[self.fence]) }?;
        // SAFETY: valid swapchain/semaphore.
        let (idx, _) = unsafe { self.loader.acquire_next_image(self.handle, u64::MAX, self.acquire, vk::Fence::null()) }?;
        let dst = self.images[idx as usize];
        // Record the blit into a command buffer, submitted waiting on `acquire`
        // and signaling `present_done` (so the write is ordered after acquire).
        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(self.ctx.queue_family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // SAFETY: valid device/queue family.
        let pool = unsafe { dev.create_command_pool(&pool_info, None) }?;
        let ai = vk::CommandBufferAllocateInfo::default().command_pool(pool)
            .level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1);
        // SAFETY: valid pool.
        let cmd = unsafe { dev.allocate_command_buffers(&ai) }?[0];
        let bi = vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: valid command buffer.
        unsafe { dev.begin_command_buffer(cmd, &bi) }?;
        let range = vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 };
        let dst_to_transfer = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::UNDEFINED).new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .image(dst).subresource_range(range)
            .src_access_mask(vk::AccessFlags::empty()).dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
        // SAFETY: valid command buffer/barrier.
        unsafe { dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], std::slice::from_ref(&dst_to_transfer)) };
        let region = vk::ImageBlit::default()
            .src_subresource(vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 })
            .src_offsets([vk::Offset3D { x: 0, y: 0, z: 0 }, vk::Offset3D { x: rt.width as i32, y: rt.height as i32, z: 1 }])
            .dst_subresource(vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 })
            .dst_offsets([vk::Offset3D { x: 0, y: 0, z: 0 }, vk::Offset3D { x: self.extent.width as i32, y: self.extent.height as i32, z: 1 }]);
        // SAFETY: valid command buffer/images (src TRANSFER_SRC_OPTIMAL after the render pass).
        unsafe { dev.cmd_blit_image(cmd, rt.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, dst, vk::ImageLayout::TRANSFER_DST_OPTIMAL, std::slice::from_ref(&region), vk::Filter::NEAREST) };
        let dst_to_present = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL).new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
            .image(dst).subresource_range(range)
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE).dst_access_mask(vk::AccessFlags::empty());
        // SAFETY: valid command buffer/barrier.
        unsafe { dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::BOTTOM_OF_PIPE, vk::DependencyFlags::empty(), &[], &[], std::slice::from_ref(&dst_to_present)) };
        // SAFETY: valid command buffer.
        unsafe { dev.end_command_buffer(cmd) }?;
        let wait_stage = vk::PipelineStageFlags::TRANSFER;
        let cmds = [cmd];
        let si = vk::SubmitInfo::default()
            .wait_semaphores(std::slice::from_ref(&self.acquire))
            .wait_dst_stage_mask(std::slice::from_ref(&wait_stage))
            .command_buffers(&cmds)
            .signal_semaphores(std::slice::from_ref(&self.present_done));
        // SAFETY: valid queue/semaphores/fence/command buffer.
        unsafe { dev.queue_submit(self.ctx.queue, std::slice::from_ref(&si), self.fence) }?;
        let swapchains = [self.handle];
        let indices = [idx];
        let pi = vk::PresentInfoKHR::default()
            .wait_semaphores(std::slice::from_ref(&self.present_done))
            .swapchains(&swapchains)
            .image_indices(&indices);
        // SAFETY: valid queue/present info.
        unsafe { self.loader.queue_present(self.ctx.queue, &pi) }?;
        // Wait for the blit submit to finish, then free its command pool.
        // SAFETY: valid fence/pool.
        unsafe { dev.wait_for_fences(&[self.fence], true, u64::MAX) }?;
        // SAFETY: valid pool (submit complete).
        unsafe { dev.destroy_command_pool(pool, None) };
        Ok(())
    }
}

impl Drop for Swapchain<'_> {
    fn drop(&mut self) {
        // SAFETY: valid handles owned here.
        unsafe {
            self.ctx.device.destroy_fence(self.fence, None);
            self.ctx.device.destroy_semaphore(self.acquire, None);
            self.ctx.device.destroy_semaphore(self.present_done, None);
            for &v in &self.views { self.ctx.device.destroy_image_view(v, None); }
            self.loader.destroy_swapchain(self.handle, None);
            self.surface_loader.destroy_surface(self.surface, None);
        }
    }
}