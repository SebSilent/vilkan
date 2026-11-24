#!/usr/bin/env python3
"""Offline SPIR-V compiler sidecar: raw RDNA2 code -> Leonida -> validated SPIR-V.

VIlkan's native Vulkan path consumes SPIR-V compiled by Leonida's clean-room
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
  compile-native.py <raw.blob> <out-dir> [--leonida DIR]

The blob, its .json metadata and outputs are game-derived: keep them outside
the repo (e.g. the external data dir). Nothing here is tracked.
"""
import argparse
import json
import os
import struct
import subprocess
import sys
from pathlib import Path


def compute_header(group, rsrc2, wave64):
    # Leonida compute_fixture.compute_header layout: 0x148 bytes, rsrc2 + group
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
    ap.add_argument("--leonida", default=os.environ.get("LEONIDA_DIR", "../leonida"))
    a = ap.parse_args()
    leonida = Path(a.leonida).resolve()
    tools = leonida / "tools"
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
    for d in (disasm, fssm, spirv):
        d.mkdir(parents=True, exist_ok=True)

    # 1. Disassemble the raw blob (shader_disasm falls back to the s_endpgm scan).
    r = subprocess.run([sys.executable, "-I", str(tools / "shader_disasm.py"),
                        a.blob, "--out", str(disasm)], capture_output=True, text=True)
    sys.stderr.write(r.stdout + r.stderr)
    src = disasm / f"{stem}.s"
    if not src.exists():
        sys.exit(f"disassembly produced no {src}")
    # 2. Name it cs_ for the compute stage; FSSM sidecar from the real header.
    (disasm / f"{name}.s").write_text(src.read_text())
    src.unlink()
    (fssm / f"{name}.fssm").write_bytes(compute_header(group, meta["rsrc2"], meta["wave64"]))

    # 3. Translate with wave64 emulation; rdna2spirv validates with spirv-val.
    env = dict(os.environ, LEONIDA_W64_EMULATE="1")
    r = subprocess.run([sys.executable, "-I", str(tools / "rdna2spirv.py"),
                        str(disasm), "--out", str(spirv), "--wave64-emulate"], env=env)
    if r.returncode:
        sys.exit(r.returncode)
    spv = spirv / f"{name}.spv"
    if not spv.exists():
        sys.exit(f"no validated SPIR-V at {spv}")

    # 4. Loud post-check against the real header.
    index = json.loads((spirv / "index.json").read_text())
    check_result(spv, index, meta)
    print(f"SPIR-V: {spv}")


if __name__ == "__main__":
    main()