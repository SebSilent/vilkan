//! Minimal crafted bindless descriptor setup for the blur compute shaders.
//!
//! Follows the reference Vulkan bindless layout (set 0): binding 0 sampled 2D
//! images, 1 samplers, 2 the FNV-1a descriptor hash table (one storage buffer),
//! 8/9/10 2D storage images by component class (float/uint/sint). For a test
//! dispatch we control the heap contents: crafted T#/S# keys in a descriptor
//! table, a hand-built hash table mapping key -> bindless index, one input
//! texture, a sampler, and output UAVs. Not a full descriptor runtime.

use anyhow::{Result, bail};
use ash::vk;

use crate::Context;

/// FNV-1a over the 8-dword descriptor key (the reference constants).
pub fn hash_key(key: &[u32; 8]) -> u32 {
    let mut h: u32 = 2166136261;
    for &w in key {
        h = (h ^ w).wrapping_mul(16777619);
    }
    h
}

/// A T# key: 8 dwords; word 1 [28:20] is the FORMAT (56 = 8_8_8_8_UNORM).
pub fn tsharp_key(tag: u32) -> [u32; 8] {
    let word1 = (0x22222222u32 & !(0x1FFu32 << 20)) | (56u32 << 20);
    [tag, word1, 0x33333333, 0x44444444, 0x55555555, 0x66666666, 0x77777777, 0x88888888]
}

/// The literal S# the blur constructs in-shader (s0..s3), padded to 8 dwords.
pub fn blur_ssharp_key() -> [u32; 8] {
    [0x92, 0xc00000, 0x14500000, 0, 0xFFFFFFFF, 0xFFFFFFFF, 0xFFFFFFFF, 0xFFFFFFFF]
}

/// Build the GPU hash-table buffer: word 0 = mask, then (mask+1) slots of
/// 10 dwords (key[8], index, pad). Insert each key at (hash & mask), probing
/// up to 8 slots on collision.
pub fn build_hash_table(entries: &[([u32; 8], u32)], capacity: u32) -> Result<Vec<u32>> {
    let mask = capacity - 1;
    let mut out = vec![0u32; 1 + (capacity as usize) * 10];
    out[0] = mask;
    for &(key, index) in entries {
        let h = hash_key(&key);
        let mut placed = false;
        for k in 0..8u32 {
            let slot = (h.wrapping_add(k)) & mask;
            let base = 1 + (slot as usize) * 10;
            if out[base + 9] == 0 {
                out[base..base + 8].copy_from_slice(&key);
                out[base + 8] = index;
                out[base + 9] = 0xA11CA7ED;
                placed = true;
                break;
            }
        }
        if !placed {
            bail!("hash table full for key {key:?}");
        }
    }
    Ok(out)
}

/// The crafted bindless set + resources for one blur dispatch.
pub struct BlurSetup<'a> {
    ctx: &'a Context,
    pub set_layout: vk::DescriptorSetLayout,
    pub pool: vk::DescriptorPool,
    pub set: vk::DescriptorSet,
    pub pipeline_layout: vk::PipelineLayout,
    pub heap: crate::Heap<'a>,
    pub table_buffer: vk::Buffer,
    pub table_memory: vk::DeviceMemory,
    images: Vec<vk::Image>,
    memories: Vec<vk::DeviceMemory>,
    views: Vec<vk::ImageView>,
    sampler: vk::Sampler,
    pub input_view: vk::ImageView,
    pub output_views: Vec<vk::ImageView>,
    pub width: u32,
    pub height: u32,
}

impl Context {
    fn make_buffer(&self, size: usize, usage: vk::BufferUsageFlags, flags: vk::MemoryPropertyFlags)
        -> Result<(vk::Buffer, vk::DeviceMemory, *mut u8)>
    {
        let bi = vk::BufferCreateInfo::default().size(size as u64).usage(usage).sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: valid device/create info.
        let buffer = unsafe { self.device.create_buffer(&bi, None) }?;
        // SAFETY: valid buffer.
        let req = unsafe { self.device.get_buffer_memory_requirements(buffer) };
        let mt = self.mem_type(req.memory_type_bits, flags)?;
        let ai = vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(mt);
        // SAFETY: valid device/allocate info.
        let memory = unsafe { self.device.allocate_memory(&ai, None) }?;
        // SAFETY: valid buffer/memory.
        unsafe { self.device.bind_buffer_memory(buffer, memory, 0) }?;
        let ptr = if flags.contains(vk::MemoryPropertyFlags::HOST_VISIBLE) {
            // SAFETY: host-visible memory.
            unsafe { self.device.map_memory(memory, 0, req.size, vk::MemoryMapFlags::empty()) }? as *mut u8
        } else {
            std::ptr::null_mut()
        };
        Ok((buffer, memory, ptr))
    }

