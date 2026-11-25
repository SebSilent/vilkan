//! Pipeline states. Both descriptions start as on PC: the root signature,
//! then the shader stages (five for graphics, one for compute); streams
//! carry the rest of the graphics state as subobjects of the standard
//! types.
//!
//! Host pipelines are made on a pipeline's first draw, not at creation:
//! The title creates some 60,000 pipelines in its first 20
//! seconds and draws with a few dozen (decision 0007, step 3). Shader
//! containers are shared by content hash and translated once, on first
//! use (decision 0003).
//!
//! shortcut: only vertex plus pixel pipelines from streams and compute
//! pipelines get host pipelines; mesh, tessellation and geometry stages,
//! and the plain graphics description, keep their containers only. The 32-byte
//! packet after the header stays zero: titles only copy it inline, which
//! our null command-list limits keep from running.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::collections::HashMap;
use std::mem::offset_of;
use std::sync::{Arc, Mutex, OnceLock};

use crate::object::{self, E_INVALIDARG, E_NOTIMPL, HRESULT, Iid, Object, PIPELINE, Table, This, guid};
use crate::resource::PAGEABLE_IIDS;

const PIPELINE_IIDS: [Iid; 4] = [
    guid(
        0x765a30f3,
        0xf624,
        0x4c6f,
        [0xa8, 0x28, 0xac, 0xe9, 0x48, 0x62, 0x24, 0x45],
    ),
    PAGEABLE_IIDS[0],
    PAGEABLE_IIDS[1],
    PAGEABLE_IIDS[2],
];

#[repr(C)]
pub(crate) struct Bytecode {
    code: *const u8,
    size: usize,
}

#[repr(C)]
pub(crate) struct GraphicsHead {
    root: This,
    /// Vertex, pixel, domain, hull, geometry.
    stages: [Bytecode; 5],
}

#[repr(C)]
pub(crate) struct ComputeHead {
    root: This,
    stage: Bytecode,
}

/// A shader container a title passed, shared by every pipeline with the
/// same bytes.
pub(crate) struct Shader {
    pub(crate) stage: &'static str,
    /// The first 8 bytes of its BLAKE3, as logged.
    pub(crate) hash: String,
    container: Vec<u8>,
    #[cfg(windows)]
    translated: OnceLock<Option<Translated>>,
}

impl std::fmt::Debug for Shader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.stage, self.hash)
    }
}

/// Every container seen, by full content hash.
static SHADERS: Mutex<Option<HashMap<[u8; 32], Arc<Shader>>>> = Mutex::new(None);

/// For the memory lines: containers kept and their bytes, translations
/// kept and the bytes of their host code. None of them is ever let go.
static KEPT: [std::sync::atomic::AtomicU64; 4] = [const { std::sync::atomic::AtomicU64::new(0) }; 4];

/// What the shader table and the pipeline library keep in memory:
/// containers and their bytes, translations and their bytes, the bytes of
/// the library's file (kept whole while the library lives) and what its
/// last save wrote.
#[cfg(windows)]
#[cfg_attr(not(feature = "diagnostics"), allow(dead_code))]
pub(crate) fn kept() -> [u64; 7] {
    use std::sync::atomic::Ordering::Relaxed;
    [
        KEPT[0].load(Relaxed),
        KEPT[1].load(Relaxed),
        KEPT[2].load(Relaxed),
        KEPT[3].load(Relaxed),
        library::BLOB.load(Relaxed),
        library::SAVED.load(Relaxed),
        library::TRIMMED_BYTES.load(Relaxed),
    ]
}

/// The shared container for `bytes`.
fn shader(stage: &'static str, bytes: &[u8]) -> Arc<Shader> {
    let full = blake3::hash(bytes);
    let mut shaders = SHADERS.lock().unwrap_or_else(|e| e.into_inner());
    shaders
        .get_or_insert_with(HashMap::new)
        .entry(*full.as_bytes())
        .or_insert_with(|| {
            KEPT[0].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            KEPT[1].fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
            Arc::new(Shader {
                stage,
                hash: full.to_hex()[..16].to_owned(),
                container: bytes.to_vec(),
                #[cfg(windows)]
                translated: OnceLock::new(),
            })
        })
        .clone()
}

/// A translated shader and the console resources its code reads.
#[cfg(windows)]
pub(crate) struct Translated {
    pub(crate) host: victory_shader::emit::HostShader,
    pub(crate) used: Vec<victory_shader::UsedResource>,
    /// Whether the shader's code can store to buffer memory ([`stores`]);
    /// true when its code is not known.
    pub(crate) stores: bool,
}

#[cfg(windows)]
impl Shader {
    /// The translation, made on first use; a shader that does not
    /// translate is logged once.
    pub(crate) fn translated(&self) -> Option<&Translated> {
        self.translated
            .get_or_init(|| {
                let done = translate(&self.container);
                // `VICTORY_SHADER_SAVE=<dir>`: each shader as the title gave it (`.cdxil`) and as
                // translated (`.dxil`), by the hash crash reports name it with
                if let Some(dir) = std::env::var_os("VICTORY_SHADER_SAVE") {
                    let dir = std::path::PathBuf::from(dir);
                    let _ = std::fs::create_dir_all(&dir);
                    let stem = format!("{}.{}", self.hash, self.stage);
                    let _ = std::fs::write(dir.join(format!("{stem}.cdxil")), &self.container);
                    if let Ok(t) = &done {
                        let _ = std::fs::write(dir.join(format!("{stem}.dxil")), &t.host.dxil);
                    }
                }
                if let Ok(t) = &done {
                    KEPT[2].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    KEPT[3].fetch_add(t.host.dxil.len() as u64, std::sync::atomic::Ordering::Relaxed);
                }
                match &done {
                    Ok(t) => tracing::info!(
                        stage = self.stage,
                        hash = %self.hash,
                        user_data = t.host.user_data.len(),
                        samplers = t.host.samplers.len(),
                        "shader translated"
                    ),
                    Err(e) => tracing::warn!(
                        stage = self.stage,
                        hash = %self.hash,
                        error = %e,
                        "shader does not translate; its pipelines' draws are dropped"
                    ),
                }
                done.ok()
            })
            .as_ref()
    }
}

