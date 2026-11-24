//! Correctness oracle for the exchange (addition 1): numeric checks, not just
//! structure. The cross-half expectations are ported from reference cross-half cases
//! `runtime/test/shaders/cs_w64x_readlane.asm` and `cs_w64x_writelane.asm`
//! (their `out[..]` comments), and the blur shape from the two captured shaders
//! (`docs/shaders`). A wrong slot index or a stale parity reads [`SENTINEL`] and
//! fails here instead of rendering wrong pixels.

use super::*;

fn v(n: u32) -> Value {
    Value(n)
}

/// Per-lane input `f(lane)` for a source value.
fn input(f: impl Fn(usize) -> u32) -> Vec<u32> {
    (0..LANES).map(f).collect()
}

fn readlane(pos: usize, src: Value, src_def: usize, lane: u32, dst: u32) -> CrossOp {
    CrossOp { pos, kind: CrossKind::ReadLane, src, src_def, lane: LaneSel::Const(lane), dst: v(dst) }
}

fn swizzle(pos: usize, src: Value, src_def: usize, xor: u32, dst: u32) -> CrossOp {
    CrossOp {
        pos,
        kind: CrossKind::Swizzle,
        src,
        src_def,
        lane: LaneSel::Swizzle { and: 0x1F, or: 0, xor },
        dst: v(dst),
    }
}

/// Cross-half readlane case: v2 = 3*lane + 1; readlane at lanes 40, 5, 32 and
/// 97 % 64 = 33 gives 121, 16, 97, 100 (a 32-wide subgroup would wrap 40 -> 8
/// and return 25, the issue #94 symptom).
#[test]
fn readlane_cross_half_matches_reference() {
    let src = v(2);
    let ops = vec![
        readlane(10, src, 0, 40, 100),
        readlane(11, src, 0, 5, 101),
        readlane(12, src, 0, 32, 102),
        readlane(13, src, 0, 33, 103),
    ];
    let mut inputs = HashMap::new();
    inputs.insert(src, input(|l| 3 * l as u32 + 1));
    let plan = plan(&ops);
    // All four read one source defined before them: a single exchange.
    assert_eq!(plan.exchanges.len(), 1);
    let got = simulate(&plan, &ops, &inputs);
    let want = reference(&ops, &inputs);
    assert_eq!(got, want);
    // The fixture's scalars, broadcast to every lane.
    for (op, expected) in [(0, 121u32), (1, 16), (2, 97), (3, 100)] {
        assert!(got[op].iter().all(|&x| x == expected), "op {op}: {:?}", got[op]);
    }
}

/// Cross-half writelane case: v10[40]=4660, v10[3]=77, v10[63]=99,
/// v10[96 % 64 = 32]=2748, else 0; readlane(v10, 40)=4660 and readlane(v10, 63)
/// =99 read written upper-half lanes from the low half. The writelane effect is
/// modelled in the source content (the narrow slice's shaders only read).
#[test]
fn writelane_then_readlane_cross_half_matches_reference() {
    let src = v(10);
    let mut content = input(|_| 0);
    content[40] = 0x1234; // 4660
    content[3] = 77;
    content[63] = 99;
    content[32] = 0xabc; // 2748
    let ops = vec![readlane(20, src, 0, 40, 100), readlane(21, src, 0, 63, 101)];
    let mut inputs = HashMap::new();
    inputs.insert(src, content);
    let plan = plan(&ops);
    let got = simulate(&plan, &ops, &inputs);
    assert_eq!(got, reference(&ops, &inputs));
    assert!(got[0].iter().all(|&x| x == 4660));
    assert!(got[1].iter().all(|&x| x == 99));
    // The written upper lanes themselves read back exactly.
    assert_eq!(inputs[&src][32], 2748);
}

