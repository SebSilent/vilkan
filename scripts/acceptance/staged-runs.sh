#!/usr/bin/env bash
# STAGED real-run orchestrator (tracked, clean-room). DO NOT RUN until the GPU
# ban lifts. Verify 0 of our processes first; one instance; full termination.
#
# Every title-derived constant (game path, crash offset, measured baselines,
# mesh-frame range, capture windows, keys file) comes from the EXTERNAL config
# via csm_config.py --shell (env CSM_ACCEPTANCE_CONFIG). With the env unset this
# SKIPs cleanly — nothing title-derived is stored in this tree. Game-derived
# artefacts and logs stay in the external data dir.
#
# Purpose: measure the stacked mitigations + name the 1a stall's blocking call,
# then issue the machine CURRENT-TREE verdict (gate_eval.py; parked excluded).
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"

# Load the external config; SKIP when unset (real runs need the external data).
if ! SHELL_EXPORTS=$(python3 "$HERE/csm_config.py" --shell); then
  echo "SKIP: CSM_ACCEPTANCE_CONFIG unset — real-run orchestrator needs the external config"
  exit 0
fi
eval "$SHELL_EXPORTS"
B="$CSM_DATA_DIR"
ROOT="$CSM_ROOT"
OUT_LOG="$(dirname "$CSM_DATA_DIR")/vilkan-out.log"
WALL_CAP=$(python3 -c "print(max($CSM_WALL_M3,$CSM_WALL_K2))")

# Our Proton prefix: run.sh sets STEAM_COMPAT_DATA_PATH=$VILKAN_DATA/prefix and
# VILKAN_DATA defaults to the repo-sibling vilkan-data (= dirname of the log dir).
# Scope every process check/cleanup to THIS prefix so a foreign Wine/Proton app
# (another game sharing the generic wineserver/winedevice/xalia names) is never
# counted by the ban nor killed by cleanup.
OUR_PREFIX="$(dirname "$CSM_DATA_DIR")/prefix"
export CSM_OUR_PREFIX="$OUR_PREFIX"
our_pids() { python3 "$HERE/our_procs.py" "$OUR_PREFIX"; }
our_cleanup() { for p in $(our_pids); do kill -9 "$p" 2>/dev/null; done; }

# 0) Verify nothing of ours is running (the ban), scoped to our prefix.
n=$(our_pids | wc -l)
if [ "$n" != 0 ]; then echo "ABORT: $n of OUR processes still running"; our_pids; exit 1; fi

KEYS=$(cat "$CSM_KEYS_FILE")               # configured-saves Enter menu script
FRAMES=$(python3 -c "import json;a=json.loads('$CSM_CAPTURE_FRAMES_R2');print(','.join(str(t) for t in range(*a)))")

# R1: no capture (the fault gate), VICTORY_INDIRECT=1. First of >=2 runs for
# the 1a crash ratio (the first-use budget fix must drop it).
date -Is > "$B/runR1-start.txt"
cd "$ROOT/vilkan"
env VILKAN_GAME_DIR="$CSM_GAME_DIR" \
    VICTORY_INDIRECT=1 VICTORY_KEYS="$KEYS" \
    timeout --kill-after=10 400 scripts/run.sh
echo "RUN-R1-EXIT:$?" >> "$B/runR1-start.txt"; date -Is >> "$B/runR1-start.txt"
sed 's/\x1b\[[0-9;]*m//g' "$OUT_LOG" > "$B/runR1-vilkan-out.log"
our_cleanup
sleep 3

# R2: capture only, the staged-logging exe.
date -Is > "$B/runR2-start.txt"
cd "$ROOT/vilkan"
env VILKAN_GAME_DIR="$CSM_GAME_DIR" \
    VICTORY_INDIRECT=1 VICTORY_KEYS="$KEYS" \
    VICTORY_CAPTURE="$CSM_CAPTURE_ROOT_WIN" \
    VICTORY_CAPTURE_FRAMES="$FRAMES" \
    timeout --kill-after=10 400 scripts/run.sh
echo "RUN-R2-EXIT:$?" >> "$B/runR2-start.txt"; date -Is >> "$B/runR2-start.txt"

