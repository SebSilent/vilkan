# Acceptance harness

Our own tooling for numeric acceptance of the Linux runtime. Tracked, clean-room
code; **no game-derived data lives here**. Everything title-derived (the absolute
game path, the crash offset, the crash-module / shader hashes, the measured
baselines, the mesh-frame range, the capture windows, the world-region rectangle)
is supplied at run time from an **external JSON config** pointed to by the
environment variable `CSM_ACCEPTANCE_CONFIG`. Game-derived artefacts and logs stay
in the external data dir, never in this tree.

## Code / data separation

- `csm_config.py` — config loader. `load()` returns the external dict or `None`
  when the env var is unset/missing (callers SKIP, never guess). `SYNTHETIC` is
  structural scaffolding (arbitrary round numbers + a generic crash marker) used
  by the synthetic suites, **not** measured title baselines. `--shell` emits
  `CSM_*` exports for the bash orchestrator.
- `png_decode.py` — minimal clean-room PNG decoder (8-bit RGB, filter 0). The
  world-region rectangle is supplied by the caller, never stored here.
- `gate_eval.py` — machine per-gate PASS/FAIL + overall CURRENT-TREE verdict.
  Thresholds and the crash marker come from the config (real runs) or the
  structural `SYNTHETIC` profile (env unset). The PARKED wave-size gate is
  reported but never blocks.
- `movement_gate.py` — paired idle/input movement delta over the world region.
  Pre-registered geometric thresholds (not fitted). Region from config, or the
  structural central-50% fraction when env unset.
- `staged-runs.sh` — real-run orchestrator. SKIPs cleanly when the config is
  unset; otherwise loads every title-derived value from the external config.

## Synthetic suites (run with `CSM_ACCEPTANCE_CONFIG` UNSET)

```sh
bash rehearsal.sh            # 15 cases: one FAIL per blocking gate + all-pass + parked-never-blocks
python3 movement_synth_test.py   # 6 cases: translation PASS; drift/loading/idle/hold-not-fired FAIL
bash preflight-parse-test.sh     # 30 cases: the grep/parse pipelines on exact emitter forms
```

A gate that cannot fail is not a gate: every blocking gate has a concrete numeric
predicate exercised by a synthetic FAIL case.

## Real runs (config SET; GPU ban lifted; one instance; verify 0 processes)

```sh
CSM_ACCEPTANCE_CONFIG=/path/to/external/acceptance-config.json bash staged-runs.sh
```

The external config is game-derived and stays outside the repo. Acceptance is
numeric proof against the configured baselines, never "it booted".