    fn make_image(&self, format: vk::Format, w: u32, h: u32, usage: vk::ImageUsageFlags)
        -> Result<(vk::Image, vk::DeviceMemory, vk::ImageView)>
    {
        let ii = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D { width: w, height: h, depth: 1 })
            .mip_levels(1).array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: valid device/create info.
        let image = unsafe { self.device.create_image(&ii, None) }?;
        // SAFETY: valid image.
        let req = unsafe { self.device.get_image_memory_requirements(image) };
        let mt = self.mem_type(req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)?;
        let ai = vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(mt);
        // SAFETY: valid device/allocate info.
        let memory = unsafe { self.device.allocate_memory(&ai, None) }?;
        // SAFETY: valid image/memory.
        unsafe { self.device.bind_image_memory(image, memory, 0) }?;
        let vi = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1,
                base_array_layer: 0, layer_count: 1,
            });
        // SAFETY: valid image/create info.
        let view = unsafe { self.device.create_image_view(&vi, None) }?;
        Ok((image, memory, view))
    }
}

/// The descriptor-table offset in the heap (the shader's s[4:5] points here).
pub const TABLE_OFF: usize = 0x100;
/// Heap size.
pub const HEAP_SIZE: usize = 65536;

impl<'a> BlurSetup<'a> {
    /// Craft the bindless set for a blur: one input texture (W×H, RGBA8 UNORM),
    /// a nearest/clamp sampler, and `n_out` output UAVs (RGBA8 UNORM storage).
    pub fn new(ctx: &'a Context, w: u32, h: u32, n_out: usize) -> Result<BlurSetup<'a>> {
        let dev = &ctx.device;
        // Bindings 0/1/2/8/9/10, runtime arrays (variable count, partially bound).
        let counts = [16u32, 16, 1, 16, 16, 16];
        let bindings_def = [
            (0u32, vk::DescriptorType::SAMPLED_IMAGE),
            (1, vk::DescriptorType::SAMPLER),
            (2, vk::DescriptorType::STORAGE_BUFFER),
            (8, vk::DescriptorType::STORAGE_IMAGE),
            (9, vk::DescriptorType::STORAGE_IMAGE),
            (10, vk::DescriptorType::STORAGE_IMAGE),
        ];
        let mut binding_infos: Vec<vk::DescriptorSetLayoutBinding> = Vec::new();
        for (i, &(binding, ty)) in bindings_def.iter().enumerate() {
            binding_infos.push(vk::DescriptorSetLayoutBinding::default()
                .binding(binding)
                .descriptor_type(ty)
                .descriptor_count(counts[i])
                .stage_flags(vk::ShaderStageFlags::COMPUTE));
        }
        let mut flags = vec![vk::DescriptorBindingFlags::PARTIALLY_BOUND; 6];
        flags[0] |= vk::DescriptorBindingFlags::VARIABLE_DESCRIPTOR_COUNT;
        let mut bflags = vk::DescriptorSetLayoutBindingFlagsCreateInfo::default()
            .binding_flags(&flags);
        let layout_info = vk::DescriptorSetLayoutCreateInfo::default()
            .bindings(&binding_infos)
            .push_next(&mut bflags);
        // SAFETY: valid device/create info.
        let set_layout = unsafe { dev.create_descriptor_set_layout(&layout_info, None) }?;

        let pool_sizes: Vec<vk::DescriptorPoolSize> = bindings_def.iter().enumerate()
            .map(|(i, &(_, ty))| vk::DescriptorPoolSize::default().ty(ty).descriptor_count(counts[i]))
            .collect();
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(1)
            .pool_sizes(&pool_sizes)
            .flags(vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND);
        // SAFETY: valid device/create info.
        let pool = unsafe { dev.create_descriptor_pool(&pool_info, None) }?;

        let layouts = [set_layout];
        let mut variable = vk::DescriptorSetVariableDescriptorCountAllocateInfo::default()
            .descriptor_counts(&counts[..1]);
        let alloc = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&layouts)
            .push_next(&mut variable);
        // SAFETY: valid pool/layout.
        let set = unsafe { dev.allocate_descriptor_sets(&alloc) }?[0];

        let pc_range = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE).offset(0).size(crate::PC_BYTES as u32)];
        let pl_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&layouts)
            .push_constant_ranges(&pc_range);
        // SAFETY: valid device/layout.
        let pipeline_layout = unsafe { dev.create_pipeline_layout(&pl_info, None) }?;

        // Input texture + output UAVs (RGBA8 UNORM).
        let fmt = vk::Format::R8G8B8A8_UNORM;
        let (input_image, input_mem, input_view) = ctx.make_image(fmt, w, h,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)?;
        let mut images = vec![input_image];
        let mut memories = vec![input_mem];
        let mut views = vec![input_view];
        let mut output_views = Vec::new();
        for _ in 0..n_out {
            let (img, mem, view) = ctx.make_image(fmt, w, h,
                vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST)?;
            images.push(img);
            memories.push(mem);
            views.push(view);
            output_views.push(view);
        }

        // Sampler: nearest, clamp to edge.
        let sci = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::NEAREST).min_filter(vk::Filter::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);
        // SAFETY: valid device/create info.
        let sampler = unsafe { dev.create_sampler(&sci, None) }?;

        // Heap: zeros, descriptor table at TABLE_OFF (input T# +0, outputs +0x20..,
        // V# +0xa0). The hash table is a separate storage buffer (binding 2).
        let heap = ctx.heap(HEAP_SIZE)?;

        // Craft keys: input -> binding 0 index 0; outputs -> binding 8 indices 0..n;
        // the literal S# -> binding 1 index 0.
        let input_key = tsharp_key(0x11111111);
        let mut entries: Vec<([u32; 8], u32)> = vec![(input_key, 0), (blur_ssharp_key(), 0)];
        let mut out_keys = Vec::new();
        for i in 0..n_out {
            let k = tsharp_key(0x20000000 + i as u32);
            out_keys.push(k);
            entries.push((k, i as u32));
        }
        let table = build_hash_table(&entries, 64)?;
        let (table_buffer, table_memory, table_ptr) = ctx.make_buffer(table.len() * 4,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)?;
        // SAFETY: host-visible, table.len()*4 bytes.
        unsafe { std::ptr::copy_nonoverlapping(table.as_ptr() as *const u8, table_ptr, table.len() * 4) };

        // Write the descriptor table into the heap: input T# at +0, output T#s at
        // +0x20*i, V# at +0xa0 (base 0, stride 0, num_records max, raw).
        // SAFETY: heap mapped for HEAP_SIZE; offsets within it.
        unsafe {
            let hp = heap.ptr;
            let put = |off: usize, words: &[u32]| {
                std::ptr::copy_nonoverlapping(words.as_ptr() as *const u8, hp.add(TABLE_OFF + off), words.len() * 4);
            };
            put(0x00, &input_key);
            for (i, k) in out_keys.iter().enumerate() {
                put(0x20 * (i + 1), k);
            }
            // V# for the scale-factor buffer load: base = a heap offset holding
            // four float scales (1.0), stride 0, num_records 4, raw.
            let scale_off = TABLE_OFF + 0x200;
            // The shader samples at (pixel + 0.5) * scale: for an 8x8 texture
            // the scale is 1/8 (texel centres, normalized), and s18 is the LOD
            // (0 = mip 0). s19 is unused here.
            let scales = [0.125f32, 0.125, 0.0, 0.0];
            std::ptr::copy_nonoverlapping(scales.as_ptr() as *const u8, hp.add(scale_off), 16);
            // Raw V# (OOB_SELECT 3): num_records is a BYTE bound, so 4 dwords
            // need 16, not 4 (else s17/s18/s19 read out of bounds as 0).
            let vsharp = [scale_off as u32, 0, 16, 0x31016FAC];
            put(0xa0, &vsharp);
        }

        // Descriptor writes: binding 0[0] = input view, binding 1[0] = sampler,
        // binding 2[0] = table buffer, binding 8[i] = output view i.
        let image_info = [vk::DescriptorImageInfo::default()
            .image_view(input_view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let sampler_info = [vk::DescriptorImageInfo::default().sampler(sampler)];
        let buffer_info = [vk::DescriptorBufferInfo::default().buffer(table_buffer).offset(0).range(vk::WHOLE_SIZE)];
        let out_infos: Vec<vk::DescriptorImageInfo> = output_views.iter()
            .map(|&v| vk::DescriptorImageInfo::default().image_view(v).image_layout(vk::ImageLayout::GENERAL))
            .collect();
        let writes = [
            vk::WriteDescriptorSet::default().dst_set(set).dst_binding(0).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&image_info),
            vk::WriteDescriptorSet::default().dst_set(set).dst_binding(1).descriptor_type(vk::DescriptorType::SAMPLER).image_info(&sampler_info),
            vk::WriteDescriptorSet::default().dst_set(set).dst_binding(2).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&buffer_info),
            vk::WriteDescriptorSet::default().dst_set(set).dst_binding(8).descriptor_type(vk::DescriptorType::STORAGE_IMAGE).image_info(&out_infos),
        ];
        // SAFETY: valid set, infos live for the call.
        unsafe { dev.update_descriptor_sets(&writes, &[]) };

        Ok(BlurSetup {
            ctx, set_layout, pool, set, pipeline_layout, heap,
            table_buffer, table_memory, images, memories, views, sampler,
            input_view, output_views, width: w, height: h,
        })
    }
}