# Name the blocking call (the stall) + the present/flip spans:
sed 's/\x1b\[[0-9;]*m//g' "$OUT_LOG" > "$B/runR2-vilkan-out.log"
echo "== blocking waits (the stall):"
grep -aE "WaitForSingleObject blocked|WaitForMultipleObjects blocked|WaitOnAddress blocked" "$B/runR2-vilkan-out.log" | tail -10
echo "== max frame wall-time + top-5 (the watchdog discriminator):"
grep -aoE "[0-9.]+ ms/frame" "$B/runR2-vilkan-out.log" | grep -oE "[0-9.]+" | sort -rn | head -5
echo "== present/flip spans:"
grep -aoE "present [0-9.]+ ms, flip [0-9.]+ ms" "$B/runR2-vilkan-out.log" | sort -t' ' -k2 -rn | head -5
echo "== crash / mesh / maxframe:"
grep -ac "unhandled exception" "$B/runR2-vilkan-out.log"
grep -a 'stage="mesh"' "$B/runR2-vilkan-out.log" | head -1
grep -aoE "frame=[0-9]+" "$B/runR2-vilkan-out.log" | sort -t= -k2 -n | tail -1
echo "== indirect drops + executed + reasons:"
grep -ac "indirect execution dropped" "$B/runR2-vilkan-out.log"
grep -a "indirect execution ran" "$B/runR2-vilkan-out.log" | tail -1
grep -a "indirect drop:" "$B/runR2-vilkan-out.log" | sed -E 's/0x[0-9a-f]+/A/g' | sort | uniq -c
echo "== E5005 (acceptance 0):"
grep -ac "E5005" "$B/runR2-vilkan-out.log"
echo "== dstorage close fix: entry-0 pending polls (should drop):"
grep -a "dstorage entry still pending" "$B/runR2-vilkan-out.log" | tail -1
echo "== per-frame wall-time vs configured baselines (fps-line max; cap $WALL_CAP):"
# Anchor on the fps line so a copystat per-copy ms/frame (a component, diag
# feature) can never be mistaken for the frame wall-time (pre-flight fix).
grep -aoE "[0-9.]+ fps, [0-9.]+ ms/frame" "$B/runR2-vilkan-out.log" | grep -oE "[0-9.]+ ms/frame" | grep -oE "^[0-9.]+" | sort -rn | head -1
echo "== XAsync wait culprit (identity/provider of the stalling block):"
grep -a "XAsync wait exceeded" "$B/runR2-vilkan-out.log" | head -3

# === Subrender first-use budget fix: 1a ratio across R1+R2 ===
echo "== 1a crash (null write +0x$CSM_CRASH_OFFSET) per run:"
for r in R1 R2; do
  c=$(grep -ac "unhandled exception" "$B/run$r-vilkan-out.log")
  sig=$(grep -ac "$CSM_CRASH_OFFSET" "$B/run$r-vilkan-out.log")
  mf=$(grep -aoE "frame=[0-9]+" "$B/run$r-vilkan-out.log" | sort -t= -k2 -n | tail -1)
  mesh=$(grep -ac 'stage="mesh"' "$B/run$r-vilkan-out.log")
  echo "$r: crashes=$c 1a-sig=$sig maxframe=$mf mesh-binds=$mesh"
done
echo "== first-use budget skips (>0 means the budget engaged):"
grep -aE "inline wait expired|skips=[0-9]+" "$B/runR1-vilkan-out.log" "$B/runR2-vilkan-out.log" | tail -2
grep -aoE "budget-skipped [0-9]+" "$B/runR1-vilkan-out.log" "$B/runR2-vilkan-out.log" | sort -t' ' -k2 -rn | head -2

