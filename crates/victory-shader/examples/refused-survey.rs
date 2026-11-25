//! Run the owned translation chain over every container the guest pipeline
//! cache refused, and say for each one what happened: a host shader (a DXIL
//! container plus its binding model - this chain does not write SPIR-V), or
//! the exact error. The point is to settle whether these shaders fail because the
//! translator lacks something (a 64-lane wave it will not widen, an opcode,
//! the metadata a record does not carry) or because the bytes handed it are
//! not a container at all - a question the game cannot answer, since the
//! cache path never reaches the translator.
//!
//!   VICTORY_WAVE_WHY=1 cargo run -p victory-shader --release --example \
//!     refused-survey -- <dir-of-containers>
//!
//! Reads only. `VICTORY_WAVE_WHY=1` makes the emitter report, per program,
//! why a wave stayed 64 lanes wide when it could not be proven 32.

use std::collections::BTreeMap;
use std::path::PathBuf;

fn main() {
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: refused-survey <directory of containers>");
    let why = std::env::var_os("VICTORY_WAVE_WHY").is_some();

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("cannot read the directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().map(|x| x == "bin").unwrap_or(false)
                && std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
        })
        .collect();
    files.sort();

    // One line per container, then the breakdown: the lines are the answer
    // ("this shader fails with this error"), the breakdown is what makes it
    // readable when there are hundreds of them.
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut ok = 0usize;
    for path in &files {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(e) => {
                println!("{stem}: unreadable: {e}");
                *kinds.entry("unreadable".to_string()).or_default() += 1;
                continue;
            }
        };
        match victory_shader::emit::translate(&data) {
            Ok(host) => {
                ok += 1;
                println!(
                    "{stem}: OK stage={:?} dxil={} bytes, {} user-data dwords, \
                     {} samplers, {} root CBV/SRV/UAV",
                    host.stage,
                    host.dxil.len(),
                    host.user_data_dwords(),
                    host.samplers.len(),
                    host.root_cbvs.len() + host.root_srvs.len() + host.root_uavs.len()
                );
                *kinds.entry("translated".to_string()).or_default() += 1;
            }
            Err(e) => {
                // The exact error, as the translator words it, plus the stage
                // record size so a container can be found again by eye.
                println!("{stem}: ERR {e} (container {} bytes)", data.len());
                *kinds.entry(classify(&e.to_string())).or_default() += 1;
            }
        }
    }

    println!("\n--- {} containers, {} translated ---", files.len(), ok);
    for (kind, n) in &kinds {
        println!("{n:5}  {kind}");
    }
    if why {
        println!("\n(VICTORY_WAVE_WHY was set: wave-width reasons above are the emitter's)");
    }
}

/// An error's first sentence, with the varying numbers removed: two shaders
/// that fail the same way should count together.
fn classify(text: &str) -> String {
    let head = text.split('\n').next().unwrap_or(text);
    let mut out = String::new();
    for c in head.chars() {
        out.push(if c.is_ascii_digit() { '#' } else { c });
    }
    out
}
