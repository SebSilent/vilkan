//! Batch-extract native VS/PS code + metadata + container from carved shader
//! containers, for offline coverage measurement. Read-only on the input; writes
//! blobs to an output dir outside the tree. Game-derived; nothing tracked.
//!
//!   cargo run -p victory-shader --example extract-all -- <carved dir> <out dir>

use victory_shader::dxbc::{Container, FourCC};
use victory_shader::native::{HwStage, ResourceType, StageIo};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: extract-all <carved dir> <out dir>");
        std::process::exit(2);
    }
    let out = std::path::PathBuf::from(&args[2]);
    std::fs::create_dir_all(&out).unwrap();
    let mut files = Vec::new();
    fn collect(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.filter_map(|e| e.ok()) {
                let p = e.path();
                if p.is_dir() { collect(&p, out); } else { out.push(p); }
            }
        }
    }
    collect(std::path::Path::new(&args[1]), &mut files);
    let (mut n_vs, mut n_ps, mut n_skip) = (0, 0, 0);
    for path in &files {
        let Ok(data) = std::fs::read(path) else { continue };
        if data.len() < 4 || &data[..4] != b"DXBC" { continue; }
        let Ok(c) = Container::parse(&data) else { continue };
        let Some(dxil) = c.part(FourCC::DXIL) else { continue };
        let Ok(progs) = victory_shader::native_programs(dxil.data) else { continue };
        for p in progs {
            let stage = match p.stage {
                HwStage::Vs => "vs", HwStage::Ps => "ps", _ => { n_skip += 1; continue; }
            };
            // Name by stage + a short hash of the code (stable, no title strings).
            let hash = blake3_short(&p.code);
            let name = format!("{stage}_{hash}");
            let (rsrc2, threads) = match &p.io {
                StageIo::Cs(io) => (io.rsrc2, io.threads),
                _ => (0, [1, 1, 1]),
            };
            let nused = p.used.len();
            let nroot = p.layout.len();
            let _ = (nused, nroot);
            std::fs::write(out.join(format!("{name}.blob")), &p.code).unwrap();
            std::fs::write(out.join(format!("{name}.dxbc")), &data).unwrap();
            let json = format!(
                "{{\"stage\":\"{stage}\",\"wave64\":{},\"vgprs\":{},\"rsrc2\":{rsrc2},\"group\":[{},{},{}],\"code_bytes\":{},\"root_params\":{nroot},\"used_resources\":{nused}}}\n",
                p.wave64, p.vgprs, threads[0], threads[1], threads[2], p.code.len());
            std::fs::write(out.join(format!("{name}.json")), json).unwrap();
            if stage == "vs" { n_vs += 1; } else { n_ps += 1; }
        }
    }
    eprintln!("extracted VS={n_vs} PS={n_ps} (skipped {n_skip} other-stage)");
}

fn blake3_short(code: &[u8]) -> String {
    // FNV-1a over the code, hex — a stable content name without a crypto dep.
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in code { h = (h ^ b as u64).wrapping_mul(0x100000001b3); }
    format!("{h:016x}")
}