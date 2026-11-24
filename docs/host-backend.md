# The host backend trait

The runtime drives one backend-neutral interface to execute a captured draw and
read back its pixels. The seam is the `vilkan-host` crate; the two execution
paths sit behind it.

```
 DrawRecord + DrawResources  (SPIR-V, view-buffer bytes, D3D12 readback)
        │  backend-neutral, in memory — no D3D12/Vulkan types, no file paths
        ▼
 vilkan-host::HostBackend  ── name() / replay(record, res, fault) -> ReplayResult
        │
        ├── D3D12Oracle  (default)  — the existing Proton/D3D12 path, unchanged:
        │                             serves the captured render-target readback
        └── VulkanBackend           — vilkan-vk executes the record on Linux
```

## The trait — record-driven, not file-path-driven

```rust
pub trait HostBackend {
    fn name(&self) -> &'static str;
    fn replay(&self, record: &DrawRecord, res: &DrawResources, fault: Fault)
        -> Result<ReplayResult>;
    // convenience wrapper, default impl:
    fn replay_dir(&self, dir: &Path, fault: Fault) -> Result<ReplayResult> {
        let (record, res) = load_capture(dir)?;
        self.replay(&record, &res, fault)
    }
}
```

The seam takes the neutral `DrawRecord` and its in-memory `DrawResources`
(`vs_spirv`, `ps_spirv`, `views: Vec<(address, bytes)>`, `oracle_rgba8`). Loading a
capture folder (`draw.json`, `vs.spv`, `ps.spv`, the view files, `oracle.bin`) is
the convenience wrapper `load_capture` / `replay_dir`, **not** the interface. The
trait is object-safe (`&dyn HostBackend`), so the runtime can hold a backend
without knowing which one answers.

`DrawRecord`, `DrawResources`, `RootBinding`, `ViewBuffer`, `RenderTarget`,
`ReplayResult` and `Fault` are backend-neutral and live in `vilkan-host`.
`vilkan-vk::replay` re-exports them; its execution core `replay_record` is
record-driven and is exactly what `VulkanBackend::replay` calls, so the trait
backend and the ad-hoc replay are bit-identical for the same record.

## Selection

`BackendKind::from_env()` reads `VILKAN_BACKEND` (`d3d12` or `vulkan`) and
**defaults to `d3d12`**. A plain run therefore drives the existing Proton path;
the boot is unchanged. The native path is opt-in.

## The D3D12 path behind the trait

`D3D12Oracle` serves the render-target readback the Proton/D3D12 path already
captured for the draw (`oracle_rgba8`, RGBA8 → RGBA float). It is the reference
the native path is checked against, so it is always fault-free: faults are
injected into the backend under test, never into the reference. `victory-gpu`
(the live D3D12 runtime) is not modified by this seam.

## What the seam still needs to serve live runtime calls

Today the seam replays **one captured, offscreen, tier-A/B View-only draw**. To
become the device/draw interface the live runtime drives, it still needs:

1. **A device/session lifecycle**, not a per-call `Context`: `create_device`,
   resource upload/eviction, and a persistent heap whose device address is stable
   across draws (the replay core allocates a fresh heap per call).
2. **The full root-signature / descriptor model**: the View-only draw uses a null
   descriptor set. The colour-quad stage (`74bf1d2`) added the **Table -> V# ->
   CBV** path: a Table root's descriptor window and the constant buffer its V#
   references ride in `DrawResources.views` (address -> bytes), the replay places
   them in the heap, and the PS buffer-loads the scale through a physical-storage
   pointer; vertex input assembly (DXGI 6/28/16) was added. **Still missing for
   live draws:** T#/S# (textures/samplers) and the bindless hash table — the
   `TexturedQuad`/`BlurSetup` machinery prototypes these but the seam does not yet
   build a descriptor set from captured T#/S# keys; root `Constants` dwords; and
   UAVs. The colour quad needed none of these (its PS is a CBV scale, not a
   texture), so they remain the gap for the textured draws.
3. **Render-target / pipeline state**: multiple targets, formats, depth, blend and
   viewport state from the record, not a single float readback target.
4. **Wave64 exchange (tier C)**: the replay core is exact only on wave32 tier A/B
   shaders; tier C needs the subgroup-exchange lowering before the seam can drive
   those draws.