#[cfg(windows)]
fn translate(container: &[u8]) -> Result<Translated, String> {
    use victory_shader::{Container, FourCC, HwStage};
    // The translation depends on it (a view index past a capped heap).
    crate::descriptor::host::probe_view_limit();
    let c = Container::parse(container).map_err(|e| e.to_string())?;
    let programs = match c.part(FourCC::DXIL) {
        Some(part) => victory_shader::native_programs(part.data).map_err(|e| e.to_string())?,
        None => Vec::new(),
    };
    let host = crate::shader_cache::host_shader(container)?;
    let stores = programs.is_empty() || programs.iter().any(stores);
    let used = programs
        .into_iter()
        .find(|p| {
            matches!(
                p.stage,
                HwStage::Vs | HwStage::Ps | HwStage::Ms | HwStage::Cs
            )
        })
        .map(|p| p.used)
        .unwrap_or_default();
    Ok(Translated { host, used, stores })
}

/// The compute pipelines of the console's ray-tracing structure builder
/// (`XboxBvhBuilder.dll`; its programs are the ones with a spilled
/// user-data table) run **all or none**. Its stages feed each other
/// through memory: a stage that runs on what a dropped stage never wrote
/// computes its store offsets from garbage. The structures lie in plain
/// title memory, between the title's own objects, and a root view has no
/// size, so those stores land on the neighbours (run 98, with 14 of 35
/// stages translated: the title died within a second of the first builder
/// submission, on pointers that had become bounding-box bit patterns).
///
/// So a builder dispatch runs only while every builder pipeline the title
/// has created has a host pipeline; the first builder dispatch makes them
/// all on the spot instead of over the next frames (a stage "still being
/// made" would be a dropped stage). One that cannot be made turns the
/// builder off for the rest of the run.
///
/// shortcut: and only with `VICTORY_BVH_BUILDER=1`, until a run has shown
/// the whole builder and the host side of title memory by address
/// (`memory.rs`) working on a card; nothing reads the structures before
/// the programs that walk them translate.
#[cfg(windows)]
struct Builder {
    pipelines: Vec<Arc<make::Pending>>,
    /// Every one of `pipelines` has a host pipeline.
    whole: bool,
    /// One could not be made.
    off: bool,
}

#[cfg(windows)]
static BUILDER: Mutex<Builder> = Mutex::new(Builder {
    pipelines: Vec::new(),
    whole: false,
    off: false,
});

/// Whether `shaders` are a pipeline of the structure builder.
#[cfg(windows)]
fn of_builder(shaders: &[Arc<Shader>]) -> bool {
    use victory_shader::{Container, FourCC};
    let [shader] = shaders else { return false };
    if shader.stage != "compute" {
        return false;
    }
    let Ok(c) = Container::parse(&shader.container) else {
        return false;
    };
    c.part(FourCC::DXIL)
        .and_then(|part| victory_shader::native_programs(part.data).ok())
        .is_some_and(|programs| programs.iter().any(victory_shader::spill::is_spilled))
}

/// `VICTORY_BVH_QUARANTINE=1`: what the structure builder's stages store
/// stays in the host's copies and is not copied back into title memory. A
/// check for a first run of the whole builder: a stage that stores where
/// it should not can then not reach the title's own objects (run 98), and
/// the programs that walk the structures read the host's copies anyway.
/// shortcut: not a way to run: the title also reads results back (sizes,
/// counters) and gets none, and a copy's pages are overwritten from title
/// memory when the title writes them.
#[cfg(windows)]
pub(crate) fn builder_quarantined(this: This) -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    is_builder(this) && *ON.get_or_init(|| std::env::var_os("VICTORY_BVH_QUARANTINE").is_some_and(|v| v == "1"))
}

/// Whether the title's pipeline `this` is one of the structure builder's.
#[cfg(windows)]
pub(crate) fn is_builder(this: This) -> bool {
    !this.is_null() && pipeline(this).builder
}

/// A stage of the builder was dropped after all: none runs from here on.
#[cfg(windows)]
pub(crate) fn builder_off(why: &'static str) {
    let mut b = BUILDER.lock().unwrap_or_else(|e| e.into_inner());
    if !b.off {
        b.off = true;
        tracing::warn!(why, "the ray-tracing structure builder is turned off for the rest of the run: its stages run all or none");
    }
}

/// At a dispatch of a builder pipeline: whether the builder runs (see
/// [`Builder`]).
#[cfg(windows)]
pub(crate) fn builder_runs() -> bool {
    static ALLOWED: OnceLock<bool> = OnceLock::new();
    if !*ALLOWED.get_or_init(|| std::env::var_os("VICTORY_BVH_BUILDER").is_some_and(|v| v == "1")) {
        return false;
    }
    let mut b = BUILDER.lock().unwrap_or_else(|e| e.into_inner());
    if b.off {
        return false;
    }
    if b.whole {
        return true;
    }
    let missing = b.pipelines.iter().filter(|p| !make::now(p)).count();
    if missing > 0 {
        b.off = true;
        tracing::warn!(
            missing,
            of = b.pipelines.len(),
            "the ray-tracing structure builder stays off: not all of its programs have a host pipeline, and its stages run all or none"
        );
        return false;
    }
    tracing::info!(
        pipelines = b.pipelines.len(),
        "the ray-tracing structure builder runs: all of its programs have host pipelines"
    );
    b.whole = true;
    true
}

