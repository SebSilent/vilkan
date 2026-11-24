//! Wave64 compute on a 32-lane host: the round-scheduler exchange, backend-neutral.
//!
//! A console wave64 program run on a host whose waves are 32 lanes wide cannot
//! use subgroup shuffles for cross-lane traffic (the host refuses `WaveSize(64)`,
//! and a 32-lane shuffle wraps a lane select >= 32). Following the round-scheduler model
//! `--wave64-emulate` (`docs/design.md`, "Wave64 emulation"), one invocation is
//! one console lane and every cross-lane value goes through thread-group shared
//! memory: a lane stores its value, the group meets at a barrier, then each lane
//! reads the slot of the lane it wants. No subgroup operation is emitted, so the
//! same plan lowers to DXIL (`groupshared` + `GroupMemoryBarrierWithGroupSync`)
//! or SPIR-V (`Workgroup` + `OpControlBarrier`) unchanged.
//!
//! This module is the part that does not depend on the backend:
//!
//! * [`classify`] — the gating predicate, with a reason when it rejects;
//! * [`plan`] — cut the cross-lane ops into exchanges (segments) and allocate
//!   their shared-memory slots, with the batching hazard rule;
//! * [`simulate`] / [`reference`] — a CPU model of the exchange protocol and the
//!   console semantics, the correctness oracle (a wrong slot index or a stale
//!   parity reads the sentinel and fails, instead of rendering wrong pixels).
//!
//! The narrow slice handles straight-line, single-wave (<= 64 threads),
//! mask-free compute — the two captured blur shaders. Branchy CFG, multi-wave
//! groups and lane masks are rejected with a reason (the general round-scheduler
//! and mask ring are later work).

use std::collections::HashMap;

use crate::ir::Value;

/// The console wave width the exchange reproduces.
pub const LANES: usize = 64;

/// A cross-lane operation, abstracted from the IR so the planner and the oracle
/// do not depend on the machine encoding or the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossOp {
    /// Position in the instruction stream (for the hazard rule).
    pub pos: usize,
    pub kind: CrossKind,
    /// The per-lane source value (a VGPR's SSA value).
    pub src: Value,
    /// Where `src` is defined (its defining instruction's position).
    pub src_def: usize,
    /// The lane(s) read.
    pub lane: LaneSel,
    /// The result value (for a dependent op's hazard check).
    pub dst: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossKind {
    /// `v_readlane_b32`: read one lane's value (broadcast to a scalar).
    ReadLane,
    /// `v_writelane_b32`: write one lane's value (scatter).
    #[allow(dead_code)] // exercised by the oracle tests; the IR extraction is read-side
    WriteLane,
    /// `ds_swizzle_b32`: each lane reads lane `((lane & and) | or) ^ xor`.
    Swizzle,
}

/// The source lane a cross-lane op reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneSel {
    /// A constant lane (masked to 0..63 by the caller).
    Const(u32),
    /// A per-lane computed lane (the value holds the lane index per invocation).
   Computed(Value),
    /// `ds_swizzle` bitmask mode: `((lane & and) | or) ^ xor`, within 32 lanes.
    Swizzle { and: u32, or: u32, xor: u32 },
}

impl LaneSel {
    #[allow(dead_code)] // used by the CPU oracle
    /// The source lane for invocation `lane`, given per-lane computed values.
    pub fn target(&self, lane: usize, computed: &HashMap<Value, Vec<u32>>) -> usize {
        match *self {
            LaneSel::Const(k) => (k as usize) & (LANES - 1),
            LaneSel::Computed(v) => {
                (computed[&v][lane] as usize) & (LANES - 1)
            }
            LaneSel::Swizzle { and, or, xor } => {
                (((lane as u32 & (and | 0xFFFF_FFE0)) | or) ^ xor) as usize & (LANES - 1)
            }
        }
    }
}

/// Why a wave64 shader is not taken by the narrow exchange path. Logged so the
/// next refusal is diagnosable (addition 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// The program is not wave64 (nothing to do).
    NotWave64,
    /// The host runs 64-lane waves: translate natively.
    HostWave64,
    /// Not a compute shader (VS/PS/GS have no workgroup memory).
    NotCompute,
    /// The group spans more than one console wave (> 64 threads).
    MultiWave,
    /// The CFG has branches/loops (needs the round-scheduler).
    BranchyCfg,
    /// The code reads lane masks as data / manipulates EXEC (needs the mask ring).
    HasMasks,
}

