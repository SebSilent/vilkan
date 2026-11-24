//! Print the FNV-1a code hash (matching extract-all's naming) of each native
//! program in a container, so captured draws can be matched to the coverage
//! index. Read-only.
//!   cargo run -p victory-shader --example code-hash -- <container.bin>
use victory_shader::dxbc::{Container, FourCC};
use victory_shader::native::HwStage;

fn fnv(code: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in code { h = (h ^ b as u64).wrapping_mul(0x100000001b3); }
    format!("{h:016x}")
}

fn main() {
    for path in std::env::args().skip(1) {
        let Ok(data) = std::fs::read(&path) else { continue };
        let Ok(c) = Container::parse(&data) else { continue };
        let Some(dxil) = c.part(FourCC::DXIL) else { continue };
        let Ok(progs) = victory_shader::native_programs(dxil.data) else { continue };
        for p in progs {
            let stage = match p.stage {
                HwStage::Vs => "vs", HwStage::Ps => "ps", HwStage::Cs => "cs",
                HwStage::Ms => "ms", HwStage::Gs => "gs", _ => "other",
            };
            println!("{stage}_{}", fnv(&p.code));
        }
    }
}