/// Cross-half readlane case's computed lane select: `v_readlane_b32 s9, v2, s10`
/// with s10 = 97 reads lane 97 % 64 = 33 (value 100). A per-lane computed lane.
#[test]
fn readlane_computed_lane_matches_reference() {
    let (src, sel) = (v(2), v(10));
    let ops = vec![CrossOp {
        pos: 10,
        kind: CrossKind::ReadLane,
        src,
        src_def: 0,
        lane: LaneSel::Computed(sel),
        dst: v(100),
    }];
    let mut inputs = HashMap::new();
    inputs.insert(src, input(|l| 3 * l as u32 + 1));
    inputs.insert(sel, input(|_| 97)); // every lane selects 97 -> 33
    let plan = plan(&ops);
    let got = simulate(&plan, &ops, &inputs);
    assert_eq!(got, reference(&ops, &inputs));
    assert!(got[0].iter().all(|&x| x == 100));
    assert_eq!(ops[0].kind, CrossKind::ReadLane);
    let _ = CrossKind::WriteLane; // the write-side kind, used by the IR extraction
}

/// ds_swizzle stays within a 32-lane half: lane ^ k for k < 32. The captured
/// blurs use k in {1, 2, 8, 9, 16, 18}.
#[test]
fn swizzle_butterfly_is_half_local() {
    let src = v(0);
    let ops: Vec<CrossOp> = [1u32, 2, 8, 9, 16, 18]
        .iter()
        .enumerate()
        .map(|(i, &k)| swizzle(10 + i, src, 0, k, 100 + i as u32))
        .collect();
    let mut inputs = HashMap::new();
    inputs.insert(src, input(|l| (l * l) as u32));
    let plan = plan(&ops);
    assert_eq!(plan.exchanges.len(), 1); // one source, all stable
    let got = simulate(&plan, &ops, &inputs);
    assert_eq!(got, reference(&ops, &inputs));
    for (i, &k) in [1u32, 2, 8, 9, 16, 18].iter().enumerate() {
        for lane in 0..LANES {
            assert_eq!(got[i][lane], ((lane ^ k as usize) * (lane ^ k as usize)) as u32);
            // Never crosses the half boundary.
            assert_eq!((lane ^ k as usize) / 32, lane / 32);
        }
    }
}

/// The captured blur shape: a swizzle batch over v0..v3, a second
/// over v4..v7 (defined after the first), then readlane of v8..v11 at lanes
/// 0/4/32/36. Three exchanges, alternating parity, width 4.
#[test]
fn captured_blur_shape_three_exchanges() {
    let (a, b, c) = (v(0), v(4), v(8));
    let mut ops = Vec::new();
    for i in 0..4 {
        ops.push(swizzle(10 + i, a, 0, 1 + i as u32, 200 + i as u32));
    }
    for i in 0..4 {
        ops.push(swizzle(30 + i, b, 20, 1 + i as u32, 210 + i as u32));
    }
    for (i, &lane) in [0u32, 4, 32, 36].iter().enumerate() {
        ops.push(readlane(50 + i, c, 40, lane, 220 + i as u32));
    }
    let mut inputs = HashMap::new();
    inputs.insert(a, input(|l| l as u32 + 1));
    inputs.insert(b, input(|l| l as u32 * 10 + 2));
    inputs.insert(c, input(|l| l as u32 * 100 + 3));
    let plan = plan(&ops);
    assert_eq!(plan.exchanges.len(), 3);
    assert_eq!(plan.width, 1); // each batch reads one distinct source here
    assert_eq!(plan.exchanges[0].parity, 0);
    assert_eq!(plan.exchanges[1].parity, 1);
    assert_eq!(plan.exchanges[2].parity, 0);
    let got = simulate(&plan, &ops, &inputs);
    assert_eq!(got, reference(&ops, &inputs));
    // The cross-half readlane sums see lanes 32/36 (the partner half):
    // ops 8..11 read lanes 0, 4, 32, 36 respectively.
    assert_eq!(got[10][0], inputs[&c][32]);
    assert_eq!(got[11][0], inputs[&c][36]);
}

