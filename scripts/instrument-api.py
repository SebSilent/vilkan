#!/usr/bin/env python3
"""Re-appliable measurement overlay for the console D3D12/DXGI dispatch.

The upstream dispatch files stay PRISTINE in git (zero merge cost). This script
inserts a per-method call tally on demand and removes it again, so a sync never
conflicts on instrumentation. Every inserted line carries the marker
`// apicount-overlay` (for `--check`); `--revert` restores the tracked pristine
files with `git checkout`, byte-identical.

    python3 scripts/instrument-api.py            # apply the overlay
    python3 scripts/instrument-api.py --revert   # restore pristine upstream
    python3 scripts/instrument-api.py --check    # report applied/pristine

The tallies and the `mod apicount;` declaration are gated on
`#[cfg(all(windows, feature = "diagnostics"))]`, so a plain build compiles them
out entirely (the default Proton path is byte-identical). To measure, build with
the diagnostics feature enabled for victory-gpu and set VILKAN_API_COUNT:

    FEATURES=diagnostics scripts/build.sh
    VILKAN_API_COUNT='Z:\\path\\counts.txt' scripts/run.sh

Counts dump to that path at present. The module itself (apicount.rs) is our own
file, tracked; only the insertions into upstream files are transient.
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent / "crates" / "victory-gpu" / "src"
FILES = ["device", "list", "queue", "resource", "descriptor", "pipeline",
         "dxgi", "fence", "signature", "query", "root"]
# IUnknown / shared helpers: not part of the per-interface API surface.
SKIP = {"query", "release", "add_ref", "query_interface", "get_private_data",
        "set_private_data", "parent_device", "ignore", "missing"}
MARK = "// apicount-overlay"
CFG = '#[cfg(all(windows, feature = "diagnostics"))]'

FN = re.compile(r'(?P<indent>[ \t]*)(?:pub(?:\([^)]*\))?\s+)?extern\s+"system"\s+fn\s+(?P<name>[a-z_0-9]+)')


def body_open(text, start):
    """Index just after the body-opening '{' of the fn whose name ends at start."""
    i = text.find('(', start)
    if i < 0:
        return None
    depth = 0
    while i < len(text):
        c = text[i]
        if c == '(':
            depth += 1
        elif c == ')':
            depth -= 1
            if depth == 0:
                break
        i += 1
    while i < len(text) and text[i] != '{':
        i += 1
    return i + 1 if i < len(text) else None


def apply_file(path):
    text = path.read_text()
    stem = path.stem
    edits = []
    for m in FN.finditer(text):
        name = m.group("name")
        if name in SKIP:
            continue
        indented = bool(m.group("indent"))
        label = f"{stem}::state::{name}" if indented else f"{stem}::{name}"
        bo = body_open(text, m.end())
        if bo is None:
            continue
        if MARK in text[bo:bo + 160]:  # idempotent
            continue
        base = m.group("indent")
        ins = f'\n{base}    {CFG} crate::apicount::tally("{label}"); {MARK}'
        # Empty body `{}`: the closing brace must stay on its own line — left
        # in place it would land after the marker comment and be swallowed.
        end = bo
        while end < len(text) and text[end] in " \t":
            end += 1
        if end < len(text) and text[end] == '}':
            ins += f'\n{base}}}'
            edits.append((bo, end + 1, ins))
        else:
            edits.append((bo, bo, ins))
    # The present handler also dumps the accumulated counts.
    if stem == "queue":
        m = re.search(r'extern\s+"system"\s+fn\s+present\b', text)
        if m:
            bo = body_open(text, m.end())
            if bo is not None and "apicount::dump" not in text[bo:bo + 200]:
                edits.append((bo, bo, f'\n    {CFG} crate::apicount::dump(); {MARK}'))
    for start, end, ins in sorted(edits, reverse=True):
        text = text[:start] + ins + text[end:]
    path.write_text(text)
    return len(edits)


def apply_lib():
    p = ROOT / "lib.rs"
    text = p.read_text()
    if "mod apicount;" in text:
        return 0
    # Insert before `mod capture;`, with its OWN cfg (do not steal capture's).
    anchor = "mod capture;"
    i = text.find(anchor)
    if i < 0:
        return 0
    line_start = text.rfind("\n", 0, i) + 1
    ins = f'{CFG} mod apicount; {MARK}\n'
    text = text[:line_start] + ins + text[line_start:]
    p.write_text(text)
    return 1


def revert():
    # Restore the tracked pristine version exactly (the overlay is transient and
    # never committed, so HEAD is always pristine). This avoids line surgery on
    # empty `{}` bodies and guarantees a byte-identical upstream tree.
    import subprocess
    repo = ROOT.parent.parent.parent
    files = ["crates/victory-gpu/src/lib.rs"] + [
        f"crates/victory-gpu/src/{f}.rs" for f in FILES]
    subprocess.run(["git", "checkout", "--"] + files, cwd=repo, check=True)
    return len(files)


def check():
    applied = 0
    for p in [ROOT / "lib.rs"] + [ROOT / f"{f}.rs" for f in FILES]:
        applied += p.read_text().count(MARK)
    return applied


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "apply"
    if mode == "--revert":
        print(f"reverted {revert()} overlay lines; upstream pristine")
    elif mode == "--check":
        n = check()
        print(f"{n} overlay lines applied" if n else "pristine (no overlay)")
    else:
        total = apply_lib()
        for f in FILES:
            total += apply_file(ROOT / f"{f}.rs")
        print(f"applied {total} overlay lines (cfg-gated, default-off)")
        print("measure: FEATURES=diagnostics scripts/build.sh")


if __name__ == "__main__":
    main()