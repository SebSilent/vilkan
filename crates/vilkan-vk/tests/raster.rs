//! Native Vulkan rasterization, checked numerically against analytically known
//! results. No Proton, no D3D12.
//!
//! Fixtures (VS/PS SPIR-V compiled by the vendored translator) come from
//! `VILKAN_VK_FIXTURES`; the tests skip cleanly when unset or missing, so a
//! clean clone with no game data passes. Nothing game-derived is tracked.
//! Acceptance is the correct pixels, not "it drew something".

use vilkan_vk::Context;
use vilkan_vk::graphics::{RenderTarget, TexturedQuad};

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
        Ok(c) => {
            eprintln!("device: {} (subgroup {})", c.device_name, c.subgroup_size);
            Some(c)
        }
        Err(e) => {
            eprintln!("skip: no Vulkan device ({e:#})");
            None
        }
    }
}

fn f32_at(px: &[u32], i: usize) -> f32 {
    f32::from_bits(px[i])
}

/// A solid clear must put the exact clear color in every pixel. Proves the
/// render target + readback path with no shaders.
#[test]
fn solid_clear_is_exact() {
    let Some(ctx) = context() else { return };
    let Some(rt) = RenderTarget::new(&ctx, 16, 16).ok() else { return };
    let clear = [0.25f32, 0.5, 0.75, 1.0];
    let Some(px) = rt.clear_and_readback(clear).ok() else { return };
    assert_eq!(px.len(), 16 * 16 * 4);
    for p in 0..16 * 16 {
        for c in 0..4 {
            let got = f32_at(&px, p * 4 + c);
            assert!((got - clear[c]).abs() < 1e-6, "pixel {p} channel {c}: {got} != {}", clear[c]);
        }
    }
    eprintln!("solid clear: all 256 pixels exact ✓");
}

/// vs_ids draws a fullscreen triangle from VertexIndex; ps_flat outputs
/// mrt0.zw = ((x+0.5)/64, (y+0.5)/64) — an analytically known gradient over a
/// 64x64 target. Check every pixel's gradient channels against the formula.
#[test]
fn fullscreen_triangle_gradient_is_exact() {
    let Some(vs) = fixture("vs_ids.spv") else { return };
    let Some(ps) = fixture("ps_flat.spv") else { return };
    let Some(ctx) = context() else { return };
    let Some(rt) = RenderTarget::new(&ctx, 64, 64).ok() else { return };
    let Some(px) = rt.draw_and_readback(&vs, &ps, [0.0, 0.0, 0.0, 0.0]).ok() else { return };
    assert_eq!(px.len(), 64 * 64 * 4);
    let mut max_err = 0.0f32;
    for y in 0..64 {
        for x in 0..64 {
            let p = (y * 64 + x) * 4;
            let want_z = (x as f32 + 0.5) / 64.0;
            let want_w = (y as f32 + 0.5) / 64.0;
            let z = f32_at(&px, p + 2);
            let w = f32_at(&px, p + 3);
            let err = (z - want_z).abs().max((w - want_w).abs());
            max_err = max_err.max(err);
            assert!(err < 1e-3, "pixel ({x},{y}): z={z} want {want_z}, w={w} want {want_w}");
        }
    }
    eprintln!("fullscreen gradient: 4096 pixels, max err {max_err:.2e} ✓");
}

/// ps_tex samples an 8x8 gradient texture (texel (i,j) = (i/8, j/8, 0.5, 1))
/// through the crafted bindless set, at the UV vs_ids runs 0..1 over a 64x64
/// target. With a nearest sampler, pixel (x,y) reads texel
/// (floor((x+0.5)/8), floor((y+0.5)/8)). Check the output against that.
#[test]
fn textured_quad_samples_known_gradient() {
    let Some(vs) = fixture("vs_ids.spv") else { return };
    let Some(ps) = fixture("ps_tex.spv") else { return };
    let Some(ctx) = context() else { return };
    let Some(tex) = TexturedQuad::new(&ctx, 8, 8).ok() else { return };
    // 8x8 gradient texels as RGBA8 UNORM.
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
    let pc = tex.push_constants();
    let Some(px) = rt.draw_with_set(&vs, &ps, [0.0, 0.0, 0.0, 0.0],
        std::slice::from_ref(&tex.set_layout), tex.set, &pc).ok() else { return };
    assert_eq!(px.len(), 64 * 64 * 4);
    let mut max_err = 0.0f32;
    for y in 0..64 {
        for x in 0..64 {
            let tx = ((x as f32 + 0.5) / 8.0).floor() as usize;
            let ty = ((y as f32 + 0.5) / 8.0).floor() as usize;
            let want_r = tx as f32 / 8.0;
            let want_g = ty as f32 / 8.0;
            let p = (y * 64 + x) * 4;
            let r = f32_at(&px, p);
            let g = f32_at(&px, p + 1);
            let err = (r - want_r).abs().max((g - want_g).abs());
            max_err = max_err.max(err);
            assert!(err < 0.02, "pixel ({x},{y}): r={r} want {want_r}, g={g} want {want_g} (texel {tx},{ty})");
        }
    }
    eprintln!("textured quad: 4096 pixels, max err {max_err:.2e} ✓");
}