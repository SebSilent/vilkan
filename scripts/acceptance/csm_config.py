#!/usr/bin/env python3
"""Acceptance-harness configuration loader (tracked, clean-room).

Separates CODE from DATA. Every title-derived constant — the absolute game
path, the crash offset, the crash-module / shader hashes, the measured
baselines, the mesh-frame range, the capture frame windows — lives in an
EXTERNAL JSON file pointed to by the environment variable
``CSM_ACCEPTANCE_CONFIG``. Nothing title-derived is stored in this tree.

Contract:
  * ``load()``    -> dict from the external JSON, or None when the env var is
                     unset / the file is missing (callers SKIP, never guess).
  * ``require()`` -> dict, or prints a SKIP notice and exits 0 (real-run tools
                     that cannot proceed without the external data).
  * ``SYNTHETIC`` -> structural scaffolding for the rehearsal / parse / movement
                     synthetic suites. These are arbitrary round numbers and a
                     generic crash marker, NOT measured title baselines; the
                     synthetic suites run with the env var UNSET and use these.
  * ``--shell``   -> emit ``CSM_<KEY>='value'`` exports for bash ``eval`` (the
                     real-run orchestrator). Exits 1 when the config is unset
                     so the orchestrator can SKIP cleanly.

The external JSON keys (all title-derived, supplied at run time):
  game_dir, capture_root_win, data_dir, root, keys_file,
  crash_offset, crash_module, shader_hash,
  mesh_frame, mesh_frame_max, maxframe_min,
  refused_expect, sigdrops_expect, declined_expect,
  indirect_min, drawn_min, dispatch_min, wall_m3, wall_k2,
  parked_refused_max, world_region, synth, capture_frames_r2, move_frames
"""
import json
import os
import sys
from pathlib import Path

ENV = "CSM_ACCEPTANCE_CONFIG"

# Structural synthetic scaffolding — arbitrary round numbers + a generic crash
# marker. NOT title-derived; used only by the synthetic suites (env unset).
SYNTHETIC = {
    "crash_marker": "deadbeef",
    "refused_expect": 20,
    "sigdrops_expect": 3,
    "declined_expect": 20,
    "indirect_min": 4000,
    "drawn_min": 10000,
    "dispatch_min": 2500,
    "wall_cap": 5000.0,
    "maxframe_min": 1000,
    "parked_refused_max": 10,
    "mesh_frame": 1000,
    "synth_wall": 1000.0,
    "synth_maxframe": 1100,
}


def load():
    """External config dict, or None when unset/missing (caller SKIPs)."""
    p = os.environ.get(ENV)
    if not p:
        return None
    f = Path(p)
    if not f.is_file():
        return None
    return json.loads(f.read_text())


def require():
    cfg = load()
    if cfg is None:
        print(f"SKIP: {ENV} unset or missing — external acceptance config required")
        sys.exit(0)
    return cfg


def thresholds(cfg):
    """Numeric gate thresholds from the external config, or SYNTHETIC scaffolding.

    Real runs supply measured baselines via the config; the synthetic suites
    (cfg None) use the structural SYNTHETIC profile. The crash marker is the
    config's crash_offset for real runs, the generic SYNTHETIC marker otherwise.
    """
    if cfg is None:
        s = dict(SYNTHETIC)
        s["wall_m3"] = s["wall_cap"]
        s["wall_k2"] = s["wall_cap"]
        return s
    return {
        "crash_marker": cfg["crash_offset"],
        "refused_expect": cfg["refused_expect"],
        "sigdrops_expect": cfg["sigdrops_expect"],
        "declined_expect": cfg["declined_expect"],
        "indirect_min": cfg["indirect_min"],
        "drawn_min": cfg["drawn_min"],
        "dispatch_min": cfg["dispatch_min"],
        "wall_cap": max(cfg["wall_m3"], cfg["wall_k2"]),
        "wall_m3": cfg["wall_m3"],
        "wall_k2": cfg["wall_k2"],
        "maxframe_min": cfg["maxframe_min"],
        "parked_refused_max": cfg["parked_refused_max"],
        "mesh_frame": cfg["mesh_frame"],
    }


def _shell():
    cfg = load()
    if cfg is None:
        print(f"SKIP: {ENV} unset or missing", file=sys.stderr)
        return 1
    flat = dict(cfg)
    for k, v in cfg.get("synth", {}).items():
        flat[f"synth_{k}"] = v
    for k, v in flat.items():
        if isinstance(v, (list, dict)):
            v = json.dumps(v)
        print(f"CSM_{k.upper()}='{v}'")
    return 0


if __name__ == "__main__":
    if "--shell" in sys.argv:
        sys.exit(_shell())
    cfg = load()
    print(json.dumps(cfg if cfg is not None else {"skip": ENV + " unset"}, indent=2))