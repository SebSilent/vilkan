//! The backend-neutral host interface.
//!
//! The runtime drives a [`HostBackend`] to execute one captured draw and read
//! back its pixels. The seam is **record-driven**: the trait takes the neutral
//! [`DrawRecord`] and its in-memory [`DrawResources`] (SPIR-V, view-buffer bytes
//! and the D3D12 readback), not a directory. [`load_capture`] and
//! [`HostBackend::replay_dir`] are the convenience wrappers that read a capture
//! folder first. Nothing here names D3D12 or Vulkan. Two paths sit behind the
//! trait:
//!
//! * **D3D12** ([`D3D12Oracle`]): the existing Proton/D3D12 path, unchanged. It
//!   serves the render-target readback that path already captured for the draw
//!   (`oracle.bin`). This is the default, so the Proton boot is untouched.
//! * **native Vulkan**: implemented by `vilkan-vk` (stage two), which executes
//!   the same record on a Linux Vulkan device instead of serving a readback.
//!
//! [`BackendKind::from_env`] selects the path (`VILKAN_BACKEND`, default
//! `d3d12`). The trait is the seam: a caller drives `replay` without knowing
//! which backend answers.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

/// A root parameter binding in a captured draw (backend-neutral).
#[derive(Debug, Clone, Deserialize)]
pub struct RootBinding {
    pub kind: String,
    pub address: Option<u64>,
}

/// A root-view buffer placed in the heap at its console address.
#[derive(Debug, Clone, Deserialize)]
pub struct ViewBuffer {
    pub address: u64,
    pub data_file: String,
}

/// A captured render target's format and size.
#[derive(Debug, Clone, Deserialize)]
pub struct RenderTarget {
    pub format: u32,
    pub width: u32,
    pub height: u32,
}

/// One captured draw, backend-neutral. Unknown fields in `draw.json` (frame,
/// ordinal, shader containers, ...) are ignored: a backend reads what it needs.
#[derive(Debug, Clone, Deserialize)]
pub struct DrawRecord {
    pub topology: u32,
    pub vertex_count: u32,
    pub instance_count: u32,
    pub first_vertex: u32,
    pub roots: Vec<RootBinding>,
    pub view_buffers: Vec<ViewBuffer>,
    pub targets: Vec<RenderTarget>,
}

impl DrawRecord {
    /// Parse a captured `draw.json`.
    pub fn from_json(text: &str) -> Result<Self> {
        serde_json::from_str(text).context("parse draw.json")
    }
}

/// The in-memory resources one draw needs, backend-neutral: translated SPIR-V
/// shaders, the root-view buffer contents keyed by console address, and (for the
/// D3D12 reference) the captured render-target readback. This is what the seam
/// takes as input — not a directory — so it can grow into the device/draw
/// interface the live runtime drives (see `docs/host-backend.md`).
#[derive(Debug, Clone, Default)]
pub struct DrawResources {
    /// Vertex shader, SPIR-V bytes (already translated, exact on wave32).
    pub vs_spirv: Vec<u8>,
    /// Pixel shader, SPIR-V bytes.
    pub ps_spirv: Vec<u8>,
    /// Root-view buffer contents, `(console address, bytes)`.
    pub views: Vec<(u64, Vec<u8>)>,
    /// The D3D12 render-target readback (RGBA8), if captured. The D3D12
    /// reference serves this; the native path ignores it.
    pub oracle_rgba8: Option<Vec<u8>>,
}

/// Load a capture directory into the neutral record and its resources. This is
/// the convenience wrapper over the record-driven seam: `draw.json`, `vs.spv`,
/// `ps.spv`, each `view_buffers[].data_file`, and `oracle.bin` if present.
pub fn load_capture(dir: &Path) -> Result<(DrawRecord, DrawResources)> {
    let record = DrawRecord::from_json(
        &std::fs::read_to_string(dir.join("draw.json")).context("read draw.json")?,
    )?;
    let mut res = DrawResources {
        vs_spirv: std::fs::read(dir.join("vs.spv")).context("read vs.spv")?,
        ps_spirv: std::fs::read(dir.join("ps.spv")).context("read ps.spv")?,
        ..Default::default()
    };
    for vb in &record.view_buffers {
        let data = std::fs::read(dir.join(&vb.data_file))
            .with_context(|| format!("read view buffer {}", vb.data_file))?;
        res.views.push((vb.address, data));
    }
    // The oracle is optional: the native path does not need it.
    if let Ok(oracle) = std::fs::read(dir.join("oracle.bin")) {
        res.oracle_rgba8 = Some(oracle);
    }
    Ok((record, res))
}