# === MASKING CHECK: a "fixed" run must NOT pass by doing less work ===
echo "== MASKING: executed work must not fall vs baseline, refusals must still report:"
for r in R1 R2; do
  L="$B/run$r-vilkan-out.log"
  dr=$(grep -aoE "drawn=[0-9]+" "$L" | grep -oE '[0-9]+' | sort -rn | head -1)
  di=$(grep -aoE "dispatched=[0-9]+" "$L" | grep -oE '[0-9]+' | sort -rn | head -1)
  ie=$(grep -aoE "executed=[0-9]+" "$L" | grep -oE '[0-9]+' | sort -rn | head -1)
  rf=$(grep -ac "host pipeline refused" "$L")
  sd=$(grep -ac "first time for this signature" "$L")
  bs=$(grep -aoE "budget-skipped [0-9]+" "$L" | grep -oE '[0-9]+' | sort -rn | head -1)
  wt=$(grep -aoE "[0-9.]+ fps, [0-9.]+ ms/frame" "$L" | grep -oE '[0-9.]+ ms/frame' | grep -oE '^[0-9.]+' | sort -rn | head -1)
  echo "$r: drawn=$dr (>=$CSM_DRAWN_MIN) dispatched=$di (>=$CSM_DISPATCH_MIN) indirect-exec=$ie (>=$CSM_INDIRECT_MIN)"
  echo "    refused=$rf (==$CSM_REFUSED_EXPECT) sig-drops=$sd (==$CSM_SIGDROPS_EXPECT) budget-skips=${bs:-0} (==0) max-ms/frame=$wt"
done
# CURRENT-TREE ACCEPTANCE (BLOCKING). lower.rs unchanged so refusals STAY at the
# expected count — the reduction target is the PARKED emitter milestone below.
echo "CURRENT-TREE PASS only if ALL of (machine verdict: gate_eval.py at the end):"
echo "  refused==$CSM_REFUSED_EXPECT (still reported, not hidden; lower.rs unchanged)"
echo "  sig-drops==$CSM_SIGDROPS_EXPECT OR ==0 WITH work intact (exec>=$CSM_INDIRECT_MIN AND drawn>=$CSM_DRAWN_MIN)"
echo "  indirect-exec>=$CSM_INDIRECT_MIN AND drawn>=$CSM_DRAWN_MIN AND dispatched>=$CSM_DISPATCH_MIN (work not masked)"
echo "  budget-skips==0; pool fail==0 AND done+fallback>=q (no hung waiter)"
echo "  no per-frame wall-time regression vs configured baselines (cap $WALL_CAP, fps-line max)"
echo "  1a crash ratio ==0/2 across R1,R2; mesh progress (mesh-binds>0 AND maxframe>$CSM_MAXFRAME_MIN each run)"
echo "  movement gate (paired idle/input runs below): input world cum_change_frac>=0.50 &"
echo "      >=idle+0.30 & cum_mad>=20 & >=2x idle, hold-fired-after-mesh present."
echo "  NOTE: the refusal-reduction target is PARKED (emitter milestone), NOT a tonight gate."

# === POOL HARDENING CHECK ===
echo "== POOL: failures 0, waiters released (no hang), depth reported, waits bounded:"
for r in R1 R2; do
  L="$B/run$r-vilkan-out.log"
  pl=$(grep -aoE "pool q=[0-9]+ done=[0-9]+ fail=[0-9]+ fallback=[0-9]+ maxdepth=[0-9]+ \| inline-waits=[0-9]+ wait-ms=[0-9]+" "$L" | tail -1)
  echo "$r: ${pl:-NO POOL LINE (hang before first fps report?)}"
done
echo "A missing pool line in a run that reached fps means a waiter hung before"
echo "the first report — FAIL (the XAsync wait-forever bug we are guarding)."

# === ATTRIBUTION SEQUENCE (mechanical post-ban; see HANDOUT decision tree) ===
echo "== SELF-ID: backend label + active mitigation set per run:"
for r in R1 R2; do
  echo "$r: $(grep -aE '^backend ' "$B/run$r-vilkan-out.log" | head -1) | $(grep -aE '^mitigations ' "$B/run$r-vilkan-out.log" | head -1)"
done

# All-on 1a ratio from R1/R2 (the deciding number for the first branch):
ALLON_CR=0
for r in R1 R2; do
  c=$(grep -ac "$CSM_CRASH_OFFSET" "$B/run$r-vilkan-out.log")
  [ "$c" -gt 0 ] && ALLON_CR=$((ALLON_CR+1))
done
echo "ALL-ON 1a ratio: $ALLON_CR/2"

