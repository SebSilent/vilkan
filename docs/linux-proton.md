# The bootstrap backend and the native Vulkan target

VIlkan's destination is **CSM Xbox native on Linux** — a native process with
our own Vulkan backend, no Proton. The D3D12 → VKD3D-Proton path described
here is a **temporary bootstrap backend**: it gets the title running by
reusing a Windows binary under Proton while the native backend is built.
Status: the title boots through the menus and into the world on an NVIDIA
RTX 2070 (wave32); the world picture is black (the indirect-draw/mesh-shader
gap).

## The bootstrap stack today

```
CSM Xbox title (x86-64, runs unchanged)
  └─ vilkan.exe         PE loader + console HLE
       └─ Proton (Wine)     Win32, WASAPI audio, XInput
            └─ VKD3D-Proton host D3D12 → Vulkan
                 └─ NVIDIA RTX 2070 (Turing: SM 6.6, mesh shaders, wave32)
```

We cross-compile the Windows binary from Linux (`cargo xwin`, MSVC target)
and run it under Proton, so the host D3D12 it emits is translated by
VKD3D-Proton instead of a Windows driver. No DXC is needed: `vilkan-shader`
writes DXIL containers itself.

## Why bootstrap through Proton first

- It reuses everything that already works (loader, HLE, shader translation,
  formats) and changes only the host underneath.
- Proton's D3D12→Vulkan layer is mature; we get SM 6.6 bindless, mesh shaders
  and wave ops on NVIDIA without writing them ourselves.
- It is a stepping stone, not the goal: every backend-neutral part (the
  wave64 exchange, the formats, the HLE) carries over to the native backend
  unchanged.

## The target: our own RDNA2 → SPIR-V Vulkan backend

Replace `vilkan-gpu`'s host D3D12 with a direct Vulkan backend and drop
Proton entirely. The pieces to build or salvage:

- A measured Xbox D3D12 ABI → Vulkan design: a device proxy, descriptor
  heaps, presentation, BVH builds, with the exact vtable deviations of the
  console ABI.
- An RDNA2 → SPIR-V translator and its bindless shader contract.
- ABI measurements our `vilkan-gpu/src/console.rs` can be cross-checked
  against.

### The wave64 exchange is backend-neutral (reused by the SPIR-V emitter)

Console wave64 compute on a 32-lane host (NVIDIA) cannot use subgroup
shuffles. We lower it with a **round-scheduler model**
(`crates/vilkan-shader/src/emit/exchange.rs`): one invocation per console lane
(workgroup-local index), cross-lane values through a groupshared buffer
behind a workgroup barrier, **no subgroup operations**. This is backend-neutral:
the planner, slot layout and protocol emit identically to DXIL (`groupshared`
+ `GroupMemoryBarrierWithGroupSync`) and SPIR-V (`Workgroup` +
`OpControlBarrier`). The narrow slice (straight-line, single-wave, mask-free
compute) is implemented and verified by a CPU oracle; the technique is in
[wave64-exchange.md](wave64-exchange.md). The SPIR-V emitter reuses this
planner unchanged.

### What must be true to switch backends

- A native Vulkan device proxy behind the console D3D12 ABI (the console's
  vtable deviations measured and mapped).
- The RDNA2 → SPIR-V translator wired to the same `HostShader` contract the
  DXIL emitter uses: bindless descriptor heap, user-data dwords, root CBV/SRV
  views, immediate samplers.
- The exchange's groupshared buffer + barrier lowered to SPIR-V `Workgroup`
  + `OpControlBarrier` (planner already backend-neutral).
- Presentation, fences and title memory on Vulkan instead of DXGI/D3D12; the
  host-capability gate (`WaveLaneCountMax`) becomes a Vulkan subgroup-size
  query.
- The world's indirect draws and mesh shaders translated (the current black
  picture), independent of the backend switch.

## Known risks on this machine

| Risk | Note |
|------|------|
| wave64 compute on NVIDIA | **fixed (narrow slice)**: the blur computes lower via the backend-neutral exchange (subgroup-free DXIL); boot accepts them, 0 refusals, world stays up. Branchy/multi-wave/masked wave64 still declined with a logged reason |
| RAM | 23 GB + zram; the title commits ~15 GB in menus on a 32 GB machine |
| VRAM | 8 GB vs the console's shared pool; menus fit, world unknown |
| Proton quirks | WASAPI/XInput are Wine builtins; crash reporter uses Win32 VEH |

## Scripts

- `scripts/build.sh` — cross-compile `vilkan.exe` (needs rustup + cargo-xwin
  + clang; MSVC SDK auto-downloaded).
- `scripts/run.sh` — run under Proton with the title path and data dir from
  config/env (`GAME_DIR`/`VILKAN_GAME_DIR`, `VILKAN_DATA`; nothing hardcoded).
  The prefix/cache/data live outside the repo per the asset policy.