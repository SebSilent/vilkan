#!/usr/bin/env python3
"""Offline SPIR-V compiler sidecar: raw RDNA2 code -> the reference translator -> validated SPIR-V.

VIlkan's native Vulkan path consumes SPIR-V compiled by a clean-room
RDNA2 translator (GPL-3.0), not a port of it. This script wires the steps:

  1. shader_disasm.py disassembles the raw RDNA2 blob (llvm-mc, AMDGPU target).
  2. a synthetic FSSM compute header carries the thread-group size, rsrc2 and
     wave flag READ FROM THE REAL HEADER (the <blob>.json sidecar written by
     extract-native), not asserted. The .s is named cs_<stem> for the stage.
  3. rdna2spirv.py --wave64-emulate translates to SPIR-V and validates it.
  4. a loud post-check: a shader the real header says is wave64 MUST come out
     wave64-emulated with zero subgroup ops and the real LocalSize; otherwise
     this fails (no silent wave32 fallback).

Usage:
  compile-native.py <raw.blob> <out-dir> [--translator DIR]

The blob, its .json metadata and outputs are game-derived: keep them outside
the repo (e.g. the external data dir). Nothing here is tracked.
"""
import argparse
import hashlib
import json
import os
import struct
import subprocess
import sys
from pathlib import Path


def strip_vnop_prologue(code):
    """The console compiler pads the code start with VOP3 v_nop instructions
    (12 bytes each: an 8-byte VOP3 plus an ignored literal). llvm-mc misdecodes
    these, and shader_disasm's scan fallback then skips real code (its minimum
    start is 0x100). Strip the leading v_nops so the functional code disassembles
    from its true start. v_nop is VOP3 (word & 0xFC000000 == 0xD4000000) with
    opcode 0x180 ((word >> 16) & 0x3FF)."""
    off = 0
    while off + 4 <= len(code):
        w = int.from_bytes(code[off:off + 4], "little")
        if (w & 0xFC000000) == 0xD4000000 and ((w >> 16) & 0x3FF) == 0x180:
            off += 12
        else:
            break
    return code[off:], off


def compute_header(group, rsrc2, wave64):
    # the reference compute_fixture.compute_header layout: 0x148 bytes, rsrc2 + group
    # at 0x138. The FSSM magic makes fssm_header.wave_size() read the wave flag
    # at 0xD4 (bit 5 set = wave32, clear = wave64); without the magic the
    # translator falls back to code markers and a blur shader has none.
    h = bytearray(0x148)
    h[0:4] = b"FSSM"
    struct.pack_into("<4I", h, 0x138, rsrc2, *group)
    if not wave64:
        h[0xD4] |= 0x20
    return h


def spirv_instructions(spv):
    """Yield (opcode, operands) per instruction, walking word counts (not every
    word: operands must not be mistaken for opcodes). Skips the 5-word header."""
    data = Path(spv).read_bytes()
    words = [int.from_bytes(data[i:i + 4], "little") for i in range(0, len(data), 4)]
    i = 5
    while i < len(words):
        first = words[i]
        count, opcode = first >> 16, first & 0xFFFF
        if count == 0:
            break
        yield opcode, words[i + 1:i + count]
        i += count


