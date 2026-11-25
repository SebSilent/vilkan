//! Native swapchain presentation: a real window with a first native frame.
//! No Proton, no D3D12. Renders offscreen and blits to the swapchain image.
//! Enabled by the `present` feature (winit + ash-window).

use anyhow::{Context as _, Result};
use ash::vk;
use std::io::Write as _;
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
            // TRANSFER_DST for the blit path, COLOR_ATTACHMENT so a bare
            // vkCmdClearColorImage is legal (the fixture-free milestone path
            // presents a solid colour with no pipeline at all).
            // TRANSFER_SRC as well: the surface caps advertise it (usage=0x9f),
            // so a swapchain image can be copied out for verification.
            .image_usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::COLOR_ATTACHMENT)
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

    /// Present a solid colour: acquire, one command buffer that clears the
    /// swapchain image, submit, present. No SPIR-V, no pipeline, no render
    /// pass - the smallest thing that can put real pixels on a real screen.
    pub fn present_clear(&self, rgba: [f32; 4]) -> Result<()> {
        let dev = &self.ctx.device;
        // SAFETY: valid fence.
        unsafe { dev.wait_for_fences(&[self.fence], true, u64::MAX) }?;
        // SAFETY: valid fence.
        unsafe { dev.reset_fences(&[self.fence]) }?;
        // SAFETY: valid swapchain/semaphore.
        let (idx, _) = unsafe { self.loader.acquire_next_image(self.handle, u64::MAX, self.acquire, vk::Fence::null()) }?;
        let dst = self.images[idx as usize];
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
        let to_color = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::UNDEFINED).new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .image(dst).subresource_range(range)
            .src_access_mask(vk::AccessFlags::empty()).dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
        // SAFETY: valid command buffer/barrier.
        unsafe { dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT, vk::DependencyFlags::empty(), &[], &[], std::slice::from_ref(&to_color)) };
        let value = vk::ClearColorValue { float32: rgba };
        // SAFETY: valid command buffer/image; image has COLOR_ATTACHMENT usage.
        unsafe { dev.cmd_clear_color_image(cmd, dst, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, &value, std::slice::from_ref(&range)) };
        let to_present = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL).new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
            .image(dst).subresource_range(range)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE).dst_access_mask(vk::AccessFlags::empty());
        // SAFETY: valid command buffer/barrier.
        unsafe { dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT, vk::PipelineStageFlags::BOTTOM_OF_PIPE, vk::DependencyFlags::empty(), &[], &[], std::slice::from_ref(&to_present)) };
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
/// One frame copied out of the GPU into host memory.
pub struct Readback {
    pub width: u32,
    pub height: u32,
    /// Texels per row in the packed copy (`bufferRowLength`).
    pub row_length: u32,
    /// Bytes per row, from the image's linear layout when it has one. An
    /// OPTIMAL-tiling image has no meaningful linear layout, so this falls back
    /// to the row length we asked for.
    pub row_pitch: u32,
    pub format: vk::Format,
    /// Row-major RGB8, `width * height * 3`, swizzle and pitch resolved.
    pub rgb: Vec<u8>,
    /// Exactly what the GPU wrote: `row_length` stride, in `format`'s order.
    pub packed: Vec<u8>,
}

impl Readback {
    /// Packed copy -> RGB8. `packed` is the driver's bytes, so the channel
    /// order comes from the *format* (B8G8R8A8 = blue first), and rows step by
    /// `row_length` texels, not by `width`.
    fn assemble(width: u32, height: u32, row_length: u32, format: vk::Format, packed: &[u8]) -> Vec<u8> {
        // vk::Format is a plain enum here (no Debug in ash 0.38); 44/50 are the
        // BGRA forms, 37/43 the RGBA forms.
        // 44 = B8G8R8A8_UNORM, 50 = its packed sibling; both are blue-first.
        let blue_first = matches!(format.as_raw(), 44 | 50);
        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        'rows: for y in 0..height {
            for x in 0..width {
                let i = ((y * row_length + x) * 4) as usize;
                let Some(p) = packed.get(i..i + 4) else { break 'rows };
                let (b, g, r) = (p[0], p[1], p[2]);
                let px = if blue_first { [r, g, b] } else { [b, g, r] };
                rgb.extend_from_slice(&px);
            }
        }
        rgb
    }

    /// Distinct BGRA quads, in first-seen order (max 8).
    pub fn distinct_bgra(&self) -> Vec<[u8; 4]> {
        let mut out: Vec<[u8; 4]> = Vec::new();
        for y in 0..self.height {
            for x in 0..self.width {
                if let Some(q) = self.sample_bgra(x, y) && !out.contains(&q) {
                    out.push(q);
                    if out.len() >= 8 { return out; }
                }
            }
        }
        out
    }

