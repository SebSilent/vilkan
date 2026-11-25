//! Print the raw bytes of every stage record in a real console container, so a
//! layout claim can be checked byte-for-byte against a measurement instead of
//! against a guess. Reads a path given on the command line; game-derived input
//! stays outside the tree and nothing is written.
//!
//!   cargo run -p victory-shader --example dump-fssm -- <container.bin>
//!
//! Accepts a raw `DXBC` container or a `VSH1`-wrapped one (as the on-disk
//! shader cache stores them). For each native blob it inflates the `XBCH`
//! stream, walks the `FSSM` slot table and dumps each present record's 0x54
//! bytes as dwords, annotated with the field names the owned decoder reads, so
//! a foreign record shape is visible rather than inferred.

use victory_shader::dxbc::{Container, FourCC};

const XDAT_OFFSET: usize = 0x18;
const KIND_NATIVE: u32 = 0x0d;
const XBCH_HEADER: usize = 0x18;
const FSSM_HEADER: usize = 0x10;
const RECORD_SIZE: usize = 0x54;
const ABSENT: u64 = 0xDEAD_BEEF_DEAD_BEEF;
/// (stage, offset of its record-offset slot in the FSSM payload).
const SLOTS: [(&str, usize); 8] = [
    ("LS", 0x08),
    ("HS", 0x10),
    ("ES", 0x18),
    ("GS", 0x20),
    ("VS", 0x28),
    ("PS", 0x30),
    ("MS", 0x48),
    ("CS", 0x60),
];

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}
fn u64_at(d: &[u8], at: usize) -> Option<u64> {
    d.get(at..at + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))
}

fn main() {
    let path = std::env::args().nth(1).expect("container path");
    let raw = std::fs::read(&path).expect("read container");
    let container: &[u8] = if raw.len() >= 12 && &raw[..4] == b"VSH1" {
        let len = u32_at(&raw, 8).unwrap() as usize;
        raw.get(12..12 + len).expect("VSH1 payload")
    } else {
        &raw
    };
    let c = Container::parse(container).expect("parse DXBC");
    println!(
        "{}: parts {:?}",
        path,
        c.parts.iter().map(|p| String::from_utf8_lossy(&p.fourcc.0).into_owned()).collect::<Vec<_>>()
    );
    let Some(dxil) = c.part(FourCC::DXIL) else {
        println!("  no DXIL part");
        return;
    };
    if dxil.data.get(XDAT_OFFSET..XDAT_OFFSET + 4) != Some(b"XDAT") {
        println!("  DXIL has no XDAT index at +{XDAT_OFFSET:#x}");
        return;
    }
    for fourcc in [b"HASH", b"SFI0"] {
        if let Some(part) = c.parts.iter().find(|p| &p.fourcc.0 == fourcc) {
            println!("  part {}: {}", String::from_utf8_lossy(fourcc), hex_words(part.data));
        }
    }
    let count = u32_at(dxil.data, XDAT_OFFSET + 4).unwrap() as usize;
    for i in 0..count.min(64) {
        let at = XDAT_OFFSET + 12 + 12 * i;
        let (Some(kind), Some(offset), Some(size)) = (
            u32_at(dxil.data, at),
            u32_at(dxil.data, at + 4),
            u32_at(dxil.data, at + 8),
        ) else {
            break;
        };
        if kind != KIND_NATIVE {
            println!("  xdat[{i}] kind {kind:#x} off {offset:#x} size {size:#x}");
            continue;
        }
        let Some(blob) = dxil.data.get(offset as usize..offset as usize + size as usize) else {
            println!("  xdat[{i}] native blob out of bounds");
            continue;
        };
        if blob.get(0..4) != Some(b"XBCH") {
            println!("  xdat[{i}] kind 0x0d lacks XBCH magic");
            continue;
        }
        let (Some(packed), Some(unpacked)) = (u64_at(blob, 8), u64_at(blob, 16)) else {
            continue;
        };
        let Some(stream) = blob.get(XBCH_HEADER..XBCH_HEADER + packed as usize) else {
            println!("  xdat[{i}] XBCH stream out of bounds");
            continue;
        };
        let Some(payload) = inflate(stream, unpacked as usize) else {
            println!("  xdat[{i}] XBCH stream does not inflate");
            continue;
        };
        dump_fssm(i, &payload);
    }
}

fn inflate(source: &[u8], expected: usize) -> Option<Vec<u8>> {
    let mut state = zlib_rs::Inflate::new(true, 15);
    let mut out = vec![0u8; expected];
    let mut pos = 0usize;
    loop {
        let status = state
            .decompress(&source[pos..], &mut out[pos..], zlib_rs::InflateFlush::Finish)
            .ok()?;
        let next_in = state.total_in() as usize;
        let next_out = state.total_out() as usize;
        if next_out <= pos && next_in <= pos {
            return None;
        }
        pos = next_out;
        if pos >= expected || status == zlib_rs::Status::StreamEnd {
            if pos != expected {
                return None;
            }
            out.truncate(pos);
            return Some(out);
        }
    }
}

