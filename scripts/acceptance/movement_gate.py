#!/usr/bin/env python3
"""Movement gate — paired idle-input baseline vs event-relative input run.

Tracked, clean-room; no game bytes. Decodes the presented-frame PNGs
(frame-<n>.png) and measures the WORLD region (HUD-excluded centre). The region
rectangle is title-derived HUD geometry: for real runs it comes from the
external config (world_region, env CSM_ACCEPTANCE_CONFIG); with the env unset
the synthetic suite uses the structural central 50% x 50% fraction.

Both runs reach the world via the same Enter menu script. The INPUT run adds
`mesh+2:W:10` (MeshAfter): W drives the left stick forward, firing 2 s after the
FIRST mesh bind and holding 10 s. The IDLE run has no movement hold. The
hold-fired line proves it fired after mesh; the mesh first-bind frame anchors
the window.

WINDOW (mesh-relative, fps-aware): the hold spans mesh+2..mesh+12 s. At the
run's measured seconds/frame that is ceil(12/spf)+2 frames after mesh. We select
captured frames in (mesh_frame, mesh_frame+W]. Each run's OWN mesh frame + fps
makes the comparison fair despite mesh frame and fps varying between runs.

METRIC (per run, over the window):
  cum_change_frac = fraction of world pixels with |last-first| > 16/255 (any
                    channel) — how much of the view changed across the hold.
  cum_mad         = mean |last-first| over world pixels (0..255).
  consec_mad      = mean over consecutive captured pairs of mean |delta|.

MOVEMENT PASS (all required) — defined a priori from geometry, NOT fitted to
game data (translation moves the whole view; drift/loading do not):
  1. input hold-fired line present (the hold actually fired after mesh).
  2. input cum_change_frac >= 0.50  (>=50% of the world view changed).
  3. input cum_change_frac - idle cum_change_frac >= 0.30 (30 pp margin).
  4. input cum_mad >= 20.0 AND input cum_mad >= 2.0*max(idle cum_mad,1.0).
Camera drift / idle sway perturbs a small pixel fraction with low MAD; a loading
image is static -> ~0 change. Character translation re-renders the whole view ->
large fraction + high MAD. The 30 pp margin and 2x MAD make a drift/loading/idle
false positive impossible by construction. Thresholds are pre-registered;
change only with owner approval after observing real numbers (never fitted).

usage: movement_gate.py IDLE_DIR IDLE_LOG INPUT_DIR INPUT_LOG
prints numbers + `MOVEMENT: PASS|FAIL <reason>`; exit 0 pass / 1 fail.
"""
import math
import re
import sys
from pathlib import Path

import numpy as np

import csm_config
from png_decode import decode

HOLD_END_S = 12.0          # mesh+2 fire + 10 s hold
PIX_THRESH = 16            # /255 any-channel change to count a pixel
MIN_FRAC = 0.50
MARGIN_FRAC = 0.30
MIN_MAD = 20.0
MAD_RATIO = 2.0

_CFG = csm_config.load()
_REGION = _CFG.get("world_region") if _CFG else None   # [x0,x1,y0,y1] or None


def region(w, h):
    if _REGION:
        return tuple(_REGION)
    return (w // 4, 3 * w // 4, h // 4, 3 * h // 4)   # structural central 50%


def mesh_frame(log):
    txt = Path(log).read_text(errors="replace")
    for line in txt.splitlines():
        if 'stage="mesh"' in line and "first bind" in line:
            m = re.search(r"frame=(\d+)", line)
            if m:
                return int(m.group(1))
    return None


def hold_fired(log):
    return "event-relative input hold fired" in Path(log).read_text(errors="replace")


def sec_per_frame(log):
    txt = Path(log).read_text(errors="replace")
    vals = [float(m) for m in re.findall(r"[0-9.]+ fps, ([0-9.]+) ms/frame", txt)]
    if not vals:
        return 1.0
    return max(vals) / 1000.0   # worst (slowest) frame -> widest window


def world_arr(folder, n):
    p = Path(folder) / f"frame-{n}.png"
    if not p.exists():
        return None
    w, h, rows = decode(p)
    a = np.frombuffer(b"".join(rows), dtype=np.uint8).reshape(h, w, 3).astype(np.int32)
    x0, x1, y0, y1 = region(w, h)
    return a[y0:y1, x0:x1, :]


def measure(folder, log):
    mf = mesh_frame(log)
    if mf is None:
        return None, "no mesh first-bind frame in log"
    spf = sec_per_frame(log)
    W = math.ceil(HOLD_END_S / spf) + 2
    avail = sorted(int(p.stem.split("-")[1]) for p in Path(folder).glob("frame-*.png")
                   if re.fullmatch(r"frame-\d+", p.stem))
    win = [n for n in avail if mf < n <= mf + W]
    if len(win) < 2:
        return None, f"window (mesh {mf}, +{W}) has <2 captured frames (avail {len(avail)})"
    first, last = world_arr(folder, win[0]), world_arr(folder, win[-1])
    d = np.abs(last - first)
    changed = (d.max(axis=2) > PIX_THRESH)
    frac = float(changed.mean())
    mad = float(d.mean())
    consec = []
    prev = first
    for n in win[1:]:
        cur = world_arr(folder, n)
        consec.append(float(np.abs(cur - prev).mean()))
        prev = cur
    return {"mesh": mf, "spf": round(spf, 3), "W": W, "win": (win[0], win[-1]),
            "n": len(win), "frac": round(frac, 4), "mad": round(mad, 3),
            "consec_mad": round(sum(consec) / len(consec), 3)}, None


def main():
    if len(sys.argv) != 5:
        print("usage: movement_gate.py IDLE_DIR IDLE_LOG INPUT_DIR INPUT_LOG")
        return 2
    idle_dir, idle_log, in_dir, in_log = sys.argv[1:5]
    idle, e1 = measure(idle_dir, idle_log)
    inp, e2 = measure(in_dir, in_log)
    print(f"idle : {idle if idle else e1}")
    print(f"input: {inp if inp else e2}")
    if e1 or e2:
        print(f"MOVEMENT: FAIL measurement ({e1 or ''} {e2 or ''})")
        return 1
    fired = hold_fired(in_log)
    dfrac = inp["frac"] - idle["frac"]
    mad_ok = inp["mad"] >= MIN_MAD and inp["mad"] >= MAD_RATIO * max(idle["mad"], 1.0)
    checks = [
        ("hold-fired-after-mesh", fired),
        (f"input frac>={MIN_FRAC}", inp["frac"] >= MIN_FRAC),
        (f"frac margin>={MARGIN_FRAC}", dfrac >= MARGIN_FRAC),
        (f"input mad>={MIN_MAD} & >={MAD_RATIO}x idle", mad_ok),
    ]
    for name, ok in checks:
        print(f"  {'ok ' if ok else 'FAIL'} {name}")
    print(f"  delta frac={dfrac:.4f} (input {inp['frac']} - idle {idle['frac']}); "
          f"mad input={inp['mad']} idle={idle['mad']}")
    if all(ok for _, ok in checks):
        print("MOVEMENT: PASS")
        return 0
    print("MOVEMENT: FAIL " + ",".join(n for n, ok in checks if not ok))
    return 1


if __name__ == "__main__":
    sys.exit(main())