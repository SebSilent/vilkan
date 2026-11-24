# Multi-wave / cross-lane compute milestone — AUDIT & DESIGN (no implementation yet)

Status: **audit + design only**, written under the GPU ban (no launches). Owner
approval required before any emitter code. Nothing here is implemented.

The narrow wave64 exchange (`crates/victory-shader/src/emit/exchange.rs`) emulates
only single-wave (≤64 threads), straight-line, mask-free compute with
`ds_swizzle_b32` (non-quad) / `v_readlane_b32`. Everything else is declined and,
when the emitted single-wave DXIL is invalid for the host, the compute PSO is
refused. This milestone widens that slice. `requiredSubgroupSize=64` is **ruled
out** by the host probe (host min=max subgroup 32) and is not revisited.

## 1. What we decline — measured from existing artefacts (no new runs)

Sources: `vilkan-data/baseline-e7a9a2e/runM3-vilkan-out.log` and `runK2-…`
(ANSI-stripped), `vilkan-data/cache/refused/compute-<hash>.{bin,dxil}`
(`dump_refused`, `pipeline/host.rs:325`). All 23 M3-refused PSOs have artefacts.

Two DISTINCT events, do not conflate:
- **Exchange decline** — `lower.rs:434` WARN `wave64 exchange declined
  reason="…"`. The exchange path did not take the shader. Logged once per
  shader via `Shader.translated: OnceLock` (`pipeline.rs:65,112`).
- **Host refusal** — `pipeline/host.rs:485` WARN `host pipeline refused
  error=Invalid parameter. (0x80070057)` after `CreateComputePipelineState`
  fails; `dump_refused` writes the `.bin`/`.dxil`. This is the "23".

Aggregate counts (the "23 multi-wave" label in earlier notes was WRONG — the
bulk is **branchy CFG**, not multi-wave):

| run | exchange declines | branchy CFG | multi-wave | host-refused PSOs |
|-----|------------------|-------------|------------|-------------------|
| M3  | 26               | 24          | 2          | **23**            |
| K2  | 25               | 22          | 3          | 21                |
| victory-out (older) | 24   | 22          | 2          | 17                |

`classify` (`exchange.rs:142`) checks in order: NotWave64 → HostWave64 →
NotCompute → **MultiWave (threads>64)** → **BranchyCfg (!straight_line)** →
**HasMasks (!mask_free)**. MultiWave is checked BEFORE branchy/masks, so a
>64-thread group is reported multi-wave even if it is also branchy. The 24
branchy declines are ≤64-thread groups with loops/branches.

`facts` (`exchange.rs:336`): `straight_line` = every block exit is End/Jump;
`mask_free` = no EXEC/VCC write, no `v_cmpx*`/`saveexec`/`wrexec`/`v_mbcnt*`/
`s_bcnt1_i32*`/`s_ff1_i32*`/`s_wqm*`. `extract` (`exchange.rs:366`) only takes
`ds_swizzle_b32` (offset bit 0x8000 clear) and `v_readlane_b32`; any other
cross-lane op (writelane, DPP, permute, quad swizzle) → `None`.

### ROOT CAUSE of the 23 refusals — CONFIRMED (byte evidence, no new runs)

**The emitter declares `WaveSize(64)` in PSV0 for every declined wave64 compute
shader; the 32-wide host rejects it with E_INVALIDARG.** Read from the refused
`.dxil` PSV0 runtime-info (wave min at info[16], max at info[20]):

| hash | PSV0 wave_min | wave_max | numthreads |
|------|--------------|----------|------------|
| 02384a60… | **64** | **64** | (128,1,1) |
| 339d476c… | **64** | **64** | (64,1,1) |
| 0e90505e… | **64** | **64** | (8,8,1) |
| 196bf611… | **64** | **64** | (64,1,1) |

Host probe: `wave_min=32 wave_max=32 wave64=false` (M3 log). Mechanism:
`lower.rs:393-397` sets `s.wave_size = Some(64)` when `console_lanes ||
(one_wave && lds_dwords>0)`. The exchange sets `wave_size = None`
(`lower.rs:420`) ONLY when `classify==Emulate` AND `extract` succeeds. A
DECLINED shader (branchy/multi-wave/masks) keeps `Some(64)` →
`container.rs:164` writes min=max=64 → host refuses. **So the refusal is a
wave-size declaration mismatch, not primarily a missing cross-lane op.**

