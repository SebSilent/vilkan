#!/usr/bin/env python3
"""Parallel, resumable full-coverage translation of the carved VS/PS set.

Reuses an existing disasm/fssm/dxbc work dir (from coverage.py), runs the vendored
translator in N shards across cores with the in-process Python assembler (one
spirv-val subprocess per shader, not two), resumes completed shards, merges the
indexes, and reports the three metrics separately: translated, spirv-val passed,
and wave64 tier A/B (exact on wave32) vs C. Ranks actual translate/val failures.

    python3 scripts/coverage-parallel.py <work dir> [--jobs N]

Nothing game-derived is tracked; all output stays in the work dir.
"""
import argparse
import glob
import json
import os
import subprocess
import sys
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
TOOLS = HERE.parent / "third_party" / "rdna2spirv" / "tools"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("work")
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 4)
    a = ap.parse_args()
    work = Path(a.work)
    disasm = work / "disasm"
    fssm = work / "fssm"
    n = a.jobs

    # Launch shards (resume: skip a shard whose index part already exists).
    procs = []
    for i in range(n):
        out = work / f"spirv-shard{i}"
        out.mkdir(exist_ok=True)
        part = out / f"index.part{i}.json"
        if part.exists():
            print(f"shard {i}: already complete (resume)", flush=True)
            continue
        log = open(work / f"shard{i}.log", "w")
        cmd = [sys.executable, "-I", str(TOOLS / "rdna2spirv.py"), str(disasm),
               "--out", str(out), "--fssm-dir", str(fssm), "--shard", f"{i}/{n}",
               "--assembler", "python", "--wave64-emulate"]
        procs.append((i, subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT), log))
    for i, p, log in procs:
        p.wait()
        log.close()
        print(f"shard {i}: exit {p.returncode}", flush=True)

    # Merge indexes + count the three metrics.
    translated = valid = 0
    tiers = Counter()
    tier_c_reasons = Counter()
    tier_c_example = {}
    for part in sorted(glob.glob(str(work / "spirv-shard*" / "index.part*.json"))):
        d = json.load(open(part))
        for name, v in d.items():
            translated += 1  # in the index => translated AND spirv-val passed
            valid += 1
            t = v.get("wave64_graphics")
            if t:
                tiers[t] += 1
                if t == "C":
                    for r in v.get("wave64_graphics_reasons", []) or []:
                        tier_c_reasons[r] += 1
                        tier_c_example.setdefault(r, name)
    # .spvasm count includes shaders that translated but failed spirv-val.
    spvasm = sum(len(glob.glob(str(work / f"spirv-shard{i}" / "*.spvasm"))) for i in range(n))
    spv = sum(len(glob.glob(str(work / f"spirv-shard{i}" / "*.spv"))) for i in range(n))

    # Rank actual translate failures (Unsupported) + val failures from shard logs.
    why = Counter()
    for i in range(n):
        log = work / f"shard{i}.log"
        if not log.exists():
            continue
        for line in log.read_text().splitlines():
            # the translator prints "  <count>  <reason>" ranking lines
            s = line.strip()
            if s and s[0].isdigit() and "  " in s:
                parts = s.split(None, 1)
                if len(parts) == 2 and parts[0].isdigit():
                    why[parts[1]] += int(parts[0])

    total = len(glob.glob(str(disasm / "*.s")))
    print(f"\n=== COVERAGE ({total} VS/PS) ===")
    print(f"translated (spvasm):      {spvasm}")
    print(f"spirv-val passed (spv):   {spv}")
    print(f"index entries (val ok):   {valid}")
    print(f"wave64 tiers: {dict(tiers)}")
    exact = tiers.get("A", 0) + tiers.get("B", 0)
    print(f"exact on wave32 (A+B):    {exact}   tier C (not exact): {tiers.get('C', 0)}")
    print(f"\n=== translate/val failure ranking ===")
    for r, c in why.most_common(20):
        print(f"  {c:5d}  {r}")
    print(f"\n=== tier-C correctness reasons (not translate failures) ===")
    for r, c in tier_c_reasons.most_common(10):
        print(f"  {c:5d}  {r}  e.g. {tier_c_example[r]}")


if __name__ == "__main__":
    main()