//! Extract the native RDNA2 code of the compute program in a console shader
//! container, for the offline Leonida compiler sidecar. Game-derived input and
//! output live outside the tree; this tool only reads a path given on the
//! command line and writes a raw code blob.
//!
//!   cargo run -p vilkan-shader --example extract-native -- <container.bin> <out.blob>

use vilkan_shader::dxbc::{Container, FourCC};
use vilkan_shader::native::HwStage;

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
    eprintln!(
        "compute: wave64={} vgprs={} code={} bytes",
        cs.wave64,
        cs.vgprs,
        cs.code.len()
    );
    std::fs::write(&args[2], &cs.code).expect("write blob");
}