/// The replayed draw's pixels (RGBA float, row-major) and size.
#[derive(Debug, Clone)]
pub struct ReplayResult {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<f32>,
}

/// A deliberate fault, for negative controls: a check that cannot fail is not
/// acceptance. Each fault must make a backend's output disagree with the
/// fault-free D3D12 oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    None,
    /// Skip the draw entirely (the target stays at the clear colour).
    SkipDraw,
    /// Corrupt the root-view buffer bytes before placing them in the heap.
    CorruptViewBuffer,
    /// Put the root-view address in the wrong push-constant SGPRs.
    WrongPushConstants,
}

/// The backend-neutral interface the runtime drives: execute one captured draw
/// and read back its pixels. The seam is **record-driven**: it takes the neutral
/// [`DrawRecord`] and its in-memory [`DrawResources`], not a directory, so it can
/// grow into the device/draw interface the live runtime drives. [`replay_dir`]
/// is the convenience wrapper that loads a capture folder first.
///
/// [`replay_dir`]: HostBackend::replay_dir
pub trait HostBackend {
    /// A short, stable name for logs and reports.
    fn name(&self) -> &'static str;
    /// Execute one captured draw from its record and resources, optionally
    /// injecting `fault`.
    fn replay(&self, record: &DrawRecord, res: &DrawResources, fault: Fault) -> Result<ReplayResult>;
    /// Convenience: load the record and resources from a capture directory, then
    /// replay. The default impl reads the folder via [`load_capture`].
    fn replay_dir(&self, dir: &Path, fault: Fault) -> Result<ReplayResult> {
        let (record, res) = load_capture(dir)?;
        self.replay(&record, &res, fault)
    }
}

/// The D3D12 path behind the trait: the existing Proton/D3D12 execution,
/// unchanged. It serves the render-target readback that path captured for the
/// draw (`oracle.bin`, RGBA8). This is the reference the native path is checked
/// against, so it is always fault-free: faults are injected into the backend
/// under test, not into the reference.
pub struct D3D12Oracle;

impl HostBackend for D3D12Oracle {
    fn name(&self) -> &'static str {
        "d3d12"
    }

    fn replay(&self, record: &DrawRecord, res: &DrawResources, _fault: Fault) -> Result<ReplayResult> {
        let target = record.targets.first().context("no render target")?;
        let oracle = res.oracle_rgba8.as_ref()
            .context("no D3D12 render-target readback (oracle) in resources")?;
        let want = (target.width as usize) * (target.height as usize) * 4;
        if oracle.len() != want {
            bail!(
                "oracle size {} != target {}x{}x4 = {want}",
                oracle.len(), target.width, target.height
            );
        }
        // RGBA8 -> RGBA float in [0, 1].
        let pixels: Vec<f32> = oracle.iter().map(|&b| b as f32 / 255.0).collect();
        Ok(ReplayResult { width: target.width, height: target.height, pixels })
    }
}

/// Which backend answers the trait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// The existing Proton/D3D12 path (default; the boot is unchanged).
    D3D12,
    /// The native Linux Vulkan path.
    Vulkan,
}