`console_lanes` (`lower.rs:361-368`) fires on: `ds_swizzle*`, `*permute*`,
`v_mbcnt*`, `v_readlane_b32`, `v_writelane_b32`, DPP, `reads_mask_as_data`,
lane-mask branch guards, OR single-wave LDS sharing. **This is why 13/23 with
NO cross-lane op still refuse:** they read lane masks as data / branch on lane
masks / share LDS within a wave, so they need the console's 64-lane order.

### Per-PSO class table (23 M3-refused) — measured

Read-only analysis tool over `cache/refused/*.bin` (group size from
`native_programs` compute `threads`, ops + facts from `ir::build`). Audit
tooling, not emitter code; untracked. ALL 23 are wave64 and mask-free=false.
Class totals: **22 BranchyCfg, 1 MultiWave**. Group sizes: 1×32, 21×64, 1×128.
Cross-lane: 10 have ops, 13 have none (mask/LDS-driven).

| hash | threads | wave64 | straight | mask-free | cross-lane ops | class | blocks |
|------|---------|--------|----------|-----------|----------------|-------|--------|
| 02384a60fda6f2d0 | [128,1,1] | true | false | false | v_readlane_b32 | MultiWave | 77 |
| 0e90505eff274aa4 | [8,8,1] | true | false | false | (none) | BranchyCfg | 8 |
| 196bf6114ff3a131 | [64,1,1] | true | false | false | (none) | BranchyCfg | 9 |
| 1f1fbd86759b3da2 | [8,8,1] | true | false | false | v_permlanex16_b32,v_readlane_b32 | BranchyCfg | 105 |
| 2dc602192abf6288 | [8,8,1] | true | false | false | ds_swizzle_b32,v_readlane_b32 | BranchyCfg | 8 |
| 2f03a2df10503cef | [8,8,1] | true | false | false | v_permlanex16_b32,v_readlane_b32,ds_swizzle_b32 | BranchyCfg | 75 |
| 339d476c267c03c7 | [64,1,1] | true | false | false | (none) | BranchyCfg | 12 |
| 369cbe906ea72764 | [64,1,1] | true | false | false | (none) | BranchyCfg | 22 |
| 42683eebec081520 | [8,8,1] | true | false | false | ds_swizzle_b32,v_readlane_b32 | BranchyCfg | 89 |
| 5b2ce592c3a5dd09 | [8,8,1] | true | false | false | (none) | BranchyCfg | 8 |
| 6336cb3769fc53d7 | [8,8,1] | true | false | false | (none) | BranchyCfg | 17 |
| 78c04f8d197a2904 | [32,2,1] | true | false | false | (none) | BranchyCfg | 14 |
| 82849f7a614208eb | [8,8,1] | true | false | false | ds_swizzle_b32,v_readlane_b32 | BranchyCfg | 8 |
| 9afd549c4f530901 | [8,8,1] | true | false | false | ds_swizzle_b32 | BranchyCfg | 4 |
| a1f6cdf8874960a9 | [64,1,1] | true | false | false | (none) | BranchyCfg | 45 |
| b96880482a17f632 | [8,8,1] | true | false | false | (none) | BranchyCfg | 136 |
| bfad8c6743ac5977 | [8,8,1] | true | false | false | v_permlanex16_b32,v_readlane_b32 | BranchyCfg | 10 |
| c4183d3cbee28e8e | [64,1,1] | true | false | false | v_readlane_b32,ds_swizzle_b32 | BranchyCfg | 225 |
| c9da71da40af2e28 | [64,1,1] | true | false | false | v_permlanex16_b32,v_readlane_b32 | BranchyCfg | 122 |
| d3e5b737ea7cc150 | [4,4,4] | true | false | false | (none) | BranchyCfg | 183 |
| e24dff38794241ab | [16,4,1] | true | false | false | (none) | BranchyCfg | 12 |
| e798cc38df9dabc0 | [8,8,1] | true | false | false | (none) | BranchyCfg | 8 |
| e81fd01512ba1002 | [32,1,1] | true | false | false | (none) | BranchyCfg | 8 |

Cross-lane op census across the 23: `v_readlane_b32` (10), `ds_swizzle_b32` (7),
`v_permlanex16_b32` (6). No writelane/DPP/permute in the refused set (those
appear in the wider declined set). The dominant need is **branchy CFG + EXEC
mask emulation**, then readlane/swizzle/permlanex16 cross-lane.

## 2. Leonida wave64 audit — clean-room, SPIR-V-only emission

