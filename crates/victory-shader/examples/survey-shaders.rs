//! Read-only survey: find the simplest real VS/PS pair in the shader cache.
//! Reports stage, code size, root params, used resources and vertex inputs so
//! we can scope the minimal crafted setup. Game-derived; reads a cache path
//! given on the command line, writes nothing into the tree.
//!
//!   cargo run -p vilkan-shader --example survey-shaders -- <cache/shaders dir>

use victory_shader::native::{HwStage, StageIo};

fn main() {
    let dir = std::env::args().nth(1).expect("cache/shaders dir");
    let mut files = Vec::new();
    fn collect(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.filter_map(|e| e.ok()) {
                let p = e.path();
                if p.is_dir() { collect(&p, out); } else { out.push(p); }
            }
        }
    }
    collect(std::path::Path::new(&dir), &mut files);
    let mut rows: Vec<(usize, String, usize, usize, usize, String)> = Vec::new();
    for path in &files {
        let Ok(data) = std::fs::read(path) else { continue };
        // Accept raw DXBC containers (carved) or VSH1-wrapped (cache).
        let container: &[u8] = if data.len() >= 12 && &data[..4] == b"VSH1" {
            let len = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
            if data.len() < 12 + len { continue; }
            &data[12..12 + len]
        } else if data.len() >= 4 && &data[..4] == b"DXBC" {
            &data
        } else { continue };
        let Ok(c) = victory_shader::dxbc::Container::parse(container) else { continue };
        let Some(dxil) = c.part(victory_shader::dxbc::FourCC::DXIL) else { continue };
        let Ok(progs) = victory_shader::native_programs(dxil.data) else { continue };
        for p in progs {
            if !matches!(p.stage, HwStage::Vs | HwStage::Ps) { continue; }
            let nroot = p.layout.len();
            let nused = p.used.len();
            let inputs = match &p.io {
                StageIo::Vs { inputs, .. } => inputs.len(),
                _ => 0,
            };
            let kinds: Vec<&str> = p.used.iter().map(|u| match u.kind {
                victory_shader::native::ResourceType::Srv => "srv",
                victory_shader::native::ResourceType::Sampler => "smp",
                victory_shader::native::ResourceType::Cbv => "cbv",
                victory_shader::native::ResourceType::Uav => "uav",
                victory_shader::native::ResourceType::Other(_) => "oth",
            }).collect();
            let stage_s = if p.stage == HwStage::Vs { "VS" } else { "PS" };
            rows.push((p.code.len(), format!("{} root={nroot} used={nused} in={inputs} [{}]",
                stage_s, kinds.join(",")), nroot, nused, inputs,
                path.display().to_string()));
        }
    }
    rows.sort_by_key(|r| (r.2 + r.3, r.0));
    println!("{} VS/PS programs; simplest first:", rows.len());
    for r in rows.iter().take(20) {
        println!("  code={:5} {}  {}", r.0, r.1, r.5);
    }
}