    /// The packed BGRA quad at (x, y), honouring `row_length`.
    pub fn sample_bgra(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        let i = ((y * self.row_length + x) * 4) as usize;
        let p = self.packed.get(i..i + 4)?;
        Some([p[0], p[1], p[2], p[3]])
    }

    /// Write the frame as binary PPM (P6). The file is written by the process
    /// itself, so it needs no screen capture to be believed.
    pub fn write_ppm(&self, path: &std::path::Path) -> Result<()> {
        let mut f = std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
        write!(f, "P6\n{} 1\n255\n", self.width * self.height).context("write PPM header")?;
        f.write_all(&self.rgb).context("write PPM pixels")?;
        Ok(())
    }
}

impl Swapchain<'_> {
    /// Clear the next swapchain image, present it, and copy a *second* clear
    /// (same colour, into an image we created) into host memory so the frame can
    /// be verified without screen capture.
    ///
    /// The readback deliberately does not copy the swapchain image. On this
    /// NVIDIA-under-Wine combination that copy hangs the device even though the
    /// surface caps advertise `TRANSFER_SRC`; the offscreen image exercises the
    /// same clear -> transfer -> host-visible path with parameters we control.
    pub fn present_clear_readback(&self, rgba: [f32; 4], out: Option<&std::path::Path>) -> Result<Readback> {
        let dev = &self.ctx.device;
        // SAFETY: valid fence.
        unsafe { dev.wait_for_fences(&[self.fence], true, u64::MAX) }.context("fence (readback)")?;
        // SAFETY: valid fence.
        unsafe { dev.reset_fences(&[self.fence]) }.context("reset fence (readback)")?;
        // SAFETY: valid swapchain/semaphore.
        let (idx, _) = unsafe { self.loader.acquire_next_image(self.handle, u64::MAX, self.acquire, vk::Fence::null()) }.context("acquire (readback)")?;
        let img = self.images[idx as usize];

        let row_length = self.extent.width * 4;
        let size: u64 = u64::from(row_length) * u64::from(self.extent.height);
        let bi = vk::BufferCreateInfo::default().size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_DST);
        // SAFETY: valid device/create info.
        let buffer = unsafe { dev.create_buffer(&bi, None) }.context("create readback buffer")?;
        // SAFETY: valid buffer.
        let req = unsafe { dev.get_buffer_memory_requirements(buffer) };
        let mem_type = self.ctx.mem_type(req.memory_type_bits, vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)?;
        let alloc = vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(mem_type);
        // SAFETY: valid device.
        let memory = unsafe { dev.allocate_memory(&alloc, None) }.context("allocate readback memory")?;
        // SAFETY: valid buffer/memory, offset 0.
        unsafe { dev.bind_buffer_memory(buffer, memory, 0) }.context("bind readback memory")?;

        // Offscreen image: our own, COLOR_ATTACHMENT so the clear is legal,
        // TRANSFER_SRC so the copy out is legal.
        let off_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(self.format)
            .extent(vk::Extent3D { width: self.extent.width, height: self.extent.height, depth: 1 })
            .mip_levels(1).array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: valid device/create info.
        let off = unsafe { dev.create_image(&off_info, None) }.context("create offscreen image")?;
        // SAFETY: valid image.
        let off_req = unsafe { dev.get_image_memory_requirements(off) };
        let off_mem_type = self.ctx.mem_type(off_req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)?;
        let off_alloc = vk::MemoryAllocateInfo::default().allocation_size(off_req.size).memory_type_index(off_mem_type);
        // SAFETY: valid device.
        let off_memory = unsafe { dev.allocate_memory(&off_alloc, None) }.context("allocate offscreen memory")?;
        // SAFETY: valid image/memory, offset 0.
        unsafe { dev.bind_image_memory(off, off_memory, 0) }.context("bind offscreen memory")?;

        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(self.ctx.queue_family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // SAFETY: valid device/queue family.
        let pool = unsafe { dev.create_command_pool(&pool_info, None) }.context("create cmd pool (readback)")?;
        let cai = vk::CommandBufferAllocateInfo::default().command_pool(pool)
            .level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1);
        // SAFETY: valid pool.
        let cmd = unsafe { dev.allocate_command_buffers(&cai) }.context("alloc cmd (readback)")?[0];
        let beg = vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: valid command buffer.
        unsafe { dev.begin_command_buffer(cmd, &beg) }.context("begin cmd (readback)")?;

        let range = vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 };
        let value = vk::ClearColorValue { float32: rgba };

        // Swapchain image: UNDEFINED -> COLOR_ATTACHMENT, clear, -> PRESENT_SRC.
        let sc_color = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::UNDEFINED).new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .image(img).subresource_range(range)
            .src_access_mask(vk::AccessFlags::empty()).dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
        // SAFETY: valid command buffer/barrier.
        unsafe { dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT, vk::DependencyFlags::empty(), &[], &[], std::slice::from_ref(&sc_color)) };
        // SAFETY: valid command buffer/image (COLOR_ATTACHMENT usage).
        unsafe { dev.cmd_clear_color_image(cmd, img, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, &value, std::slice::from_ref(&range)) };
        let sc_present = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL).new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
            .image(img).subresource_range(range)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE).dst_access_mask(vk::AccessFlags::empty());
        // SAFETY: valid command buffer/barrier.
        unsafe { dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT, vk::PipelineStageFlags::BOTTOM_OF_PIPE, vk::DependencyFlags::empty(), &[], &[], std::slice::from_ref(&sc_present)) };

        // Offscreen image: same clear, then TRANSFER_SRC and the copy out.
        let off_color = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::UNDEFINED).new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .image(off).subresource_range(range)
            .src_access_mask(vk::AccessFlags::empty()).dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
        // SAFETY: valid command buffer/barrier.
        unsafe { dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT, vk::DependencyFlags::empty(), &[], &[], std::slice::from_ref(&off_color)) };
        // SAFETY: valid command buffer/image (COLOR_ATTACHMENT usage).
        unsafe { dev.cmd_clear_color_image(cmd, off, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, &value, std::slice::from_ref(&range)) };
        let off_src = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL).new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .image(off).subresource_range(range)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE).dst_access_mask(vk::AccessFlags::TRANSFER_READ);
        let buf_barrier = vk::BufferMemoryBarrier::default()
            .buffer(buffer).offset(0).size(size)
            .src_access_mask(vk::AccessFlags::empty()).dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
        // SAFETY: valid command buffer/barriers.
        unsafe { dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], std::slice::from_ref(&buf_barrier), std::slice::from_ref(&off_src)) };
        let region = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .buffer_row_length(self.extent.width)
            .buffer_image_height(self.extent.height)
            .image_subresource(vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 })
            .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
            .image_extent(vk::Extent3D { width: self.extent.width, height: self.extent.height, depth: 1 });
        // SAFETY: valid command buffer/image/buffer, region within the buffer.
        unsafe { dev.cmd_copy_image_to_buffer(cmd, off, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, buffer, std::slice::from_ref(&region)) };
        // SAFETY: valid command buffer.
        unsafe { dev.end_command_buffer(cmd) }.context("end cmd (readback)")?;

        let wait_stage = vk::PipelineStageFlags::TRANSFER;
        let cmds = [cmd];
        let si = vk::SubmitInfo::default()
            .wait_semaphores(std::slice::from_ref(&self.acquire))
            .wait_dst_stage_mask(std::slice::from_ref(&wait_stage))
            .command_buffers(&cmds)
            .signal_semaphores(std::slice::from_ref(&self.present_done));
        // SAFETY: valid queue/semaphores/fence/command buffer.
        unsafe { dev.queue_submit(self.ctx.queue, std::slice::from_ref(&si), self.fence) }.context("submit (readback)")?;
        let swapchains = [self.handle];
        let indices = [idx];
        let pi = vk::PresentInfoKHR::default()
            .wait_semaphores(std::slice::from_ref(&self.present_done))
            .swapchains(&swapchains)
            .image_indices(&indices);
        // SAFETY: valid queue/present info.
        unsafe { self.loader.queue_present(self.ctx.queue, &pi) }.context("present (readback)")?;
        // Copy and present share one submission, so this fence covers both.
        // SAFETY: valid fence.
        unsafe { dev.wait_for_fences(&[self.fence], true, u64::MAX) }.context("fence after copy (readback)")?;

        // SAFETY: HOST_VISIBLE memory, mapped for exactly its allocation size.
        let ptr = unsafe { dev.map_memory(memory, 0, req.size, vk::MemoryMapFlags::empty()) }.context("map (readback)")? as *const u8;
        // SAFETY: ptr valid for req.size bytes, which is >= size.
        let packed = unsafe { std::slice::from_raw_parts(ptr, size as usize) }.to_vec();
        // SAFETY: matching unmap.
        unsafe { dev.unmap_memory(memory) };
        let rgb = Readback::assemble(self.extent.width, self.extent.height, self.extent.width, self.format, &packed);
        let rb = Readback {
            width: self.extent.width,
            height: self.extent.height,
            row_length: self.extent.width,
            row_pitch: self.extent.width * 4,
            format: self.format,
            rgb,
            packed,
        };
        if let Some(p) = out {
            rb.write_ppm(p)?;
        }
        // SAFETY: submission complete; these are no longer referenced.
        unsafe {
            dev.destroy_command_pool(pool, None);
            dev.destroy_buffer(buffer, None);
            dev.free_memory(memory, None);
            dev.destroy_image(off, None);
            dev.free_memory(off_memory, None);
        }
        Ok(rb)
    }
}
