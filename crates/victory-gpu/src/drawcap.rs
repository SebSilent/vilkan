//! Backend-neutral draw capture: one real title draw serialized into a record
//! that any backend (the SPIR-V/Vulkan path, or a future native one) can consume
//! unchanged. No D3D12 types appear in the record itself — only plain data.
//!
//! Enabled by `VILKAN_DRAW_CAPTURE=<folder>`: the first graphics draw whose VS
//! and PS both translate is written as `draw-<n>.json` plus its shader containers
//! and buffer bytes beside it. The Proton/D3D12 path is untouched and remains the
//! default; this only observes it.

use serde::{Deserialize, Serialize};

/// A shader stage's container (the console DXBC bytes, backend-neutral).
#[derive(Serialize, Deserialize)]
pub struct ShaderRecord {
    /// "vertex", "pixel", "compute", ...
    pub stage: String,
    /// Content hash (hex), for naming the sidecar file.
    pub hash: String,
    /// The container bytes, written to `<hash>.bin` beside the record.
    pub container_file: String,
}

/// One vertex input element.
#[derive(Serialize, Deserialize)]
pub struct VertexInput {
    pub slot: u32,
    pub offset: u32,
    /// DXGI format code (a plain integer, backend-neutral).
    pub format: u32,
    pub per_instance: bool,
}

/// A bound vertex buffer's bytes (copied out of title memory).
#[derive(Serialize, Deserialize)]
pub struct VertexBuffer {
    pub slot: u32,
    pub stride: u32,
    pub data_file: String,
}

/// A root parameter binding, backend-neutral.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum RootBinding {
    None,
    /// A descriptor table: the console heap address of its first descriptor.
    Table { address: u64 },
    /// Inline root constants.
    Constants { dwords: Vec<u32> },
    /// A root view's GPU address.
    View { address: u64 },
}

/// A render target's format and size.
#[derive(Serialize, Deserialize)]
pub struct RenderTarget {
    pub format: u32,
    pub width: u32,
    pub height: u32,
}

/// RT0 blend / write-mask state, backend-neutral (D3D12 enum codes as integers).
#[derive(Clone, Serialize, Deserialize)]
pub struct BlendState {
    /// D3D12_COLOR_WRITE_ENABLE flags (RED=1, GREEN=2, BLUE=4, ALPHA=8).
    pub write_mask: u8,
    pub blend_enable: bool,
    /// D3D12_BLEND codes, 1-based as the enum is defined (ZERO=1, ONE=2,
    /// SRC_COLOR=3, SRC_ALPHA=5, INV_SRC_ALPHA=6, ...). Stored verbatim from
    /// `SrcBlend.0` so the replay maps the same values the host PSO used.
    pub src_blend: u32,
    pub dest_blend: u32,
    /// D3D12_BLEND_OP codes, 1-based (ADD=1, SUBTRACT=2, REV_SUBTRACT=3, MIN=4, MAX=5).
    pub blend_op: u32,
    pub src_blend_alpha: u32,
    pub dest_blend_alpha: u32,
    pub blend_op_alpha: u32,
}

impl Default for BlendState {
    /// D3D12's overwrite default: blend off, write all channels, ONE/ZERO/ADD
    /// (the 1-based enum codes the host PSO desc carries for a plain overwrite).
    fn default() -> Self {
        Self {
            write_mask: 0xF,
            blend_enable: false,
            src_blend: 2,
            dest_blend: 1,
            blend_op: 1,
            src_blend_alpha: 2,
            dest_blend_alpha: 1,
            blend_op_alpha: 1,
        }
    }
}

/// A root view's buffer bytes (a root CBV/SRV the shaders read directly),
/// captured from console memory so the replay can place them in its heap.
#[derive(Serialize, Deserialize)]
pub struct ViewBuffer {
    pub address: u64,
    pub data_file: String,
}

#[derive(Serialize, Deserialize)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

/// One captured draw, backend-neutral.
#[derive(Serialize, Deserialize)]
pub struct DrawRecord {
    pub frame: u64,
    pub ordinal: u64,
    /// D3D12_PRIMITIVE_TOPOLOGY_TYPE as a plain integer.
    pub topology: u32,
    pub vertex_count: u32,
    pub instance_count: u32,
    pub first_vertex: u32,
    pub shaders: Vec<ShaderRecord>,
    pub inputs: Vec<VertexInput>,
    pub vertex_buffers: Vec<VertexBuffer>,
    pub roots: Vec<RootBinding>,
    /// Bytes of any root-view buffers the shaders read (address -> sidecar file).
    pub view_buffers: Vec<ViewBuffer>,
    pub targets: Vec<RenderTarget>,
    /// RT0 blend / write-mask state from the bound pipeline.
    pub blend: BlendState,
    pub viewport: Viewport,
    /// The D3D12 render-target readback for this draw (the oracle), if captured.
    pub oracle_file: Option<String>,
}

/// Whether capture is enabled, and where.
pub fn folder() -> Option<&'static std::path::PathBuf> {
    use std::sync::OnceLock;
    static F: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    F.get_or_init(|| std::env::var_os("VILKAN_DRAW_CAPTURE").map(std::path::PathBuf::from))
        .as_ref()
}