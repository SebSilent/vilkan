#!/usr/bin/env python3
"""Offline SPIR-V compiler sidecar: raw RDNA2 code -> Leonida -> validated SPIR-V.

VIlkan's native Vulkan path consumes SPIR-V compiled by Leonida's clean-room
RDNA2 translator (GPL-3.0), not a port of it. This script wires the steps:

  1. shader_disasm.py disassembles the raw RDNA2 blob (llvm-mc, AMDGPU target).
  2. a synthetic FSSM compute header carries the thread-group size (the raw
     blob has none); the .s is named cs_<stem> so the stage is compute.
  3. rdna2spirv.py --wave64-emulate translates to SPIR-V and validates it.

Usage:
  compile-native.py <raw.blob> <out-dir> [--group X,Y,Z] [--leonida DIR]

The blob and outputs are game-derived: keep them outside the repo (e.g. the
external data dir). Nothing here is tracked.
"""
import argparse
import os
import struct
import subprocess
import sys
from pathlib import Path


def compute_header(group, rsrc2=0x8088):
    # Leonida compute_fixture.compute_header: 0x148 bytes, rsrc2 + group at 0x138.
    # The FSSM magic makes fssm_header.wave_size() read the wave flag at 0xD4
    # (bit 5 clear = wave64); without it the translator falls back to code
    # markers, and a blur shader has none, so it would be taken as wave32.
    h = bytearray(0x148)
    h[0:4] = b"FSSM"
    struct.pack_into("<4I", h, 0x138, rsrc2, *group)
    return h


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("blob")
    ap.add_argument("out")
    ap.add_argument("--group", default="8,8,1")
    ap.add_argument("--leonida", default=os.environ.get("LEONIDA_DIR", "../leonida"))
    a = ap.parse_args()
    leonida = Path(a.leonida).resolve()
    tools = leonida / "tools"
    out = Path(a.out).resolve()
    stem = Path(a.blob).stem
    name = f"cs_{stem}"
    group = tuple(int(x) for x in a.group.split(","))

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
    # 2. Name it cs_ for the compute stage; add the FSSM group-size sidecar.
    (disasm / f"{name}.s").write_text(src.read_text())
    src.unlink()
    (fssm / f"{name}.fssm").write_bytes(compute_header(group))

    # 3. Translate with wave64 emulation; rdna2spirv validates with spirv-val.
    env = dict(os.environ, LEONIDA_W64_EMULATE="1")
    r = subprocess.run([sys.executable, "-I", str(tools / "rdna2spirv.py"),
                        str(disasm), "--out", str(spirv), "--wave64-emulate"], env=env)
    if r.returncode:
        sys.exit(r.returncode)
    spv = spirv / f"{name}.spv"
    if not spv.exists():
        sys.exit(f"no validated SPIR-V at {spv}")
    print(f"SPIR-V: {spv}")


if __name__ == "__main__":
    main()