impl Context {
    /// Run a one-shot command buffer on the compute queue and wait for it.
    pub(crate) fn one_shot<F>(&self, f: F) -> Result<()>
    where F: FnOnce(vk::CommandBuffer) -> Result<()> {
        let dev = &self.device;
        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(self.queue_family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // SAFETY: valid device/queue family.
        let pool = unsafe { dev.create_command_pool(&pool_info, None) }?;
        let ai = vk::CommandBufferAllocateInfo::default()
            .command_pool(pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1);
        // SAFETY: valid pool.
        let cmd = unsafe { dev.allocate_command_buffers(&ai) }?[0];
        let bi = vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: valid command buffer.
        unsafe { dev.begin_command_buffer(cmd, &bi) }?;
        f(cmd)?;
        // SAFETY: valid command buffer.
        unsafe { dev.end_command_buffer(cmd) }?;
        let cmds = [cmd];
        let si = vk::SubmitInfo::default().command_buffers(&cmds);
        // SAFETY: valid queue/command buffer.
        unsafe { dev.queue_submit(self.queue, &[si], vk::Fence::null()) }?;
        // SAFETY: valid queue.
        unsafe { dev.queue_wait_idle(self.queue) }?;
        // SAFETY: valid pool.
        unsafe { dev.destroy_command_pool(pool, None) };
        Ok(())
    }
}

impl<'a> BlurSetup<'a> {
    /// Upload RGBA8 texels into the input texture (row-major, w*h words).
    pub fn upload_input(&self, texels: &[u32]) -> Result<()> {
        let dev = &self.ctx.device;
        let bytes = texels.len() * 4;
        let (stage, stage_mem, ptr) = self.ctx.make_buffer(bytes,
            vk::BufferUsageFlags::TRANSFER_SRC,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)?;
        // SAFETY: host-visible, bytes valid.
        unsafe { std::ptr::copy_nonoverlapping(texels.as_ptr() as *const u8, ptr, bytes) };
        let input_image = self.images[0];
        self.ctx.one_shot(|cmd| {
            let to_transfer = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::UNDEFINED).new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .image(input_image)
                .subresource_range(vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 })
                .src_access_mask(vk::AccessFlags::empty()).dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
            // SAFETY: valid command buffer/barrier.
            unsafe { dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], &[to_transfer]) };
            let region = vk::BufferImageCopy::default()
                .image_subresource(vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 })
                .image_extent(vk::Extent3D { width: self.width, height: self.height, depth: 1 });
            // SAFETY: valid command buffer/buffer/image.
            unsafe { dev.cmd_copy_buffer_to_image(cmd, stage, input_image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region]) };
            let to_read = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL).new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .image(input_image)
                .subresource_range(vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 })
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE).dst_access_mask(vk::AccessFlags::SHADER_READ);
            // SAFETY: valid command buffer/barrier.
            unsafe { dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::COMPUTE_SHADER, vk::DependencyFlags::empty(), &[], &[], &[to_read]) };
            Ok(())
        })?;
        // SAFETY: staging buffer/memory owned here.
        unsafe { dev.destroy_buffer(stage, None); dev.free_memory(stage_mem, None) };
        Ok(())
    }

    /// Dispatch the blur SPIR-V over `groups` workgroups.
    pub fn dispatch(&self, spirv: &[u8], groups: [u32; 3]) -> Result<()> {
        let dev = &self.ctx.device;
        let words: Vec<u32> = spirv.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let smi = vk::ShaderModuleCreateInfo::default().code(&words);
        // SAFETY: valid device/code.
        let module = unsafe { dev.create_shader_module(&smi, None) }?;
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE).module(module).name(c"main");
        let cpi = vk::ComputePipelineCreateInfo::default().stage(stage).layout(self.pipeline_layout);
        // SAFETY: valid device/layout/stage.
        let pipeline = unsafe { dev.create_compute_pipelines(vk::PipelineCache::null(), &[cpi], None) }
            .map_err(|e| anyhow::anyhow!("create blur pipeline: {:?}", e.1))?[0];
        // Output images to GENERAL for storage writes.
        let out_barriers: Vec<vk::ImageMemoryBarrier> = self.images[1..].iter().map(|&img| {
            vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::UNDEFINED).new_layout(vk::ImageLayout::GENERAL)
                .image(img)
                .subresource_range(vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 })
                .src_access_mask(vk::AccessFlags::empty()).dst_access_mask(vk::AccessFlags::SHADER_WRITE)
        }).collect();
        self.ctx.one_shot(|cmd| {
            // SAFETY: valid command buffer/barriers.
            unsafe { dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::COMPUTE_SHADER, vk::DependencyFlags::empty(), &[], &[], &out_barriers) };
            // SAFETY: valid command buffer/pipeline.
            unsafe { dev.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipeline) };
            let sets = [self.set];
            // SAFETY: valid command buffer/layout/set.
            unsafe { dev.cmd_bind_descriptor_sets(cmd, vk::PipelineBindPoint::COMPUTE, self.pipeline_layout, 0, &sets, &[]) };
            // Push constants: s[4:5] = descriptor table VA; heap base at 32/33.
            let mut pc = [0u32; 36];
            pc[4] = TABLE_OFF as u32; pc[5] = 0;
            pc[32] = self.heap.address as u32; pc[33] = (self.heap.address >> 32) as u32;
            let pc_bytes: &[u8] = unsafe { std::slice::from_raw_parts(pc.as_ptr() as *const u8, crate::PC_BYTES) };
            // SAFETY: valid command buffer/layout, pc bytes live.
            unsafe { dev.cmd_push_constants(cmd, self.pipeline_layout, vk::ShaderStageFlags::COMPUTE, 0, pc_bytes) };
            // SAFETY: valid command buffer.
            unsafe { dev.cmd_dispatch(cmd, groups[0], groups[1], groups[2]) };
            Ok(())
        })?;
        // SAFETY: module/pipeline owned here.
        unsafe { dev.destroy_pipeline(pipeline, None); dev.destroy_shader_module(module, None) };
        Ok(())
    }

    /// Read output UAV `idx` back as RGBA8 texels (row-major).
    pub fn read_output(&self, idx: usize) -> Result<Vec<u32>> {
        let dev = &self.ctx.device;
        let img = self.images[1 + idx];
        let n = (self.width * self.height) as usize;
        let (readback, rb_mem, ptr) = self.ctx.make_buffer(n * 4,
            vk::BufferUsageFlags::TRANSFER_DST,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)?;
        self.ctx.one_shot(|cmd| {
            let to_transfer = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::GENERAL).new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .image(img)
                .subresource_range(vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 })
                .src_access_mask(vk::AccessFlags::SHADER_WRITE).dst_access_mask(vk::AccessFlags::TRANSFER_READ);
            // SAFETY: valid command buffer/barrier.
            unsafe { dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::COMPUTE_SHADER, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], &[to_transfer]) };
            let region = vk::BufferImageCopy::default()
                .image_subresource(vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 })
                .image_extent(vk::Extent3D { width: self.width, height: self.height, depth: 1 });
            // SAFETY: valid command buffer/image/buffer.
            unsafe { dev.cmd_copy_image_to_buffer(cmd, img, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, readback, &[region]) };
            Ok(())
        })?;
        // SAFETY: host-visible, n words.
        let out = unsafe { std::slice::from_raw_parts(ptr as *const u32, n) }.to_vec();
        // SAFETY: readback buffer/memory owned here.
        unsafe { dev.destroy_buffer(readback, None); dev.free_memory(rb_mem, None) };
        Ok(out)
    }
}

impl Drop for BlurSetup<'_> {
    fn drop(&mut self) {
        let dev = &self.ctx.device;
        // SAFETY: handles owned by this setup.
        unsafe {
            dev.destroy_sampler(self.sampler, None);
            for &v in &self.views { dev.destroy_image_view(v, None); }
            for &i in &self.images { dev.destroy_image(i, None); }
            for &m in &self.memories { dev.free_memory(m, None); }
            dev.destroy_buffer(self.table_buffer, None);
            dev.free_memory(self.table_memory, None);
            dev.destroy_pipeline_layout(self.pipeline_layout, None);
            dev.destroy_descriptor_pool(self.pool, None);
            dev.destroy_descriptor_set_layout(self.set_layout, None);
        }
    }
}