//! Native Vulkan dispatch of sidecar-compiled SPIR-V, checked numerically.
//!
//! Fixtures (SPIR-V compiled offline by the reference translator with --wave64-emulate)
//! come from `VILKAN_VK_FIXTURES` (a dir of `.spv`); the tests skip cleanly when
//! it is unset or a file is missing, so a clean clone with no game data passes.
//! Nothing game-derived is tracked. Acceptance is a correct dispatch (the output
//! words match the console reference), not "it ran".

use vilkan_vk::Context;

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

/// Dispatch one workgroup and read the first `n` output words.
fn dispatch_words(spv: &[u8], n: usize) -> Option<Vec<u32>> {
    let ctx = match Context::new() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skip: no Vulkan device ({e:#})");
            return None;
        }
    };
    eprintln!("device: {} (subgroup {})", ctx.device_name, ctx.subgroup_size);
    let heap = ctx.heap(64 * 1024).ok()?;
    ctx.dispatch(spv, &heap, [1, 1, 1], &[]).ok()?;
    Some(heap.read_words(n))
}

/// Reference fixture cs_w64x_readlane: v2 = 3*lane+1; cross-half readlane at lanes
/// 40/5/32/33 -> out = 121 121 16 97 16 100 100 121. A 32-lane subgroup that
/// wrapped the lane select would give 25 for lane 40 (the issue this catches).
#[test]
fn readlane_cross_half_native() {
    let Some(spv) = fixture("cs_w64x_readlane.spv") else { return };
    let Some(out) = dispatch_words(&spv, 8) else { return };
    assert_eq!(out, vec![121, 121, 16, 97, 16, 100, 100, 121], "readlane output");
}

/// Reference fixture cs_w64x_writelane: writes into lanes >= 32 then reads them back from
/// the low half -> out = 4660 77 99 2748 0 0 4660 99.
#[test]
fn writelane_cross_half_native() {
    let Some(spv) = fixture("cs_w64x_writelane.spv") else { return };
    let Some(out) = dispatch_words(&spv, 8) else { return };
    assert_eq!(out, vec![4660, 77, 99, 2748, 0, 0, 4660, 99], "writelane output");
}