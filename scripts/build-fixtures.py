#!/usr/bin/env python3
"""Build the vendored synthetic cross-half fixtures into SPIR-V for the native
dispatch tests, with wave64 emulation enabled (so they run on a 32-lane host).

    python3 scripts/build-fixtures.py <out-dir>

Uses the vendored translator (third_party/rdna2spirv); no external dependency.
The output dir holds spirv/cs_w64x_*.spv; point VILKAN_VK_FIXTURES at it to run
`cargo test -p vilkan-vk --test dispatch`. The blur fixtures are game-derived
and come from the user's own captured shaders (compile-native.py), not here.
"""
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BUILD = ROOT / "third_party" / "rdna2spirv" / "runtime" / "test" / "shaders" / "build.py"


def main():
    if len(sys.argv) != 2:
        sys.exit("usage: build-fixtures.py <out-dir>")
    out = Path(sys.argv[1]).resolve()
    out.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, VILKAN_W64_EMULATE="1")
    r = subprocess.run([sys.executable, "-I", str(BUILD), str(out),
                        "cs_w64x_readlane", "cs_w64x_writelane"], env=env)
    sys.exit(r.returncode)


if __name__ == "__main__":
    main()