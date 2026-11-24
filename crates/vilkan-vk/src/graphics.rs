//! Native Vulkan rasterization harness: render target, graphics pipeline,
//! vertex input, draw, readback. No Proton, no D3D12.
//!
//! The translated vertex shaders read VertexIndex (a fullscreen triangle needs
//! no vertex buffer: `draw(3,1,0,0)`), output Position + interpolants; the pixel
//! shader outputs one vec4 to location 0. The render target is
//! R32G32B32A32_SFLOAT so readback is exact for numeric checks.

use anyhow::{Result, bail};
use ash::vk;

use crate::Context;

/// A render target with a render pass and framebuffer, plus readback.
pub struct RenderTarget<'a> {
    ctx: &'a Context,
    pub width: u32,
    pub height: u32,
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    render_pass: vk::RenderPass,
    framebuffer: vk::Framebuffer,
}

impl<'a> RenderTarget<'a> {
    pub fn new(ctx: &'a Context, width: u32, height: u32) -> Result<Self> {
        let dev = &ctx.device;
        let format = vk::Format::R32G32B32A32_SFLOAT;
        let (image, memory, view) = ctx.make_image(format, width, height,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)?;

        let attachment = vk::AttachmentDescription::default()
            .format(format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
        let color_ref = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(std::slice::from_ref(&color_ref));
        // Make the color write visible to the transfer readback.
        let dep = vk::SubpassDependency::default()
            .src_subpass(0)
            .dst_subpass(vk::SUBPASS_EXTERNAL)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(vk::PipelineStageFlags::TRANSFER)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
        let rp_info = vk::RenderPassCreateInfo::default()
            .attachments(std::slice::from_ref(&attachment))
            .subpasses(std::slice::from_ref(&subpass))
            .dependencies(std::slice::from_ref(&dep));
        // SAFETY: valid device/create info.
        let render_pass = unsafe { dev.create_render_pass(&rp_info, None) }?;

        let fb_info = vk::FramebufferCreateInfo::default()
            .render_pass(render_pass)
            .attachments(std::slice::from_ref(&view))
            .width(width).height(height).layers(1);
        // SAFETY: valid device/render pass/view.
        let framebuffer = unsafe { dev.create_framebuffer(&fb_info, None) }?;

        Ok(RenderTarget { ctx, width, height, image, memory, view, render_pass, framebuffer })
    }

    /// Clear the target to `clear` (RGBA floats) and read it back. Proves the
    /// render target + readback path with no shaders.
    pub fn clear_and_readback(&self, clear: [f32; 4]) -> Result<Vec<u32>> {
        let dev = &self.ctx.device;
        self.ctx.one_shot(|cmd| {
            let clear_value = vk::ClearValue { color: vk::ClearColorValue { float32: clear } };
            let rp_begin = vk::RenderPassBeginInfo::default()
                .render_pass(self.render_pass)
                .framebuffer(self.framebuffer)
                .render_area(vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width: self.width, height: self.height } })
                .clear_values(std::slice::from_ref(&clear_value));
            // SAFETY: valid command buffer/render pass/framebuffer.
            unsafe { dev.cmd_begin_render_pass(cmd, &rp_begin, vk::SubpassContents::INLINE) };
            // SAFETY: valid command buffer.
            unsafe { dev.cmd_end_render_pass(cmd) };
            Ok(())
        })?;
        self.readback()
    }

    /// Draw a fullscreen triangle (3 vertices from VertexIndex) with the given
    /// VS/PS SPIR-V and read back the pixels.
    pub fn draw_and_readback(&self, vs_spv: &[u8], ps_spv: &[u8], clear: [f32; 4]) -> Result<Vec<u32>> {
        let dev = &self.ctx.device;
        let vs = self.module(vs_spv)?;
        let ps = self.module(ps_spv)?;

        let pc_range = vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::ALL_GRAPHICS)
            .offset(0).size(crate::PC_BYTES as u32);
        let pl_info = vk::PipelineLayoutCreateInfo::default()
            .push_constant_ranges(std::slice::from_ref(&pc_range));
        // SAFETY: valid device/range.
        let layout = unsafe { dev.create_pipeline_layout(&pl_info, None) }?;

        let stages = [
            vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(vs).name(c"main"),
            vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(ps).name(c"main"),
        ];
        // No vertex input: the VS reads VertexIndex.
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::Viewport { x: 0.0, y: 0.0, width: self.width as f32, height: self.height as f32, min_depth: 0.0, max_depth: 1.0 };
        let scissor = vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width: self.width, height: self.height } };
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewports(std::slice::from_ref(&viewport))
            .scissors(std::slice::from_ref(&scissor));
        let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let blend_attach = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA);
        let blend = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(std::slice::from_ref(&blend_attach));
        let pipe_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterizer)
            .multisample_state(&multisample)
            .color_blend_state(&blend)
            .layout(layout)
            .render_pass(self.render_pass)
            .subpass(0);
        // SAFETY: valid device/stages/layout/render pass.
        let pipeline = unsafe { dev.create_graphics_pipelines(vk::PipelineCache::null(), std::slice::from_ref(&pipe_info), None) }
            .map_err(|e| anyhow::anyhow!("create graphics pipeline: {:?}", e.1))?[0];

        let clear_value = vk::ClearValue { color: vk::ClearColorValue { float32: clear } };
        self.ctx.one_shot(|cmd| {
            let rp_begin = vk::RenderPassBeginInfo::default()
                .render_pass(self.render_pass)
                .framebuffer(self.framebuffer)
                .render_area(vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width: self.width, height: self.height } })
                .clear_values(std::slice::from_ref(&clear_value));
            // SAFETY: valid command buffer/render pass/framebuffer.
            unsafe { dev.cmd_begin_render_pass(cmd, &rp_begin, vk::SubpassContents::INLINE) };
            // SAFETY: valid command buffer/pipeline.
            unsafe { dev.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline) };
            let mut pc = [0u32; 36];
            pc[32] = 0; pc[33] = 0; // heap base (unused by these fixtures)
            let pc_bytes: &[u8] = unsafe { std::slice::from_raw_parts(pc.as_ptr() as *const u8, crate::PC_BYTES) };
            // SAFETY: valid command buffer/layout, pc bytes live.
            unsafe { dev.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL_GRAPHICS, 0, pc_bytes) };
            // SAFETY: valid command buffer; 3 vertices from VertexIndex.
            unsafe { dev.cmd_draw(cmd, 3, 1, 0, 0) };
            // SAFETY: valid command buffer.
            unsafe { dev.cmd_end_render_pass(cmd) };
            Ok(())
        })?;

        // SAFETY: pipeline/layout/modules owned here.
        unsafe { dev.destroy_pipeline(pipeline, None); dev.destroy_pipeline_layout(layout, None); dev.destroy_shader_module(vs, None); dev.destroy_shader_module(ps, None) };
        self.readback()
    }

    fn module(&self, spv: &[u8]) -> Result<vk::ShaderModule> {
        if spv.len() % 4 != 0 { bail!("SPIR-V length not a multiple of 4"); }
        let words: Vec<u32> = spv.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let info = vk::ShaderModuleCreateInfo::default().code(&words);
        // SAFETY: valid device/code.
        Ok(unsafe { self.ctx.device.create_shader_module(&info, None) }?)
    }

    /// Copy the rendered image to a host-visible buffer and read it as words.
    fn readback(&self) -> Result<Vec<u32>> {
        let dev = &self.ctx.device;
        let n = (self.width * self.height * 4) as usize; // 4 floats per pixel
        let (buf, mem, ptr) = self.ctx.make_buffer(n * 4,
            vk::BufferUsageFlags::TRANSFER_DST,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)?;
        self.ctx.one_shot(|cmd| {
            let region = vk::BufferImageCopy::default()
                .image_subresource(vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 })
                .image_extent(vk::Extent3D { width: self.width, height: self.height, depth: 1 });
            // SAFETY: valid command buffer/image (TRANSFER_SRC_OPTIMAL after pass)/buffer.
            unsafe { dev.cmd_copy_image_to_buffer(cmd, self.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, buf, std::slice::from_ref(&region)) };
            Ok(())
        })?;
        // SAFETY: host-visible, n words.
        let out = unsafe { std::slice::from_raw_parts(ptr as *const u32, n) }.to_vec();
        // SAFETY: readback buffer/memory owned here.
        unsafe { dev.destroy_buffer(buf, None); dev.free_memory(mem, None) };
        Ok(out)
    }
}

impl Drop for RenderTarget<'_> {
    fn drop(&mut self) {
        // SAFETY: valid handles owned here.
        unsafe {
            self.ctx.device.destroy_framebuffer(self.framebuffer, None);
            self.ctx.device.destroy_render_pass(self.render_pass, None);
            self.ctx.device.destroy_image_view(self.view, None);
            self.ctx.device.destroy_image(self.image, None);
            self.ctx.device.free_memory(self.memory, None);
        }
    }
}