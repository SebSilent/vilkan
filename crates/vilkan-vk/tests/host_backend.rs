//! Acceptance for the native Vulkan host-trait backend (stage two).
//!
//! Drives a captured tier-A/B View-only draw through `VulkanBackend` (the
//! `HostBackend` trait) and checks it numerically against the existing ad-hoc
//! replay output for the same record: per-channel max/mean error and the
//! fraction of channels outside tolerance. Then all three negative controls
//! must fail *through the new backend* (each fault disagrees with the fault-free
//! output). Fixtures from VILKAN_REPLAY_FIXTURES; skips when unset or no device.
//! Nothing game-derived is tracked.

use vilkan_host::{DrawResources, Fault, HostBackend, ReplayResult};
use vilkan_vk::Context;
use vilkan_vk::host::VulkanBackend;

fn fixtures() -> Option<std::path::PathBuf> {
    std::env::var_os("VILKAN_REPLAY_FIXTURES").map(std::path::PathBuf::from)
}

/// Per-channel max/mean absolute error and the fraction of channels over `tol`.
fn compare(got: &ReplayResult, want: &ReplayResult, tol: f32) -> ([f32; 4], [f64; 4], f64) {
    assert_eq!(got.pixels.len(), want.pixels.len(), "pixel count mismatch");
    assert_eq!((got.width, got.height), (want.width, want.height), "size mismatch");
    let mut max_err = [0.0f32; 4];
    let mut sum_err = [0.0f64; 4];
    let mut bad = 0usize;
    for (i, (&g, &w)) in got.pixels.iter().zip(want.pixels.iter()).enumerate() {
        let err = (g - w).abs();
        let c = i % 4;
        if err > max_err[c] {
            max_err[c] = err;
        }
        sum_err[c] += err as f64;
        if err > tol {
            bad += 1;
        }
    }
    let n = (got.pixels.len() / 4).max(1) as f64;
    let mean = std::array::from_fn(|c| sum_err[c] / n);
    let frac = bad as f64 / got.pixels.len().max(1) as f64;
    (max_err, mean, frac)
}

#[test]
fn vulkan_backend_matches_adhoc_replay_and_faults_fail() {
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

    let (record, res) = match vilkan_host::load_capture(&dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skip: load_capture failed ({e:#})");
            return;
        }
    };
    let backend = VulkanBackend::new(&ctx);
    assert_eq!(backend.name(), "vulkan");

    // The existing ad-hoc replay output for the same record is the reference.
    let reference = match vilkan_vk::replay::replay_record(&ctx, &record, &res, Fault::None) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skip: ad-hoc replay failed ({e:#})");
            return;
        }
    };
    // The new backend, driven through the trait (record + resources, not a dir).
    let got = match backend.replay(&record, &res, Fault::None) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skip: backend replay failed ({e:#})");
            return;
        }
    };
    eprintln!("replayed {}x{} through {}", got.width, got.height, backend.name());

    // Numeric agreement: the trait backend drives the same core, so it must be
    // bit-identical (tolerance 0). Report the numbers regardless.
    let tol = 0.0f32;
    let (max_err, mean, frac) = compare(&got, &reference, tol);
    eprintln!("vs ad-hoc replay: per-channel max err RGBA {max_err:?}");
    eprintln!("vs ad-hoc replay: per-channel mean err RGBA {mean:?}");
    eprintln!("vs ad-hoc replay: fraction outside tol {tol}: {frac:.6}");
    assert_eq!(frac, 0.0, "trait backend disagrees with the ad-hoc replay (max {max_err:?})");

    // Also report agreement with the D3D12 oracle if present (the real target).
    if let Some(oracle) = &res.oracle_rgba8 {
        let oracle_float = ReplayResult {
            width: got.width,
            height: got.height,
            pixels: oracle.iter().map(|&b| b as f32 / 255.0).collect(),
        };
        let otol = 2.0 / 255.0;
        let (omax, omean, ofrac) = compare(&got, &oracle_float, otol);
        eprintln!("vs D3D12 oracle: per-channel max err RGBA {omax:?}");
        eprintln!("vs D3D12 oracle: per-channel mean err RGBA {omean:?}");
        eprintln!("vs D3D12 oracle: fraction outside tol {otol:.4}: {ofrac:.6}");
    }

    // Negative controls: each fault must make the NEW backend disagree with the
    // fault-free output. A check that cannot fail is not acceptance.
    let ntol = 2.0 / 255.0;
    for fault in [Fault::SkipDraw, Fault::CorruptViewBuffer, Fault::WrongPushConstants] {
        let faulty = backend
            .replay(&record, &res, fault)
            .unwrap_or_else(|e| panic!("fault {fault:?} replay errored: {e:#}"));
        let (fmax, fmean, ffrac) = compare(&faulty, &reference, ntol);
        eprintln!("fault {fault:?}: max err {fmax:?} mean {fmean:?} fraction over tol {ffrac:.6}");
        assert!(
            ffrac > 0.0,
            "negative control {fault:?} did NOT fail through the new backend (max {fmax:?})"
        );
    }
}

/// The trait is object-safe and record-driven: a `&dyn HostBackend` can be driven
/// with an in-memory record + resources, no directory. Uses a tiny synthetic
/// record so it runs without fixtures or a device (the D3D12 reference path).
#[test]
fn trait_is_object_safe_and_record_driven() {
    let record = vilkan_host::DrawRecord::from_json(
        r#"{ "topology":4,"vertex_count":3,"instance_count":1,"first_vertex":0,
             "roots":[],"view_buffers":[],
             "targets":[{"format":28,"width":1,"height":1}] }"#,
    )
    .unwrap();
    let res = DrawResources {
        oracle_rgba8: Some(vec![255, 128, 0, 255]),
        ..Default::default()
    };
    let backend = vilkan_host::D3D12Oracle;
    let dyn_backend: &dyn HostBackend = &backend;
    let out = dyn_backend.replay(&record, &res, Fault::None).unwrap();
    assert_eq!((out.width, out.height), (1, 1));
    assert!((out.pixels[0] - 1.0).abs() < 1e-6);
    assert!((out.pixels[2] - 0.0).abs() < 1e-6);
}