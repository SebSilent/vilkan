#!/usr/bin/env python3
"""Current-tree acceptance evaluator — machine PASS/FAIL per blocking gate.

Tracked, clean-room. Parses rendered run logs (the exact emitter forms) and the
movement verdict, issuing a per-gate + overall CURRENT-TREE verdict. The PARKED
wave-size gate is reported but NEVER contributes to the blocking verdict.

All title-derived constants (crash offset, measured baselines, expected refusal
/ signature-drop counts, work minimums, mesh-frame floor) come from the external
config via csm_config (env CSM_ACCEPTANCE_CONFIG). With the env unset the
structural SYNTHETIC profile is used, so the rehearsal runs without game data.

A gate that cannot fail is not a gate: every blocking gate has a concrete
numeric predicate exercised by rehearsal.sh (one synthetic FAIL case each).

Run-level validity precedes every gate: a run that ended via a window close
(end_title "window closed"), the loader suspend-timeout, or never reached world
entry (maxframe <= floor), is INVALID, not FAIL. If either run is invalid, NO
PASS/FAIL verdict is issued — the invalid runs are reported with their reason
(`RUN-VALIDITY ...`) and the evaluator exits 2 (NO VERDICT). An ignored SC_CLOSE
(VICTORY_CLOSE_ENDS off) does NOT invalidate (the run continued); it is reported
as a diagnostic. A crash firing AT world entry stays a valid FAIL data point.

usage: gate_eval.py R1_LOG R2_LOG MOVEMENT_VERDICT
  MOVEMENT_VERDICT = PASS | FAIL | NA  (from movement_gate.py)
prints `RUN-VALIDITY ...`, `GATE <id>: PASS|FAIL ...`, `PARKED ...`,
`CURRENT-TREE: PASS|FAIL` (or `CURRENT-TREE: NO VERDICT ...`).
exit 0 iff CURRENT-TREE PASS; 1 iff FAIL; 2 iff NO VERDICT (invalid run).
"""
import re
import sys
from pathlib import Path

import csm_config

T = csm_config.thresholds(csm_config.load())
CRASH_MARKER = T["crash_marker"]
WALL_CAP = T["wall_cap"]
REFUSED_EXPECT = T["refused_expect"]
SIGDROPS_EXPECT = T["sigdrops_expect"]
INDIRECT_MIN, DRAWN_MIN, DISPATCH_MIN = T["indirect_min"], T["drawn_min"], T["dispatch_min"]
MAXFRAME_MIN = T["maxframe_min"]
PARKED_MAX = T["parked_refused_max"]


def txt(p):
    return Path(p).read_text(errors="replace")


def maxint(t, pat):
    v = [int(x) for x in re.findall(pat, t)]
    return max(v) if v else 0


def metrics(log):
    t = txt(log)
    pool = re.findall(
        r"pool q=(\d+) done=(\d+) fail=(\d+) fallback=(\d+) maxdepth=\d+ "
        r"\| inline-waits=\d+ wait-ms=\d+", t)
    q = dn = fb = fail = 0
    pool_present = bool(pool)
    if pool:
        q, dn, fail, fb = (int(x) for x in pool[-1])
    wall = [float(x) for x in re.findall(r"[0-9.]+ fps, ([0-9.]+) ms/frame", t)]
    return {
        "refused": t.count("host pipeline refused"),
        "declined": t.count("wave64 exchange declined"),
        "sigdrops": t.count("first time for this signature"),
        "exec": maxint(t, r"executed=(\d+)"),
        "drawn": maxint(t, r"drawn=(\d+)"),
        "dispatched": maxint(t, r"dispatched=(\d+)"),
        "budget": maxint(t, r"budget-skipped (\d+)"),
        "pool_present": pool_present, "q": q, "done": dn, "fail": fail, "fb": fb,
        "wall": max(wall) if wall else 0.0,
        "crash": t.count(CRASH_MARKER),
        "mesh": len(re.findall(r'stage="mesh"', t)),
        "maxframe": maxint(t, r"frame=(\d+)"),
        # The loader's suspend deadline fired ("title did not finish suspending
        # in time"): the window was closed and the title terminated before it
        # could run to the world. Known-good runs never carry this marker.
        "suspend_timeout": t.count("did not finish suspending in time"),
        # end_title("window closed", ...) fired: the run was ended by a window
        # close (SC_CLOSE with VICTORY_CLOSE_ENDS on), gracefully or not. A
        # close-ended run is not a mitigation measurement either.
        "close_ended": t.count("window closed: suspending the title"),
        # SC_CLOSE seen but ignored (VICTORY_CLOSE_ENDS off): the run continued,
        # so this does NOT invalidate; reported as a diagnostic only.
        "sc_close_ignored": t.count("SC_CLOSE ignored"),
    }