/// Whether a console program can store to buffer memory: it has a buffer,
/// typed-buffer, scalar-memory, flat or global store or atomic, or code
/// that is not all known (an instruction of those kinds outside our table,
/// an indirect jump, code that does not decode). Stores to textures
/// (image instructions) and to the program's own scratch memory do not
/// count: neither reaches a buffer's host copy. After upstream #413,
/// which leaves scalar-memory stores out and counts image stores.
#[cfg(windows)]
fn stores(program: &victory_shader::NativeProgram) -> bool {
    use victory_shader::gfx10::{Encoding, decode};
    let Ok(code) = decode(&program.code) else {
        return true;
    };
    code.iter().any(|i| {
        let name = i.name();
        match i.encoding {
            Encoding::Mubuf
            | Encoding::Mtbuf
            | Encoding::Smem
            | Encoding::Flat
            | Encoding::Global => name.is_none_or(|n| {
                (n.contains("store") || n.contains("atomic")) && !n.contains("scratch")
            }),
            Encoding::Sop1 => name.is_none_or(|n| matches!(n, "s_setpc_b64" | "s_swappc_b64")),
            _ => false,
        }
    })
}

#[repr(C)]
struct Pipeline {
    object: Object,
    packet: [u32; 8],
    /// Each used stage's container.
    shaders: Vec<Arc<Shader>>,
    /// Whether a list has bound it yet (its shaders are logged then).
    bound: std::sync::atomic::AtomicBool,
    /// What its host pipeline is made from, and the host pipeline once made
    /// (by a worker: the title may release the pipeline meanwhile).
    #[cfg(windows)]
    host: Arc<make::Pending>,
    /// One of the ray-tracing structure builder's ([`Builder`]).
    #[cfg(windows)]
    builder: bool,
}
const _: () = assert!(offset_of!(Pipeline, packet) == 0x10);

/// Stage names, in description order.
const GRAPHICS_STAGES: [&str; 5] = ["vertex", "pixel", "domain", "hull", "geometry"];

fn pipeline<'a>(this: This) -> &'a Pipeline {
    // SAFETY: the title passes one of our pipelines.
    unsafe { &*this.cast::<Pipeline>() }
}

/// List method: logs each pipeline's shaders the first time a list binds
/// it, so a run lists the shaders its frames need first.
pub(crate) fn bound(this: This) {
    if this.is_null() {
        return;
    }
    let p = pipeline(this);
    if p.bound.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let frame = crate::frame::current();
    for shader in &p.shaders {
        tracing::info!(pipeline = ?this, frame, stage = shader.stage, size = shader.container.len(), hash = %shader.hash, "first bind");
        if shader.stage == "mesh" {
            MESH_START.get_or_init(std::time::Instant::now);
        }
    }
}

/// The instant of the first mesh-shader bind (world entry), for event-relative
/// input scripting. `None` until a mesh pipeline binds.
static MESH_START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

pub fn mesh_start() -> Option<std::time::Instant> {
    MESH_START.get().copied()
}

/// The host pipeline of a title pipeline; `None` if one of its shaders does
/// not translate, the host refuses it, or it is still being made (its first
/// use starts that, see [`make`]).
#[cfg(windows)]
pub(crate) fn host_pipeline<'a>(this: This) -> Option<&'a host::HostPipeline> {
    if this.is_null() {
        return None;
    }
    make::ready(&pipeline(this).host)
}

/// Host pipelines queued and not made yet.
#[cfg(windows)]
pub(crate) fn making() -> u64 {
    make::waiting()
}

/// Draws and dispatches dropped since the last [`take_drops`], by reason
/// (diagnostics).
#[cfg(all(windows, feature = "diagnostics"))]
static DROPS: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

/// Shader hashes as printed in the pipeline diagnostics.
#[cfg(all(windows, feature = "diagnostics"))]
fn parse_skip_hashes(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|hash| !hash.is_empty())
        .filter_map(|hash| {
            if hash.len() == 16 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                Some(hash.to_ascii_lowercase())
            } else {
                tracing::warn!(
                    hash,
                    "VICTORY_SKIP_PIPELINES expects 16-digit shader hashes"
                );
                None
            }
        })
        .collect()
}

#[cfg(all(windows, feature = "diagnostics"))]
fn skipped_shader(p: &Pipeline) -> Option<&str> {
    static HASHES: OnceLock<Vec<String>> = OnceLock::new();
    let hashes = HASHES.get_or_init(|| {
        let value = std::env::var("VICTORY_SKIP_PIPELINES").unwrap_or_default();
        parse_skip_hashes(&value)
    });
    if hashes.is_empty() {
        return None;
    }
    p.shaders
        .iter()
        .find(|shader| hashes.contains(&shader.hash))
        .map(|shader| shader.hash.as_str())
}

#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn is_skipped(this: This) -> bool {
    !this.is_null() && skipped_shader(pipeline(this)).is_some()
}

/// Counts a draw or dispatch of `this` intentionally skipped or dropped
/// for want of a host pipeline (diagnostics).
#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn note_drop(this: This) {
    let why = if this.is_null() {
        Some("no pipeline set".to_owned())
    } else {
        let p = pipeline(this);
        skipped_shader(p)
            .map(|hash| format!("skipped shader {hash}"))
            .or_else(|| make::missing(&p.host))
    };
    if let Some(why) = why {
        note_why(why);
    }
}

