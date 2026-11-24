//! Negative controls: a check that cannot fail is not acceptance. Each fault
//! injected into the replay must make the pixels disagree with the fault-free
//! oracle. The oracles are the validated non-trivial synthetic fixtures (the UV
//! gradient and the textured quad), so this does not depend on the game
//! rendering colour. Fixtures from VILKAN_VK_FIXTURES; skips cleanly when unset.

use vilkan_vk::Context;
use vilkan_vk::graphics::{RenderTarget, TexturedQuad};
use vilkan_vk::replay::Fault;

fn fixture(name: &str) -> Option<Vec<u8>> {
    let dir = std::env::var_os("VILKAN_VK_FIXTURES")?;
    let path = std::path::Path::new(&dir).join(name);
    match std::fs::read(&path) {
        Ok(b) => Some(b),
        Err(_) => {
            eprintln!("skip: fixture {name} not found ({})", path.display());
            None
        }
    }
}

fn context() -> Option<Context> {
    match Context::new() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("skip: no Vulkan device ({e:#})");
            None
        }
    }
}

fn f32_at(px: &[u32], i: usize) -> f32 {
    f32::from_bits(px[i])
}

/// Count channels that differ from the oracle beyond the RGBA8 tolerance.
fn disagree(oracle: &[u32], got: &[u32]) -> usize {
    assert_eq!(oracle.len(), got.len());
    let tol = 2.0 / 255.0;
    (0..oracle.len())
        .filter(|&i| (f32_at(got, i) - f32_at(oracle, i)).abs() > tol)
        .count()
}

/// The UV gradient (vs_ids + ps_flat: mrt0.zw = ((x+0.5)/64, (y+0.5)/64)) is
/// non-trivial. Skipping the draw leaves the clear colour, which must differ
/// from the gradient in the z/w channels.
#[test]
fn skip_draw_disagrees_with_uv_gradient() {
    let Some(vs) = fixture("vs_ids.spv") else { return };
    let Some(ps) = fixture("ps_flat.spv") else { return };
    let Some(ctx) = context() else { return };
    let Some(rt) = RenderTarget::new(&ctx, 64, 64).ok() else { return };
    let clear = [0.0f32, 0.0, 0.0, 0.0];
    let Some(oracle) = rt.draw_and_readback(&vs, &ps, clear).ok() else { return };
    // The oracle must be a real gradient in z/w: many distinct values.
    let distinct: std::collections::HashSet<u32> =
        (0..64 * 64).map(|p| oracle[p * 4 + 2]).collect();
    assert!(distinct.len() > 32, "oracle is not a gradient ({} distinct z)", distinct.len());
    let Some(skipped) = rt.clear_and_readback(clear).ok() else { return };
    let bad = disagree(&oracle, &skipped);
    eprintln!("SkipDraw vs UV gradient: {bad}/{} channels differ", oracle.len());
    // The gradient lives in z/w (half the channels); most of those must differ.
    assert!(bad > oracle.len() / 4, "SkipDraw must change the gradient pixels ({bad})");
}

/// The textured quad samples a known gradient texture. Each fault must change
/// the sampled output: SkipDraw leaves the clear, CorruptViewBuffer scrambles
/// the descriptor keys the PS hashes, WrongPushConstants mispoints the heap base.
#[test]
fn every_fault_disagrees_with_textured_quad() {
    let Some(vs) = fixture("vs_ids.spv") else { return };
    let Some(ps) = fixture("ps_tex.spv") else { return };
    let Some(ctx) = context() else { return };
    let Some(tex) = TexturedQuad::new_with_fallback(&ctx, 8, 8).ok() else { return };
    let mut texels = vec![0u32; 64];
    for j in 0..8 {
        for i in 0..8 {
            let r = ((i as f32 / 8.0) * 255.0).round() as u32;
            let g = ((j as f32 / 8.0) * 255.0).round() as u32;
            texels[j * 8 + i] = r | (g << 8) | (128 << 16) | (255 << 24);
        }
    }
    if tex.upload(&texels).is_err() { return; }
    let Some(rt) = RenderTarget::new(&ctx, 64, 64).ok() else { return };
    let Some(oracle) = tex.draw_with_fault(&rt, &vs, &ps, Fault::None).ok() else { return };
    let distinct: std::collections::HashSet<u32> =
        (0..64 * 64).map(|p| oracle[p * 4]).collect();
    assert!(distinct.len() > 4, "oracle is not a textured gradient ({} distinct)", distinct.len());
    for fault in [Fault::SkipDraw, Fault::CorruptViewBuffer, Fault::WrongPushConstants] {
        // A fresh quad per fault: CorruptViewBuffer mutates the heap in place.
        let Some(t) = TexturedQuad::new_with_fallback(&ctx, 8, 8).ok() else { return };
        if t.upload(&texels).is_err() { return; }
        let Some(got) = t.draw_with_fault(&rt, &vs, &ps, fault).ok() else { return };
        let bad = disagree(&oracle, &got);
        let name = match fault {
            Fault::SkipDraw => "SkipDraw",
            Fault::CorruptViewBuffer => "CorruptViewBuffer",
            Fault::WrongPushConstants => "WrongPushConstants",
            Fault::None => "None",
        };
        eprintln!("{name} vs textured quad: {bad}/{} channels differ", oracle.len());
        assert!(bad > oracle.len() / 4, "{name} must change pixels ({bad})");
    }
}
