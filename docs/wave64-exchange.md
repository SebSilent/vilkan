# The wave64 exchange: console wave64 compute on a 32-lane host

A console GPU program compiled for 64-lane waves uses cross-lane operations
(`v_readlane_b32`, `v_writelane_b32`, `ds_swizzle_b32`, `ds_bpermute_b32`,
`v_permlane*`, wave votes and ballots). A host GPU whose waves are 32 lanes
wide cannot run these directly: a `WaveSize(64)` shader is refused at
pipeline creation, and a 32-lane shuffle wraps a lane select ≥ 32 (so a read
of lane 40 silently returns lane 8). This note describes the backend-neutral
lowering we use to run such programs exactly on a 32-lane host.

## The model: a round-scheduler over workgroup memory

One invocation is one console lane. The console lane is the **flattened
workgroup-local thread index** (`SV_GroupIndex` / `LocalInvocationIndex`),
not the host subgroup lane — so the host's wave formation cannot misnumber
the lanes, and **no subgroup operation is emitted at all**.

Every cross-lane value goes through a thread-group shared buffer:

1. **Publish.** Each lane stores the source value(s) it will read from into
   its own slot of the buffer.
2. **Barrier.** The whole workgroup meets at one group barrier
   (`GroupMemoryBarrierWithGroupSync` in DXIL, `OpControlBarrier` with
   `Workgroup` scope in SPIR-V), in uniform control flow.
3. **Read.** Each lane reads the slot of the lane it wants (any lane 0..63,
   including the other half of the console wave).

The program is cut into **segments** at each cross-lane operation; a segment
of per-lane code runs, then an exchange (publish → barrier → read), then the
next segment. Because every invocation runs every round, the barrier is
always in uniform control flow — no forward-progress assumption, no spinning.

### Slot layout and double buffering

The buffer is `2 × width × 64` dwords, where `width` is the largest number of
distinct source values any single exchange publishes. The factor of two is
**double buffering by exchange parity**: exchange *k* writes parity `k & 1`,
so a reader never sees the previous exchange's data. A read that lands on a
slot the current exchange did not publish (a wrong column, a wrong lane, or a
stale parity) returns a sentinel, which the correctness oracle treats as a
failure rather than a silently wrong pixel.

### The batching hazard rule

Consecutive cross-lane ops can share one exchange (one publish, one barrier)
only while their sources are stable. In SSA terms: an op joins the current
exchange **iff its source was defined before the exchange's first op**. A
source defined at or after that point was either rewritten since the
publication, or is the result of an earlier cross-lane op in the same
exchange — in both cases the value does not exist yet at the barrier, so the
op must start a new exchange. This is what makes a batch safe: every value
read behind a barrier was published before it.

## Gating

The lowering applies to compute shaders that are **straight-line** (no
branches/loops), **single-wave** (≤ 64 threads per group) and **mask-free**
(no lane mask read as data, no EXEC manipulation). The host's wave width is
queried at device creation; a 64-lane host runs the program natively. A
shader the narrow predicate rejects is declined with a logged reason
(multi-wave, branchy CFG, masks, not compute), so the next refusal is
diagnosable. Branchy CFG (a full round-scheduler), multi-wave synchronization
and the mask/ballot ring are later work.

## Backend-neutral by construction

The planner, the slot layout and the exchange protocol depend on neither the
shader language nor the API: they operate on the IR and emit a plan. The
DXIL backend lowers the plan to a `groupshared` array plus
`GroupMemoryBarrierWithGroupSync`; a SPIR-V backend lowers the same plan to a
`Workgroup` array plus `OpControlBarrier`. The emitter and the CPU oracle
compute slot addresses from one shared formula, so they cannot disagree.

## Correctness oracle

A CPU model executes the plan over a simulated workgroup buffer and compares
each cross-lane op's per-lane result against the console reference (read the
source lane's value directly, all 64 lanes available). The numeric
expectations include cross-half readlane/writelane cases (a lane ≥ 32 read
from the other half, a computed lane select wrapped mod 64) and half-local
swizzle butterflies. An unsafe batch or a stale parity reads the sentinel and
fails the test. Synthetic fixtures and these numeric cases live in the repo;
tests on captured title shaders read from an external fixture path
(`VILKAN_SHADER_FIXTURES`) and skip cleanly when it is unset, so a clean clone
with no game data builds and passes the suite.