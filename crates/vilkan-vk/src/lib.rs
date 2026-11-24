//! Linux-native Vulkan runtime for sidecar-compiled SPIR-V compute shaders.
//!
//! No Proton, no D3D12: this owns the Vulkan device, pipelines and dispatch
//! directly on Linux (ash). The SPIR-V is compiled offline by the reference translator's
//! clean-room RDNA2 translator (`rdna2spirv.py`, GPL-3.0) with `--wave64-emulate`,
//! so wave64 console programs run exactly on a 32-lane host with no subgroup
//! operations. The binding contract is the reference translator's (`docs/design.md`): 144 push
//! constant bytes = s0..s31 (128) + heap base lo/hi + code base lo/hi; a single
//! "heap" buffer at Xbox VA 0, the output V# in s[0:3].

use anyhow::{Context as _, Result, bail};
use ash::vk;

pub mod bindless;

/// The push-constant block: s0..s31, heap base, code base (36 dwords = 144 B).
pub const PC_BYTES: usize = 144;
/// The output V# (s[0:3]): base 0, stride 0, num_records max, OOB_SELECT 3 (raw).
pub const OUTPUT_VSHARP: [u32; 4] = [0, 0, 0xFFFF_FFFF, 0x3101_6FAC];

pub struct Context {
    _entry: ash::Entry,
    instance: ash::Instance,
    pub device: ash::Device,
    pub physical: vk::PhysicalDevice,
    pub queue: vk::Queue,
    pub queue_family: u32,
    pub subgroup_size: u32,
    pub device_name: String,
    pub(crate) mem_props: vk::PhysicalDeviceMemoryProperties,
}

