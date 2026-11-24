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
   descriptor set. Live draws bind T#/S# (textures/samplers), CBVs and the bindless
   hash table; the seam needs a `DrawResources` that carries those (descriptor
   keys, image/sampler handles, constant buffers) and a backend that builds the
   sets. This is the descriptor machinery the textured path (`TexturedQuad`)
   prototypes but does not yet generalise.
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