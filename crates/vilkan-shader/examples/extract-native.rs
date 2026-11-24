//! Extract the native RDNA2 code of the compute program in a console shader
//! container, plus its real header metadata, for the offline compiler
//! sidecar. Game-derived input and output live outside the tree; this tool only
//! reads a path given on the command line and writes a raw code blob and a JSON
//! metadata sidecar (group size, wave flag, resource layout) read from the real
//! FSSM header — not asserted.
//!
//!   cargo run -p vilkan-shader --example extract-native -- <container.bin> <out.blob>
//!
//! Writes <out.blob> (raw RDNA2 code) and <out.blob>.json (real metadata).

use vilkan_shader::dxbc::{Container, FourCC};
use vilkan_shader::native::{HwStage, StageIo};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: extract-native <container.bin> <out.blob>");
        std::process::exit(2);
    }
    let data = std::fs::read(&args[1]).expect("read container");
    let c = Container::parse(&data).expect("parse DXBC");
    let dxil = c.part(FourCC::DXIL).expect("DXIL part");
    let programs = vilkan_shader::native_programs(dxil.data).expect("native programs");
    let cs = programs
        .into_iter()
        .find(|p| p.stage == HwStage::Cs)
        .expect("a compute program");
    let (rsrc2, threads) = match &cs.io {
        StageIo::Cs(io) => (io.rsrc2, io.threads),
        _ => panic!("compute program without compute io"),
    };
    eprintln!(
        "compute: wave64={} vgprs={} code={} bytes rsrc2={:#x} group={:?}",
        cs.wave64, cs.vgprs, cs.code.len(), rsrc2, threads
    );
    std::fs::write(&args[2], &cs.code).expect("write blob");
    // Real header metadata, for the sidecar to use instead of asserting it.
    // Resource metadata: the root layout and the descriptors the code reads,
    // straight from the FSSM record (verified, not asserted).
    let kinds: Vec<&str> = cs.used.iter().map(|u| match u.kind {
        vilkan_shader::native::ResourceType::Srv => "srv",
        vilkan_shader::native::ResourceType::Sampler => "sampler",
        vilkan_shader::native::ResourceType::Cbv => "cbv",
        vilkan_shader::native::ResourceType::Uav => "uav",
        vilkan_shader::native::ResourceType::Other(_) => "other",
    }).collect();
    eprintln!("resources: {} root params, {} used {:?}", cs.layout.len(), cs.used.len(), kinds);
    let json = format!(
        "{{\"wave64\":{},\"vgprs\":{},\"rsrc2\":{},\"group\":[{},{},{}],\"code_bytes\":{},\"root_params\":{},\"used_resources\":{}}}\n",
        cs.wave64, cs.vgprs, rsrc2, threads[0], threads[1], threads[2], cs.code.len(),
        cs.layout.len(), cs.used.len()
    );
    std::fs::write(format!("{}.json", args[2]), json).expect("write metadata");
}