/// Counts a draw or dispatch dropped for `why` (diagnostics).
#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn note_why(why: String) {
    crate::capture::op(0, || format!("DROPPED {why}"));
    let mut drops = DROPS.lock().unwrap_or_else(|e| e.into_inner());
    *drops.get_or_insert_with(HashMap::new).entry(why).or_default() += 1;
}

/// The drops counted since the last call, most first (diagnostics).
#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn take_drops() -> Vec<(String, u64)> {
    let drops = DROPS.lock().unwrap_or_else(|e| e.into_inner()).take();
    let mut drops: Vec<(String, u64)> = drops.into_iter().flatten().collect();
    // Frame reports only show the first 40 reasons. Keep ties ordered so
    // a HashMap iteration order change cannot change which reasons appear.
    drops.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    drops
}

/// The stage's container, if it has one.
fn copy(stage: &'static str, code: &Bytecode) -> Option<Arc<Shader>> {
    if code.code.is_null() || code.size == 0 {
        return None;
    }
    // SAFETY: the title passes `size` bytes of shader code.
    Some(shader(stage, unsafe {
        std::slice::from_raw_parts(code.code, code.size)
    }))
}

fn hand_out(
    shaders: Vec<Arc<Shader>>,
    #[cfg(windows)] graphics: Option<Box<host::GraphicsState>>,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    tracing::trace!(?shaders, "pipeline state");
    #[cfg(windows)]
    let host = Arc::new(make::Pending::new(shaders.clone(), graphics));
    #[cfg(windows)]
    make::early(&host);
    #[cfg(windows)]
    let builder = of_builder(&shaders);
    #[cfg(windows)]
    if builder {
        // Another stage: the group is looked at again at its next dispatch.
        let mut b = BUILDER.lock().unwrap_or_else(|e| e.into_inner());
        b.pipelines.push(host.clone());
        b.whole = false;
    }
    let pipeline = Box::new(Pipeline {
        object: Object::new(&FUNCTIONS),
        packet: [0; 8],
        #[cfg(windows)]
        host,
        #[cfg(windows)]
        builder,
        shaders,
        bound: std::sync::atomic::AtomicBool::new(false),
    });
    object::hand_out(PIPELINE, pipeline, iid, &PIPELINE_IIDS, out)
}

/// Device method.
pub(crate) extern "system" fn create_graphics(
    _: This,
    desc: *const GraphicsHead,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // SAFETY: the title passes a description (null is tolerated).
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    let stages = GRAPHICS_STAGES.iter().zip(&desc.stages);
    let shaders = stages.filter_map(|(s, c)| copy(s, c)).collect();
    hand_out(
        shaders,
        #[cfg(windows)]
        None,
        iid,
        out,
    )
}

/// Device method.
pub(crate) extern "system" fn create_compute(
    _: This,
    desc: *const ComputeHead,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // SAFETY: the title passes a description (null is tolerated).
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    hand_out(
        copy("compute", &desc.stage).into_iter().collect(),
        #[cfg(windows)]
        None,
        iid,
        out,
    )
}

/// Device method 0x1f0: the plain compute description plus console
/// extensions `(desc, extension count, extensions, iid, out)`.
/// shortcut: the extensions (console shader settings) are ignored, as is
/// the rest of the description.
pub(crate) extern "system" fn create_compute_x(
    this: This,
    desc: *const ComputeHead,
    _extensions: u32,
    _extended: *const u8,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    create_compute(this, desc, iid, out)
}

/// One shader slot of a pipeline-cache record: its code, then the metadata
/// the cache carries with it.
#[repr(C)]
#[derive(Clone, Copy)]
struct CacheSlot {
    code: *const u8,
    size: usize,
    meta: *const u8,
    meta_size: usize,
}

/// The description the title hands device methods 0x1c8 and 0x218: a root
/// signature, two metadata pairs, then seven shader slots. Graphics
/// descriptions use all seven (0x108 bytes); compute ones only the first
/// (0x48) — five-star's `docs/d3d12x-pipelines.md`.
///
/// These are NOT the cache file's records. The raw records in the blob are
/// index-based and much smaller (0x70 graphics, 0x18 compute: hashes and
/// `{i32 shader index, i32 metadata entry}` pairs resolved through the
/// cache's metadata and shader index tables); the title's own loader
/// (0x14248f680) resolves those indices into the pointer-based description
/// this struct reads. So nothing here parses the blob: by the time a slot
/// is called, the indices have become pointers and sizes.
///
/// What the first records looked like at runtime says the two kinds are not
/// equally understood. A compute description reads as `{root, 28, 20, code
/// 512 bytes at a guest address, shader metadata 344}` — the documented
/// shape. A graphics description does not: its slot 0 and slot 1 carry the
/// same code pointer, and their metadata pointers are the head's own two
/// metadata fields, which is only possible if the region past 0x28 is not
/// the seven-slot area this struct assumes. Graphics therefore never builds
/// a pipeline from these fields today (see `program`).
#[repr(C)]
pub(crate) struct CacheRecord {
    root: This,
    state: Bytecode,
    layout: Bytecode,
    slots: [CacheSlot; 7],
}

const _: () = assert!(std::mem::size_of::<CacheRecord>() == 0x108);