def validity(r):
    """A run is a valid mitigation measurement only if it ran to the world.

    INVALID (no verdict may be issued from it) when:
      * it ended via a window close (end_title "window closed"), or
      * it ended via the loader suspend-timeout (window closed early), or
      * it never reached world entry (maxframe at/below the measured floor).
    An ignored SC_CLOSE (VICTORY_CLOSE_ENDS off) does NOT invalidate: the run
    continued. Returns the list of reasons (empty == valid). A crash that fires
    AT world entry is still a valid data point: the crash marker is judged by G7,
    not here, so a genuine 1a crash is FAIL, never silently INVALID.
    """
    reasons = []
    if r["close_ended"] > 0:
        reasons.append("ended via window close (end_title 'window closed'; not a mitigation run)")
    if r["suspend_timeout"] > 0:
        reasons.append("ended via suspend-timeout (window closed before the world)")
    if r["maxframe"] <= MAXFRAME_MIN:
        reasons.append(f"never reached world entry (maxframe={r['maxframe']} <= floor={MAXFRAME_MIN})")
    return reasons


def main():
    if len(sys.argv) != 4:
        print("usage: gate_eval.py R1_LOG R2_LOG MOVEMENT_VERDICT")
        return 2
    r1, r2, mov = metrics(sys.argv[1]), metrics(sys.argv[2]), sys.argv[3].upper()
    # Run-level validity FIRST: a run that ended via a window close / suspend-
    # timeout or never reached world entry is INVALID, not FAIL. No PASS/FAIL
    # verdict may be issued from invalid runs; report them separately with the
    # reason and exit 2 (NO VERDICT). This stops an early-ended run from masking
    # a regression (0/2 crashes on a run that never exposed the crash window is
    # not a fix).
    v1, v2 = validity(r1), validity(r2)
    for name, r, reasons in (("R1", r1, v1), ("R2", r2, v2)):
        if reasons:
            print(f"RUN-VALIDITY {name}: INVALID  ({'; '.join(reasons)})")
        else:
            print(f"RUN-VALIDITY {name}: valid")
        # Ignored SC_CLOSE (VICTORY_CLOSE_ENDS off) is a diagnostic, not a
        # validity failure: the run continued past the external close.
        if r["sc_close_ignored"] > 0:
            print(f"  note {name}: {r['sc_close_ignored']} external SC_CLOSE ignored "
                  "(VICTORY_CLOSE_ENDS off); run continued")
    invalid = [n for n, v in (("R1", v1), ("R2", v2)) if v]
    if invalid:
        print("CURRENT-TREE: NO VERDICT (invalid run(s): " + ", ".join(invalid)
              + "; a close-ended / suspend-timeout / pre-world-entry run is not a "
              "mitigation measurement — re-run clean to the world)")
        return 2
    # Presence counters (work/refusals/sig-drops must appear in EVERY run): MIN
    # across runs, so one run doing less work cannot hide behind a busier run.
    # Ceiling counters (budget skips, wall-time must not exceed in ANY run): MAX.
    mn = {k: min(r1[k], r2[k]) for k in ("refused", "exec", "drawn", "dispatched")}
    mx = {k: max(r1[k], r2[k]) for k in ("budget", "wall", "mesh", "maxframe")}
    ratio = sum(1 for r in (r1, r2) if r["crash"] > 0)

    def run_work_intact(r):
        return r["exec"] >= INDIRECT_MIN and r["drawn"] >= DRAWN_MIN

    def run_sigdrops_ok(r):
        # ==expected (still reported) OR ==0 WITH that run's work intact.
        return r["sigdrops"] == SIGDROPS_EXPECT or (r["sigdrops"] == 0 and run_work_intact(r))

    gates = []
    g1 = all(r["refused"] == REFUSED_EXPECT for r in (r1, r2))
    gates.append(("G1-refused", g1, f"R1={r1['refused']} R2={r2['refused']} expect={REFUSED_EXPECT}"))
    g2 = all(run_sigdrops_ok(r) for r in (r1, r2))
    gates.append(("G2-sigdrops", g2, f"R1={r1['sigdrops']} R2={r2['sigdrops']} expect={SIGDROPS_EXPECT}"))
    g3 = mn["exec"] >= INDIRECT_MIN and mn["drawn"] >= DRAWN_MIN and mn["dispatched"] >= DISPATCH_MIN
    gates.append(("G3-work", g3, f"min exec={mn['exec']} drawn={mn['drawn']} dispatched={mn['dispatched']}"))
    gates.append(("G4-budget-skips==0", mx["budget"] == 0, f"budget={mx['budget']}"))
    g5 = all(r["pool_present"] and r["fail"] == 0 and r["done"] + r["fb"] >= r["q"]
             for r in (r1, r2))
    gates.append(("G5-pool", g5,
                  f"R1(q={r1['q']},done={r1['done']},fb={r1['fb']},fail={r1['fail']},present={r1['pool_present']}) "
                  f"R2(q={r2['q']},done={r2['done']},fb={r2['fb']},fail={r2['fail']},present={r2['pool_present']})"))
    gates.append(("G6-wall-time", 0 < mx["wall"] <= WALL_CAP, f"max ms/frame={mx['wall']} cap={WALL_CAP}"))
    # A 0/2 crash ratio is only proof the crash is gone if BOTH runs actually
    # reached the world-entry crash window (maxframe past the floor). A run that
    # ended early (e.g. suspend-timeout) never exposed the crash, so 0/2 there is
    # an INVALID ratio, not a PASS. Guard against that false positive.
    reached = all(r["maxframe"] > MAXFRAME_MIN for r in (r1, r2))
    g7 = ratio == 0 and reached
    gates.append(("G7-1a-ratio", g7,
                  f"ratio={ratio}/2 reached-world={reached} "
                  f"(maxframe R1={r1['maxframe']} R2={r2['maxframe']} floor={MAXFRAME_MIN})"))
    g8 = all(r["mesh"] > 0 and r["maxframe"] > MAXFRAME_MIN for r in (r1, r2))
    gates.append(("G8-mesh-progress", g8,
                  f"R1(mesh={r1['mesh']},maxframe={r1['maxframe']}) R2(mesh={r2['mesh']},maxframe={r2['maxframe']})"))
    gates.append(("G9-movement", mov == "PASS", f"movement={mov}"))

    for name, ok, detail in gates:
        print(f"GATE {name}: {'PASS' if ok else 'FAIL'}  ({detail})")
    refused_max = max(r1["refused"], r2["refused"])
    parked_ok = refused_max <= PARKED_MAX
    print(f"PARKED wave-size refusals<={PARKED_MAX}: {'met' if parked_ok else 'not-met'} "
          f"(refused={refused_max}; NON-BLOCKING, emitter milestone pending owner)")
    overall = all(ok for _, ok, _ in gates)
    print(f"CURRENT-TREE: {'PASS' if overall else 'FAIL'}")
    return 0 if overall else 1


if __name__ == "__main__":
    sys.exit(main())