Reference: the local Leonida clone (`../leonida`, dead, scavenge-only). Core in
`tools/rdna2spirv.py` (18,229 lines, SPIR-V assembly string emitter) plus three
analysis modules. **Verdict: clean-room, ISA-derived, algorithmic — NOT
title-fitted.** Constants are ISA constants (0xFFFF lane masks, 32/64 widths,
DPP control encodings), not game offsets.

| module | role | DXIL reuse |
|--------|------|-----------|
| `tools/lane_masks.py` (304 ln) `analyse` | EXEC/lane-mask dataflow: which SGPR holds mask vs data, parked masks | **design reusable** (target-independent analysis) |
| `tools/wave64_emulate_opt.py` (1,812 ln) `scc_live_outs:397`, `find_local_exec_folds:1118`, `plan_structured_emulation:1481`, `_postdominators:682` | SCC liveness, exec-guarded folding, structured-emulation plan | **design reusable** (IR-level, target-independent) |
| `tools/wave64_halves.py` (1,758 ln) `Analysis:263`, `analyse:1675` | wave64 VS/PS/GS partition-invariance prover (tiers A/B/C): prove a graphics shader is wave-size-invariant, else reject | **design reusable**; graphics is prove-or-reject, NOT emulated |
| `rdna2spirv.py` `x_round:5228`, `x_scheduler:17347`, `x_round_loop_head:17259`, `x_memory_order:5345` | round-scheduler: uniform-control-flow loop + OpSwitch on `%pc`, groupshared exchange buffer, cross-half memory ordering | **algorithm reusable as design**; emission is SPIR-V `OpSwitch`/`OpGroupNonUniform*`/ext-inst — NOT transferable as code to DXIL |
| `rdna2spirv.py` `swizzle:2020`, `ds_permute:5430`, `dpp_insn:6433`, `readlane/writelane:7880` | cross-lane op semantics | semantics reusable; SPIR-V emission not |

**Boundary:** the analyses (lane_masks, emulate_opt, halves) and the round-
scheduler ALGORITHM are target-independent and reusable as designs for a DXIL
target. The emission layer is SPIR-V assembly strings (subgroup builtins,
`OpGroupNonUniform*`, GLSL.std.450) — DXIL has no subgroup ops at wave64 on a
32-wide host, so emission must be rewritten as HLSL/groupshared intrinsics.
**Facts and clean code only; anything imported is scrubbed and owned.**

Title strings to scrub if imported: `rdna2spirv.py:3086,3109,15550`
(ScreenProbes shader names in comments). The three analysis modules are
grep-clean. Non-wave64 paths (buffer/format/texture) carry silicon-calibrated
tables — NOT audited clean, do not import those.

## 3. Design options for a fixed 32-wide host

`requiredSubgroupSize=64` ruled out (probe). **Reframed by the root cause:** the
23 refusals are a `WaveSize(64)` PSV0 declaration the 32-wide host rejects. The
exchange already produces a valid `wave_size=None` shader when it takes a
program. So the milestone is: **take the declined wave64 compute shaders into a
correct 32-wide emission that declares no fixed wave size** — emulating the
console's 64-lane order, EXEC masks, within-wave LDS sharing and cross-lane ops
through groupshared memory + a scheduler. Merely clearing `wave_size` without
emulating the 64-lane semantics would be WRONG (masks/LDS/lane order assume 64).
Options the audit surfaces:

### Option A — groupshared emulation of ≤64-thread groups (extend current)
Keep ≤64-thread groups; take the branchy/masked ones into the exchange path so
`wave_size=None`. Widen `extract` to ALL cross-lane ops (writelane, DPP,
permute, permlanex16, quad swizzle), add a round-scheduler for branchy CFG
(Leonida `x_round` design), and a mask ring for EXEC/lane-mask-as-data (Leonida
`lane_masks.py` design). Within-wave LDS sharing emulated via groupshared +
barrier. **Must cover the 13 no-cross-lane shaders too** (mask/LDS-driven), not
just the 10 with ops.
- **Cost:** medium-high. Reuses our exchange buffer + plan; add op coverage, a
  scheduler, and mask/LDS emulation. No host subgroup dependency.
- **Risk:** branchy CFG with divergent barriers can deadlock if the scheduler
  is not uniform-control-flow; mask handling is the hard part; the CPU oracle
  must be extended to branchy/masked programs to validate.
- **Breaks:** nothing existing (additive). Does NOT fix the 1 multi-wave
  (>64) group in M3 (02384a60, 128 threads) — that still declines.
- **Covers:** the 22 BranchyCfg refusals (the bulk of the 23).

