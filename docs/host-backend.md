# The host backend trait

The runtime drives one backend-neutral interface to execute a captured draw and
read back its pixels. The seam is the `vilkan-host` crate; the two execution
paths sit behind it.

```
 captured draw (draw.json + vs.spv + ps.spv + view buffers + oracle.bin)
        │  backend-neutral, no D3D12/Vulkan types
        ▼
 vilkan-host::HostBackend  ── name() / replay(dir, fault) -> ReplayResult
        │
        ├── D3D12Oracle  (default)  — the existing Proton/D3D12 path, unchanged:
        │                             serves the captured render-target readback
        └── native Vulkan           — vilkan-vk executes the record on Linux
                                      (stage two)
```

## The trait

```rust
pub trait HostBackend {
    fn name(&self) -> &'static str;
    fn replay(&self, dir: &Path, fault: Fault) -> Result<ReplayResult>;
}
```

`DrawRecord`, `RootBinding`, `ViewBuffer`, `RenderTarget`, `ReplayResult` and
`Fault` are backend-neutral and live in `vilkan-host`. `vilkan-vk::replay`
re-exports them, so the ad-hoc replay and the trait share one definition.

## Selection

`BackendKind::from_env()` reads `VILKAN_BACKEND` (`d3d12` or `vulkan`) and
**defaults to `d3d12`**. A plain run therefore drives the existing Proton path;
the boot is unchanged. The native path is opt-in.

## The D3D12 path behind the trait

`D3D12Oracle` serves the render-target readback the Proton/D3D12 path already
captured for the draw (`oracle.bin`, RGBA8 → RGBA float). It is the reference the
native path is checked against, so it is always fault-free: faults are injected
into the backend under test, never into the reference. `victory-gpu` (the live
D3D12 runtime) is not modified by this seam.

## Status

- **Stage one (done):** the trait, the neutral types, the D3D12 backend and the
  selector. Unit tests cover record parsing (unknown fields ignored), the
  default selection, the readback→float conversion and the size-mismatch
  rejection. The Proton boot is verified unchanged.
- **Stage two (next):** the native Vulkan implementation of `HostBackend` for
  one exact tier-A/B draw (no wave64 exchange), replaying a captured record
  through the trait rather than the ad-hoc module. Acceptance is numeric
  agreement with the existing replay output for the same record, plus negative
  controls (each `Fault` must make the output disagree).