impl Context {
    /// Create a device on the first discrete GPU (the RTX 2070 here), with the
    /// features the SPIR-V needs: buffer device address, shader int64,
    /// and the float-controls denorm/signed-zero preserve modes.
    pub fn new() -> Result<Context> {
        // Loads the system Vulkan loader (safe in ash 0.38).
        let entry = ash::Entry::linked();
        let app = vk::ApplicationInfo::default()
            .application_name(c"vilkan-vk")
            .api_version(vk::API_VERSION_1_3);
        let inst_info = vk::InstanceCreateInfo::default().application_info(&app);
        // SAFETY: valid create info, no dangling pointers.
        let instance = unsafe { entry.create_instance(&inst_info, None) }
            .context("create Vulkan instance (is the loader/driver present?)")?;

        // SAFETY: valid instance handle.
        let physicals = unsafe { instance.enumerate_physical_devices() }?;
        let mut chosen = None;
        for &p in &physicals {
            // SAFETY: valid physical device.
            let props = unsafe { instance.get_physical_device_properties(p) };
            if props.device_type == vk::PhysicalDeviceType::DISCRETE_GPU {
                chosen = Some((p, props));
                break;
            }
        }
        let (physical, props) = chosen
            .or_else(|| physicals.first().map(|&p| {
                // SAFETY: valid physical device.
                (p, unsafe { instance.get_physical_device_properties(p) })
            }))
            .context("no Vulkan physical device")?;
        let device_name = props.device_name_as_c_str()
            .map(|c| c.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "unknown".into());

        // Subgroup size (log it: the wave64 emulation target is a 32-lane host).
        let mut sg = vk::PhysicalDeviceSubgroupProperties::default();
        let mut props2 = vk::PhysicalDeviceProperties2::default().push_next(&mut sg);
        // SAFETY: valid physical device, chained properties struct.
        unsafe { instance.get_physical_device_properties2(physical, &mut props2) };
        let subgroup_size = sg.subgroup_size;

        // Queue family with compute.
        // SAFETY: valid physical device.
        let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
        let queue_family = families.iter().position(|f| f.queue_flags.contains(vk::QueueFlags::COMPUTE))
            .context("no compute queue family")? as u32;
        let priority = [1.0f32];
        let qci = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family)
            .queue_priorities(&priority)];

        // Features: Vulkan 1.2 buffer device address; base shader int64. The
        // float-controls preserve modes the SPIR-V declares are accepted by the
        // driver without a feature enable (as in the reference harness).
        let mut f12q = vk::PhysicalDeviceVulkan12Features::default();
        let mut f2q = vk::PhysicalDeviceFeatures2::default().push_next(&mut f12q);
        // SAFETY: valid physical device, chained feature structs.
        unsafe { instance.get_physical_device_features2(physical, &mut f2q) };
        if f12q.buffer_device_address != vk::TRUE {
            bail!("device lacks bufferDeviceAddress (needed for the heap)");
        }
        // Descriptor indexing for the bindless set lives on Vulkan12Features
        // (runtime arrays of images / samplers / storage images, partially
        // bound, update-after-bind).
        let mut f12 = vk::PhysicalDeviceVulkan12Features::default()
            .buffer_device_address(true)
            .runtime_descriptor_array(true)
            .descriptor_binding_variable_descriptor_count(true)
            .descriptor_binding_partially_bound(true)
            .descriptor_binding_sampled_image_update_after_bind(true)
            .descriptor_binding_storage_image_update_after_bind(true)
            .descriptor_binding_storage_buffer_update_after_bind(true)
            .shader_sampled_image_array_non_uniform_indexing(true)
            .shader_storage_image_array_non_uniform_indexing(true)
            .shader_storage_buffer_array_non_uniform_indexing(true);
        let mut f2 = vk::PhysicalDeviceFeatures2::default().push_next(&mut f12);
        f2.features.shader_int64 = vk::TRUE;
        f2.features.shader_storage_image_array_dynamic_indexing = vk::TRUE;
        f2.features.shader_sampled_image_array_dynamic_indexing = vk::TRUE;

        let dev_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&qci)
            .push_next(&mut f2);
        // SAFETY: valid physical device and create info.
        let device = unsafe { instance.create_device(physical, &dev_info, None) }
            .context("create Vulkan device")?;
        // SAFETY: valid device, queue family/index.
        let queue = unsafe { device.get_device_queue(queue_family, 0) };
        // SAFETY: valid physical device.
        let mem_props = unsafe { instance.get_physical_device_memory_properties(physical) };

        Ok(Context {
            _entry: entry, instance, device, physical, queue, queue_family,
            subgroup_size, device_name, mem_props,
        })
    }

    pub(crate) fn mem_type(&self, type_bits: u32, flags: vk::MemoryPropertyFlags) -> Result<u32> {
        for i in 0..self.mem_props.memory_type_count {
            let suitable = type_bits & (1 << i) != 0
                && self.mem_props.memory_types[i as usize].property_flags.contains(flags);
            if suitable {
                return Ok(i);
            }
        }
        bail!("no suitable memory type for the heap")
    }

    /// A host-visible heap buffer with a device address, zero-filled.
    pub fn heap(&self, bytes: usize) -> Result<Heap<'_>> {
        let buf_info = vk::BufferCreateInfo::default()
            .size(bytes as u64)
            .usage(vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: valid device and create info.
        let buffer = unsafe { self.device.create_buffer(&buf_info, None) }?;
        // SAFETY: valid buffer.
        let req = unsafe { self.device.get_buffer_memory_requirements(buffer) };
        let mem_type = self.mem_type(req.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)?;
        let alloc = vk::MemoryAllocateInfo::default()
            .allocation_size(req.size)
            .memory_type_index(mem_type);
        let mut flags = vk::MemoryAllocateFlagsInfo::default()
            .flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
        let alloc = alloc.push_next(&mut flags);
        // SAFETY: valid device, allocate info with device-address flag.
        let memory = unsafe { self.device.allocate_memory(&alloc, None) }?;
        // SAFETY: valid buffer/memory, offset 0.
        unsafe { self.device.bind_buffer_memory(buffer, memory, 0) }?;
        let addr_info = vk::BufferDeviceAddressInfo::default().buffer(buffer);
        // SAFETY: valid buffer with device-address usage.
        let address = unsafe { self.device.get_buffer_device_address(&addr_info) };
        // SAFETY: host-visible memory, map the whole allocation.
        let ptr = unsafe { self.device.map_memory(memory, 0, req.size, vk::MemoryMapFlags::empty()) }?;
        // SAFETY: ptr is valid for req.size bytes; zero-fill.
        unsafe { std::ptr::write_bytes(ptr as *mut u8, 0, req.size as usize) };
        Ok(Heap { buffer, memory, address, ptr: ptr as *mut u8, size: req.size as usize, device: &self.device })
    }

    /// Dispatch a SPIR-V compute module over `groups` workgroups, with the heap
    /// device address in the push constants. Returns after the queue is idle.
    pub fn dispatch(&self, spirv: &[u8], heap: &Heap, groups: [u32; 3], sgprs: &[u32]) -> Result<()> {
        if spirv.len() % 4 != 0 {
            bail!("SPIR-V length not a multiple of 4");
        }
        let words: Vec<u32> = spirv.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let sm_info = vk::ShaderModuleCreateInfo::default().code(&words);
        // SAFETY: valid device, code slice lives for the call.
        let module = unsafe { self.device.create_shader_module(&sm_info, None) }?;

        let pc_range = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .offset(0)
            .size(PC_BYTES as u32)];
        let pl_info = vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&pc_range);
        // SAFETY: valid device, push-constant range slice.
        let layout = unsafe { self.device.create_pipeline_layout(&pl_info, None) }?;

        let entry = c"main";
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(module)
            .name(entry);
        let pipe_info = vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .layout(layout);
        // SAFETY: valid device, stage/layout handles, create info.
        let pipeline = unsafe { self.device.create_compute_pipelines(vk::PipelineCache::null(), &[pipe_info], None) }
            .map_err(|e| anyhow::anyhow!("create compute pipeline: {:?}", e.1))?;
        let pipeline = pipeline[0];

        let cmd_pool = vk::CommandPoolCreateInfo::default()
            .queue_family_index(self.queue_family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // SAFETY: valid device, queue family.
        let pool = unsafe { self.device.create_command_pool(&cmd_pool, None) }?;
        let alloc_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: valid pool.
        let cmd = unsafe { self.device.allocate_command_buffers(&alloc_info) }?[0];
        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: valid command buffer.
        unsafe { self.device.begin_command_buffer(cmd, &begin) }?;

        // Push constants: s0..s31 (output V# + user SGPRs), heap base, code base.
        let mut pc = [0u32; 36];
        pc[0..4].copy_from_slice(&OUTPUT_VSHARP);
        for (i, &w) in sgprs.iter().take(28).enumerate() {
            pc[4 + i] = w;
        }
        pc[32] = heap.address as u32;
        pc[33] = (heap.address >> 32) as u32;
        pc[34] = 0;
        pc[35] = 0;
        let pc_bytes: &[u8] = bytemuck_cast(&pc);
        // SAFETY: valid command buffer/layout, pc bytes live for the call.
        unsafe { self.device.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::COMPUTE, 0, pc_bytes) };
        // SAFETY: valid command buffer, pipeline.
        unsafe { self.device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipeline) };
        // SAFETY: valid command buffer; dispatch within device limits.
        unsafe { self.device.cmd_dispatch(cmd, groups[0], groups[1], groups[2]) };
        // SAFETY: valid command buffer.
        unsafe { self.device.end_command_buffer(cmd) }?;

        let cmds = [cmd];
        let submit = vk::SubmitInfo::default().command_buffers(&cmds);
        // SAFETY: valid queue, command buffer recorded.
        unsafe { self.device.queue_submit(self.queue, &[submit], vk::Fence::null()) }?;
        // SAFETY: valid queue.
        unsafe { self.device.queue_wait_idle(self.queue) }?;

        // SAFETY: valid handles created above.
        unsafe {
            self.device.destroy_command_pool(pool, None);
            self.device.destroy_pipeline(pipeline, None);
            self.device.destroy_pipeline_layout(layout, None);
            self.device.destroy_shader_module(module, None);
        }
        Ok(())
    }
}

/// Reinterpret a `[u32; 36]` as bytes without a dependency (little-endian host).
fn bytemuck_cast(pc: &[u32; 36]) -> &[u8] {
    // SAFETY: u32 has no padding; the slice is valid for 36*4 bytes; x86 is LE.
    unsafe { std::slice::from_raw_parts(pc.as_ptr() as *const u8, PC_BYTES) }
}

pub struct Heap<'a> {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    pub address: u64,
    pub(crate) ptr: *mut u8,
    pub size: usize,
    device: &'a ash::Device,
}

impl Heap<'_> {
    /// Read `n` u32 words from the heap (after a dispatch + queue idle).
    pub fn read_words(&self, n: usize) -> Vec<u32> {
        // SAFETY: host-coherent memory mapped for self.size; n words within it.
        unsafe {
            let p = self.ptr as *const u32;
            (0..n).map(|i| std::ptr::read_volatile(p.add(i))).collect()
        }
    }
}

impl Drop for Heap<'_> {
    fn drop(&mut self) {
        // SAFETY: valid buffer/memory owned by this heap.
        unsafe {
            self.device.unmap_memory(self.memory);
            self.device.destroy_buffer(self.buffer, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: valid device/instance owned by this context.
        unsafe {
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}