def check_result(spv, index, meta):
    """Fail loudly if a wave64 shader did not come out wave64-emulated."""
    local = None
    subgroup_ops = 0
    subgroup_builtin = False
    for opcode, ops in spirv_instructions(spv):
        # OpExecutionMode = 16, LocalSize = 17: operands [entry, mode, x, y, z].
        if opcode == 16 and len(ops) >= 5 and ops[1] == 17:
            local = ops[2:5]
        # OpGroupNonUniform* opcodes 330..365.
        if 330 <= opcode <= 365:
            subgroup_ops += 1
        # OpDecorate = 71, BuiltIn = 11, SubgroupLocalInvocationId = 41.
        if opcode == 71 and len(ops) >= 2 and ops[1] == 11 and ops[2] == 41:
            subgroup_builtin = True
    entry = next(iter(index.values())) if isinstance(index, dict) else index[0]
    emulated = entry.get("wave64_emulated", False)
    problems = []
    if meta["wave64"]:
        if not emulated:
            problems.append("real header is wave64 but the module is not wave64-emulated")
        if subgroup_ops or subgroup_builtin:
            problems.append(f"subgroup traffic in a wave64-emulated module ({subgroup_ops} ops, builtin={subgroup_builtin})")
        if local != list(meta["group"]):
            problems.append(f"LocalSize {local} != real group {meta['group']}")
    if problems:
        sys.exit("FAIL " + str(spv) + ": " + "; ".join(problems))
    print(f"checked: wave64_emulated={emulated} subgroup_ops={subgroup_ops} LocalSize={local}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("blob")
    ap.add_argument("out")
    ap.add_argument("--translator", default=os.environ.get("TRANSLATOR_DIR"),
                    help="the clean-room RDNA2 translator checkout (or TRANSLATOR_DIR)")
    a = ap.parse_args()
    if not a.translator:
        sys.exit("set --translator or TRANSLATOR_DIR to the RDNA2 translator checkout")
    translator = Path(a.translator).resolve()
    tools = translator / "tools"
    out = Path(a.out).resolve()
    stem = Path(a.blob).stem
    name = f"cs_{stem}"

    # Real header metadata (group, rsrc2, wave flag) — not asserted.
    meta_path = Path(a.blob + ".json")
    if not meta_path.exists():
        sys.exit(f"missing real metadata {meta_path}; run extract-native first")
    meta = json.loads(meta_path.read_text())
    group = tuple(meta["group"])
    print(f"real header: wave64={meta['wave64']} rsrc2={meta['rsrc2']:#x} group={group}")

    work = out / "work"
    disasm = work / "disasm"
    fssm = work / "fssm"
    spirv = out / "spirv"
    cache = out / "cache"
    for d in (disasm, fssm, spirv, cache):
        d.mkdir(parents=True, exist_ok=True)

    # Strip the v_nop padding the console compiler puts at the code start;
    # llvm-mc misdecodes it. Then place the code at a known offset and point the
    # FSSM code-range field (u32 at 0xC0, instructions start 0x28 into the
    # section) at it, so shader_disasm uses the header range instead of its scan
    # fallback (whose minimum start is 0x100 and would skip real code).
    raw = Path(a.blob).read_bytes()
    code, stripped = strip_vnop_prologue(raw)
    if stripped:
        print(f"stripped {stripped} bytes of v_nop prologue")
    code_off = 0x100
    framed = bytearray(code_off + len(code))
    struct.pack_into("<I", framed, 0xC0, code_off - 0x28)
    framed[code_off:] = code
    blob = work / f"{stem}.blob"
    blob.parent.mkdir(parents=True, exist_ok=True)
    blob.write_bytes(framed)

    # Content-addressed cache key: the code AND the FSSM header metadata that
    # affects translation (rsrc2, group, wave flag) AND the emulation marker.
    # A code-only key would miss an rsrc2 correction; this does not.
    key_src = json.dumps(
        {"code_sha256": hashlib.sha256(code).hexdigest(),
         "rsrc2": meta["rsrc2"], "group": meta["group"], "wave64": meta["wave64"],
         "mode": "wave64-emulate"},
        sort_keys=True).encode()
    key = hashlib.sha256(key_src).hexdigest()[:32]
    cached_spv = cache / f"{key}.spv"
    cached_index = cache / f"{key}.index.json"
    spv = spirv / f"{name}.spv"

    if cached_spv.exists() and cached_index.exists():
        spv.write_bytes(cached_spv.read_bytes())
        (spirv / "index.json").write_text(cached_index.read_text())
        print(f"cache hit {key}")
    else:
        # 1. Disassemble the stripped blob (shader_disasm falls back to the
        # s_endpgm scan; with the v_nop prologue gone the clean start is found).
        r = subprocess.run([sys.executable, "-I", str(tools / "shader_disasm.py"),
                            str(blob), "--out", str(disasm)], capture_output=True, text=True)
        sys.stderr.write(r.stdout + r.stderr)
        src = disasm / f"{stem}.s"
        if not src.exists():
            sys.exit(f"disassembly produced no {src}")
        # 2. Name it cs_ for the compute stage; FSSM sidecar from the real header.
        (disasm / f"{name}.s").write_text(src.read_text())
        src.unlink()
        (fssm / f"{name}.fssm").write_bytes(compute_header(group, meta["rsrc2"], meta["wave64"]))

        # 3. Translate with wave64 emulation; rdna2spirv validates with spirv-val.
        r = subprocess.run([sys.executable, "-I", str(tools / "rdna2spirv.py"),
                            str(disasm), "--out", str(spirv), "--wave64-emulate"])
        if r.returncode:
            sys.exit(r.returncode)
        if not spv.exists():
            sys.exit(f"no validated SPIR-V at {spv}")
        cached_spv.write_bytes(spv.read_bytes())
        cached_index.write_text((spirv / "index.json").read_text())
        print(f"compiled {key}")

    # 4. Loud post-check against the real header (always, even on a cache hit).
    index = json.loads((spirv / "index.json").read_text())
    check_result(spv, index, meta)
    print(f"SPIR-V: {spv}")


if __name__ == "__main__":
    main()