#!/usr/bin/env python3
"""Repeatable game-name scrub for the publishable tree.

Strips the title, publisher, release/scene tags and title-identifying strings
from tracked source/docs, replacing them with the generic alias "Cuck Simulator
Miami" (CSM). Idempotent: re-running after an upstream sync changes nothing that
is already scrubbed. Project names (VIctory, the upstream) are NOT touched —
they are credited in NOTICE, not scrubbed.

Usage:
    python3 scripts/scrub.py [--check] [paths...]

With no paths, scrubs every tracked text file. --check reports what would change
without writing (exit 1 if anything would). Nothing game-derived is ever written
into the tree; this only rewrites prose/comments.
"""
import argparse
import re
import subprocess
import sys
from pathlib import Path

# Explicit, ordered mapping. Longer phrases first so they win over substrings.
# Each replacement must not re-match a later rule (idempotence).
# The identifying strings live in an untracked local mapping (gitignored), so
# nothing identifying is tracked. A clean clone without it scrubs nothing.
import importlib.util as _ilu
_map_path = Path(__file__).with_name("scrub-map.local.py")
LITERAL, REGEX = [], []
if _map_path.exists():
    _spec = _ilu.spec_from_file_location("scrub_map", _map_path)
    _m = _ilu.module_from_spec(_spec)
    _spec.loader.exec_module(_m)
    LITERAL, REGEX = _m.LITERAL, _m.REGEX
# Regex rules: title-specific code offsets and release/signature dates.
TEXT_EXT = {".rs", ".toml", ".md", ".sh", ".py", ".bat", ".txt"}


def scrub_text(s: str) -> str:
    for a, b in LITERAL:
        s = s.replace(a, b)
    for pat, repl in REGEX:
        s = pat.sub(repl, s)
    return s


def tracked_files():
    out = subprocess.check_output(["git", "ls-files"]).decode().splitlines()
    return [f for f in out if Path(f).suffix in TEXT_EXT and Path(f).is_file()]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    ap.add_argument("paths", nargs="*")
    a = ap.parse_args()
    files = a.paths or tracked_files()
    changed = []
    for f in files:
        p = Path(f)
        if not p.is_file() or p.suffix not in TEXT_EXT:
            continue
        try:
            s = p.read_text()
        except (UnicodeDecodeError, OSError):
            continue
        n = scrub_text(s)
        if n != s:
            changed.append(f)
            if not a.check:
                p.write_text(n)
    if a.check:
        for f in changed:
            print("would scrub:", f)
        sys.exit(1 if changed else 0)
    print(f"scrubbed {len(changed)} file(s)")


if __name__ == "__main__":
    main()