/// The stages a graphics description's seven slots hold, in order.
///
/// This is the cache's own order, not the desktop `GRAPHICS_STAGES` one:
/// `docs/pipeline-cache.md` records slot 0 as the pixel shader, slot 1 the
/// vertex shader, slot 2 the mesh/NGG program, slots 3 to 5 unused, and slot
/// 6 a fetch shader, which has no metadata entry and no desktop stage. The
/// reference dump agrees with what this tree sees at runtime: the records it
/// fills carry pixel and vertex (13,383 of them) or pixel and mesh (10,726),
/// and the first graphics descriptions here had exactly two slots filled.
const CACHE_STAGES: [Option<&'static str>; 7] = [
    Some("pixel"),
    Some("vertex"),
    Some("mesh"),
    None,
    None,
    None,
    None,
];

/// One shader slot of a description, read for real.
///
/// The size in these records is the title's word for a region of its shader
/// blob, and a wrong parse turns it into a wild read: `copy` would hand the
/// compiler hundreds of kilobytes from an address that holds something else
/// entirely. A slot is therefore accepted only when its three facts agree —
/// a pointer inside the guest address space and aligned like the cache's
/// code area, a size in the range a program actually occupies, and code
/// that starts with the DXBC fourcc every console shader container starts
/// with (`docs/d3d12x-pipelines.md`, "Shader bytecode"; the FSSM program
/// rides inside its XDAT section, so a container is what we are handed).
///
/// Anything else is refused and left unbuilt, which drops that pipeline's
/// work the way a stand-in already does. That is the deliberate trade: a
/// dropped dispatch is visible and recoverable, a compile fed from a wrong
/// offset is neither.
/// Where the title maps the memory its descriptions point into: far above
/// the null page, well below the kernel half.
const ADDRESS_MIN: usize = 0x1_0000;
const ADDRESS_MAX: usize = 0x0000_7fff_ffff_ffff;
/// A program container: the smallest real ones are a few hundred bytes, the
/// largest in the reference cache are the 5-10 MB screen-probe and
/// ray-traced shadow compute shaders.
const CODE_MIN: usize = 0x100;
const CODE_MAX: usize = 16 << 20;

/// The four bytes a slot's code starts with, zeroed when the address is not
/// plausibly mapped at all — so the shape can be reported without reading
/// wherever a wrong parse happens to point.
fn head4(code: *const u8) -> [u8; 4] {
    let at = code as usize;
    if at < ADDRESS_MIN || at > ADDRESS_MAX {
        return [0; 4];
    }
    // SAFETY: the address is inside the guest address space and four bytes
    // of it are read.
    unsafe { std::slice::from_raw_parts(code, 4) }
        .try_into()
        .unwrap_or([0; 4])
}

fn program(slot: &CacheSlot) -> Option<Bytecode> {
    let at = slot.code as usize;
    if at < ADDRESS_MIN
        || at > ADDRESS_MAX
        || at % 16 != 0
        || slot.size < CODE_MIN
        || slot.size > CODE_MAX
    {
        return None;
    }
    if head4(slot.code) != *b"DXBC" {
        return None;
    }
    Some(Bytecode {
        code: slot.code,
        size: slot.size,
    })
}