impl BackendKind {
    /// Select from `VILKAN_BACKEND` (`d3d12` or `vulkan`); default D3D12 so the
    /// Proton path stays the one a plain run drives.
    pub fn from_env() -> Self {
        match std::env::var("VILKAN_BACKEND").as_deref() {
            Ok("vulkan") => Self::Vulkan,
            _ => Self::D3D12,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::D3D12 => "d3d12",
            Self::Vulkan => "vulkan",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_json() -> String {
        r#"{
            "frame": 2, "ordinal": 1,
            "topology": 4, "vertex_count": 3, "instance_count": 1, "first_vertex": 0,
            "shaders": [], "inputs": [], "vertex_buffers": [],
            "roots": [{"kind": "View", "address": 432078720}],
            "view_buffers": [{"address": 432078720, "data_file": "view.bin"}],
            "targets": [{"format": 28, "width": 2, "height": 2}]
        }"#
        .to_owned()
    }

    #[test]
    fn record_ignores_unknown_fields() {
        let rec = DrawRecord::from_json(&record_json()).unwrap();
        assert_eq!(rec.vertex_count, 3);
        assert_eq!(rec.targets[0].width, 2);
        assert_eq!(rec.roots[0].kind, "View");
        assert_eq!(rec.roots[0].address, Some(432078720));
    }

    #[test]
    fn selector_defaults_to_d3d12() {
        // No VILKAN_BACKEND in the test environment by default.
        assert_eq!(BackendKind::from_env(), BackendKind::D3D12);
        assert_eq!(BackendKind::D3D12.name(), "d3d12");
        assert_eq!(BackendKind::Vulkan.name(), "vulkan");
    }

    #[test]
    fn d3d12_oracle_serves_readback_as_float() {
        let dir = std::env::temp_dir().join(format!("vilkan-host-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("draw.json"), record_json()).unwrap();
        std::fs::write(dir.join("vs.spv"), [0u8; 4]).unwrap();
        std::fs::write(dir.join("ps.spv"), [0u8; 4]).unwrap();
        std::fs::write(dir.join("view.bin"), [0u8; 8]).unwrap();
        // 2x2 RGBA8: a known pattern.
        let bytes: Vec<u8> = (0..16).map(|i| (i * 17) as u8).collect();
        std::fs::write(dir.join("oracle.bin"), &bytes).unwrap();
        let out = D3D12Oracle.replay_dir(&dir, Fault::None).unwrap();
        assert_eq!((out.width, out.height), (2, 2));
        assert_eq!(out.pixels.len(), 16);
        assert!((out.pixels[0] - 0.0).abs() < 1e-6);
        assert!((out.pixels[15] - 1.0).abs() < 1e-6);
        // The reference is fault-free: a fault does not change it.
        let faulty = D3D12Oracle.replay_dir(&dir, Fault::SkipDraw).unwrap();
        assert_eq!(out.pixels, faulty.pixels);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn record_driven_seam_matches_dir_wrapper() {
        // The trait takes the record + resources directly; replay_dir is just a
        // loader over the same call. Both must agree.
        let dir = std::env::temp_dir().join(format!("vilkan-host-seam-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("draw.json"), record_json()).unwrap();
        std::fs::write(dir.join("vs.spv"), [0u8; 4]).unwrap();
        std::fs::write(dir.join("ps.spv"), [0u8; 4]).unwrap();
        std::fs::write(dir.join("view.bin"), [7u8; 8]).unwrap();
        let bytes: Vec<u8> = (0..16).map(|i| (i * 17) as u8).collect();
        std::fs::write(dir.join("oracle.bin"), &bytes).unwrap();
        let (record, res) = load_capture(&dir).unwrap();
        assert_eq!(res.views.len(), 1);
        assert_eq!(res.views[0].0, 432078720);
        assert_eq!(res.vs_spirv.len(), 4);
        let direct = D3D12Oracle.replay(&record, &res, Fault::None).unwrap();
        let wrapped = D3D12Oracle.replay_dir(&dir, Fault::None).unwrap();
        assert_eq!(direct.pixels, wrapped.pixels);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn d3d12_oracle_rejects_size_mismatch() {
        let dir = std::env::temp_dir().join(format!("vilkan-host-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("draw.json"), record_json()).unwrap();
        std::fs::write(dir.join("vs.spv"), [0u8; 4]).unwrap();
        std::fs::write(dir.join("ps.spv"), [0u8; 4]).unwrap();
        std::fs::write(dir.join("view.bin"), [0u8; 8]).unwrap();
        std::fs::write(dir.join("oracle.bin"), [0u8; 8]).unwrap(); // want 16
        assert!(D3D12Oracle.replay_dir(&dir, Fault::None).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}