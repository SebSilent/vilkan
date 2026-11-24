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
    pub(crate) image: vk::Image,
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
        self.draw_with_set(vs_spv, ps_spv, clear, &[], vk::DescriptorSet::null(), &[0u32; 36])
    }

    /// Draw with an optional descriptor set bound at set 0 and explicit push
    /// constant dwords (s0..s31, heap/code base). The textured path uses this.
    pub fn draw_with_set(&self, vs_spv: &[u8], ps_spv: &[u8], clear: [f32; 4],
                         set_layouts: &[vk::DescriptorSetLayout], set: vk::DescriptorSet,
                         pc: &[u32; 36]) -> Result<Vec<u32>> {
        let dev = &self.ctx.device;
        let vs = self.module(vs_spv)?;
        let ps = self.module(ps_spv)?;

        let pc_range = vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::ALL_GRAPHICS)
            .offset(0).size(crate::PC_BYTES as u32);
        let pl_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(set_layouts)
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
            if set != vk::DescriptorSet::null() {
                // SAFETY: valid command buffer/layout/set.
                unsafe { dev.cmd_bind_descriptor_sets(cmd, vk::PipelineBindPoint::GRAPHICS, layout, 0, std::slice::from_ref(&set), &[]) };
            }
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

/// A minimal crafted bindless set for a textured fullscreen draw: one sampled
/// image (binding 0), a nearest sampler (binding 1), and the FNV-1a descriptor
/// hash table (binding 2). The shader reads a T# at heap+0 and an S# at heap+32
/// (s2:s3 = 0), hashes them, and the table maps them to image/sampler index 0.
/// Reuses the bindless hash-table helpers; not a full descriptor runtime.
pub struct TexturedQuad<'a> {
    ctx: &'a Context,
    pub set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    pub set: vk::DescriptorSet,
    heap: crate::Heap<'a>,
    table_buffer: vk::Buffer,
    table_memory: vk::DeviceMemory,
    image: vk::Image,
    image_memory: vk::DeviceMemory,
    view: vk::ImageView,
    /// A black fallback image at bindless index 0 (negative-control mode only):
    /// a descriptor lookup that misses lands here, so a corrupted key or heap
    /// base samples black instead of the gradient at index 1.
    fallback: Option<(vk::Image, vk::DeviceMemory, vk::ImageView)>,
    sampler: vk::Sampler,
    pub width: u32,
    pub height: u32,
}

impl<'a> TexturedQuad<'a> {
    pub fn new(ctx: &'a Context, w: u32, h: u32) -> Result<Self> {
        Self::build(ctx, w, h, false)
    }

    /// Like `new`, but the gradient lives at bindless index 1 and a black
    /// fallback at index 0, with the T# mapped to index 1. A lookup miss (a
    /// corrupted descriptor key or a wrong heap base) then samples black, so the
    /// descriptor faults are observable. Used only by the negative controls.
    pub fn new_with_fallback(ctx: &'a Context, w: u32, h: u32) -> Result<Self> {
        Self::build(ctx, w, h, true)
    }

