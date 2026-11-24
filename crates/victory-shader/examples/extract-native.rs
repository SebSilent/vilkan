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

use victory_shader::dxbc::{Container, FourCC};
use victory_shader::native::{HwStage, StageIo};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: extract-native <container.bin> <out.blob>");
        std::process::exit(2);
    }
    let data = std::fs::read(&args[1]).expect("read container");
    let c = Container::parse(&data).expect("parse DXBC");
    let dxil = c.part(FourCC::DXIL).expect("DXIL part");
    let programs = victory_shader::native_programs(dxil.data).expect("native programs");
    let cs = programs.into_iter().next().expect("a native program");
    let (rsrc2, threads) = match &cs.io {
        StageIo::Cs(io) => (io.rsrc2, io.threads),
        _ => (0, [1, 1, 1]), // VS/PS: no compute group; stage comes from the name prefix
    };
    eprintln!(
        "{:?}: wave64={} vgprs={} code={} bytes rsrc2={:#x} group={:?}",
        cs.stage, cs.wave64, cs.vgprs, cs.code.len(), rsrc2, threads
    );
    std::fs::write(&args[2], &cs.code).expect("write blob");
    // Graphics stages need the container's signatures/PSV0; keep it beside the blob.
    std::fs::write(format!("{}.dxbc", args[2]), &data).expect("write container");
    // Real header metadata, for the sidecar to use instead of asserting it.
    // Resource metadata: the root layout and the descriptors the code reads,
    // straight from the FSSM record (verified, not asserted).
    let kinds: Vec<&str> = cs.used.iter().map(|u| match u.kind {
        victory_shader::native::ResourceType::Srv => "srv",
        victory_shader::native::ResourceType::Sampler => "sampler",
        victory_shader::native::ResourceType::Cbv => "cbv",
        victory_shader::native::ResourceType::Uav => "uav",
        victory_shader::native::ResourceType::Other(_) => "other",
    }).collect();
    eprintln!("resources: {} root params, {} used {:?}", cs.layout.len(), cs.used.len(), kinds);
    let stage = match cs.stage {
        HwStage::Vs => "vs", HwStage::Ps => "ps", HwStage::Cs => "cs",
        HwStage::Ms => "ms", HwStage::Gs => "gs", _ => "cs",
    };
    let json = format!(
        "{{\"stage\":\"{}\",\"wave64\":{},\"vgprs\":{},\"rsrc2\":{},\"group\":[{},{},{}],\"code_bytes\":{},\"root_params\":{},\"used_resources\":{}}}\n",
        stage, cs.wave64, cs.vgprs, rsrc2, threads[0], threads[1], threads[2], cs.code.len(),
        cs.layout.len(), cs.used.len()
    );
    std::fs::write(format!("{}.json", args[2]), json).expect("write metadata");
}