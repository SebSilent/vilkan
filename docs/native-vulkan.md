# The native Vulkan path

VIlkan's first native execution on Linux: **no Proton, no D3D12**. The
`vilkan-vk` crate (ash) owns the Vulkan device, compute pipelines and dispatch
directly on Linux, built and run with plain `cargo` (not cargo-xwin).

## The compiler is a clean-room sidecar, not ours

We do **not** port the RDNA2 translator to Rust and do **not** write our own
IR→SPIR-V emitter. The shader compiler is a clean-room RDNA2 → SPIR-V
translator, reused offline as a sidecar (a separate process). Its provenance,
attribution and license position are recorded in a local, untracked note
(`LOCAL-NOTES.md`, gitignored) to keep the publishable tree free of upstream
project names. We invoke it; we do not redistribute its source.

- **Input contract:** the translator takes LLVM AMDGPU disassembly text (`.s`)
  produced from raw RDNA2/FSSM blobs via `llvm-mc` (AMDGPU target). We extract
  the raw RDNA2 compute code from a console container with `vilkan-shader`
  (`examples/extract-native.rs`); the code ends in `s_endpgm`, which the
  disassembler's scan fallback accepts.
- **Real header, not asserted:** `extract-native` reads the real FSSM header
  (group size, `rsrc2`, wave flag, resource layout) and writes it as a JSON
  sidecar; `scripts/compile-native.py` uses it. A loud post-check fails if a
  shader the header says is wave64 comes out non-emulated, with subgroup ops,
  or with the wrong `LocalSize` — no silent wave32 fallback.
- **Wave64:** `--wave64-emulate` makes wave64 compute exact on any subgroup
  size with **no subgroup operations** (cross-lane traffic through workgroup
  memory). On the RTX 2070 (subgroup 32) this runs the console's wave64
  programs. Our own `exchange.rs` stays the DXIL/Proton path and a cross-check;
  we do not reimplement wave64 here.

`scripts/compile-native.py` wires the steps: disassemble the raw blob, attach a
synthetic FSSM header carrying the real group size / `rsrc2` / wave flag, then
translate with `--wave64-emulate` and validate with `spirv-val`. Game-derived
blobs and SPIR-V stay outside the tree.

## The runtime contract

The reference binding model: 144 push-constant bytes = s0..s31 (128) + heap
base lo/hi + code base lo/hi; a single "heap" buffer at Xbox VA 0 mapped to its
device address; the output V# in s[0:3] (`{0, 0, 0xFFFFFFFF, 0x31016FAC}`).
`vilkan-vk` enables `bufferDeviceAddress` and `shaderInt64`, creates the heap,
pushes the constants, dispatches, and reads the heap back. The host subgroup
size is logged at device creation.

## Status

- **Done:** the cross-half `cs_w64x_readlane` / `cs_w64x_writelane` fixtures
  dispatch natively on the RTX 2070 and match the console reference
  numerically (a correct dispatch, not "it ran"). Every module passes
  `spirv-val`. Fixtures come from `VILKAN_VK_FIXTURES` and skip when unset.
- **Done:** both captured blur shaders compile to validated wave64-emulated
  SPIR-V (zero subgroup ops, LocalSize 8×8×1), from the real header metadata.
- **Blur dispatch — tractable, not a fundamental wall:** the blur shaders
  sample a texture and write UAV images through a **bindless descriptor heap**
  (runtime arrays of sampled images, samplers and storage images at set 0,
  indexed by an FNV-1a descriptor hash table over the T#/S# keys). The
  reference Vulkan test harness already binds sampled images/samplers this way
  and crafts the heap contents itself. For a test dispatch we control the heap,
  so a **minimal crafted setup** suffices — a small fixed bindless set
  (bindings 0/1/2/8/9/10), a hand-built hash-table buffer, crafted T#/S# keys,
  an input texture + sampler + output UAVs — not a full descriptor runtime.
  The blurs use 1 sampled image, 1 sampler, 4 storage images, 1 buffer. This is
  the next step (details in the local note).

The black-world indirect-draw/mesh-shader gap is a separate blocker and is not
chased here.