5. **A draw stream, not a folder**: the runtime would call `replay` (or a future
   `draw`) per command-list entry with resources already resident, never touching
   the filesystem. `load_capture` stays a test/offline convenience.

None of these are needed for the stage-two acceptance below; they are the gap
between "replay a captured draw" and "drive the game".

## Status and acceptance

- **Stage one (done, `56c653d`):** the trait, the neutral types, the D3D12 backend
  and the selector; Proton boot verified unchanged.
- **Stage two (done):** `VulkanBackend` implements the trait for the tier-A/B
  View-only draw, driving `replay_record`. Acceptance is numeric, against the
  existing ad-hoc replay output for the same record (fixture `replay-fix`,
  3840×2160, RTX 2070 / subgroup 32):
  - vs ad-hoc replay: per-channel max err RGBA `[0, 0, 0, 0]`, mean `[0, 0, 0, 0]`,
    fraction outside tol 0 = **0.000000** (bit-identical).
  - vs D3D12 oracle: max err `[0, 0, 0, 0]`, fraction outside tol 2/255 =
    **0.000000** (exact).
  - negative controls, **through the new backend**, each must disagree with the
    fault-free output (tol 2/255): `SkipDraw`, `CorruptViewBuffer`,
    `WrongPushConstants` all give fraction over tol = **0.250000** (the alpha
    channel flips 1→0). All three fail, as required.
  - The trait is object-safe and record-driven (a `&dyn HostBackend` driven with
    an in-memory record + resources, no directory).
- **Descriptor stage (done, `74bf1d2`):** the colour quad's Table -> V# -> CBV
  path. Crafted non-uniform CBV (64x64): vs ad-hoc max err `[0,0,0,0]` frac 0; vs
  analytic colour x scale max err `[0,5.8e-6,0,0]` frac outside 2e-3 = 0; 64
  distinct reds; all three negative controls fail (frac 0.746).
- **Real captured colour quad (done):** frame-250 ordinal 1779, whose DXBC
  containers are byte-identical to the translated `fa982323` pair. Its captured V#
  has `num_records = 1`, stride 0, so the PS's scale load at +0x20 is OOB and the
  guarded load branch is skipped -> scale 0 -> the D3D12 oracle is black. The
  referenced CBV is at a ~54 TB console VA the capture could not map (an empty
  window); the replay now skips empty windows so the heap spans only the table
  address. Replayed 3840x2160 through `VulkanBackend`: vs ad-hoc max err **0**; vs
  D3D12 oracle **RGB max err `[0,0,0]`** (exact), 0 nonzero channels. **Alpha gap:**
  the oracle keeps alpha 1 while the replay writes the PS's alpha 0 (reported, not
  asserted). A black draw cannot make the negative controls fail, so the crafted
  variant carries those.
- **Blend / write-mask stage (done):** the record now carries RT0's
  `colorWriteMask` + blend state (`BlendState`, the raw 1-based `D3D12_BLEND` /
  `D3D12_BLEND_OP` codes from the host PSO), and the Vulkan replay applies them
  (`blend_attachment`). The codes are **1-based** (ZERO=1, ONE=2, SRC_COLOR=3...;
  ADD=1, SUBTRACT=2...) — an initial 0-based mapping was caught and fixed against a
  captured blended draw (`src=2,dst=6,op=1` = ONE/INV_SRC_ALPHA/ADD, standard alpha
  blend). Crafted acceptance (`colour_quad.rs`, 64x64, analytic per-pixel over the
  black clear): write-mask RGB-only keeps alpha at the clear (max err
  `[0,5.8e-6,0,0]`, masked alpha exactly 0); SRC_ALPHA/INV_SRC_ALPHA/ADD with PS
  alpha 0.5 gives RGB x 0.5 and alpha 0.25 (max err `[0,2.9e-6,0,0]`, frac 0); all
  three negative controls fail (frac 0.746). **Remaining alpha gap:** the real
  draw's oracle alpha 1 is the target's state *before* the draw (a prior clear/pass),
  preserved by the draw's mask/blend. The blend is now modeled, but the isolated
  single-draw replay clears alpha to 0 and does not carry the pre-draw clear value,
  so reproducing it needs the pre-draw target state too. The frame-250 legal-screen
  quads are timing-dependent (a re-boot captured different shader pairs, not
  `fa982323`), so the real draw's own blend was not re-captured this round.