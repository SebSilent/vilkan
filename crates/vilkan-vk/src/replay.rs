//! Replay a backend-neutral captured draw through native Vulkan, offscreen.
//! Consumes the DrawRecord written by victory-gpu's draw capture (no D3D12
//! types): SPIR-V shaders, a root-view buffer placed in the heap, push
//! constants, a draw, and a render-target readback. The record's shaders must
//! already be translated to SPIR-V (the vendored translator) and exact on
//! wave32 (tier A/B).
//!
//! The execution core is record-driven ([`replay_record`]): it takes the neutral
//! record and its in-memory resources, not a directory. The folder-based
//! [`replay`] / [`replay_with_fault`] are convenience wrappers over it, and the
//! [`crate::host::VulkanBackend`] host-trait implementation drives the same core.

use anyhow::{Context as _, Result};
use ash::vk;

use crate::Context;

// The capture types and the fault model are backend-neutral and live in
// `vilkan-host` (the host-trait seam). Re-exported here so existing callers
// (`vilkan_vk::replay::{DrawRecord, Fault, ...}`) keep working unchanged.
pub use vilkan_host::{
    DrawRecord, DrawResources, Fault, ReplayResult, RenderTarget, RootBinding, ViewBuffer,
};

/// Replay a captured draw from a folder: SPIR-V VS/PS, root-view buffers in the
/// heap, draw, readback. `dir` holds draw.json, vs.spv, ps.spv and the view files.
pub fn replay(ctx: &Context, dir: &std::path::Path) -> Result<ReplayResult> {
    replay_with_fault(ctx, dir, Fault::None)
}

/// Folder convenience wrapper: load the record + resources, then run the
/// record-driven core with a fault.
pub fn replay_with_fault(ctx: &Context, dir: &std::path::Path, fault: Fault) -> Result<ReplayResult> {
    let (record, res) = vilkan_host::load_capture(dir)?;
    replay_record(ctx, &record, &res, fault)
}

/// The record-driven execution core: place the root-view buffers in the heap at
/// their console addresses, build the push constants, draw (or skip), read back.
/// This is what the host trait drives; no file paths here.
pub fn replay_record(
    ctx: &Context,
    rec: &DrawRecord,
    res: &DrawResources,
    fault: Fault,
) -> Result<ReplayResult> {
    let target = rec.targets.first().context("no render target")?;
    // The heap spans the console address window the shaders read; place each
    // root-view buffer at its console address.
    let mut heap_end: u64 = 0;
    for (address, data) in &res.views {
        heap_end = heap_end.max(address + data.len() as u64);
    }
    let heap_size = (heap_end.max(1 << 20)) as usize;
    let heap = ctx.heap(heap_size)?;
    for (address, data) in &res.views {
        let mut data = data.clone();
        if fault == Fault::CorruptViewBuffer {
            for b in data.iter_mut() {
                *b = b.wrapping_mul(31).wrapping_add(17);
            }
        }
        // SAFETY: the heap is mapped for heap_size; the buffer fits at its address.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), heap.ptr.add(*address as usize), data.len()) };
    }

    // Render at the captured size; the readback is float (R32G32B32A32_SFLOAT),
    // compared against the RGBA8 oracle with a stated tolerance.
    let rt = crate::graphics::RenderTarget::new(ctx, target.width, target.height)?;

    // Push constants: s0..s31 + heap base. The root view address goes in s8/s9
    // (the VS builds its buffer V# from there); the heap base at offset 128.
    let mut pc = [0u32; 36];
    for r in &rec.roots {
        if r.kind == "View"
            && let Some(a) = r.address
        {
            if fault == Fault::WrongPushConstants {
                // Wrong SGPRs: the VS reads s8/s9, so s4/s5 leaves it zero.
                pc[4] = a as u32;
                pc[5] = (a >> 32) as u32;
            } else {
                pc[8] = a as u32;
                pc[9] = (a >> 32) as u32;
            }
        }
    }
    pc[32] = heap.address as u32;
    pc[33] = (heap.address >> 32) as u32;

    let words = if fault == Fault::SkipDraw {
        // No draw: read back the cleared target.
        rt.clear_and_readback([0.0, 0.0, 0.0, 0.0])?
    } else {
        rt.draw_with_set(&res.vs_spirv, &res.ps_spirv, [0.0, 0.0, 0.0, 0.0], &[], vk::DescriptorSet::null(), &pc)?
    };
    let pixels: Vec<f32> = words.iter().map(|&w| f32::from_bits(w)).collect();
    Ok(ReplayResult { width: target.width, height: target.height, pixels })
}