//! Pipeline states. Both descriptions start as on PC: the root signature,
//! then the shader stages (five for graphics, one for compute); streams
//! carry the rest of the graphics state as subobjects of the standard
//! types.
//!
//! Host pipelines are made on a pipeline's first draw, not at creation:
//! Chores Simulator Miami creates some 60,000 pipelines in its first 20
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

use crate::object::{self, E_INVALIDARG, HRESULT, Iid, Object, PIPELINE, Table, This, guid};
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

/// The shared container for `bytes`.
fn shader(stage: &'static str, bytes: &[u8]) -> Arc<Shader> {
    let full = blake3::hash(bytes);
    let mut shaders = SHADERS.lock().unwrap_or_else(|e| e.into_inner());
    shaders
        .get_or_insert_with(HashMap::new)
        .entry(*full.as_bytes())
        .or_insert_with(|| {
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
}

#[cfg(windows)]
impl Shader {
    /// The console container bytes, for backend-neutral draw capture.
    pub(crate) fn container_bytes(&self) -> &[u8] {
        &self.container
    }

    /// The translation, made on first use; a shader that does not
    /// translate is logged once.
    pub(crate) fn translated(&self) -> Option<&Translated> {
        self.translated
            .get_or_init(|| {
                let done = translate(&self.container);
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
    let host = crate::shader_cache::host_shader(container)?;
    let c = Container::parse(container).map_err(|e| e.to_string())?;
    let used = match c.part(FourCC::DXIL) {
        Some(part) => victory_shader::native_programs(part.data)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|p| {
                matches!(
                    p.stage,
                    HwStage::Vs | HwStage::Ps | HwStage::Ms | HwStage::Cs
                )
            })
            .map(|p| p.used)
            .unwrap_or_default(),
        None => Vec::new(),
    };
    Ok(Translated { host, used })
}

#[repr(C)]
struct Pipeline {
    object: Object,
    packet: [u32; 8],
    /// Each used stage's container.
    shaders: Vec<Arc<Shader>>,
    /// Whether a list has bound it yet (its shaders are logged then).
    bound: std::sync::atomic::AtomicBool,
    /// The rest of a stream's graphics state, for the host pipeline. Shared
    /// (`Arc`) so the background compilation job can hold it too.
    #[cfg(windows)]
    graphics: Option<Arc<host::GraphicsState>>,
    /// The host pipeline, compiled on the background pool from creation.
    #[cfg(windows)]
    host: Arc<HostSlot>,
}
const _: () = assert!(offset_of!(Pipeline, packet) == 0x10);

/// The completion state of a background host compilation. The result lives
/// in a `OnceLock` so a bind can hold a stable `&HostPipeline`; the condvar
/// only signals completion (the lock is never held across the wait).
#[cfg(windows)]
pub(crate) struct HostSlot {
    result: OnceLock<Option<host::HostPipeline>>,
    done: std::sync::Condvar,
    /// Held only to pair with `done`; the value is unused.
    lock: std::sync::Mutex<()>,
    /// Set when the pool could not take the job (queue full/closed): the
    /// result is compiled inline at first use instead. Never blocks here.
    abandoned: std::sync::atomic::AtomicBool,
}

#[cfg(windows)]
impl HostSlot {
    pub(crate) fn new() -> Self {
        Self {
            result: OnceLock::new(),
            done: std::sync::Condvar::new(),
            lock: std::sync::Mutex::new(()),
            abandoned: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Store the compilation result (success or refusal) and wake waiters.
    /// Called on EVERY pool exit path so a waiter is always released.
    pub(crate) fn complete(&self, h: Option<host::HostPipeline>) {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let _ = self.result.set(h);
        self.done.notify_all();
    }

    /// Mark for inline compilation at first use (pool queue full/closed).
    pub(crate) fn abandon(&self) {
        self.abandoned
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether the pool declined the job (compile inline at first use).
    pub(crate) fn is_abandoned(&self) -> bool {
        self.abandoned
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Whether a result has been stored (success or refusal).
    #[cfg(test)]
    pub(crate) fn done(&self) -> bool {
        self.result.get().is_some()
    }

    /// The result if compilation has finished, without waiting.
    pub(crate) fn ready(&self) -> Option<Option<&host::HostPipeline>> {
        self.result.get().map(|h| h.as_ref())
    }

    /// Wait up to `bound` for completion. `Some(result)` when done (the
    /// inner `Option` is the refusal path), `None` on timeout (still
    /// compiling — the caller counts a residual skip, work continues).
    pub(crate) fn wait(&self, bound: std::time::Duration) -> Option<Option<&host::HostPipeline>> {
        if let Some(h) = self.result.get() {
            return Some(h.as_ref());
        }
        let g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let (_g, _timeout) = self
            .done
            .wait_timeout_while(g, bound, |_| self.result.get().is_none())
            .unwrap_or_else(|e| e.into_inner());
        self.result.get().map(|h| h.as_ref())
    }
}

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

/// A shared time bucket for synchronous first-use pipeline compilation.
///
/// The loading-screen frame renders on five `Subrender` worker threads; the
/// title's hang watchdog ends the game after ~30 s without a frame. A frame
/// meeting hundreds of new pipelines whose host compilation each takes
/// 0.25-0.5 s slept all five workers past the watchdog (five-star
/// FirstUseBudget.cs / debugging.md). The bucket caps the total first-use
/// compilation time: capacity 1 s, refilling 0.25 s per second. When it is
/// empty a first use skips compilation (returns `None`, the draw/dispatch is
/// skipped) rather than blocking the worker; the pipeline compiles on a
/// later bind once the bucket refills. Mechanism after five-star (GPL);
/// implementation ours.
#[cfg(windows)]
mod first_use_budget {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    const CAPACITY: Duration = Duration::from_secs(1);
    const REFILL_PER_SEC: f64 = 0.25;
    /// A single bind never waits longer than this, so one slow compilation
    /// cannot sleep a Subrender worker toward the hang watchdog.
    const PER_WAIT_CAP: Duration = Duration::from_millis(250);
    // (available time, last refill instant); lazily started on first use.
    static BUCKET: Mutex<Option<(Duration, Instant)>> = Mutex::new(None);

    fn refill(state: &mut (Duration, Instant)) {
        let now = Instant::now();
        let add = now.duration_since(state.1).as_secs_f64() * REFILL_PER_SEC;
        state.0 = (state.0 + Duration::from_secs_f64(add)).min(CAPACITY);
        state.1 = now;
    }

    /// The bound on THIS bind's inline wait: the available budget, capped
    /// per wait. This bounds the wait only — compilation itself always runs
    /// on the background pool regardless of the bucket (five-star semantics).
    pub fn wait_bound() -> Duration {
        let mut g = BUCKET.lock().unwrap_or_else(|e| e.into_inner());
        let state = g.get_or_insert((CAPACITY, Instant::now()));
        refill(state);
        state.0.min(PER_WAIT_CAP)
    }

    /// Charge `elapsed` inline-wait time to the bucket.
    pub fn spend(elapsed: Duration) {
        let mut g = BUCKET.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(state) = g.as_mut() {
            refill(state);
            state.0 = state.0.saturating_sub(elapsed);
        }
    }
}

/// Residual first-use binds whose background compilation had not finished
/// within the inline-wait bound. The work is dropped THIS bind only (no
/// stand-in); compilation continues on the pool and the next bind finds it
/// ready. Distinct from genuine refusals so it cannot mask them. With
/// compilation kicked at creation this should be ~0.
#[cfg(windows)]
static BUDGET_SKIPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Total residual inline-wait timeouts so far.
#[cfg(windows)]
pub(crate) fn budget_skips() -> u64 {
    BUDGET_SKIPS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Binds that had to wait inline (queued result not ready, or inline
/// fallback). Diagnostic for the first post-ban run.
#[cfg(windows)]
static INLINE_WAITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Total microseconds spent in inline waits (queued waits + fallback compiles).
#[cfg(windows)]
static INLINE_WAIT_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(windows)]
pub(crate) fn inline_waits() -> u64 {
    INLINE_WAITS.load(std::sync::atomic::Ordering::Relaxed)
}
#[cfg(windows)]
pub(crate) fn inline_wait_us() -> u64 {
    INLINE_WAIT_US.load(std::sync::atomic::Ordering::Relaxed)
}

/// The host pipeline of a title pipeline. Compilation is kicked on the
/// background pool at creation (`hand_out`); here we wait only on the
/// in-flight result, bounded by the time budget (five-star semantics — the
/// budget bounds the inline WAIT, never licences skipping compilation).
/// `None` if a shader does not translate or the host refuses it (the
/// refusal path), or — counted as a residual skip — if the bound expires
/// before the background result lands.
#[cfg(windows)]
pub(crate) fn host_pipeline<'a>(this: This) -> Option<&'a host::HostPipeline> {
    if this.is_null() {
        return None;
    }
    let p = pipeline(this);
    // Already compiled (the common case by bind time): no wait, no cost.
    if let Some(h) = p.host.ready() {
        return h;
    }
    let start = std::time::Instant::now();
    INLINE_WAITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // The pool declined the job (queue full/closed): compile inline here at
    // first use. This is the explicit, counted fallback — never a blocking
    // enqueue on the title's CreatePipelineState thread.
    if p.host.is_abandoned() {
        let h = host::HostPipeline::new(&p.shaders, p.graphics.as_deref());
        p.host.complete(h);
        INLINE_WAIT_US.fetch_add(start.elapsed().as_micros() as u64, std::sync::atomic::Ordering::Relaxed);
        return p.host.ready().flatten();
    }
    // Queued: wait only on the in-flight result, bounded by the budget.
    let bound = first_use_budget::wait_bound();
    let result = p.host.wait(bound);
    first_use_budget::spend(start.elapsed());
    INLINE_WAIT_US.fetch_add(start.elapsed().as_micros() as u64, std::sync::atomic::Ordering::Relaxed);
    match result {
        Some(h) => h, // done: success or genuine refusal
        None => {
            // Bound expired, compilation still in flight. Explicit + counted;
            // the caller drops this bind's work (no stand-in) but the pool
            // finishes it, so the next bind is ready — not a permanent skip.
            let n = BUDGET_SKIPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if n % 100 == 1 {
                tracing::warn!(
                    skips = n,
                    "inline wait expired: bind deferred, compilation continues"
                );
            }
            None
        }
    }
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
    let graphics = graphics.map(Arc::from);
    #[cfg(windows)]
    let slot = Arc::new(HostSlot::new());
    #[cfg(windows)]
    {
        // Kick host compilation on the bounded background pool NOW (creation
        // time), so by bind time the result is usually ready and no Subrender
        // worker sleeps on a synchronous Create*PipelineState. Inputs are
        // Send (Arc<Shader>, Arc<GraphicsState>); errors take the refusal
        // path inside HostPipeline::new, a panic is caught by the pool, and
        // device loss surfaces as a refused result — never a crash.
        if crate::mitigations::get().pool {
            let job_shaders = shaders.clone();
            let job_graphics = graphics.clone();
            // The pool sets the slot on success, error AND panic; a full queue
            // falls back to inline compilation at first use (never blocks here).
            crate::hostpool::submit(
                move || host::HostPipeline::new(&job_shaders, job_graphics.as_deref()),
                Arc::clone(&slot),
            );
        } else {
            // Attribution: pool OFF => the pre-pool synchronous path. Mark the
            // slot abandoned so host_pipeline compiles inline at first use
            // (no background thread, no budget wait). All-on is unchanged.
            slot.abandon();
        }
    }
    let pipeline = Box::new(Pipeline {
        object: Object::new(&FUNCTIONS),
        packet: [0; 8],
        shaders,
        bound: std::sync::atomic::AtomicBool::new(false),
        #[cfg(windows)]
        graphics,
        #[cfg(windows)]
        host: slot,
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
    let mut graphics = host::GraphicsStateBuilder::default();
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
        Some(Box::new(graphics.build())),
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

#[cfg(test)]
mod tests {
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

    /// The stream layout Chores Simulator Miami builds (a title offset): root
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
            let g = pipeline(p).graphics.as_deref().unwrap();
            assert_eq!(g.topology.0, 3);
            assert_eq!(
                (g.targets.NumRenderTargets, g.targets.RTFormats[0].0),
                (1, 28)
            );
        }
    }
}
