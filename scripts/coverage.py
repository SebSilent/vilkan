#!/usr/bin/env python3
"""Measure VS/PS translation coverage over the carved shaders with the vendored
translator. Read-only on game data; all output outside the tree.

    python3 scripts/coverage.py <blobs dir> <work dir> [--limit N]

<blobs dir> holds <name>.blob/.json/.dxbc from extract-all. Frames each (strip
prologue, embed code after the FSSM header), disassembles, translates, and ranks
the failure classes. Nothing here is tracked.
"""
import argparse
import importlib.util
import json
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("cn", HERE / "compile-native.py")
cn = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cn)
TOOLS = HERE.parent / "third_party" / "rdna2spirv" / "tools"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("blobs")
    ap.add_argument("work")
    ap.add_argument("--limit", type=int, default=0)
    a = ap.parse_args()
    blobs = Path(a.blobs)
    work = Path(a.work)
    fssm = work / "fssm"
    disasm = work / "disasm"
    spirv = work / "spirv"
    for d in (fssm, disasm, spirv):
        d.mkdir(parents=True, exist_ok=True)

    names = sorted(p.stem for p in blobs.glob("*.blob"))
    if a.limit:
        names = names[:a.limit]
    framed_count = 0
    for name in names:
        meta = json.loads((blobs / f"{name}.json").read_text())
        raw = (blobs / f"{name}.blob").read_bytes()
        code, _ = cn.strip_vnop_prologue(raw)
        code_off = 0x148
        framed = bytearray(cn.compute_header(tuple(meta["group"]), meta["rsrc2"], meta["wave64"]))
        framed.extend(bytearray(code_off + len(code) - len(framed)))
        import struct
        struct.pack_into("<I", framed, 0xC0, code_off - 0x28)
        framed[code_off:code_off + len(code)] = code
        (fssm / f"{name}.fssm").write_bytes(framed)
        dxbc = blobs / f"{name}.dxbc"
        if dxbc.exists():
            shutil.copy(dxbc, work / f"{name}.dxbc")
        framed_count += 1
    print(f"framed {framed_count} shaders", flush=True)

    # Disassemble all framed blobs.
    r = subprocess.run([sys.executable, "-I", str(TOOLS / "shader_disasm.py"),
                        str(fssm), "--out", str(disasm)], capture_output=True, text=True)
    sys.stderr.write(r.stdout[-2000:] + r.stderr[-2000:])
    print("disassembled", flush=True)

    # Translate all; capture the failure ranking the translator prints.
    r = subprocess.run([sys.executable, "-I", str(TOOLS / "rdna2spirv.py"),
                        str(disasm), "--out", str(spirv), "--fssm-dir", str(fssm),
                        "--wave64-emulate"], capture_output=True, text=True)
    out = r.stdout + r.stderr
    (work / "translate.log").write_text(out)
    # Print the coverage summary + failure ranking lines.
    for line in out.splitlines():
        if any(k in line for k in ("translated", "pass spirv-val", "shaders:", "tier", "wave64")) or line.startswith("  "):
            print(line)
    print(f"full log: {work / 'translate.log'}")


if __name__ == "__main__":
    main()