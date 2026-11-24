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
- **Done: blur dispatch with a minimal crafted bindless set.** The blur shaders
  sample a texture and write UAV images through a **bindless descriptor heap**
  (runtime arrays of sampled images, samplers and storage images at set 0,
  indexed by an FNV-1a descriptor hash table over the T#/S# keys). For a test
  dispatch we control the heap, so a **minimal crafted setup** suffices — a
  small fixed bindless set (bindings 0/1/2/8/9/10), a hand-built hash-table
  buffer, crafted T#/S# keys with valid FORMAT fields, an input texture +
  sampler + output UAVs — not a full descriptor runtime
  (`crates/vilkan-vk/src/bindless.rs`).
- **Done: numeric box-filter acceptance.** The blur is a mip-chain downsample
  (8×8 → 4×4 → 2×2 → 1×1). Dispatched against analytically known inputs and
  checked numerically (`crates/vilkan-vk/tests/blur.rs`): a **constant** image
  is a fixed point (1×1 average and 8×8 raw all equal the constant); an
  **impulse** averages to peak/64 with energy conserved; a **gradient** averages
  to its mean and stays monotonic in the mip. Both blurs pass. A wrong slot
  index, stale parity or wrong descriptor fails the check. Two setup bugs were
  caught and fixed this way: the sample-coordinate scale (1/8 for an 8×8
  texture, not 1.0) and the scale-buffer V# byte bound (16, not 4 — a raw V#'s
  num_records is bytes, so 4 truncated the Y scale to 0).

The black-world indirect-draw/mesh-shader gap is a separate blocker and is not
chased here.