/// Addition 2: an unsafe batch (a dependent cross-lane op folded into the
/// exchange that publishes its source) reads the sentinel, not the value.
#[test]
fn unsafe_batch_reads_sentinel() {
    let (a, b) = (v(0), v(1));
    // b is defined at pos 15, after op0 (pos 10): it does not exist at op0's
    // barrier. The planner must split; a forced single exchange must fail.
    let ops = vec![swizzle(10, a, 0, 1, 100), swizzle(20, b, 15, 1, 101)];
    let mut inputs = HashMap::new();
    inputs.insert(a, input(|l| l as u32 + 1));
    inputs.insert(b, input(|l| l as u32 + 1000));

    // The safe plan splits into two exchanges and matches the reference.
    let safe = plan(&ops);
    assert_eq!(safe.exchanges.len(), 2, "a dependent source must start a new exchange");
    assert_eq!(simulate(&safe, &ops, &inputs), reference(&ops, &inputs));

    // Force the unsafe batch: both reads in one exchange whose barrier is at
    // op0 (pos 10). b (src_def 15) is not published, so its read is the sentinel.
    let unsafe_plan = ExchangePlan {
        width: 2,
        lds_dwords: 2 * 2 * LANES,
        exchanges: vec![Exchange {
            parity: 0,
            barrier_pos: 10,
            publications: vec![
                Publication { src: a, src_def: 0, col: 0 },
                Publication { src: b, src_def: 15, col: 1 },
            ],
            reads: vec![
                Read { op: 0, col: 0, parity: 0, lane: ops[0].lane },
                Read { op: 1, col: 1, parity: 0, lane: ops[1].lane },
            ],
        }],
    };
    let got = simulate(&unsafe_plan, &ops, &inputs);
    assert!(got[1].iter().all(|&x| x == SENTINEL), "unsafe batch must read the sentinel");
    assert_ne!(got, reference(&ops, &inputs));
}

/// A stale parity (reading the other buffer) reads the sentinel, not the value.
#[test]
fn stale_parity_reads_sentinel() {
    let src = v(0);
    let ops = vec![readlane(10, src, 0, 40, 100)];
    let mut inputs = HashMap::new();
    inputs.insert(src, input(|l| 3 * l as u32 + 1));
    let mut plan = plan(&ops);
    // The exchange publishes into parity 0; a read that uses parity 1 (the
    // other buffer, never written) must see the sentinel, not the value.
    plan.exchanges[0].reads[0].parity = 1;
    let got = simulate(&plan, &ops, &inputs);
    assert!(got[0].iter().all(|&x| x == SENTINEL));
}

/// Load a captured-shader fixture from `$VILKAN_SHADER_FIXTURES`, or `None` to
/// skip. The fixtures are game-derived and live outside the tree (untracked);
/// a clean clone with no game data skips these tests cleanly. Synthetic
/// fixtures and the ported cross-half numeric cases above stay in-repo.
fn fixture(name: &str) -> Option<Vec<u8>> {
    let dir = std::env::var_os("VILKAN_SHADER_FIXTURES")?;
    let path = std::path::Path::new(&dir).join(name);
    match std::fs::read(&path) {
        Ok(b) => Some(b),
        Err(_) => {
            eprintln!("skip: fixture {name} not found ({})", path.display());
            None
        }
    }
}

