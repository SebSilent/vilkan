//! Acceptance for the descriptor / root-signature stage: the colour quad.
//!
//! The colour quad's PS reads a V# from a Table root, buffer-loads a 4-float
//! scale from the constant buffer the V# references, and multiplies the
//! interpolated vertex colour by it (no image/sampler). This test crafts a
//! vertex buffer + V# + constant buffer with a KNOWN non-uniform scale, replays
//! through `VulkanBackend`, and checks:
//!
//! 1. agreement with the ad-hoc replay for the same record (tolerance 0);
//! 2. per-pixel agreement with the analytic expectation (colour x scale), with a
//!    tolerance covering the PS's f16 output quantization — the captured oracle
//!    is black, so this crafted variant is what makes acceptance non-vacuous;
//! 3. all three negative controls fail through the NEW backend.
//!
//! The colour-quad VS/PS SPIR-V come from VILKAN_QUAD_FIXTURES (vs.spv, ps.spv);
//! skips cleanly when unset or no device. Nothing game-derived is tracked: the
//! vertex buffer, V# and constant buffer here are all crafted.

use vilkan_host::{DrawRecord, DrawResources, Fault, HostBackend, ReplayResult};
use vilkan_vk::Context;
use vilkan_vk::host::VulkanBackend;

fn fixtures() -> Option<std::path::PathBuf> {
    std::env::var_os("VILKAN_QUAD_FIXTURES").map(std::path::PathBuf::from)
}

/// The crafted constant-buffer scale (f16-exact values so the analytic check is
/// tight): per-channel multipliers for the interpolated vertex colour.
const SCALE: [f32; 4] = [0.5, 0.75, 1.0, 1.0];
/// Console addresses for the crafted table V# and the constant buffer it points
/// at. Both fit in 32 bits (the PS reconstructs the address from s8/s9).
const TABLE_VA: u64 = 0x1000;
const CBV_VA: u64 = 0x2000;
const W: u32 = 64;
const H: u32 = 64;

/// Build the crafted record + resources: a fullscreen triangle whose red channel
/// ramps with screen x, a V# at TABLE_VA pointing at CBV_VA, and the scale at
/// CBV_VA + 0x20 (the offset the PS buffer-loads).
fn crafted() -> (DrawRecord, DrawResources) {
    let record = DrawRecord::from_json(&format!(
        r#"{{ "topology":4,"vertex_count":3,"instance_count":1,"first_vertex":0,
             "roots":[{{"kind":"Table","address":{TABLE_VA}}}],
             "view_buffers":[],
             "inputs":[
               {{"slot":0,"offset":0,"format":6,"per_instance":false}},
               {{"slot":0,"offset":12,"format":28,"per_instance":false}},
               {{"slot":0,"offset":16,"format":16,"per_instance":false}}],
             "vertex_buffers":[{{"slot":0,"stride":24,"data_file":"vb.bin"}}],
             "targets":[{{"format":28,"width":{W},"height":{H}}}] }}"#
    ))
    .unwrap();

    // Fullscreen triangle: clip (-1,-1), (3,-1), (-1,3). Red ramps 0->1 with u
    // (screen x), green is constant 128/255, blue 0, alpha 1. Stride 24:
    // position float3 @0, colour RGBA8 @12, uv float2 @16.
    let verts: [(f32, f32, f32, [u8; 4], f32, f32); 3] = [
        (-1.0, -1.0, 0.0, [0, 128, 0, 255], 0.0, 0.0),
        (3.0, -1.0, 0.0, [255, 128, 0, 255], 1.0, 0.0),
        (-1.0, 3.0, 0.0, [0, 128, 0, 255], 0.0, 1.0),
    ];
    let mut vb = Vec::new();
    for (x, y, z, c, u, v) in verts {
        vb.extend_from_slice(&x.to_le_bytes());
        vb.extend_from_slice(&y.to_le_bytes());
        vb.extend_from_slice(&z.to_le_bytes());
        vb.extend_from_slice(&c);
        vb.extend_from_slice(&u.to_le_bytes());
        vb.extend_from_slice(&v.to_le_bytes());
    }

    // V# (4 dwords): base = CBV_VA (word0 low, word1 high), num_records a byte
    // bound large enough for the load at +0x20, raw flags (OOB_SELECT 3).
    let vsharp: [u32; 4] = [CBV_VA as u32, (CBV_VA >> 32) as u32, 4096, 0x31016FAC];
    let mut table = vec![0u8; 64];
    table[..16].copy_from_slice(unsafe {
        std::slice::from_raw_parts(vsharp.as_ptr() as *const u8, 16)
    });

    // Constant buffer: the scale at offset 0x20 (the PS's buffer_load offset).
    let mut cbv = vec![0u8; 4096];
    for (i, s) in SCALE.iter().enumerate() {
        cbv[0x20 + i * 4..0x20 + i * 4 + 4].copy_from_slice(&s.to_le_bytes());
    }

    let vs_spirv = std::fs::read(fixtures().unwrap().join("vs.spv")).unwrap();
    let ps_spirv = std::fs::read(fixtures().unwrap().join("ps.spv")).unwrap();
    let res = DrawResources {
        vs_spirv,
        ps_spirv,
        views: vec![(TABLE_VA, table), (CBV_VA, cbv)],
        vertex_buffers: vec![(0, 24, vb)],
        oracle_rgba8: None,
    };
    (record, res)
}

