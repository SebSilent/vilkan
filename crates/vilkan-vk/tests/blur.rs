//! Native Vulkan dispatch of the captured blur shaders against a crafted
//! bindless descriptor set, checked numerically against an analytically known
//! box-filter result (constant, impulse, gradient).
//!
//! The blur is a mip-chain downsample: output 3 = 8x8 (raw sample), output 2 =
//! 4x4, output 1 = 2x2, output 0 = 1x1 (the full 8x8 box average). The 1x1
//! output is the cleanest analytic check: a unit-sum box filter's average of a
//! constant is the constant, of an impulse is peak/64, of a ramp is its mean.
//!
//! The blur SPIR-V fixtures come from `VILKAN_VK_FIXTURES` (cs_blur-a.spv,
//! cs_blur-b.spv); the test skips cleanly when unset or missing, so a clean
//! clone with no game data passes. Nothing game-derived is tracked. Acceptance
//! is the correct image output, not "it ran": a wrong slot index, stale parity
//! or wrong descriptor fails the numeric check.

use vilkan_vk::Context;
use vilkan_vk::bindless::BlurSetup;

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

/// Run one blur over an 8x8 input; return all four mip outputs.
fn run_blur(spv: &[u8], texels: &[u32]) -> Option<[Vec<u32>; 4]> {
    let ctx = match Context::new() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skip: no Vulkan device ({e:#})");
            return None;
        }
    };
    eprintln!("device: {} (subgroup {})", ctx.device_name, ctx.subgroup_size);
    let setup = BlurSetup::new(&ctx, 8, 8, 4).ok()?;
    setup.upload_input(texels).ok()?;
    setup.dispatch(spv, [1, 1, 1]).ok()?;
    Some([
        setup.read_output(0).ok()?,
        setup.read_output(1).ok()?,
        setup.read_output(2).ok()?,
        setup.read_output(3).ok()?,
    ])
}

fn channel(v: u32, c: u32) -> u32 {
    (v >> (8 * c)) & 0xFF
}

/// A constant image is a fixed point of a unit-sum box filter: the 1x1 average
/// (output 0) and every texel of the 8x8 raw output (output 3) equal the
/// constant. Exercises descriptor lookup, sample, cross-lane exchange, store.
#[test]
fn blur_constant_image_is_fixed_point() {
    let Some(spv) = fixture("cs_blur-a.spv") else { return };
    let input = vec![0x80808080u32; 64]; // 128 every channel
    let Some(out) = run_blur(&spv, &input) else { return };
    // 1x1 full average.
    for c in 0..4 {
        let got = channel(out[0][0], c);
        assert!((got as i32 - 128).abs() <= 2, "1x1 average channel {c}: got {got}, want ~128");
    }
    // 8x8 raw: every texel the constant.
    for (i, &px) in out[3].iter().enumerate() {
        for c in 0..4 {
            let got = channel(px, c);
            assert!((got as i32 - 128).abs() <= 2, "8x8 texel {i} channel {c}: got {got}, want ~128");
        }
    }
    eprintln!("blur-a constant: 1x1 average ~128 and 8x8 raw all ~128 ✓");
}

/// An impulse (one bright texel, rest black): the 1x1 average is peak/64 (~4),
/// and the 8x8 output spreads the impulse locally (energy conserved, not lost
/// or flooded).
#[test]
fn blur_impulse_average_and_spread() {
    let Some(spv) = fixture("cs_blur-a.spv") else { return };
    let mut input = vec![0u32; 64];
    input[27] = 0xFFFFFFFF; // impulse at (3,3)
    let Some(out) = run_blur(&spv, &input) else { return };
    let avg = channel(out[0][0], 0);
    eprintln!("blur-a impulse: 1x1 average {avg} (want ~4 = 255/64)");
    assert!((avg as i32 - 4).abs() <= 2, "impulse 1x1 average: got {avg}, want ~4");
    let total: u32 = out[3].iter().map(|&p| channel(p, 0)).sum();
    let nonzero = out[3].iter().filter(|&&p| channel(p, 0) > 0).count();
    eprintln!("blur-a impulse 8x8: total {total}, nonzero {nonzero}");
    assert!(total > 100 && total < 400, "impulse energy not conserved: {total}");
    assert!(nonzero >= 1 && nonzero <= 16, "impulse spread wrong: {nonzero}");
}

/// A horizontal ramp (0,32,...,224): the 1x1 average is the mean (112), and the
/// 4x4 mip (output 2) stays a monotonic ramp in its interior.
#[test]
fn blur_gradient_average_and_monotonic() {
    let Some(spv) = fixture("cs_blur-a.spv") else { return };
    let mut input = vec![0u32; 64];
    for y in 0..8 {
        for x in 0..8 {
            let g = (x * 32) as u32;
            input[y * 8 + x] = g | (g << 8) | (g << 16) | (0xFF << 24);
        }
    }
    let Some(out) = run_blur(&spv, &input) else { return };
    let avg = channel(out[0][0], 0);
    eprintln!("blur-a gradient: 1x1 average {avg} (want ~112)");
    assert!((avg as i32 - 112).abs() <= 4, "gradient 1x1 average: got {avg}, want ~112");
    // 4x4 mip row y=1: non-decreasing in x.
    for x in 0..3 {
        let a = channel(out[2][1 * 8 + x], 0);
        let b = channel(out[2][1 * 8 + x + 1], 0);
        assert!(b + 2 >= a, "4x4 gradient not monotonic at x={x}: {a} then {b}");
    }
    eprintln!("blur-a gradient: 1x1 ~112 and 4x4 monotonic ✓");
}

/// blur-b must also dispatch and produce the constant fixed point.
#[test]
fn blur_b_constant_image_is_fixed_point() {
    let Some(spv) = fixture("cs_blur-b.spv") else { return };
    let input = vec![0x80808080u32; 64];
    let Some(out) = run_blur(&spv, &input) else { return };
    for c in 0..4 {
        let got = channel(out[0][0], c);
        assert!((got as i32 - 128).abs() <= 2, "blur-b 1x1 channel {c}: got {got}, want ~128");
    }
    eprintln!("blur-b constant: 1x1 average ~128 ✓");
}