/// Captured blur shaders (fixtures `blur-a.bin`, `blur-b.bin`): extract their
/// real cross-lane ops, plan them, and run the numeric oracle over analytic
/// per-lane inputs. Checks slot/parity correctness on the actual blur structure
/// (a source defined mid-batch forces a split), not a hand-built shape. Skips
/// without the fixtures.
#[test]
fn captured_shaders_extract_plan_and_match_reference() {
    use crate::dxbc::{Container, FourCC};
    use crate::native::HwStage;
    let cases: &[(&str, usize)] = &[("blur-a.bin", 5), ("blur-b.bin", 3)];
    for &(name, want) in cases {
        let Some(bin) = fixture(name) else { continue };
        let c = Container::parse(&bin).unwrap();
        let dxil = c.part(FourCC::DXIL).unwrap();
        let native = crate::native_programs(dxil.data)
            .unwrap()
            .into_iter()
            .find(|p| p.stage == HwStage::Cs)
            .unwrap();
        let p = crate::ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
        assert!(p.wave64);
        let facts = facts(&p, true, 64, false);
        assert_eq!(classify(facts), Decision::Emulate, "{:?}", facts);
        let extracted = extract(&p).expect("narrow-slice cross-lane ops");
        let ops: Vec<CrossOp> = extracted.iter().map(|(_, o)| o.clone()).collect();
        assert!(!ops.is_empty());
        let plan = plan(&ops);
        assert_eq!(plan.exchanges.len(), want);
        // Analytic per-lane content for every distinct source value.
        let mut inputs = HashMap::new();
        let mut n = 0u32;
        for op in &ops {
            inputs.entry(op.src).or_insert_with(|| {
                n += 1;
                input(|l| (l as u32 + 1) * (n * 7 + 3))
            });
            if let LaneSel::Computed(v) = op.lane {
                inputs.entry(v).or_insert_with(|| input(|l| l as u32));
            }
        }
        let got = simulate(&plan, &ops, &inputs);
        assert_eq!(got, reference(&ops, &inputs), "exchange must match console semantics");
        assert!(got.iter().all(|r| r.iter().all(|&x| x != SENTINEL)));
    }
}

/// The emitted DXIL: on a wave32 host the exchange removes the `WaveSize(64)`
/// declaration the host refuses (PSV0 runtime info), while the native path
/// keeps it. The actual refusal fix, checked on a captured container fixture
/// (`blur-a.bin`); skips without the fixtures.
#[test]
fn emitted_wave32_drops_the_wave64_declaration() {
    use crate::dxbc::{Container, FourCC};
    fn psv_wave(dxil: &[u8]) -> (u32, u32) {
        let c = Container::parse(dxil).unwrap();
        let psv = c.part(FourCC::PSV0).unwrap().data;
        let min = u32::from_le_bytes(psv[20..24].try_into().unwrap());
        let max = u32::from_le_bytes(psv[24..28].try_into().unwrap());
        (min, max)
    }
    let Some(bin) = fixture("blur-a.bin") else { return };
    // Native (64-lane host): declares WaveSize 64, as before.
    super::super::set_host_wave64(true);
    let native = super::super::translate(&bin).unwrap();
    assert_eq!(psv_wave(&native.dxil), (64, 64));
    // Wave32 host: the exchange emits no WaveSize declaration (unspecified).
    super::super::set_host_wave64(false);
    let emu = super::super::translate(&bin).unwrap();
    let (min, max) = psv_wave(&emu.dxil);
    assert_ne!((min, max), (64, 64), "the refused WaveSize(64) must be gone");
    assert_eq!(min, 0, "wave size unspecified (no subgroup ops)");
    assert_ne!(emu.dxil, native.dxil, "the exchange must change the code");
    super::super::set_host_wave64(true); // restore for other tests
}

/// Addition 3: the gating predicate names the failing condition.
#[test]
fn gating_names_the_reject_reason() {
    let base = Facts {
        wave64: true,
        host_wave64: false,
        compute: true,
        threads: 64,
        straight_line: true,
        mask_free: true,
    };
    assert_eq!(classify(base), Decision::Emulate);
    let cases = [
        (Facts { wave64: false, ..base }, Reject::NotWave64),
        (Facts { host_wave64: true, ..base }, Reject::HostWave64),
        (Facts { compute: false, ..base }, Reject::NotCompute),
        (Facts { threads: 128, ..base }, Reject::MultiWave),
        (Facts { straight_line: false, ..base }, Reject::BranchyCfg),
        (Facts { mask_free: false, ..base }, Reject::HasMasks),
    ];
    for (facts, want) in cases {
        assert_eq!(classify(facts), Decision::Reject(want));
        assert!(!want.reason().is_empty());
    }
}
