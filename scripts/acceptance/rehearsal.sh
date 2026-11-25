#!/usr/bin/env bash
# End-to-end rehearsal of the acceptance evaluator on SYNTHETIC logs (no game,
# no launches). One FAIL case per blocking gate + one all-pass + parked-never-
# blocks proofs. A gate that cannot fail is not a gate. Tracked; runs with
# CSM_ACCEPTANCE_CONFIG UNSET (structural SYNTHETIC profile).
set -u
cd "$(dirname "$0")"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fail=0
# Structural variant numbers derived from the SYNTHETIC profile (not title data).
R=$(python3 -c "import csm_config;print(csm_config.SYNTHETIC['refused_expect'])")
SD=$(python3 -c "import csm_config;print(csm_config.SYNTHETIC['sigdrops_expect'])")
EX=$(python3 -c "import csm_config;print(csm_config.SYNTHETIC['indirect_min'])")
WC=$(python3 -c "import csm_config;print(int(csm_config.SYNTHETIC['wall_cap']))")
PK=$(python3 -c "import csm_config;print(csm_config.SYNTHETIC['parked_refused_max'])")
gen() { python3 synth_log.py "$@" >/dev/null; }
# expect NAME R1 R2 MOV OVERALL [GATE_ID_that_must_FAIL]
expect() {
  local name="$1" r1="$2" r2="$3" mov="$4" want="$5" gate="${6:-}"
  local out; out=$(python3 gate_eval.py "$r1" "$r2" "$mov")
  local got; got=$(echo "$out" | grep -oE 'CURRENT-TREE: (PASS|FAIL)' | grep -oE 'PASS|FAIL')
  local ok=1
  [ "$got" = "$want" ] || ok=0
  if [ -n "$gate" ]; then
    echo "$out" | grep -qE "GATE $gate: FAIL" || ok=0
  fi
  if [ "$ok" = 1 ]; then echo "ok   $name -> CURRENT-TREE $got${gate:+ ($gate FAIL)}";
  else echo "FAIL $name -> got $got want $want${gate:+ ($gate must FAIL)}"; echo "$out" | sed 's/^/      /'; fail=1; fi
}
# expect_noverdict NAME R1 R2 MOV — an INVALID run must yield NO VERDICT (exit 2),
# never PASS or FAIL, with no per-gate verdict, and report the run INVALID.
expect_noverdict() {
  local name="$1" r1="$2" r2="$3" mov="$4" out rc ok=1
  out=$(python3 gate_eval.py "$r1" "$r2" "$mov"); rc=$?
  [ "$rc" = 2 ] || ok=0
  echo "$out" | grep -q "CURRENT-TREE: NO VERDICT" || ok=0
  echo "$out" | grep -qE "CURRENT-TREE: (PASS|FAIL)" && ok=0
  echo "$out" | grep -qE "GATE .*: (PASS|FAIL)" && ok=0
  echo "$out" | grep -q "RUN-VALIDITY .*: INVALID" || ok=0
  if [ "$ok" = 1 ]; then echo "ok   $name -> NO VERDICT (exit 2; invalid reported; no PASS/FAIL)";
  else echo "FAIL $name -> rc=$rc want 2/NO VERDICT"; echo "$out" | sed 's/^/      /'; fail=1; fi
}

# All-pass baseline (structural defaults: work intact, budget 0, pool clean,
# wall under cap, no crash, mesh+maxframe, movement PASS).
gen "$T/base.log"
gen "$T/base2.log"
expect "all-pass" "$T/base.log" "$T/base2.log" PASS PASS

