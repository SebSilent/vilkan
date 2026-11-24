#!/usr/bin/env bash
# Pre-flight parsing exercise: feed synthetic log lines in the EXACT rendered
# forms (tracing prefix on info/warn; bare println/eprintln for startup/crash)
# through the SAME grep/parse pipelines the orchestrator uses, and assert the
# extracted numbers. No launches; pure text. Tracked; runs with
# CSM_ACCEPTANCE_CONFIG UNSET (structural numbers, generic crash marker).
# Run: bash preflight-parse-test.sh
set -u
cd "$(dirname "$0")"
T=$(mktemp); trap 'rm -f "$T"' EXIT
CM=$(python3 -c "import csm_config;print(csm_config.SYNTHETIC['crash_marker'])")
PFX='2026-11-24T20:00:00.000000Z  INFO '
WFX='2026-11-24T20:00:00.000000Z  WARN '
cat > "$T" <<EOF
backend        DX12
mitigations    [DX12] pool=on close-abort=on shadow-seek=on dispatch-single=on arg-clamp=on
${PFX}victory_gpu::host: 60.0 fps, 1000.0 ms/frame (10.0 preparing: copies 5.0 ms 1.0 MB, textures 3.0 ms 2.0 MB; 800.0 on the GPU; 1.0 copying back 0.5 MB; present 13.7 ms, flip 2.1 ms), 100 draws/frame, dropped 0 without a host pipeline and 0 indirect, budget-skipped 0; pool q=10 done=10 fail=0 fallback=0 maxdepth=3 | inline-waits=2 wait-ms=5
${PFX}victory_gpu::queue: frame=200 draws=1000 drawn=10000 dispatches=100 dispatched=2500 frame presented (since the last line: draws and dispatches, and those run on the host)
${PFX}victory_gpu::list: executed=4000 indirect execution ran
${PFX}victory_gpu::list: indirect execution dropped (first time for this signature)
${WFX}victory_gpu::pipeline::host: error=refused shaders=[] host pipeline refused
${WFX}victory_shader::emit::lower: reason=wave64 wave64 exchange declined
${PFX}victory_gpu::pipeline: pipeline=0x1 frame=1000 stage="mesh" size=2048 hash=abc first bind
${WFX}victory_gpu::pipeline: skips=1 inline wait expired: bind deferred, compilation continues
${WFX}victory_hle::modules::kernel: WaitOnAddress blocked address=0x1 timeout=100 waited=1500
${WFX}victory_hle::modules::kernel: WaitForSingleObject blocked handle=0x2 timeout=100 waited=1200
${WFX}victory_hle::modules::kernel: WaitForMultipleObjects blocked count=2 waited=1300
${WFX}victory_hle::modules::xasync: XAsync wait exceeded 1s: block still pending
${WFX}victory_hle::modules::dstorage: index=0 pending_polls=5 dstorage entry still pending
${WFX}victory_gpu::indirect: argument clamp: clamp:17:27: E5005: Method 'Load' is not defined on type
${WFX}victory_gpu::indirect: no argument clamp: indirect execution dropped error=E_INVALIDARG
${WFX}victory_gpu::draw: indirect drop: argument buffer not shadowed arguments=0x1 len=4 kind=0
VIctory: unhandled exception 0xc0000005 at synthetic-image.dll+0x${CM} (write 0x0)
${PFX}victory_gpu::copystat:   copied from 0x10+0x20 (memory 0x1): 1.0x/frame, 0.50 MB/frame, 9999.99 ms/frame
EOF
fail=0
chk() { # name expected actual
  if [ "$2" = "$3" ]; then echo "ok   $1 = $3"; else echo "FAIL $1: expected [$2] got [$3]"; fail=1; fi
}
# startup self-id (^ anchors valid: println, no tracing prefix)
chk backend-line "backend        DX12" "$(grep -aE '^backend ' "$T" | head -1)"
chk mitigations-line "mitigations    [DX12] pool=on close-abort=on shadow-seek=on dispatch-single=on arg-clamp=on" "$(grep -aE '^mitigations ' "$T" | head -1)"
# fps-line fields
chk ms/frame-anchored "1000.0" "$(grep -aoE '[0-9.]+ fps, [0-9.]+ ms/frame' "$T" | grep -oE '[0-9.]+ ms/frame' | grep -oE '^[0-9.]+' | sort -rn | head -1)"
chk present/flip "present 13.7 ms, flip 2.1 ms" "$(grep -aoE 'present [0-9.]+ ms, flip [0-9.]+ ms' "$T" | head -1)"
chk budget-skipped "0" "$(grep -aoE 'budget-skipped [0-9]+' "$T" | grep -oE '[0-9]+' | sort -rn | head -1)"
chk pool-line "pool q=10 done=10 fail=0 fallback=0 maxdepth=3 | inline-waits=2 wait-ms=5" "$(grep -aoE 'pool q=[0-9]+ done=[0-9]+ fail=[0-9]+ fallback=[0-9]+ maxdepth=[0-9]+ \| inline-waits=[0-9]+ wait-ms=[0-9]+' "$T" | tail -1)"
pl=$(grep -aoE 'pool q=[0-9]+ done=[0-9]+ fail=[0-9]+ fallback=[0-9]+ maxdepth=[0-9]+ \| inline-waits=[0-9]+ wait-ms=[0-9]+' "$T" | tail -1)
chk pool-fail "0" "$(echo "$pl" | grep -oE 'fail=[0-9]+' | grep -oE '[0-9]+')"
chk pool-q "10" "$(echo "$pl" | grep -oE 'q=[0-9]+' | grep -oE '[0-9]+')"
chk pool-done "10" "$(echo "$pl" | grep -oE 'done=[0-9]+' | grep -oE '[0-9]+')"
# milestone deltas
chk drawn "10000" "$(grep -aoE 'drawn=[0-9]+' "$T" | grep -oE '[0-9]+' | sort -rn | head -1)"
chk dispatched "2500" "$(grep -aoE 'dispatched=[0-9]+' "$T" | grep -oE '[0-9]+' | sort -rn | head -1)"
chk executed "4000" "$(grep -aoE 'executed=[0-9]+' "$T" | grep -oE '[0-9]+' | sort -rn | head -1)"
chk frame "1000" "$(grep -aoE 'frame=[0-9]+' "$T" | sort -t= -k2 -n | tail -1 | grep -oE '[0-9]+')"
chk mesh-bind 'stage="mesh"' "$(grep -a 'stage="mesh"' "$T" | head -1 | grep -oE 'stage="mesh"')"
# counts
chk refused "1" "$(grep -ac 'host pipeline refused' "$T")"
chk sig-drops "1" "$(grep -ac 'first time for this signature' "$T")"
chk declined "1" "$(grep -ac 'wave64 exchange declined' "$T")"
chk indirect-dropped "2" "$(grep -ac 'indirect execution dropped' "$T")"
chk E5005 "1" "$(grep -ac 'E5005' "$T")"
chk crash "1" "$(grep -ac 'unhandled exception' "$T")"
chk 1a-sig "1" "$(grep -ac "$CM" "$T")"
chk budget-skip-log "1" "$(grep -ac 'inline wait expired' "$T")"
chk skips-field "1" "$(grep -aoE 'skips=[0-9]+' "$T" | grep -oE '[0-9]+' | head -1)"
chk wait-onaddr "1" "$(grep -ac 'WaitOnAddress blocked' "$T")"
chk wait-single "1" "$(grep -ac 'WaitForSingleObject blocked' "$T")"
chk wait-multi "1" "$(grep -ac 'WaitForMultipleObjects blocked' "$T")"
chk xasync "1" "$(grep -ac 'XAsync wait exceeded' "$T")"
chk dstorage "1" "$(grep -ac 'dstorage entry still pending' "$T")"
# CONFLATION GUARD: the bare ms/frame grep would pick the copystat 9999.99;
# the fps-anchored grep must NOT. Prove the anchor rejects the copystat value.
bare=$(grep -aoE '[0-9.]+ ms/frame' "$T" | grep -oE '^[0-9.]+' | sort -rn | head -1)
chk bare-ms/frame-picks-copystat "9999.99" "$bare"   # documents the hazard
chk anchored-rejects-copystat "1000.0" "$(grep -aoE '[0-9.]+ fps, [0-9.]+ ms/frame' "$T" | grep -oE '[0-9.]+ ms/frame' | grep -oE '^[0-9.]+' | sort -rn | head -1)"
echo "----"
if [ "$fail" = 0 ]; then echo "ALL PARSE CHECKS PASS (synthetic, no launches)"; else echo "PARSE FAILURES PRESENT"; fi
exit $fail