impl Reject {
    pub fn reason(self) -> &'static str {
        match self {
            Reject::NotWave64 => "not wave64",
            Reject::HostWave64 => "host runs 64-lane waves",
            Reject::NotCompute => "not compute (no workgroup memory)",
            Reject::MultiWave => "multi-wave group (> 64 threads)",
            Reject::BranchyCfg => "branchy CFG (needs the round-scheduler)",
            Reject::HasMasks => "lane masks / EXEC manipulation (needs the mask ring)",
        }
    }
}

/// The gating decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Emulate,
    Reject(Reject),
}

/// The facts the predicate needs, gathered from the IR / native program.
#[derive(Debug, Clone, Copy)]
pub struct Facts {
    pub wave64: bool,
    pub host_wave64: bool,
    pub compute: bool,
    /// Product of the thread-group dimensions.
    pub threads: u32,
    /// Every block falls through or ends (no branch/loop/indirect exit).
    pub straight_line: bool,
    /// No lane mask read as data and no EXEC narrowing.
    pub mask_free: bool,
}

/// The narrow gating predicate. The first failing condition is the reason.
pub fn classify(f: Facts) -> Decision {
    if !f.wave64 {
        return Decision::Reject(Reject::NotWave64);
    }
    if f.host_wave64 {
        return Decision::Reject(Reject::HostWave64);
    }
    if !f.compute {
        return Decision::Reject(Reject::NotCompute);
    }
    if f.threads > LANES as u32 {
        return Decision::Reject(Reject::MultiWave);
    }
    if !f.straight_line {
        return Decision::Reject(Reject::BranchyCfg);
    }
    if !f.mask_free {
        return Decision::Reject(Reject::HasMasks);
    }
    Decision::Emulate
}

/// One published source in an exchange: its SSA value and its slot column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    pub src: Value,
    /// Where `src` is defined; the oracle only publishes it if defined before
    /// the barrier (a value defined later does not exist yet at the barrier).
    pub src_def: usize,
    /// Column index (0..width); the slot is `parity*width*LANES + col*LANES + lane`.
    pub col: usize,
}

/// A read performed after the exchange's barrier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    /// Index into the plan's `ops` (the cross-lane op this read serves).
    pub op: usize,
    /// Column of the source publication.
    pub col: usize,
    /// The parity the read uses (the emitter computes it; normally the
    /// exchange's). A read of the other parity sees the previous buffer.
    pub parity: usize,
    pub lane: LaneSel,
}

/// One exchange: publish sources, barrier, read. `parity` alternates 0/1 so a
/// reader never sees the previous exchange's data (double buffering).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exchange {
    pub parity: usize,
    /// Position of the first op: the barrier sits here, after the publications.
    pub barrier_pos: usize,
    pub publications: Vec<Publication>,
    pub reads: Vec<Read>,
}

/// The whole plan: the exchanges in order and the shared-memory layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExchangePlan {
    pub exchanges: Vec<Exchange>,
    /// Max distinct sources published by any exchange.
    pub width: usize,
    /// Dwords of thread-group shared memory the exchange needs.
    pub lds_dwords: usize,
}

impl ExchangePlan {
    #[allow(dead_code)] // used by the CPU oracle
    /// The shared-memory slot for (parity, column, lane).
    pub fn slot(&self, parity: usize, col: usize, lane: usize) -> usize {
        parity * self.width * LANES + col * LANES + lane
    }
}

