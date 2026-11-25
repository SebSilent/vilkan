#!/usr/bin/env python3
"""Generate a synthetic run log in the EXACT rendered emitter forms (no game
data; pure text). Used by rehearsal.sh to exercise gate_eval.py. Tracked.

Defaults are the structural SYNTHETIC profile (csm_config) — arbitrary round
numbers and a generic crash marker, NOT measured title baselines. The synthetic
suites run with CSM_ACCEPTANCE_CONFIG unset.

usage: synth_log.py OUT KEY=VAL ...
keys: refused sigdrops declined exec drawn dispatched budget q done fail fb
      wall crash mesh maxframe hold pool(0/1) suspend(0/1) close(0/1) scignore(N)
"""
import sys
from pathlib import Path

import csm_config

S = csm_config.SYNTHETIC
PFX = "2026-11-24T20:00:00.000000Z  INFO "
WFX = "2026-11-24T20:00:00.000000Z  WARN "
D = dict(refused=S["refused_expect"], sigdrops=S["sigdrops_expect"],
         declined=S["declined_expect"], exec=S["indirect_min"], drawn=S["drawn_min"],
         dispatched=S["dispatch_min"], budget=0, q=10, done=10, fail=0, fb=0,
         wall=S["synth_wall"], crash=0, mesh=1, maxframe=S["synth_maxframe"], hold=0, pool=1,
         suspend=0, close=0, scignore=0)
out = sys.argv[1]
for kv in sys.argv[2:]:
    k, v = kv.split("=", 1)
    D[k] = float(v) if k == "wall" else int(v)
L = []
L.append("backend        DX12")
L.append("mitigations    [DX12] pool=on close-abort=on shadow-seek=on dispatch-single=on arg-clamp=on")
if D["pool"]:
    L.append(f"{PFX}victory_gpu::host: 1.0 fps, {D['wall']} ms/frame (10.0 preparing; "
             f"800.0 on the GPU; present 13.7 ms, flip 2.1 ms), 100 draws/frame, "
             f"dropped 0 without a host pipeline and 0 indirect, budget-skipped {D['budget']}; "
             f"pool q={D['q']} done={D['done']} fail={D['fail']} fallback={D['fb']} maxdepth=3 "
             f"| inline-waits=2 wait-ms=5")
L.append(f"{PFX}victory_gpu::queue: frame=200 draws=1000 drawn={D['drawn']} dispatches=100 "
         f"dispatched={D['dispatched']} frame presented")
L.append(f"{PFX}victory_gpu::list: executed={D['exec']} indirect execution ran")
for _ in range(D["sigdrops"]):
    L.append(f"{PFX}victory_gpu::list: indirect execution dropped (first time for this signature)")
for _ in range(D["refused"]):
    L.append(f"{WFX}victory_gpu::pipeline::host: error=refused shaders=[] host pipeline refused")
for _ in range(D["declined"]):
    L.append(f"{WFX}victory_shader::emit::lower: reason=wave64 wave64 exchange declined")
if D["mesh"]:
    L.append(f'{PFX}victory_gpu::pipeline: pipeline=0x1 frame={S["mesh_frame"]} stage="mesh" '
             f'size=2048 hash=abc first bind')
L.append(f"{PFX}victory_gpu::queue: frame={D['maxframe']} draws=1 drawn=1 dispatches=0 dispatched=0 frame presented")
if D["hold"]:
    L.append(f"{PFX}victory_hle::modules::gameinput: key_hold=10s since_mesh=2s "
             f"event-relative input hold fired (after first mesh bind)")
for _ in range(D["crash"]):
    L.append("VIctory: unhandled exception 0xc0000005 at "
             f"synthetic-image.dll+0x{S['crash_marker']} (write 0x0)")
if D["suspend"]:
    # The loader's generic suspend-deadline message (our own emitter; not game
    # data). Marks a run ended by an early window-close before the world.
    L.append(f"{PFX}victory_hle::modules::lifecycle: window closed: suspending the title routines=2")
    L.append(f"{PFX}victory_hle::modules::lifecycle: title did not finish suspending in time; ending the title")
if D["close"]:
    # A graceful window-close end (end_title "window closed", suspended in time):
    # close-ended WITHOUT the suspend-timeout marker. Our own emitter; not data.
    L.append(f"{PFX}victory_hle::modules::lifecycle: window closed: suspending the title routines=2")
    L.append(f"{PFX}victory_hle::modules::lifecycle: title suspended; ending the title")
for _ in range(D["scignore"]):
    # SC_CLOSE seen but ignored (VICTORY_CLOSE_ENDS off): run continued. Diagnostic.
    L.append(f"{WFX}victory_hle::modules::lifecycle: wparam=0xf060 lparam=0x0 thread=1 "
             f"elapsed_ms=5000 count=1 preceding_crashes=0 preceding_fatal=none "
             f"SC_CLOSE ignored (VICTORY_CLOSE_ENDS off); run continues, timeout authoritative")
Path(out).write_text("\n".join(L) + "\n")
print(f"wrote {out}")