    fn build(ctx: &'a Context, w: u32, h: u32, with_fallback: bool) -> Result<Self> {
        use crate::bindless::{build_hash_table, tsharp_key, HEAP_SIZE};
        let dev = &ctx.device;
        let n_img = if with_fallback { 2 } else { 1 };
        let bindings_def = [
            (0u32, vk::DescriptorType::SAMPLED_IMAGE),
            (1, vk::DescriptorType::SAMPLER),
            (2, vk::DescriptorType::STORAGE_BUFFER),
        ];
        let binding_infos: Vec<vk::DescriptorSetLayoutBinding> = bindings_def.iter()
            .map(|&(binding, ty)| vk::DescriptorSetLayoutBinding::default()
                .binding(binding).descriptor_type(ty)
                .descriptor_count(if binding == 0 { n_img } else { 1 })
                .stage_flags(vk::ShaderStageFlags::FRAGMENT))
            .collect();
        let layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&binding_infos);
        // SAFETY: valid device/create info.
        let set_layout = unsafe { dev.create_descriptor_set_layout(&layout_info, None) }?;
        let pool_sizes: Vec<vk::DescriptorPoolSize> = bindings_def.iter()
            .map(|&(_, ty)| vk::DescriptorPoolSize::default().ty(ty)
                .descriptor_count(if ty == vk::DescriptorType::SAMPLED_IMAGE { n_img } else { 1 })).collect();
        let pool_info = vk::DescriptorPoolCreateInfo::default().max_sets(1).pool_sizes(&pool_sizes);
        // SAFETY: valid device/create info.
        let pool = unsafe { dev.create_descriptor_pool(&pool_info, None) }?;
        let layouts = [set_layout];
        let alloc = vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool).set_layouts(&layouts);
        // SAFETY: valid pool/layout.
        let set = unsafe { dev.allocate_descriptor_sets(&alloc) }?[0];

        // The sampled texture (RGBA8 UNORM) + nearest sampler.
        let fmt = vk::Format::R8G8B8A8_UNORM;
        let (image, image_memory, view) = ctx.make_image(fmt, w, h,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)?;
        // Negative-control mode: a black fallback image at index 0, cleared and
        // left shader-read so a lookup miss samples black.
        let fallback = if with_fallback {
            let (fi, fm, fv) = ctx.make_image(fmt, w, h,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)?;
            let range = vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 };
            ctx.one_shot(|cmd| {
                let to_dst = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::UNDEFINED).new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .image(fi).subresource_range(range)
                    .src_access_mask(vk::AccessFlags::empty()).dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
                // SAFETY: valid command buffer/barrier.
                unsafe { dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], std::slice::from_ref(&to_dst)) };
                let black = vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 0.0] };
                // SAFETY: valid command buffer/image in TRANSFER_DST.
                unsafe { dev.cmd_clear_color_image(cmd, fi, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &black, std::slice::from_ref(&range)) };
                let to_read = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL).new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .image(fi).subresource_range(range)
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE).dst_access_mask(vk::AccessFlags::SHADER_READ);
                // SAFETY: valid command buffer/barrier.
                unsafe { dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::FRAGMENT_SHADER, vk::DependencyFlags::empty(), &[], &[], std::slice::from_ref(&to_read)) };
                Ok(())
            })?;
            Some((fi, fm, fv))
        } else {
            None
        };
        let sci = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::NEAREST).min_filter(vk::Filter::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);
        // SAFETY: valid device/create info.
        let sampler = unsafe { dev.create_sampler(&sci, None) }?;

        // Heap: T# at +0, S# at +32 (the shader's s2:s3 = 0).
        let heap = ctx.heap(HEAP_SIZE)?;
        let tkey = tsharp_key(0x11111111);
        let skey4 = [0x92u32, 0xc00000, 0x14500000, 0];
        let mut skey = [0xFFFFFFFFu32; 8];
        skey[..4].copy_from_slice(&skey4);
        // SAFETY: heap mapped for HEAP_SIZE; offsets 0 and 32 within it.
        unsafe {
            std::ptr::copy_nonoverlapping(tkey.as_ptr() as *const u8, heap.ptr, 32);
            std::ptr::copy_nonoverlapping(skey4.as_ptr() as *const u8, heap.ptr.add(32), 16);
        }
        // The gradient is at index 1 in fallback mode (index 0 is the black
        // fallback); otherwise it is the only image, at index 0.
        let img_index = if with_fallback { 1 } else { 0 };
        let table = build_hash_table(&[(tkey, img_index), (skey, 0)], 64)?;
        let (table_buffer, table_memory, table_ptr) = ctx.make_buffer(table.len() * 4,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)?;
        // SAFETY: host-visible, table.len()*4 bytes.
        unsafe { std::ptr::copy_nonoverlapping(table.as_ptr() as *const u8, table_ptr, table.len() * 4) };

        // Bind image(s) (0), sampler (1), hash table (2). In fallback mode the
        // array is [black fallback (0), gradient (1)]; otherwise [gradient (0)].
        let gradient_info = vk::DescriptorImageInfo::default().image_view(view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let image_infos: Vec<vk::DescriptorImageInfo> = if let Some((_, _, fv)) = fallback {
            vec![
                vk::DescriptorImageInfo::default().image_view(fv).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
                gradient_info,
            ]
        } else {
            vec![gradient_info]
        };
        let sampler_info = vk::DescriptorImageInfo::default().sampler(sampler);
        let buffer_info = vk::DescriptorBufferInfo::default().buffer(table_buffer).offset(0).range((table.len() * 4) as u64);
        let writes = [
            vk::WriteDescriptorSet::default().dst_set(set).dst_binding(0).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&image_infos),
            vk::WriteDescriptorSet::default().dst_set(set).dst_binding(1).descriptor_type(vk::DescriptorType::SAMPLER).image_info(std::slice::from_ref(&sampler_info)),
            vk::WriteDescriptorSet::default().dst_set(set).dst_binding(2).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(std::slice::from_ref(&buffer_info)),
        ];
        // SAFETY: valid set/infos live for the call.
        unsafe { dev.update_descriptor_sets(&writes, &[]) };

        Ok(TexturedQuad { ctx, set_layout, pool, set, heap, table_buffer, table_memory, image, image_memory, view, fallback, sampler, width: w, height: h })
    }

    /// Upload RGBA8 texels and transition the image to shader-read.
    pub fn upload(&self, texels: &[u32]) -> Result<()> {
        let dev = &self.ctx.device;
        let bytes = texels.len() * 4;
        let (stage, stage_mem, ptr) = self.ctx.make_buffer(bytes, vk::BufferUsageFlags::TRANSFER_SRC,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)?;
        // SAFETY: host-visible, bytes valid.
        unsafe { std::ptr::copy_nonoverlapping(texels.as_ptr() as *const u8, ptr, bytes) };
        let image = self.image;
        let range = vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 };
        self.ctx.one_shot(|cmd| {
            let to_transfer = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::UNDEFINED).new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .image(image).subresource_range(range)
                .src_access_mask(vk::AccessFlags::empty()).dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
            // SAFETY: valid command buffer/barrier.
            unsafe { dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], std::slice::from_ref(&to_transfer)) };
            let region = vk::BufferImageCopy::default()
                .image_subresource(vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 })
                .image_extent(vk::Extent3D { width: self.width, height: self.height, depth: 1 });
            // SAFETY: valid command buffer/buffer/image.
            unsafe { dev.cmd_copy_buffer_to_image(cmd, stage, image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, std::slice::from_ref(&region)) };
            let to_read = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL).new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .image(image).subresource_range(range)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE).dst_access_mask(vk::AccessFlags::SHADER_READ);
            // SAFETY: valid command buffer/barrier.
            unsafe { dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::FRAGMENT_SHADER, vk::DependencyFlags::empty(), &[], &[], std::slice::from_ref(&to_read)) };
            Ok(())
        })?;
        // SAFETY: staging buffer/memory owned here.
        unsafe { dev.destroy_buffer(stage, None); dev.free_memory(stage_mem, None) };
        Ok(())
    }

    /// Push constants: s2:s3 = 0 (the descriptor table VA at heap+0).
    pub fn push_constants(&self) -> [u32; 36] {
        let mut pc = [0u32; 36];
        pc[2] = 0; pc[3] = 0;
        pc[32] = self.heap.address as u32;
        pc[33] = (self.heap.address >> 32) as u32;
        pc
    }

    /// Draw with a deliberate fault, for negative controls. The fault-free draw
    /// is the oracle; each fault must change the pixels. The textured quad reads
    /// its T#/S# descriptor keys from the heap (the "view buffer") and the heap
    /// base from the push constants, so all three faults perturb real inputs:
    /// SkipDraw leaves the clear, CorruptViewBuffer scrambles the descriptor
    /// keys, WrongPushConstants points the heap base elsewhere.
    pub fn draw_with_fault(&self, rt: &RenderTarget<'_>, vs: &[u8], ps: &[u8],
                           fault: crate::replay::Fault) -> Result<Vec<u32>> {
        use crate::replay::Fault;
        let mut pc = self.push_constants();
        pc[8] = 0; pc[9] = 0; pc[11] = 32; // T# at heap+0, S# at heap+32
        match fault {
            Fault::None => {}
            Fault::SkipDraw => return rt.clear_and_readback([0.0, 0.0, 0.0, 0.0]),
            Fault::WrongPushConstants => pc[32] = pc[32].wrapping_add(0x10000),
            Fault::CorruptViewBuffer => {
                // SAFETY: heap mapped for HEAP_SIZE; the first 12 words hold the
                // T# (8) and S# (4) keys the PS hashes. Scramble them in place.
                unsafe {
                    let p = self.heap.ptr as *mut u32;
                    for i in 0..12 {
                        *p.add(i) = (*p.add(i)).wrapping_mul(31).wrapping_add(17);
                    }
                }
            }
        }
        rt.draw_with_set(vs, ps, [0.0, 0.0, 0.0, 0.0],
            std::slice::from_ref(&self.set_layout), self.set, &pc)
    }
}

impl Drop for TexturedQuad<'_> {
    fn drop(&mut self) {
        // SAFETY: valid handles owned here.
        unsafe {
            self.ctx.device.destroy_sampler(self.sampler, None);
            self.ctx.device.destroy_image_view(self.view, None);
            self.ctx.device.destroy_image(self.image, None);
            self.ctx.device.free_memory(self.image_memory, None);
            if let Some((fi, fm, fv)) = self.fallback {
                self.ctx.device.destroy_image_view(fv, None);
                self.ctx.device.destroy_image(fi, None);
                self.ctx.device.free_memory(fm, None);
            }
            self.ctx.device.destroy_buffer(self.table_buffer, None);
            self.ctx.device.free_memory(self.table_memory, None);
            self.ctx.device.destroy_descriptor_pool(self.pool, None);
            self.ctx.device.destroy_descriptor_set_layout(self.set_layout, None);
        }
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