# iso LABEL ENV... : run TWO instances with one toggle overridden, report the
# 1a ratio + masking counters. Two runs per branch, ratio reported.
iso() {
  local label="$1"; shift
  local cr=0 k c L dr di ie rf sd
  for k in 1 2; do
    date -Is > "$B/iso-$label-$k-start.txt"
    cd "$ROOT/vilkan"
    env VILKAN_GAME_DIR="$CSM_GAME_DIR" \
        VICTORY_INDIRECT=1 VICTORY_KEYS="$KEYS" "$@" \
        timeout --kill-after=10 400 scripts/run.sh
    echo "EXIT:$?" >> "$B/iso-$label-$k-start.txt"
    sed 's/\x1b\[[0-9;]*m//g' "$OUT_LOG" > "$B/iso-$label-$k.log"
    our_cleanup
    sleep 3
    c=$(grep -ac "$CSM_CRASH_OFFSET" "$B/iso-$label-$k.log")
    [ "$c" -gt 0 ] && cr=$((cr+1))
  done
  echo "ISO $label: 1a ratio $cr/2 | $(grep -a '^mitigations ' "$B/iso-$label-1.log" | head -1)"
  for k in 1 2; do
    L="$B/iso-$label-$k.log"
    dr=$(grep -aoE "drawn=[0-9]+" "$L" | grep -oE '[0-9]+' | sort -rn | head -1)
    di=$(grep -aoE "dispatched=[0-9]+" "$L" | grep -oE '[0-9]+' | sort -rn | head -1)
    ie=$(grep -aoE "executed=[0-9]+" "$L" | grep -oE '[0-9]+' | sort -rn | head -1)
    rf=$(grep -ac "wave64 exchange declined" "$L")
    sd=$(grep -ac "first time for this signature" "$L")
    echo "  $label-$k: drawn=$dr dispatched=$di indirect-exec=$ie declined=$rf sig-drops=$sd"
  done
  ISO_CR=$cr
}

# DECISION (HANDOUT has the full tree). Isolation order, largest first:
#   pool -> shadow-seek -> arg-clamp -> close-abort -> dispatch-single.
if [ "$ALLON_CR" -ge 1 ]; then
  echo "== BRANCH B: 1a persists all-on ($ALLON_CR/2). Isolate pool (largest) first."
  iso pool VILKAN_MIT_POOL=0
  echo "DECIDE: pool-off 1a $ISO_CR/2 vs all-on $ALLON_CR/2."
  echo "  If pool-off drops to 0/2 -> the pool is implicated. Next: keep pool OFF,"
  echo "  measure geometry; do NOT touch other toggles yet. If pool-off still crashes:"
  echo "    iso shadow VILKAN_MIT_SHADOW_SEEK=0   (then arg-clamp, close-abort, dispatch-single)"
else
  echo "== BRANCH A/C: 1a gone all-on (0/2). Check masking (above)."
  echo "  If masking PASSES -> all-on is the fix; STOP, no isolation needed."
  echo "  If geometry/counters REGRESS (drawn<$CSM_DRAWN_MIN / dispatched<$CSM_DISPATCH_MIN /"
  echo "  indirect-exec<$CSM_INDIRECT_MIN / declined<$CSM_DECLINED_EXPECT / sig-drops>$CSM_SIGDROPS_EXPECT) -> isolate same order:"
  echo "    iso pool VILKAN_MIT_POOL=0  (then shadow-seek, arg-clamp, close-abort, dispatch-single)"
fi

# === WAVE-SIZE NARROW FIX: VERIFIED A WALL — PARKED, DO NOT EXECUTE ===
# lower.rs UNCHANGED (zero safe wave-size-agnostic candidates). The refusal-
# reduction target requires 64-lane emulation = the EMITTER MILESTONE, parked
# for owner approval. These numbers are INFORMATIONAL, NON-BLOCKING tonight.
echo "== WAVE-SIZE (PARKED emitter milestone — INFORMATIONAL, NON-BLOCKING tonight):"
echo "   lower.rs unchanged; refused==$CSM_REFUSED_EXPECT is EXPECTED tonight, NOT a failure."
for r in R1 R2; do
  L="$B/run$r-vilkan-out.log"
  rf=$(grep -ac "host pipeline refused" "$L")
  ie=$(grep -aoE "executed=[0-9]+" "$L" | grep -oE '[0-9]+' | sort -rn | head -1)
  di=$(grep -aoE "dispatched=[0-9]+" "$L" | grep -oE '[0-9]+' | sort -rn | head -1)
  dr=$(grep -aoE "drawn=[0-9]+" "$L" | grep -oE '[0-9]+' | sort -rn | head -1)
  sd=$(grep -ac "first time for this signature" "$L")
  wt=$(grep -aoE "[0-9.]+ fps, [0-9.]+ ms/frame" "$L" | grep -oE '[0-9.]+ ms/frame' | grep -oE '^[0-9.]+' | sort -rn | head -1)
  echo "$r: refused=$rf (parked target <=$CSM_PARKED_REFUSED_MAX; tonight $CSM_REFUSED_EXPECT) indirect-exec=$ie dispatched=$di"
  echo "    drawn=$dr sig-drops=$sd max-ms/frame=$wt"