/// Cut the cross-lane ops into exchanges and allocate slots.
///
/// **Batching hazard rule (addition 2).** An op joins the current exchange iff
/// its source was defined *before* the exchange's first op (`src_def <
/// first_pos`). In SSA a value defined before the exchange is never rewritten
/// inside it, so all such ops read stable values that can be published once,
/// behind one barrier. An op whose source is defined at or after the exchange's
/// first op must start a new exchange: either the source was (re)written since
/// the publication (a write to a published source), or it is the result of an
/// earlier cross-lane op in this exchange (a dependent cross-lane op) — in both
/// cases the value does not exist yet at the barrier, so reading it from the
/// shared slot would return stale data. Closing the exchange places a barrier
/// before the dependent read, exactly where the value becomes available.
pub fn plan(ops: &[CrossOp]) -> ExchangePlan {
    let mut exchanges: Vec<Exchange> = Vec::new();
    let mut width = 0usize;
    for (i, op) in ops.iter().enumerate() {
        let join = match exchanges.last() {
            // The first op of an exchange always publishes its own source.
            None => false,
            Some(ex) => {
                let first_pos = ops[ex.reads[0].op].pos;
                op.src_def < first_pos
            }
        };
        if !join {
            let parity = exchanges.len() & 1;
            exchanges.push(Exchange {
                parity,
                barrier_pos: op.pos,
                publications: Vec::new(),
                reads: Vec::new(),
            });
        }
        let ex = exchanges.last_mut().expect("just opened");
        // Reuse a column if this source is already published, else add one.
        let col = match ex.publications.iter().find(|p| p.src == op.src) {
            Some(p) => p.col,
            None => {
                let col = ex.publications.len();
                ex.publications.push(Publication { src: op.src, src_def: op.src_def, col });
                col
            }
        };
        let parity = ex.parity;
        ex.reads.push(Read { op: i, col, parity, lane: op.lane });
        width = width.max(ex.publications.len());
    }
    // Double-buffered data area: two parities × width columns × LANES.
    let lds_dwords = 2 * width * LANES;
    ExchangePlan { exchanges, width, lds_dwords }
}

/// The sentinel a slot holds before any exchange writes it. A read that lands
/// on a slot this exchange did not publish (wrong column, wrong lane, or a stale
/// parity) returns this, so the oracle fails loudly instead of matching by luck.
pub const SENTINEL: u32 = 0xDEAD_BEEF;

/// CPU model of the exchange protocol: execute the plan over a simulated
/// thread-group buffer, returning each cross-lane op's per-lane result.
///
/// `inputs` gives every source value's per-lane content (length `LANES`). The
/// buffer starts at the sentinel and is never cleared: an exchange writes only
/// its own publications into its parity region, so a read of an unpublished slot
/// (a batching/parity bug) yields the sentinel.
#[allow(dead_code)] // the CPU correctness oracle, exercised by tests
pub fn simulate(
    plan: &ExchangePlan,
    ops: &[CrossOp],
    inputs: &HashMap<Value, Vec<u32>>,
) -> Vec<Vec<u32>> {
    let mut buf = vec![SENTINEL; plan.lds_dwords.max(1)];
    let mut results = vec![vec![0u32; LANES]; ops.len()];
    for ex in &plan.exchanges {
        // Publish: every lane stores its value of each source into its slot —
        // but only a source defined before the barrier exists yet. A source
        // defined later (an unsafe batch) is not published, so its read below
        // lands on the sentinel: the oracle catches the hazard.
        for pub_ in &ex.publications {
            if pub_.src_def >= ex.barrier_pos {
                continue;
            }
            let vals = &inputs[&pub_.src];
            for lane in 0..LANES {
                let slot = plan.slot(ex.parity, pub_.col, lane);
                buf[slot] = vals[lane];
            }
        }
        // (GroupMemoryBarrierWithGroupSync / OpControlBarrier here.)
        // Read: each op reads its source's slot at the lane it wants.
        for read in &ex.reads {
            let op = &ops[read.op];
            for lane in 0..LANES {
                let target = op.lane.target(lane, inputs);
                let slot = plan.slot(read.parity, read.col, target);
                results[read.op][lane] = buf[slot];
            }
        }
    }
    results
}

/// Console reference: a cross-lane op reads the source lane's value directly,
/// with all 64 lanes available (no exchange, no slots).
#[allow(dead_code)] // the console reference, exercised by tests
pub fn reference(ops: &[CrossOp], inputs: &HashMap<Value, Vec<u32>>) -> Vec<Vec<u32>> {
    ops.iter()
        .map(|op| {
            (0..LANES)
                .map(|lane| {
                    let target = op.lane.target(lane, inputs);
                    inputs[&op.src][target]
                })
                .collect()
        })
        .collect()
}

