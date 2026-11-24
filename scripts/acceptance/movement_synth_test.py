#!/usr/bin/env python3
"""Synthetic exercise of movement_gate.py (no game data; generated images).
Proves the numeric delta rejects camera drift / loading animation / idle sway
and accepts whole-view translation. Tracked; runs with CSM_ACCEPTANCE_CONFIG
UNSET (structural central-50% world region, structural mesh frame).
Run: python3 movement_synth_test.py
"""
import struct
import subprocess
import sys
import tempfile
import zlib
from pathlib import Path

import numpy as np

import csm_config

HERE = Path(__file__).parent
W, H = 1920, 1080
MESH = csm_config.SYNTHETIC["mesh_frame"]
WALL = csm_config.SYNTHETIC["synth_wall"]
FRAMES = list(range(MESH + 1, MESH + 16))   # 15 frames, mesh-relative window


def png(path, arr):
    raw = bytearray()
    for y in range(H):
        raw.append(0)
        raw.extend(arr[y].astype(np.uint8).tobytes())
    def chunk(k, b):
        return struct.pack(">I", len(b)) + k + b + struct.pack(">I", zlib.crc32(k + b) & 0xffffffff)
    out = b"\x89PNG\r\n\x1a\n"
    out += chunk(b"IHDR", struct.pack(">IIBB", W, H, 8, 2) + b"\x00\x00\x00")
    out += chunk(b"IDAT", zlib.compress(bytes(raw), 3))
    out += chunk(b"IEND", b"")
    Path(path).write_bytes(out)


def log(path, hold):
    p = "2026-11-24T20:00:00.000000Z  INFO "
    L = [f"{p}victory_gpu::host: 1.0 fps, {WALL} ms/frame (present 13.7 ms, flip 2.1 ms), "
         f"budget-skipped 0; pool q=10 done=10 fail=0 fallback=0 maxdepth=3 | inline-waits=0 wait-ms=0",
         f'{p}victory_gpu::pipeline: pipeline=0x1 frame={MESH} stage="mesh" size=2048 hash=abc first bind']
    if hold:
        L.append(f"{p}victory_hle::modules::gameinput: key_hold=10s since_mesh=2s "
                 f"event-relative input hold fired (after first mesh bind)")
    Path(path).write_text("\n".join(L) + "\n")


def make(kind, folder, rng):
    base = rng.integers(0, 256, (H, W, 3), dtype=np.int32)
    Path(folder).mkdir(parents=True, exist_ok=True)
    for i, n in enumerate(FRAMES):
        if kind == "translate":
            a = np.roll(base, i * 25, axis=1)          # whole-view translation
        elif kind == "drift":
            a = base.copy()
            m = rng.random((H, W)) < 0.10              # 10% of pixels, +-5 (sub-threshold)
            a[m] = np.clip(a[m] + rng.integers(-5, 6, (m.sum(), 3)), 0, 255)
        elif kind == "loading":
            a = base                                    # static loading image
        png(Path(folder) / f"frame-{n}.png", a)


def run(idle_kind, in_kind, want, hold=True):
    with tempfile.TemporaryDirectory() as t:
        t = Path(t)
        rng = np.random.default_rng(7)
        make(idle_kind, t / "idle", rng)
        make(in_kind, t / "input", np.random.default_rng(7))  # same base scene
        log(t / "idle.log", False)
        log(t / "input.log", hold)
        r = subprocess.run([sys.executable, str(HERE / "movement_gate.py"),
                            str(t / "idle"), str(t / "idle.log"),
                            str(t / "input"), str(t / "input.log")],
                           capture_output=True, text=True)
        got = "PASS" if "MOVEMENT: PASS" in r.stdout else "FAIL"
        ok = got == want
        print(f"{'ok  ' if ok else 'FAIL'} idle={idle_kind:9s} input={in_kind:9s} hold={hold} -> {got} (want {want})")
        if not ok:
            print(r.stdout)
        return ok


def main():
    cases = [
        ("drift", "translate", "PASS", True),    # real movement beats drift
        ("drift", "drift", "FAIL", True),        # drift cannot satisfy movement
        ("loading", "loading", "FAIL", True),    # static loading cannot satisfy
        ("drift", "loading", "FAIL", True),      # loading vs drift fails
        ("loading", "translate", "PASS", True),  # movement beats static loading
        ("drift", "translate", "FAIL", False),   # hold never fired -> FAIL
    ]
    ok = all(run(*c) for c in cases)
    print("----")
    print("MOVEMENT SYNTH: ALL BEHAVE" if ok else "MOVEMENT SYNTH: FAILURES")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())