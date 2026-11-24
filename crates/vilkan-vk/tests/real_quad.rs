//! Acceptance for the descriptor stage on a REAL captured colour-quad draw.
//!
//! The frame-250 colour quad (ordinal 1779) is the same shader pair as the
//! crafted test (its DXBC containers are byte-identical to the translated
//! `fa982323` blobs), but its captured V# has `num_records = 1`, stride 0: the
//! PS's scale load at +0x20 is out of bounds, the guarded load branch is skipped,
//! and the scale reads 0 — so the D3D12 oracle is black. The referenced CBV sits
//! at a 48-bit console VA (~54 TB) the capture could not map (an empty window);
//! the replay skips empty windows so the heap spans only the table address.
//!
//! This checks the real draw replays through `VulkanBackend` to EXACTLY the D3D12
//! oracle (black), and agrees bit-for-bit with the ad-hoc replay. The crafted
//! non-uniform variant (`colour_quad.rs`) carries the discriminating negative
//! controls; a black draw cannot make them fail, so they are not asserted here.
//! Fixtures from VILKAN_REAL_QUAD (a load_capture dir); skips when unset/no device.

use vilkan_host::{Fault, HostBackend};
use vilkan_vk::Context;
use vilkan_vk::host::VulkanBackend;

#[test]
fn real_colour_quad_matches_d3d12_oracle() {
    let Some(dir) = std::env::var_os("VILKAN_REAL_QUAD").map(std::path::PathBuf::from) else {
        eprintln!("skip: VILKAN_REAL_QUAD unset");
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
    let (record, res) = match vilkan_host::load_capture(&dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skip: load_capture failed ({e:#})");
            return;
        }
    };
    // The captured V# must carry the small bound that makes the load OOB.
    let table = record.roots.iter().find(|r| r.kind == "Table").and_then(|r| r.address);
    if let Some(t) = table {
        if let Some((_, data)) = res.views.iter().find(|(a, _)| *a == t) {
            let num_records = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
            eprintln!("captured V# num_records = {num_records} (load at +0x20 is OOB => scale 0)");
            assert!(num_records < 36, "expected the small bound that makes the scale load OOB");
        }
    }
    let backend = VulkanBackend::new(&ctx);
    let reference = vilkan_vk::replay::replay_record(&ctx, &record, &res, Fault::None).unwrap();
    let got = backend.replay(&record, &res, Fault::None).unwrap();
    eprintln!("replayed {}x{} real colour quad through {}", got.width, got.height, backend.name());

    // Bit-identical to the ad-hoc replay (same core).
    assert_eq!(got.pixels.len(), reference.pixels.len());
    let max_adhoc = got.pixels.iter().zip(&reference.pixels)
        .map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    eprintln!("vs ad-hoc replay: max err {max_adhoc}");
    assert_eq!(max_adhoc, 0.0, "backend disagrees with ad-hoc replay");

    // Exact agreement with the D3D12 oracle (black: the OOB scale reads 0).
    let oracle = res.oracle_rgba8.as_ref().expect("real capture has an oracle");
    assert_eq!(got.pixels.len(), oracle.len());
    let mut max_err = [0.0f32; 4];
    let mut nonzero = 0usize;
    for (i, (&g, &o)) in got.pixels.iter().zip(oracle.iter()).enumerate() {
        let err = (g - o as f32 / 255.0).abs();
        let c = i % 4;
        if err > max_err[c] {
            max_err[c] = err;
        }
        if g != 0.0 {
            nonzero += 1;
        }
    }
    eprintln!("vs D3D12 oracle: per-channel max err RGBA {max_err:?}");
    eprintln!("nonzero output channels: {nonzero} (expected 0: black OOB scale)");
    // RGB is the descriptor path's actual output and must match exactly. Alpha is
    // reported, not asserted: the D3D12 target held alpha 1 *before* this draw (a
    // prior clear/pass), and the draw preserves it via its write-mask/blend. The
    // record now carries the blend/write-mask (see colour_quad.rs's crafted blend
    // acceptance), but this isolated single-draw replay clears alpha to 0 and does
    // not yet carry the target's pre-draw clear value, so the preserved alpha cannot
    // be reproduced here. Closing it needs the pre-draw target state too.
    let rgb_frac = max_err[..3].iter().filter(|&&e| e > 2.0 / 255.0).count();
    eprintln!("alpha max err {} (needs blend + pre-draw clear alpha; see docs)", max_err[3]);
    assert_eq!(rgb_frac, 0, "real colour quad RGB disagrees with the D3D12 oracle (max {max_err:?})");
    assert_eq!(nonzero, 0, "expected a black frame (OOB scale 0), got nonzero channels");
}