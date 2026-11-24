# Descriptor / root-signature stage — scope (measured before building)

Target: the colour quad `draw-10007` (drawcap), a **Table** root + vertex buffer +
3 vertex inputs, 3840x2160 RGBA8. Budget ~700 lines; stop if exceeded or if it
needs machinery we do not have.

## What the draw actually does (translated + disassembled, not assumed)

VS `vs_0aebeefd9720fa3f` (256 B code, 0 resources): reads 3 vertex attributes
(Location 0/1/2, each v4float), exports Position = loc0, interpolant0 = loc1,
interpolant1 = loc2. No buffer loads.

PS `ps_fa982323cc0918a4` (320 B code, 1 resource): **no image, no sampler, no
bindless set.** RDNA2:

```
s_load_dwordx4 s[4:7], s[8:9], null        ; load a V# from the Table address (s8:s9)
s_buffer_load_dwordx4 s[0:3], s[4:7], 0x20 ; load 4 scale floats at V# base + 0x20
v_mul_legacy_f32 v2..v5, s0..s3, attr0     ; colour *= scale (per channel)
exp mrt0 ... compr                         ; output
```

So the "descriptor table" here is a **V# (4-dword buffer descriptor) -> constant
buffer** indirection, exactly the View-only heap pattern with one extra hop. The
bindless image/sampler machinery (TexturedQuad/BlurSetup) is **not needed**.

## Reuse (already in the tree)

- Heap placement of address->bytes: `replay_record` already places `res.views` at
  their console addresses. The table V# window and the referenced CBV window are
  just more `views` entries -> **no new placement code**.
- Push constants: View-only replay puts the root address in s8/s9 (pc[8]/pc[9]);
  the PS reads the Table address from the same s8/s9 -> **same placement**.
- V# decode for capture: `descriptor::host::buffer_range` already decodes a buffer
  view to (base, len).
- Graphics pipeline / readback / fault model: `graphics.rs`, `replay.rs`.

## New machinery (the only genuinely new piece)

- **Vertex input assembly**: `draw_with_set` currently draws 3 verts from
  VertexIndex with no vertex input. Add a vertex-buffer + attribute-description
  variant (binding stride 24; loc0 R32G32B32_SFLOAT@0, loc1 R8G8B8A8_UNORM@12,
  loc2 R32G32_SFLOAT@16; DXGI 6/28/16). ~120 lines.

## Line estimate

| piece | ~lines |
|---|---|
| neutral record: inputs + vertex_buffers + vb bytes; load_capture | 70 |
| graphics: vertex-input draw variant | 120 |
| replay: Table push constants + vertex bind + vertex_count draw | 60 |
| capture (victory-gpu): Table window + V# decode + CBV window as views | 90 |
| tests: agreement + 3 negative controls + crafted non-uniform CBV variant | 180 |
| docs | 40 |
| **total** | **~560** |

Within the ~700 budget; no bindless image runtime, no per-title calibration. The
crafted-CBV variant supplies a known non-uniform scale so acceptance is not
vacuous (the captured oracle is black).