### Option B — split >64-thread groups into ≤64 sub-groups
For multi-wave groups, partition the workgroup into console-wave-sized chunks
and run each as a 32-wide host group, emulating cross-wave traffic through
groupshared + a barrier between chunks.
- **Cost:** high. Cross-wave (not just cross-lane) traffic needs a second
  exchange tier; groupshared LDS budget doubles; barrier semantics across
  sub-groups must match the console's workgroup barrier.
- **Risk:** LDS overflow (host limit 32KB/group); a cross-wave reduction needs
  all sub-groups resident — host may not co-schedule them; deadlock.
- **Breaks:** LDS layout assumptions; possibly the dispatch geometry.
- **Covers:** the 2-3 multi-wave groups only. Small payoff, high risk.

### Option C — full round-scheduler + mask ring (Leonida design, DXIL emission)
Port the Leonida structured-emulation plan (`plan_structured_emulation`) and
round-scheduler to our IR, emit HLSL with a `%pc`-switch loop and groupshared
exchange + mask ring. Handles multi-wave AND branchy AND masks uniformly.
- **Cost:** highest. This is essentially the emitter milestone; new DXIL
  emission for the scheduler, masks, all cross-lane ops.
- **Risk:** largest correctness surface; needs the CPU oracle extended to
  multi-wave/branchy/masked programs to validate.
- **Breaks:** nothing if additive, but it is a new emitter path (owner said
  owning our emitter is a deliberate milestone — this IS that).
- **Covers:** all 26 declines + 23 refusals in principle.

### Recommendation (pending owner)
**A first** — branchy CFG is 22/23 of the refusals, reuses our exchange buffer,
additive, no host subgroup dependency. It must include mask + within-wave LDS
emulation (the 13 no-cross-lane shaders), so it is effectively the Leonida
structured-emulation design restricted to ≤64-thread groups, emitted as DXIL.
Measure, then decide B/C for the residual 1 multi-wave group. C (full multi-wave
+ scheduler) is the long-term emitter milestone and gets its own deliberate
go-ahead. Do NOT detour into a full emitter while A is unmeasured
(reuse-before-rebuild). **A and C share the same core (scheduler + mask ring +
groupshared exchange); A is C scoped to single-wave.** Building A's core IS
building C's core incrementally — not a throwaway detour.

## 4. Numeric acceptance (measured in a run, not a unit test alone)

Judged on ≥2 K2-selection runs (ratios, shared binary), masking checks so a
"fixed" run cannot pass by doing less:
- **multi-wave declines: 0** (the 2-3 → 0) — only if B/C taken; A leaves these.
- **compute refusals: 23 → 0** (M3 baseline 23 host-refused PSOs).
- **those dispatches actually execute:** indirect-executed ≥4500 (M3 baseline
  4500); dispatched ≥2900; drawn ≥10806 (M3 baselines). A run that drops work
  to "pass" fails these floors.
- **branchy declines: 24 → 0** (Option A target).
- **geometry/coherence delta vs K2/M3:** world-region pixel delta > idle;
  loading past mesh; no regression in the draw/dispatch counters.
- **no per-frame wall-time regression vs M3 926 ms / K2 5633 ms** (max frame).
- **budget-skips == 0; pool fail == 0; done+fallback >= q** (no hung waiter).
- **1a crash ratio across ≥2 runs** reported (the null-write blocker is separate).
- CPU oracle: every newly-emulated op class validated against `exchange::reference`
  / `simulate` before a run is accepted.

## 5. Walls / open questions
- **RESOLVED:** the E_INVALIDARG cause is the `WaveSize(64)` PSV0 declaration
  (min=max=64) on a 32-wide host — confirmed by reading the refused `.dxil`
  bytes (§1). Not numthreads, not an unsupported intrinsic.
- Multi-wave cross-wave co-residency on the host is unproven (Option B risk):
  a >64-thread group split into sub-groups needs all sub-groups resident for a
  cross-wave reduction; the host may not co-schedule them.
- The CPU oracle (`exchange::reference`/`simulate`) currently validates only
  straight-line single-wave mask-free programs. Branchy/masked/multi-wave
  acceptance needs the oracle extended FIRST (numeric proof before a run).
- Leonida non-wave64 tables are silicon-calibrated — excluded from import.
- The wider declined set (beyond the 23 refused) also contains translation
  FAILURES unrelated to wave64 (image_gather4_l, unknown-stride structured
  buffers, v_swap_b32, untracked lane-mask join) — those are separate blockers,
  NOT this milestone; do not conflate.