# The native Vulkan path

VIlkan's first native execution on Linux: **no Proton, no D3D12**. The
`vilkan-vk` crate (ash) owns the Vulkan device, compute pipelines and dispatch
directly on Linux, built and run with plain `cargo` (not cargo-xwin).

## The compiler is Leonida's, not ours

We do **not** port `rdna2spirv.py` to Rust and do **not** write our own
IR→SPIR-V emitter. The shader compiler is [Leonida](../leonida)'s clean-room
RDNA2 translator, reused offline as a sidecar:

- **Provenance:** Leonida is GPL-3.0 (compatible with this tree's
  GPL-3.0-or-later). Its translator is clean-room: the model is one SPIR-V
  invocation per console lane, control flow structured by `cfg_structure.py`,
  and anything not modelled raises `Unsupported` rather than being guessed.
  The BCPack/codec constants it uses are recovered from public specifications.
  We invoke it as a separate process; none of its code is copied into this tree.
- **Input contract:** `rdna2spirv.py` takes LLVM AMDGPU disassembly text (`.s`)
  that `shader_disasm.py` produces from raw RDNA2/FSSM blobs via `llvm-mc`
  (AMDGPU target). We extract the raw RDNA2 compute code from a console
  container with `vilkan-shader` (`examples/extract-native.rs`); the code ends
  in `s_endpgm`, which `shader_disasm.py`'s scan fallback accepts.
- **Wave64:** `--wave64-emulate` makes wave64 compute exact on any subgroup
  size with **no subgroup operations** (cross-lane traffic through workgroup
  memory). On the RTX 2070 (subgroup 32) this is what runs the console's
  wave64 programs. Our own `exchange.rs` stays the DXIL/Proton path and a
  cross-check; we do not reimplement wave64 here.

`scripts/compile-native.py` wires the steps: disassemble the raw blob, attach a
synthetic FSSM header carrying the thread-group size and the FSSM magic (so the
wave64 flag is read), then translate with `--wave64-emulate` and validate with
`spirv-val`. Game-derived blobs and SPIR-V stay outside the tree.

## The runtime contract

Leonida's binding model (`docs/design.md`): 144 push-constant bytes =
s0..s31 (128) + heap base lo/hi + code base lo/hi; a single "heap" buffer at
Xbox VA 0 mapped to its device address; the output V# in s[0:3]
(`{0, 0, 0xFFFFFFFF, 0x31016FAC}`). `vilkan-vk` enables `bufferDeviceAddress`
and `shaderInt64`, creates the heap, pushes the constants, dispatches, and
reads the heap back. The host subgroup size is logged at device creation.

## Status

- **Done:** the cross-half `cs_w64x_readlane` / `cs_w64x_writelane` fixtures
  dispatch natively on the RTX 2070 and match the console reference
  numerically (a correct dispatch, not "it ran"). Every module passes
  `spirv-val`. Fixtures come from `VILKAN_VK_FIXTURES` and skip when unset.
- **Done:** both captured blur shaders compile to validated wave64-emulated
  SPIR-V (zero subgroup ops, LocalSize 8×8×1).
- **Wall — blur dispatch:** the blur shaders sample a texture and write UAV
  images through Leonida's **bindless descriptor heap** (runtime arrays of
  sampled images, samplers and storage images at set 0, indexed by a descriptor
  hash table over the T#/V#/S# the game binds). Dispatching them needs that
  descriptor runtime — image/sampler management, the hash-table lookup, literal
  sampler registration — which is a major feature (Leonida's C++ runtime
  implements it), beyond the buffer-dispatch runtime built here. This is the
  next step, not a fundamental blocker.

The black-world indirect-draw/mesh-shader gap is a separate blocker and is not
chased here.