/// The analytic expectation at pixel (x, y): the fullscreen triangle's barycentric
/// u = (x+0.5)/(2W) ramps red, green is constant 128/255; multiplied by SCALE.
/// The PS quantizes to f16, so `tol` covers that rounding.
fn expected(x: u32, y: u32) -> [f32; 4] {
    let _ = y;
    let u = (x as f32 + 0.5) / (2.0 * W as f32);
    let red = u * 1.0; // c1.red=1, c0/c2.red=0
    let green = 128.0 / 255.0;
    [red * SCALE[0], green * SCALE[1], 0.0 * SCALE[2], 1.0 * SCALE[3]]
}

fn compare(got: &ReplayResult, want: &ReplayResult, tol: f32) -> ([f32; 4], f64) {
    assert_eq!(got.pixels.len(), want.pixels.len());
    let mut max_err = [0.0f32; 4];
    let mut bad = 0usize;
    for (i, (&g, &w)) in got.pixels.iter().zip(want.pixels.iter()).enumerate() {
        let err = (g - w).abs();
        let c = i % 4;
        if err > max_err[c] {
            max_err[c] = err;
        }
        if err > tol {
            bad += 1;
        }
    }
    (max_err, bad as f64 / got.pixels.len().max(1) as f64)
}

#[test]
fn colour_quad_crafted_cbv_matches_analytic_and_faults_fail() {
    let Some(_dir) = fixtures() else {
        eprintln!("skip: VILKAN_QUAD_FIXTURES unset");
        return;
    };
    let ctx = match Context::new() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skip: no Vulkan device ({e:#})");
            return;
        }
    };
    eprintln!("device: {} (subgroup {})", ctx.device_name, ctx.subgroup_size);
    let (record, res) = crafted();
    let backend = VulkanBackend::new(&ctx);

    let reference = vilkan_vk::replay::replay_record(&ctx, &record, &res, Fault::None).unwrap();
    let got = backend.replay(&record, &res, Fault::None).unwrap();
    eprintln!("replayed {}x{} colour quad through {}", got.width, got.height, backend.name());

    // 1. Agreement with the ad-hoc replay (same core): bit-identical.
    let (max_err, frac) = compare(&got, &reference, 0.0);
    eprintln!("vs ad-hoc replay: max err {max_err:?} fraction outside 0: {frac:.6}");
    assert_eq!(frac, 0.0, "backend disagrees with ad-hoc replay (max {max_err:?})");

    // 2. Per-pixel analytic expectation (colour x scale), f16 tolerance.
    let want = ReplayResult {
        width: W,
        height: H,
        pixels: (0..H).flat_map(|y| (0..W).flat_map(move |x| expected(x, y))).collect(),
    };
    let tol = 2e-3f32; // f16 output quantization step for values in [0, 1]
    let (amax, afrac) = compare(&got, &want, tol);
    eprintln!("vs analytic: max err {amax:?} fraction outside {tol}: {afrac:.6}");
    assert!(afrac < 0.01, "colour quad does not match analytic colour x scale (max {amax:?})");

    // Non-vacuous: the output must be non-uniform (a red ramp), not a flat clear.
    let reds: std::collections::HashSet<u32> =
        got.pixels.iter().step_by(4).map(|f| f.to_bits()).collect();
    eprintln!("distinct red values: {} (non-uniform check)", reds.len());
    assert!(reds.len() > 16, "output is uniform — the crafted CBV did not produce a gradient");

    // 3. Negative controls through the NEW backend: each must disagree.
    let ntol = 2e-3f32;
    for fault in [Fault::SkipDraw, Fault::CorruptViewBuffer, Fault::WrongPushConstants] {
        let faulty = backend.replay(&record, &res, fault).unwrap();
        let (fmax, ffrac) = compare(&faulty, &reference, ntol);
        eprintln!("fault {fault:?}: max err {fmax:?} fraction over tol {ffrac:.6}");
        assert!(ffrac > 0.0, "negative control {fault:?} did NOT fail (max {fmax:?})");
    }
}