/// Gather the gating facts from the IR and the native program's stage/threads.
pub fn facts(p: &crate::ir::Program, compute: bool, threads: u32, host_wave64: bool) -> Facts {
    use crate::ir::{Exit, Reg, EXEC, VCC};
    let straight_line = p
        .blocks
        .iter()
        .all(|b| matches!(b.exit, Exit::End | Exit::Jump(_)));
    // Mask-free: no EXEC/VCC mask write, no compare-into-EXEC, no saveexec /
    // wrexec, and no instruction that reads a lane mask as a number.
    let mask_free = p.blocks.iter().flat_map(|b| &b.insts).all(|inst| {
        let n = inst.name();
        let writes_mask = inst
            .writes
            .iter()
            .any(|(r, _)| matches!(r, Reg::S(c) if *c == EXEC || *c == EXEC + 1 || *c == VCC || *c == VCC + 1));
        let mask_op = n.starts_with("v_cmpx")
            || n.contains("saveexec")
            || n.contains("wrexec")
            || n.starts_with("v_mbcnt")
            || n.starts_with("s_bcnt1_i32")
            || n.starts_with("s_ff1_i32")
            || n.starts_with("s_wqm");
        !writes_mask && !mask_op
    });
    Facts { wave64: p.wave64, host_wave64, compute, threads, straight_line, mask_free }
}

/// Extract the cross-lane ops of a straight-line program, in order, with their
/// instruction offsets and source definition positions. `None` if an op is
/// outside the narrow slice (a quad-mode swizzle, a writelane, a non-VGPR
/// source). The caller maps offsets back to instructions for emission.
pub fn extract(p: &crate::ir::Program) -> Option<Vec<(usize, CrossOp)>> {
    use crate::gfx10::{Fields, Operand};
    use crate::ir::Reg;
    if p.blocks.len() != 1 {
        return None;
    }
    let block = &p.blocks[0];
    // Where each value is defined: inputs at 0, then each write's position.
    let mut def_pos: HashMap<Value, usize> = p.inputs.iter().map(|(_, v)| (*v, 0)).collect();
    let mut out = Vec::new();
    for (pos, inst) in block.insts.iter().enumerate() {
        for (_, v) in &inst.writes {
            def_pos.insert(*v, pos);
        }
        let offset = inst.machine.offset;
        let name = inst.name();
        let op = match name {
            "ds_swizzle_b32" => {
                let Fields::Ds { addr, offset0, offset1, .. } = &inst.fields else {
                    return None;
                };
                let off = u32::from(*offset0) | u32::from(*offset1) << 8;
                if off & 0x8000 != 0 {
                    return None; // quad mode: outside the narrow slice
                }
                let Operand::Vector { reg, .. } = addr.as_ref()? else {
                    return None;
                };
                let src = inst.read(Reg::V(*reg))?;
                let dst = inst.writes.first().map(|(_, v)| *v)?;
                CrossOp {
                    pos,
                    kind: CrossKind::Swizzle,
                    src,
                    src_def: *def_pos.get(&src)?,
                    lane: LaneSel::Swizzle {
                        and: off & 0x1F,
                        or: off >> 5 & 0x1F,
                        xor: off >> 10 & 0x1F,
                    },
                    dst,
                }
            }
            "v_readlane_b32" => {
                let Fields::Valu(v) = &inst.fields else {
                    return None;
                };
                let Operand::Vector { reg, .. } = v.src.first()? else {
                    return None;
                };
                let src = inst.read(Reg::V(*reg))?;
                let lane = match v.src.get(1)? {
                    Operand::Const(c) => LaneSel::Const(crate::resource::inline(*c)?),
                    Operand::Literal(k) => LaneSel::Const(*k),
                    Operand::Scalar { code, .. } => {
                        LaneSel::Computed(inst.read(Reg::S(*code))?)
                    }
                    _ => return None,
                };
                let dst = inst.writes.first().map(|(_, v)| *v)?;
                CrossOp {
                    pos,
                    kind: CrossKind::ReadLane,
                    src,
                    src_def: *def_pos.get(&src)?,
                    lane,
                    dst,
                }
            }
            _ => continue,
        };
        out.push((offset, op));
    }
    Some(out)
}

#[cfg(test)]
mod tests;