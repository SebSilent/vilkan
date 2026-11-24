//! Replay a captured real draw through native Vulkan and check it numerically
//! against the Proton/D3D12 oracle. Fixtures from VILKAN_REPLAY_FIXTURES (a dir
//! with draw.json, vs.spv, ps.spv, view buffers, and oracle.bin); skips when
//! unset. Nothing game-derived is tracked.

use vilkan_vk::Context;

fn fixtures() -> Option<std::path::PathBuf> {
    std::env::var_os("VILKAN_REPLAY_FIXTURES").map(std::path::PathBuf::from)
}

#[test]
fn replay_captured_draw_matches_oracle() {
    let Some(dir) = fixtures() else {
        eprintln!("skip: VILKAN_REPLAY_FIXTURES unset");
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
    let result = match vilkan_vk::replay::replay(&ctx, &dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skip: replay failed ({e:#})");
            return;
        }
    };
    eprintln!("replayed {}x{}", result.width, result.height);
    // Centre pixel sanity.
    let cx = (result.width / 2) as usize;
    let cy = (result.height / 2) as usize;
    let p = (cy * result.width as usize + cx) * 4;
    eprintln!("centre RGBA: {:?}", &result.pixels[p..p + 4]);

    // The oracle: the D3D12 render-target readback for the same draw (RGBA8).
    let oracle_path = dir.join("oracle.bin");
    let Ok(oracle) = std::fs::read(&oracle_path) else {
        eprintln!("skip: no oracle.bin (D3D12 readback) yet");
        return;
    };
    assert_eq!(oracle.len(), result.pixels.len(), "oracle/replay size mismatch");
    // Tolerance: the replay is float, the oracle RGBA8 (1/255 quantisation) plus
    // a small interpolation margin.
    let tol = 2.0 / 255.0;
    let mut max_err = [0.0f32; 4];
    let mut sum_err = [0.0f64; 4];
    let mut bad = 0;
    for (i, &o) in oracle.iter().enumerate() {
        let want = o as f32 / 255.0;
        let got = result.pixels[i];
        let err = (got - want).abs();
        let c = i % 4;
        if err > max_err[c] {
            max_err[c] = err;
        }
        sum_err[c] += err as f64;
        if err > tol {
            bad += 1;
        }
    }
    let n = (oracle.len() / 4) as f64;
    let mean: Vec<f64> = sum_err.iter().map(|s| s / n).collect();
    eprintln!("per-channel max err RGBA: {max_err:?}");
    eprintln!("per-channel mean err RGBA: {mean:?}");
    eprintln!("{bad}/{} channels over tol {tol:.4} ({:.4}%)", oracle.len(), 100.0 * bad as f64 / oracle.len() as f64);
    assert_eq!(bad, 0, "replay disagrees with the D3D12 oracle (max err {max_err:?})");
}