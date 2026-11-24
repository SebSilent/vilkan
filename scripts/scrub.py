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
LITERAL = [
    ("Grand Theft Auto VI", "Cuck Simulator Miami"),
    ("Grand Theft Auto", "Cuck Simulator Miami"),
    ("GTA VI", "CSM"),
    ("GTA6", "CSM"),
    ("Minecraft (March 2022)", "an older title"),
    ("Minecraft", "an older title"),
    ("Fortnite", "another title"),
    ("Unicorn Overlord", "another title"),
    ("XDefiant", "another title"),
    ("Rockstar Games", "the publisher"),
    ("Rockstar", "the publisher"),
    ("Take-Two Interactive", "the publisher"),
    ("Take-Two", "the publisher"),
    ("CYBERLEEK", "a scene release"),
    ("Vice City", "the setting"),
    ("game_scarlett_master.exe", "the title executable"),
    ("game_scarlett_master", "the title executable"),
]
# Regex rules: title-specific code offsets and release/signature dates.
REGEX = [
    # Line-wrapped title name across a comment prefix ("Grand Theft\n/// Auto VI").
    (re.compile(r"Grand Theft[\s/]+Auto(?:[\s/]+VI)?"), "Cuck Simulator Miami"),
    (re.compile(r"title\+0x[0-9a-fA-F]+"), "a title offset"),
    (re.compile(r"\(title\+0x[0-9a-fA-F]+[^)]*\)"), "(a title offset)"),
    (re.compile(r"\(20\d\d-\d\d-\d\d\)"), ""),
    (re.compile(r"\((?:April|March|May|June|July|August|September|October|November|December|January|February) 20\d\d\)"), ""),
    (re.compile(r"\b1\.0\.2\.1\b"), "the supported build"),
]
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