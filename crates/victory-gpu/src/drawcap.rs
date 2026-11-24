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
    pub targets: Vec<RenderTarget>,
    pub viewport: Viewport,
}

/// Whether capture is enabled, and where.
pub fn folder() -> Option<&'static std::path::PathBuf> {
    use std::sync::OnceLock;
    static F: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    F.get_or_init(|| std::env::var_os("VILKAN_DRAW_CAPTURE").map(std::path::PathBuf::from))
        .as_ref()
}