/// A record's slots as stages. A slot that does not read as a program is
/// dropped; when a record yields nothing, what it really holds is logged a
/// few times per kind, so the format is derived from the title's own data
/// instead of assumed again.
#[allow(dead_code)] // kept as the measurement behind the reverted slot, not its answer
fn cache_stages(
    kind: &'static str,
    record: &CacheRecord,
    stages: &[Option<&'static str>],
) -> Vec<Arc<Shader>> {
    let built = record
        .slots
        .iter()
        .zip(stages)
        .filter_map(|(s, name)| {
            let name = (*name)?;
            program(s).and_then(|code| copy(name, &code))
        })
        .collect::<Vec<_>>();
    if built.is_empty() {
        unmatched(kind, record);
    }
    built
}

/// A description that built nothing: its raw words, so the real field
/// layout can be read off them.
fn unmatched(kind: &'static str, record: &CacheRecord) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static CALLS: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
    let at = usize::from(kind == "compute");
    if CALLS[at].fetch_add(1, Ordering::Relaxed) < 4 {
        tracing::warn!(
            kind,
            head = ?[record.state.code as usize, record.state.size, record.layout.code as usize, record.layout.size],
            code = ?record.slots.map(|s| s.size),
            metadata = ?record.slots.map(|s| s.meta_size),
            fourcc = ?record.slots.map(|s| String::from_utf8_lossy(&head4(s.code)).into_owned()),
            "a cache description built nothing"
        );
    }
}

/// Device method 0x1c8: a graphics pipeline of the title's own pipeline
/// cache `(record, iid, out)`. The record carries the shader code, which is
/// what a pipeline is made of here; the fixed-function state it also
/// carries is compact cache metadata whose mapping to the desktop
/// descriptions is not ours yet, so the host pipeline is made from the
/// stages alone, as device method 0x50 already does.
///
/// A record that builds nothing still hands out a pipeline: the title reads
/// the object back without a null check (five-star found a null here fault
/// the story-mode load). Building a pipeline from that unproven parse is what
/// this function no longer does; see the note in its body. [`cache_stages`]
/// and the DXBC check it reports are kept as the measurement, not the answer.
pub(crate) extern "system" fn create_graphics_cache(
    _: This,
    record: *const CacheRecord,
    _iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // Reverted to the last known-good answer: `out` null, `E_NOTIMPL`. The
    // record was built as a pipeline from a slot layout this tree later
    // measured and found wrong (the code it hands the compiler is raw ISA
    // with no DXBC header, and the metadata fields are not yet mapped), so
    // constructing a pipeline from it fed the compiler garbage and the menu
    // stopped loading. Measuring the record stays; answering it does not.
    if !out.is_null() {
        // SAFETY: the title's own out pointer.
        unsafe { *out = std::ptr::null_mut() };
    }
    // SAFETY: the title passes a record of the documented size.
    if let Some(r) = unsafe { record.as_ref() } {
        counted("graphics", r);
    }
    E_NOTIMPL
}

/// Device method 0x218: a compute pipeline of the title's pipeline cache,
/// the same record with one shader slot.
pub(crate) extern "system" fn create_compute_cache(
    _: This,
    record: *const CacheRecord,
    _iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // Reverted with the graphics slot above: `out` null, `E_NOTIMPL`, the
    // answer the title saw before the pipeline-cache records were built. The
    // dump below is what proved the layout, and it stays.
    if !out.is_null() {
        // SAFETY: the title's own out pointer.
        unsafe { *out = std::ptr::null_mut() };
    }
    // SAFETY: the title passes a record of the documented size.
    if let Some(r) = unsafe { record.as_ref() } {
        counted("compute", r);
    }
    E_NOTIMPL
}

/// The size of every slot of the first records, so that the order assumed
/// above is measured: a graphics record whose two first slots are always
/// filled and whose last two always are not says the mesh stages are last.
fn counted(kind: &'static str, record: &CacheRecord) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static CALLS: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
    let at = usize::from(kind == "compute");
    if CALLS[at].fetch_add(1, Ordering::Relaxed) < 6 {
        // The description's own words, before any interpretation: which
        // fields are pointers, which are sizes, which are flags. Only as far
        // as the kind's documented size — the guest lays these out 0x60
        // apart, so reading past the end reports the next record's head and
        // the layout gets "confirmed" by a neighbour.
        let words = if kind == "compute" { 9 } else { 33 }; // 0x48, 0x108
        let base = record as *const CacheRecord as *const u64;
        let dump: Vec<String> = (0..words)
            // SAFETY: `words` counts the documented size of this kind.
            .map(|i| format!("{:#x}", unsafe { core::ptr::read_volatile(base.add(i)) }))
            .collect();
        tracing::warn!(
            kind,
            words = ?dump,
            addressed = ?record_addresses(record, words),
            "a pipeline-cache record: its words, and what each pointer-shaped one addresses"
        );
        tracing::warn!(
            kind,
            regions = ?slot_regions(record, words),
            "the same record's regions in full: every pointer-shaped field, dumped to its own size"
        );
    }
}

/// Every region a description points at, dumped as dwords to the size the
/// description itself gives. A 48-byte head is what left the metadata region
/// undecided: its first two dwords repeat the code region's size, which reads
/// like a shared table, while the record's own fields (register counts, the
/// resource layout, thread-group size) sit further in. `size` is the title's
/// word for how long the region is, so the dump stops there and cannot bleed
/// into the neighbour.
fn slot_regions(record: &CacheRecord, words: usize) -> Vec<String> {
    const CAP: usize = 0x200;
    let base = record as *const CacheRecord as *const u64;
    let mut out = Vec::new();
    // Word 0 is the root signature's HANDLE, not a region: the regions start
    // at word 1, each as a `{ptr, size}` pair.
    for word in 1..words {
        // SAFETY: `words` is the documented size of this kind's description.
        let value = unsafe { core::ptr::read_volatile(base.add(word)) };
        let next = unsafe {
            core::ptr::read_volatile(base.add(word + 1).min(base.add(words - 1)))
        };
        let at = value as usize;
        if at < ADDRESS_MIN || at > ADDRESS_MAX || at % 4 != 0 {
            continue;
        }
        // The size is the NEXT word when it looks like a size: small, and not
        // itself a pointer. Every region here is under 0x1000 bytes.
        let size = usize::try_from(next).unwrap_or(usize::MAX);
        let len = if size != 0 && size < 0x1_0000 { size.min(CAP) } else { CAP };
        out.push(format!("+{:#x}[{}]", word * 8, dwords_at(at as *const u8, len)));
        if out.len() >= 8 {
            break;
        }
    }
    out
}

/// `want` bytes at a guest address as little-endian dwords, cut to what is
/// actually committed so a wrong parse cannot read a page that is not there.
fn dwords_at(at: *const u8, want: usize) -> String {
    let len = committed(at, want) & !3;
    if len == 0 {
        return String::new();
    }
    // SAFETY: `len` bytes at `at` are committed memory, and only they are read.
    let bytes = unsafe { std::slice::from_raw_parts(at, len) };
    bytes
        .chunks_exact(4)
        .map(|c| format!("{:08x}", u32::from_le_bytes(c.try_into().unwrap())))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every pointer-shaped word of a description, by offset, with the bytes it
/// addresses and the magics that sit NEAR them. The offsets are already
/// known to be right (proof9's words matched this struct exactly); what is
/// unknown is the FORMAT of the code bytes, and that is what these two
/// strings settle: `head48` shows what is AT the pointer, `near48` shows
/// whether a container header sits just BEFORE it.
fn record_addresses(record: &CacheRecord, words: usize) -> Vec<String> {
    let mut out = Vec::new();
    let base = record as *const CacheRecord as *const u8;
    for word in 0..words {
        let at = word * 8;
        // SAFETY: `words` is the documented size of this kind's description
        // (0x48 compute, 0x108 graphics), so every word walked is inside it.
        let value = unsafe { core::ptr::read_volatile(base.add(at) as *const u64) };
        let addr = value as usize;
        if addr < ADDRESS_MIN || addr > ADDRESS_MAX || addr % 4 != 0 {
            continue;
        }
        let code = addr as *const u8;
        out.push(format!("0x{at:x}=[{}] near[{}]", head48(code), near48(code)));
        if out.len() >= 6 {
            break;
        }
    }
    out
}

/// How many bytes of a guest address the host really has. A wrong parse
/// points anywhere, and one read of a page that is not there takes the whole
/// run down, so every window below is cut to what is committed first.
fn committed(at: *const u8, want: usize) -> usize {
    (crate::texture::mapped_bytes(at, want) as usize).min(want)
}

/// The forty-eight bytes an address starts with, as hex and then printable.
/// Hex, not just text, because a part's own index sits past its fixed
/// header: a magic at +0x18 never shows in a sixteen-byte printable window.
fn head48(code: *const u8) -> String {
    let len = committed(code, 48);
    if len == 0 {
        return String::new();
    }
    // SAFETY: `len` bytes at `code` are committed memory, and only they are
    // read.
    let bytes = unsafe { std::slice::from_raw_parts(code, len) };
    let hex: String = bytes.iter().map(|c| format!("{c:02x}")).collect();
    let text: String = bytes
        .iter()
        .map(|c| if c.is_ascii_graphic() { *c as char } else { '.' })
        .collect();
    format!("{hex}|{text}")
}

/// The four-byte tags within 0x400 bytes BEFORE a code pointer and in the
/// bytes after it, each as the offset from the pointer. This is the
/// discriminator: a tag at a NEGATIVE offset means the description addresses
/// a part inside a container, and the fix is to hand the compiler the bytes
/// from the container's start; a tag AT or just past zero means a standalone
/// blob, and the fix is to unpack it. Guessing wrong costs a wild read.
fn near48(code: *const u8) -> String {
    const LOOKBACK: usize = 0x400;
    const AHEAD: usize = 0x40;
    const TAGS: [&[u8]; 8] = [
        b"DXBC", b"FSSM", b"XBCH", b"XDAT", b"SGD5", b"DXIL", b"ISG1", b"OSG1",
    ];
    let at = code as usize;
    if at < ADDRESS_MIN + LOOKBACK {
        return String::new();
    }
    let back = (at - LOOKBACK) as *const u8;
    let len = committed(back, LOOKBACK + AHEAD);
    if len < AHEAD {
        return format!("unmapped({len})");
    }
    // SAFETY: `len` bytes from `back` are committed memory, and only they
    // are read.
    let window = unsafe { std::slice::from_raw_parts(back, len) };
    let mut found = Vec::new();
    for tag in TAGS {
        // The container's own header is what matters most: report both its
        // position relative to the pointer and its size field, which says
        // where the container ends.
        if let Some(p) = window.windows(4).position(|w| w == tag) {
            let rel = p as isize - LOOKBACK as isize;
            let mut end = String::new();
            if tag == b"DXBC" && p + 12 <= len {
                // DXBC: magic, four zero bytes, u32 whole-file size.
                end = format!("/sz{}", u32::from_le_bytes(window[p + 8..p + 12].try_into().unwrap()));
            }
            found.push(format!("{}@{rel}{end}", String::from_utf8_lossy(tag)));
        }
    }
    found.join(",")
}

/// A pipeline described as a stream: its size and the subobjects, each a
/// type then its data, padded to 8 bytes.
#[repr(C)]
pub(crate) struct StreamDesc {
    size: usize,
    stream: *const u8,
}

/// A subobject type's size (type included), and the stage it is if it is a
/// shader. The console numbers amplification and mesh shaders 23 and 24
/// (PC: 24 and 25); its own types from 25 on are not sized yet.
fn subobject(kind: u32) -> Option<(usize, Option<&'static str>)> {
    let stage = |name| Some((24, Some(name)));
    match kind {
        1 => stage("vertex"),
        2 => stage("pixel"),
        3 => stage("domain"),
        4 => stage("hull"),
        5 => stage("geometry"),
        6 => stage("compute"),
        23 => stage("amplification"),
        24 => stage("mesh"),
        0 => Some((16, None)),
        12 | 19 => Some((24, None)),
        7 | 15 => Some((40, None)),
        8 => Some((336, None)),
        10 => Some((48, None)),
        11 => Some((56, None)),
        17 => Some((16, None)),
        21 => Some((64, None)),
        22 => Some((32, None)),
        9 | 13 | 14 | 16 | 18 | 20 => Some((8, None)),
        _ => None,
    }
}

/// Device method: a pipeline from a stream; the shaders are kept, and the
/// graphics state for the host pipeline.
pub(crate) extern "system" fn create_stream(
    _: This,
    desc: *const StreamDesc,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // SAFETY: the title passes a description (null is tolerated).
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if desc.stream.is_null() {
        return E_INVALIDARG;
    }
    let mut shaders = Vec::new();
    #[cfg(windows)]
    let mut graphics = host::GraphicsState::default();
    let mut at = 0;
    while at + 8 <= desc.size {
        // SAFETY: inside the stream the title passes.
        let kind = unsafe { desc.stream.add(at).cast::<u32>().read_unaligned() };
        let Some((size, stage)) = subobject(kind) else {
            tracing::warn!(
                kind,
                at,
                "pipeline subobject of unknown size; the rest is ignored"
            );
            break;
        };
        if let Some(stage) = stage {
            // SAFETY: a shader subobject has its bytecode at +8.
            let code = unsafe { &*desc.stream.add(at + 8).cast::<Bytecode>() };
            shaders.extend(copy(stage, code));
        }
        #[cfg(windows)]
        // SAFETY: a whole subobject of this kind lies at `at`.
        unsafe {
            graphics.read(kind, desc.stream.add(at))
        };
        at += size;
    }
    hand_out(
        shaders,
        #[cfg(windows)]
        Some(Box::new(graphics)),
        iid,
        out,
    )
}

extern "system" fn query(this: This, iid: *const Iid, out: *mut This) -> HRESULT {
    let found = object::is(iid, &PIPELINE_IIDS).then_some(this);
    object::answer(PIPELINE, iid, found, out)
}
extern "system" fn release(this: This) -> u32 {
    object::release_boxed::<Pipeline>(this)
}

/// IUnknown 3, object 4, device child 1, pipeline 3.
static FUNCTIONS: Table<24> = {
    let mut f = object::d3d12_object(object::missing_table::<PIPELINE, 24>());
    f[0] = query as *const ();
    f[2] = release as *const ();
    f[7] = object::parent_device as *const (); // 0x38
    Table(f)
};

#[cfg(windows)]
pub(crate) mod host;
#[cfg(windows)]
mod library;
#[cfg(windows)]
mod make;

/// Lets the pipeline library's mapped file go from memory when no pipeline
/// has loaded for a while (`library::trim`); called by the host's sweep.
#[cfg(windows)]
pub(crate) fn trim_library() {
    library::trim();
}

/// The pipeline library's part of the report when the host device is
/// removed (`library::removal_note`).
#[cfg(windows)]
pub(crate) fn library_removal_note() -> String {
    library::removal_note()
}

/// Keeps compiled host pipelines, and the list of title pipelines to start
/// early, in `dir` (a folder of the cache) across runs.
#[cfg(windows)]
pub fn use_pipeline_cache(dir: std::path::PathBuf) {
    library::use_pipeline_cache(dir);
}

/// Saves the pipeline cache now if anything was added (at the end of a run).
#[cfg(windows)]
pub fn save_pipeline_cache() {
    library::save(true);
}

#[cfg(test)]
mod tests {
    #[cfg(all(windows, feature = "diagnostics"))]
    #[test]
    fn skip_hashes_accepts_full_hashes_only() {
        assert!(super::parse_skip_hashes(" , ").is_empty());
        assert_eq!(
            super::parse_skip_hashes(" 0123456789ABCDEF, 0123456789abcde, xyz, 0011223344556677,"),
            ["0123456789abcdef", "0011223344556677"]
        );
    }

    use super::*;
    use crate::object::{S_OK, slot};

    fn shaders(p: This) -> Vec<(&'static str, Vec<u8>)> {
        pipeline(p)
            .shaders
            .iter()
            .map(|s| (s.stage, s.container.clone()))
            .collect()
    }

    #[test]
    fn pipelines_keep_their_shaders() {
        let (vs, ps) = ([1u8, 2, 3], [4u8; 5]);
        let none = || Bytecode {
            code: std::ptr::null(),
            size: 0,
        };
        let desc = GraphicsHead {
            root: std::ptr::null(),
            stages: [
                Bytecode {
                    code: vs.as_ptr(),
                    size: vs.len(),
                },
                Bytecode {
                    code: ps.as_ptr(),
                    size: ps.len(),
                },
                none(),
                none(),
                none(),
            ],
        };
        let mut p = std::ptr::null();
        assert_eq!(
            create_graphics(std::ptr::null(), &desc, &PIPELINE_IIDS[0], &mut p),
            S_OK
        );
        assert_eq!(
            shaders(p),
            [("vertex", vs.to_vec()), ("pixel", ps.to_vec())]
        );
        // A second pipeline with the same pixel shader shares its container.
        let mut q = std::ptr::null();
        create_graphics(std::ptr::null(), &desc, &PIPELINE_IIDS[0], &mut q);
        assert!(Arc::ptr_eq(
            &pipeline(p).shaders[1],
            &pipeline(q).shaders[1]
        ));
        assert_eq!(slot::<extern "system" fn(This) -> u32>(p, 0x10)(p), 0);
    }

    /// The stream layout the title builds (a title offset): root
    /// signature, five shader slots (the first a mesh shader), blend, sample
    /// mask, rasterizer, depth-stencil 1, sample description, input layout,
    /// topology, render target and depth formats: 0x2b0 bytes.
    #[test]
    fn streams_yield_their_shaders() {
        let (ms, ps) = ([9u8; 7], [4u8; 5]);
        let mut stream = vec![0u8; 0x2b0];
        let mut put = |at: usize, v: u64| stream[at..at + 8].copy_from_slice(&v.to_le_bytes());
        for (at, kind) in [
            (0x10, 24),
            (0x28, 2),
            (0x40, 3),
            (0x58, 4),
            (0x70, 5),
            (0x88, 8),
            (0x1d8, 9),
            (0x1e0, 10),
            (0x210, 21),
            (0x250, 17),
            (0x260, 12),
            (0x278, 14),
            (0x280, 15),
            (0x2a8, 16),
        ] {
            put(at, kind);
        }
        put(0x18, ms.as_ptr() as u64);
        put(0x20, ms.len() as u64);
        put(0x30, ps.as_ptr() as u64);
        put(0x38, ps.len() as u64);
        // Topology type 3 (triangles), one render target of format 28.
        put(0x278, 14 | 3 << 32);
        put(0x280, 15 | 28 << 32);
        put(0x2a0, 1 << 32);
        let desc = StreamDesc {
            size: stream.len(),
            stream: stream.as_ptr(),
        };
        let mut p = std::ptr::null();
        let hr = create_stream(std::ptr::null(), &desc, &PIPELINE_IIDS[0], &mut p);
        assert_eq!(hr, S_OK);
        assert_eq!(shaders(p), [("mesh", ms.to_vec()), ("pixel", ps.to_vec())]);
        #[cfg(windows)]
        {
            let g = pipeline(p).host.graphics.as_deref().unwrap();
            assert_eq!(g.topology.0, 3);
            assert_eq!(
                (g.targets.NumRenderTargets, g.targets.RTFormats[0].0),
                (1, 28)
            );
        }
    }
}
