#!/usr/bin/env python3
"""Synthetic validity-reason suite (clean-room, NO launches, NO game data).

Proves every run-level validity reason gate_eval.py parses is actually emitted
by the runtime in the exact format the parser matches. Each synthetic log line
below is copied verbatim from the runtime's tracing format string (file:line
cited), so a drift between emitter and parser fails this suite.

Reasons exercised:
  * suspend-timeout  — lifecycle.rs:300 "{how}; ending the title" with
    how = "title did not finish suspending in time" (late(), lifecycle.rs:317).
    Parser: gate_eval.py:81 t.count("did not finish suspending in time").
  * close-ended      — lifecycle.rs:294 "{why}: suspending the title" with
    why = "window closed" (end_title, lifecycle.rs:256).
    Parser: gate_eval.py:85 t.count("window closed: suspending the title").
  * ignored-SC_CLOSE — lifecycle.rs:271 "SC_CLOSE ignored (VICTORY_CLOSE_ENDS
    off); run continues, timeout authoritative".
    Parser: gate_eval.py:88 t.count("SC_CLOSE ignored"). Diagnostic only: it
    must NOT invalidate a run that continued to the world.

Runs gate_eval with CSM_ACCEPTANCE_CONFIG UNSET so it uses the structural
SYNTHETIC thresholds (csm_config.py:40, maxframe_min=1000) — nothing
title-derived is read or written. Logs go to a temp dir, never the tree.
"""
import os
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent

# Verbatim runtime format strings (the pieces the binary stores; the log line
# is these joined exactly as tracing renders them).
SUSPEND_LINE = ("title did not finish suspending in time; ending the title")
CLOSE_LINE = "window closed: suspending the title"
IGNORED_LINE = ("SC_CLOSE ignored (VICTORY_CLOSE_ENDS off); run continues, "
                "timeout authoritative")
# Work-counter lines, verbatim from queue.rs:264-271, list.rs:1198,
# host.rs:1357-1364 (tracing renders fields as name=value).
FRAME_LINE = ("frame presented (since the last line: draws and dispatches, and "
              "those run on the host) frame={f} draws=12000 drawn=12000 "
              "dispatches=3000 dispatched=3000")
INDIRECT_LINE = "indirect execution ran executed=5000"
FPS_LINE = ("30.0 fps, 33.3 ms/frame (0.2 preparing: copies 0.1 ms 0.1 MB, "
            "textures 0.0 ms 0.0 MB; 0.1 on the GPU; 0.0 copying back 0.0 MB; "
            "present 0.1 ms, flip 0.1 ms), 100 draws/frame, dropped 0 without a "
            "host pipeline and 50 indirect, budget-skipped 0; pool q=100 "
            "done=100 fail=0 fallback=0 maxdepth=5 | inline-waits=0 wait-ms=0")
MESH_LINE = 'shader translated stage="mesh" hash=0 user_data=0 samplers=0'


def world_log(extra=""):
    """A run that reached the world (maxframe 2000 > floor 1000), work intact."""
    return "\n".join([FRAME_LINE.format(f=2000), INDIRECT_LINE, FPS_LINE,
                      MESH_LINE, extra]) + "\n"


def early_log(marker):
    """A run that ended before the world (maxframe 32 <= floor 1000)."""
    return "\n".join([FRAME_LINE.format(f=32), marker]) + "\n"


def run_gate(r1, r2):
    env = dict(os.environ)
    env.pop("CSM_ACCEPTANCE_CONFIG", None)  # force SYNTHETIC thresholds
    with tempfile.TemporaryDirectory() as d:
        p1, p2 = Path(d) / "r1.log", Path(d) / "r2.log"
        p1.write_text(r1)
        p2.write_text(r2)
        proc = subprocess.run(
            [sys.executable, str(HERE / "gate_eval.py"), str(p1), str(p2), "PASS"],
            capture_output=True, text=True, env=env)
        return proc.returncode, proc.stdout


def check(name, cond, detail):
    print(f"  {'PASS' if cond else 'FAIL'}  {name}  ({detail})")
    return cond


def main():
    ok = True
    valid = world_log()

    print("== suspend-timeout: emitted line must parse as INVALID ==")
    rc, out = run_gate(early_log(SUSPEND_LINE), valid)
    ok &= check("R1 INVALID", "RUN-VALIDITY R1: INVALID" in out, out.splitlines()[0])
    ok &= check("reason names suspend-timeout", "suspend-timeout" in out, "gate_eval.py:108")
    ok &= check("exit 2 (NO VERDICT)", rc == 2, f"rc={rc}")

    print("== close-ended: emitted line must parse as INVALID ==")
    rc, out = run_gate(early_log(CLOSE_LINE), valid)
    ok &= check("R1 INVALID", "RUN-VALIDITY R1: INVALID" in out, out.splitlines()[0])
    ok &= check("reason names window close", "window close" in out, "gate_eval.py:106")
    ok &= check("exit 2 (NO VERDICT)", rc == 2, f"rc={rc}")

    print("== ignored-SC_CLOSE: emitted line must NOT invalidate a world run ==")
    rc, out = run_gate(world_log(IGNORED_LINE), valid)
    ok &= check("R1 valid", "RUN-VALIDITY R1: valid" in out, out.splitlines()[0])
    ok &= check("diagnostic note printed", "external SC_CLOSE ignored" in out,
                "gate_eval.py:134")
    ok &= check("not NO-VERDICT on ignored alone", "NO VERDICT" not in out,
                "ignored is diagnostic, gate_eval.py:99")

    print("== clean world runs: valid, proceeds to gates ==")
    rc, out = run_gate(valid, valid)
    ok &= check("R1 valid", "RUN-VALIDITY R1: valid" in out, "no markers")
    ok &= check("R2 valid", "RUN-VALIDITY R2: valid" in out, "no markers")
    ok &= check("gates evaluated (G7 present)", "GATE G7-1a-ratio" in out,
                "validity passed, gates run")

    print(f"SYNTHETIC-VALIDITY: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())