# One FAIL case per blocking gate (flip ONLY that gate's predicate).
gen "$T/g1.log" refused=$((R-1))
expect "G1-refused-wrong" "$T/base.log" "$T/g1.log" PASS FAIL "G1-refused"
gen "$T/g2.log" sigdrops=$((SD+1))
expect "G2-sigdrops-wrong" "$T/base.log" "$T/g2.log" PASS FAIL "G2-sigdrops"
gen "$T/g3.log" exec=$((EX-1000))
expect "G3-work-masked" "$T/base.log" "$T/g3.log" PASS FAIL "G3-work"
gen "$T/g4.log" budget=1
expect "G4-budget-skips>0" "$T/base.log" "$T/g4.log" PASS FAIL "G4-budget-skips==0"
gen "$T/g5.log" fail=1
expect "G5-pool-fail" "$T/base.log" "$T/g5.log" PASS FAIL "G5-pool"
gen "$T/g5b.log" pool=0
expect "G5-pool-line-missing" "$T/base.log" "$T/g5b.log" PASS FAIL "G5-pool"
gen "$T/g6.log" wall=$((WC+1000))
expect "G6-wall-regression" "$T/base.log" "$T/g6.log" PASS FAIL "G6-wall-time"
gen "$T/g7.log" crash=1
expect "G7-1a-persists" "$T/base.log" "$T/g7.log" PASS FAIL "G7-1a-ratio"
# Run-level VALIDITY: an invalid run yields NO VERDICT (exit 2), never PASS/FAIL.
MF=$(python3 -c "import csm_config;print(csm_config.SYNTHETIC['maxframe_min'])")
# (a) 0 crashes but a run ended short of world entry -> invalid ratio, NO VERDICT.
gen "$T/g7short.log" maxframe=$((MF-500))
expect_noverdict "validity-short-run-no-verdict" "$T/base.log" "$T/g7short.log" PASS
# (b) a run ended via suspend-timeout (even with a high maxframe) -> NO VERDICT.
gen "$T/susp.log" suspend=1
expect_noverdict "validity-suspend-timeout-no-verdict" "$T/base.log" "$T/susp.log" PASS
# (c) BOTH runs invalid -> NO VERDICT.
expect_noverdict "validity-both-invalid-no-verdict" "$T/g7short.log" "$T/susp.log" PASS
# (d) validity precedes every gate: an invalid run + movement FAIL still NO VERDICT
#     (cannot issue FAIL), and + movement PASS still NO VERDICT (cannot issue PASS).
expect_noverdict "validity-precedes-movement-fail" "$T/base.log" "$T/susp.log" FAIL
expect_noverdict "validity-precedes-movement-pass" "$T/susp.log" "$T/base.log" PASS
gen "$T/g8.log" mesh=0
expect "G8-no-mesh" "$T/base.log" "$T/g8.log" PASS FAIL "G8-mesh-progress"
expect "G9-movement-fail" "$T/base.log" "$T/base2.log" FAIL FAIL "G9-movement"

# G2 legitimate-zero branch: sigdrops 0 WITH work intact must PASS G2.
gen "$T/g2zero.log" sigdrops=0
expect "G2-zero-with-work-OK" "$T/base.log" "$T/g2zero.log" PASS PASS
# G2 masking branch: sigdrops 0 WITH work falling must FAIL G2 (and G3).
gen "$T/g2mask.log" sigdrops=0 exec=$((EX-1000))
expect "G2-zero-with-masked-work" "$T/base.log" "$T/g2mask.log" PASS FAIL "G2-sigdrops"

# PARKED never blocks:
# P1: refused at the expected count (parked NOT met) but all blocking pass -> PASS.
out=$(python3 gate_eval.py "$T/base.log" "$T/base2.log" PASS)
echo "$out" | grep -q "PARKED wave-size refusals<=${PK}: not-met" && echo "$out" | grep -q "CURRENT-TREE: PASS" \
  && echo "ok   parked-not-met does NOT block (CURRENT-TREE PASS)" \
  || { echo "FAIL parked-not-met blocked"; fail=1; }
# P2: refused at the parked target (parked MET) -> still FAIL (G1 wants the
#     expected count); parked meeting never rescues a blocking failure.
gen "$T/p2.log" refused=$PK
expect "parked-met-does-not-rescue" "$T/base.log" "$T/p2.log" PASS FAIL "G1-refused"

echo "----"
if [ "$fail" = 0 ]; then echo "REHEARSAL: ALL CASES BEHAVE (every blocking gate can FAIL; parked never blocks)";
else echo "REHEARSAL: FAILURES PRESENT"; fi
exit $fail