done
echo "PARKED milestone target (NOT a tonight gate, no PASS/FAIL verdict issued):"
echo "  refused<=$CSM_PARKED_REFUSED_MAX AND indirect-exec>=$CSM_INDIRECT_MIN AND dispatched>=$CSM_DISPATCH_MIN AND drawn>=$CSM_DRAWN_MIN"
echo "  AND sig-drops==$CSM_SIGDROPS_EXPECT AND geometry delta>idle AND no wall-time regression."

# === MOVEMENT GATE: paired idle-input baseline vs event-relative input run ===
# Both reach the world via the same Enter menu script. The INPUT run adds
# mesh+2:W:10 (MeshAfter): W drives the left stick forward, firing 2 s after the
# FIRST mesh bind, holding 10 s. The capture window covers the hold for any mesh
# frame in the configured range; the gate selects each run's OWN mesh-relative
# window (movement_gate.py). Drift/loading/idle sway cannot satisfy the numeric
# delta by construction (see movement_gate.py docstring).
MOVE_FRAMES=$(python3 -c "import json;a=json.loads('$CSM_MOVE_FRAMES');print(','.join(str(t) for t in range(*a)))")
moverun() {
  local label="$1" keys="$2" wdir="$3"
  mkdir -p "$B/$wdir"
  date -Is > "$B/move-$label-start.txt"
  cd "$ROOT/vilkan"
  env VILKAN_GAME_DIR="$CSM_GAME_DIR" \
      VICTORY_INDIRECT=1 VICTORY_KEYS="$keys" \
      VICTORY_CAPTURE="${CSM_CAPTURE_ROOT_WIN}\\${wdir}" \
      VICTORY_CAPTURE_FRAMES="$MOVE_FRAMES" \
      timeout --kill-after=10 400 scripts/run.sh
  echo "EXIT:$?" >> "$B/move-$label-start.txt"
  sed 's/\x1b\[[0-9;]*m//g' "$OUT_LOG" > "$B/move-$label.log"
  our_cleanup
  sleep 3
}
echo "== MOVEMENT: idle baseline (no hold) then input run (mesh+2:W:10):"
moverun idle "$KEYS" move-idle
moverun input "$KEYS,mesh+2:W:10" move-input
MOVEMENT=$(python3 "$HERE/movement_gate.py" \
    "$B/move-idle" "$B/move-idle.log" "$B/move-input" "$B/move-input.log" \
    | tee "$B/movement-verdict.txt" | grep -oE 'MOVEMENT: (PASS|FAIL)' | grep -oE 'PASS|FAIL')
echo "movement verdict: ${MOVEMENT:-FAIL}"

# === FINAL MACHINE VERDICT (gate_eval.py; PARKED wave-size NEVER blocks) ===
echo "== FINAL CURRENT-TREE VERDICT (every blocking gate; parked excluded):"
python3 "$HERE/gate_eval.py" \
    "$B/runR1-vilkan-out.log" "$B/runR2-vilkan-out.log" "${MOVEMENT:-FAIL}"
GATE_RC=$?
if [ "$GATE_RC" = 2 ]; then
  echo "CURRENT-TREE: NO VERDICT — invalid run(s) (suspend-timeout / never reached world);"
  echo "  NOT a mitigation measurement. Re-run clean (no foreign app) to the world."
fi
echo "CURRENT-TREE exit: $GATE_RC"

# Clean up; verify 0 of OURS again (prefix-scoped; a foreign app is untouched).
our_cleanup
sleep 2
echo "our procs after: $(our_pids | wc -l)"