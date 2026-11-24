//! Replay a backend-neutral captured draw through native Vulkan, offscreen.
//! Consumes the DrawRecord written by victory-gpu's draw capture (no D3D12
//! types): SPIR-V shaders, a root-view buffer placed in the heap, push
//! constants, a draw, and a render-target readback. The record's shaders must
//! already be translated to SPIR-V (the vendored translator) and exact on
//! wave32 (tier A/B).

use anyhow::{Context as _, Result, bail};
use ash::vk;
use serde::Deserialize;

use crate::Context;

#[derive(Deserialize)]
pub struct RootBinding {
    pub kind: String,
    pub address: Option<u64>,
}

#[derive(Deserialize)]
pub struct ViewBuffer {
    pub address: u64,
    pub data_file: String,
}

#[derive(Deserialize)]
pub struct RenderTarget {
    pub format: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Deserialize)]
pub struct DrawRecord {
    pub topology: u32,
    pub vertex_count: u32,
    pub instance_count: u32,
    pub first_vertex: u32,
    pub roots: Vec<RootBinding>,
    pub view_buffers: Vec<ViewBuffer>,
    pub targets: Vec<RenderTarget>,
}

/// The replayed draw's pixels (RGBA float) and size.
pub struct ReplayResult {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<f32>,
}

/// Replay a captured draw: SPIR-V VS/PS, root-view buffers in the heap, draw,
/// readback. `dir` holds draw.json, vs.spv, ps.spv and the view-buffer files.
pub fn replay(ctx: &Context, dir: &std::path::Path) -> Result<ReplayResult> {
    let rec: DrawRecord = serde_json::from_str(&std::fs::read_to_string(dir.join("draw.json"))?)
        .context("parse draw.json")?;
    let vs = std::fs::read(dir.join("vs.spv"))?;
    let ps = std::fs::read(dir.join("ps.spv"))?;
    let target = rec.targets.first().context("no render target")?;
    // The heap spans the console address window the shaders read; place each
    // root-view buffer at its console address.
    let mut heap_end: u64 = 0;
    for vb in &rec.view_buffers {
        let data = std::fs::read(dir.join(&vb.data_file))?;
        heap_end = heap_end.max(vb.address + data.len() as u64);
    }
    let heap_size = (heap_end.max(1 << 20)) as usize;
    let heap = ctx.heap(heap_size)?;
    for vb in &rec.view_buffers {
        let data = std::fs::read(dir.join(&vb.data_file))?;
        // SAFETY: the heap is mapped for heap_size; the buffer fits at its address.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), heap.ptr.add(vb.address as usize), data.len()) };
    }

    // Render at the captured size; the readback is float (R32G32B32A32_SFLOAT),
    // compared against the RGBA8 oracle with a stated tolerance.
    let rt = crate::graphics::RenderTarget::new(ctx, target.width, target.height)?;

    // Push constants: s0..s31 + heap base. The root view address goes in s8/s9
    // (the VS builds its buffer V# from there); the heap base at offset 128.
    let mut pc = [0u32; 36];
    for r in &rec.roots {
        if r.kind == "View" {
            if let Some(a) = r.address {
                pc[8] = a as u32;
                pc[9] = (a >> 32) as u32;
            }
        }
    }
    pc[32] = heap.address as u32;
    pc[33] = (heap.address >> 32) as u32;

    let words = rt.draw_with_set(&vs, &ps, [0.0, 0.0, 0.0, 0.0], &[], vk::DescriptorSet::null(), &pc)?;
    let pixels: Vec<f32> = words.iter().map(|&w| f32::from_bits(w)).collect();
    Ok(ReplayResult { width: target.width, height: target.height, pixels })
}