/// Acceptance for the blend / write-mask state the record now carries. Two
/// crafted cases, each checked per-pixel against an analytic expectation over the
/// replay's black clear, so the D3D12_BLEND / D3D12_BLEND_OP mapping (1-based) and
/// the colorWriteMask are exercised non-vacuously:
///
/// 1. write-mask RGB-only: alpha is masked, so it keeps the clear value (0) while
///    RGB still matches colour x scale — proves the mask is applied;
/// 2. SRC_ALPHA / INV_SRC_ALPHA / ADD with the PS alpha driven to 0.5: over a black
///    clear, RGB = colour x scale x 0.5 and alpha = 0.5 x 0.5 = 0.25 — proves the
///    blend factors and op. All three negative controls must still fail.
#[test]
fn colour_quad_blend_and_write_mask_match_analytic() {
    let Some(_dir) = fixtures() else {
        eprintln!("skip: VILKAN_QUAD_FIXTURES unset");
        return;
    };
    let ctx = match Context::new() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skip: no Vulkan device ({e:#})");
            return;
        }
    };
    let backend = VulkanBackend::new(&ctx);
    let tol = 2e-3f32;

    // Case 1: alpha write-mask. RGB written, alpha masked -> keeps the clear (0).
    {
        let (mut record, res) = crafted();
        record.blend.write_mask = 0x7; // R|G|B, no alpha
        let got = backend.replay(&record, &res, Fault::None).unwrap();
        let want = ReplayResult {
            width: W, height: H,
            pixels: (0..H).flat_map(|y| (0..W).flat_map(move |x| {
                let e = expected(x, y);
                [e[0], e[1], e[2], 0.0]
            })).collect(),
        };
        let (max, frac) = compare(&got, &want, tol);
        eprintln!("write-mask RGB-only: max err {max:?} fraction outside {tol}: {frac:.6}");
        assert!(frac < 0.01, "alpha write-mask not applied (max {max:?})");
        let amax = got.pixels.iter().skip(3).step_by(4).map(|f| f.abs()).fold(0.0f32, f32::max);
        eprintln!("masked alpha max: {amax} (expected 0: clear preserved)");
        assert_eq!(amax, 0.0, "masked alpha was written (max {amax})");
    }

    // Case 2: SRC_ALPHA / INV_SRC_ALPHA / ADD blend, PS alpha driven to 0.5.
    {
        let (mut record, mut res) = crafted();
        // Drive the CBV alpha scale to 0.5 so the PS outputs alpha 0.5.
        let cbv = res.views.iter_mut().find(|(a, _)| *a == CBV_VA).unwrap();
        cbv.1[0x20 + 12..0x20 + 16].copy_from_slice(&0.5f32.to_le_bytes());
        record.blend.blend_enable = true;
        record.blend.src_blend = 5;        // D3D12_BLEND_SRC_ALPHA
        record.blend.dest_blend = 6;       // D3D12_BLEND_INV_SRC_ALPHA
        record.blend.blend_op = 1;         // D3D12_BLEND_OP_ADD
        record.blend.src_blend_alpha = 5;
        record.blend.dest_blend_alpha = 6;
        record.blend.blend_op_alpha = 1;
        let got = backend.replay(&record, &res, Fault::None).unwrap();
        // Over a black clear: rgb = colour x scale_rgb x srcA(0.5); alpha = srcA^2.
        let want = ReplayResult {
            width: W, height: H,
            pixels: (0..H).flat_map(|y| (0..W).flat_map(move |x| {
                let e = expected(x, y);
                [e[0] * 0.5, e[1] * 0.5, e[2] * 0.5, 0.25]
            })).collect(),
        };
        let (max, frac) = compare(&got, &want, tol);
        eprintln!("SRC_ALPHA blend: max err {max:?} fraction outside {tol}: {frac:.6}");
        assert!(frac < 0.01, "SRC_ALPHA blend factors/op wrong (max {max:?})");
        for fault in [Fault::SkipDraw, Fault::CorruptViewBuffer, Fault::WrongPushConstants] {
            let f = backend.replay(&record, &res, fault).unwrap();
            let (fm, ff) = compare(&f, &want, tol);
            eprintln!("blend fault {fault:?}: max err {fm:?} fraction over tol {ff:.6}");
            assert!(ff > 0.0, "negative control {fault:?} did NOT fail under blend (max {fm:?})");
        }
    }

    // Case 3: pre-draw clear preservation (the real colour quad's alpha gap). The
    // target was cleared to alpha 1; the draw masks alpha and its PS outputs alpha
    // 0, so the result keeps the cleared alpha 1. Discriminating for BOTH the
    // clear-color path (a black clear would give alpha 0) and the write-mask (an
    // unmasked alpha would write the PS's 0).
    {
        let (mut record, mut res) = crafted();
        record.clear_color = [0.0, 0.0, 0.0, 1.0];
        record.blend.write_mask = 0x7; // R|G|B, no alpha
        let cbv = res.views.iter_mut().find(|(a, _)| *a == CBV_VA).unwrap();
        cbv.1[0x20 + 12..0x20 + 16].copy_from_slice(&0.0f32.to_le_bytes()); // PS alpha 0
        let got = backend.replay(&record, &res, Fault::None).unwrap();
        let want = ReplayResult {
            width: W, height: H,
            pixels: (0..H).flat_map(|y| (0..W).flat_map(move |x| {
                let e = expected(x, y);
                [e[0], e[1], e[2], 1.0] // cleared alpha preserved
            })).collect(),
        };
        let (max, frac) = compare(&got, &want, tol);
        eprintln!("clear-preserved alpha: max err {max:?} fraction outside {tol}: {frac:.6}");
        assert!(frac < 0.01, "pre-draw clear alpha not preserved (max {max:?})");
        let amin = got.pixels.iter().skip(3).step_by(4).copied().fold(1.0f32, f32::min);
        eprintln!("preserved alpha min: {amin} (expected 1: clear kept)");
        assert_eq!(amin, 1.0, "cleared alpha was overwritten (min {amin})");
        for fault in [Fault::SkipDraw, Fault::CorruptViewBuffer, Fault::WrongPushConstants] {
            let f = backend.replay(&record, &res, fault).unwrap();
            let (fm, ff) = compare(&f, &want, tol);
            eprintln!("clear fault {fault:?}: max err {fm:?} fraction over tol {ff:.6}");
            assert!(ff > 0.0, "negative control {fault:?} did NOT fail under clear (max {fm:?})");
        }
    }
}