fn dump_fssm(index: usize, fssm: &[u8]) {
    if fssm.get(0..4) != Some(b"FSSM") {
        println!("  xdat[{index}] inflated blob lacks FSSM magic");
        return;
    }
    // The slot table and every record/code offset are relative to the payload,
    // which is the buffer AFTER the 0x10-byte header (as the owned decoder
    // slices it: programs() works on &fssm[FSSM_HEADER..]).
    let payload = &fssm[FSSM_HEADER.min(fssm.len())..];
    let slots = u64_at(fssm, 0x00).unwrap_or(0);
    // The header window fssm.md documents: code block off/size, flags, record
    // count, stage, LDS and the VGPR/SGPR word. Values here are file offsets,
    // so +0x10 is already included.
    println!(
        "    header: magic {slots:#x} v{:#x} size {:#x} | code blk {:#x} size {:#x} | flags {:#x}          fetch v{:#x} | recs {:#x} | stage {:#x} | float {:#x} | lds {:#x} | regs {:#x}",
        u32_at(fssm, 4).unwrap_or(0), u64_at(fssm, 8).unwrap_or(0),
        u64_at(fssm, 0xc0).unwrap_or(0), u64_at(fssm, 0xc8).unwrap_or(0),
        u32_at(fssm, 0xd4).unwrap_or(0), u32_at(fssm, 0xd8).unwrap_or(0),
        u32_at(fssm, 0xf8).unwrap_or(0), fssm.get(0x100).copied().unwrap_or(0),
        u32_at(fssm, 0x104).unwrap_or(0), u32_at(fssm, 0x10c).unwrap_or(0),
        u32_at(fssm, 0x110).unwrap_or(0),
    );
    println!("    header words 0xc0..0x118 {}", hex_words(fssm.get(0xc0..0x118.min(fssm.len())).unwrap_or(&[])));
    let slots = u64_at(payload, 0x00).unwrap_or(0);
    println!(
        "  xdat[{index}] FSSM v{:#x} payload {:#x} bytes, slot-count field {slots:#x}",
        u32_at(payload, 4).unwrap_or(0),
        payload.len()
    );
    // The slot table starts at +0x08 per the owned decoder: u64 count, then
    // per-stage record offsets. Print the raw table so the sentinel is visible.
    let table: Vec<String> = SLOTS
        .iter()
        .map(|(stage, at)| format!("{stage}={:#x}", u64_at(payload, *at).unwrap_or(0)))
        .collect();
    println!("    slots {}", table.join(" "));
    for (stage, at) in SLOTS {
        let Some(start) = u64_at(payload, at) else { continue };
        if start == ABSENT || start == 0 {
            continue;
        }
        let Some(record) = payload.get(start as usize..start as usize + RECORD_SIZE) else {
            println!("    {stage} record at {start:#x} out of bounds");
            continue;
        };
        let code_off = u64_at(record, 0x00).unwrap_or(0);
        let code_size = u64_at(record, 0x08).unwrap_or(0);
        println!(
            "    {stage} record @{start:#x}: code off {code_off:#x} size {code_size:#x} \
             flags {:#x} layout {:#x} used {:#x} counts {} {} lds {:#x} regs {:#x}",
            u32_at(record, 0x14).unwrap_or(0),
            u64_at(record, 0x28).unwrap_or(0),
            u64_at(record, 0x30).unwrap_or(0),
            u32_at(record, 0x38).unwrap_or(0),
            u32_at(record, 0x3c).unwrap_or(0),
            u32_at(record, 0x4c).unwrap_or(0),
            u32_at(record, 0x50).unwrap_or(0),
        );
        println!("      words {}", hex_words(record));
        // `dump-fssm <container> heads`: one machine-readable line per program,
        // the 24-byte code-block prefix that docs/pipeline-cache.md calls the
        // shader hash. The prefix is compressed inside the container, so a
        // search of raw file bytes cannot find it - this inflates it first.
        if std::env::args().nth(2).as_deref() == Some("heads") {
            if let Some(pre) = payload.get(code_off as usize..code_off as usize + 24) {
                let hex = pre.iter().map(|b| format!("{b:02x}")).collect::<String>();
                println!("HEAD {stage} {hex}");
            }
        }
        if let Some(head) = payload.get(code_off as usize..code_off as usize + 16) {
            println!("      code head {}", hex_words(head));
        if let Some(blk) = payload.get(code_off as usize..code_off as usize + 0x30) {
            println!("      block head 0x30 {}", hex_words(blk));
        }
        // s_code_end padding: how far in the block the real program ends.
        let end = (code_off as usize..(code_off + code_size).min(payload.len() as u64) as usize)
            .step_by(4)
            .find(|i| u32_at(payload, *i) == Some(0xBF9F_0000));
        println!("      s_code_end at {end:?} (block size {code_size:#x})");
        }
    }
}

/// 4-byte little-endian words, as the runtime dump prints them.
fn hex_words(d: &[u8]) -> String {
    d.chunks(4)
        .map(|w| {
            let mut b = [0u8; 4];
            b[..w.len()].copy_from_slice(w);
            format!("{:#08x}", u32::from_le_bytes(b))
        })
        .collect::<Vec<_>>()
        .join(" ")
}
