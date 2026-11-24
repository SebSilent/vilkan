#!/usr/bin/env bash
# Run an Xbox Series title through our loader under Proton (bootstrap backend).
#
#   scripts/run.sh [vilkan arguments...]
#
# Defaults to `run <title exe> --execute`. Examples:
#   scripts/run.sh                          # boot the title
#   scripts/run.sh config                   # show resolved paths
#   scripts/run.sh census --missing         # import census, no execution
#
# The title path and data dir come from config/env — nothing is hardcoded:
#   GAME_DIR / VILKAN_GAME_DIR   the title's folder (required; no default)
#   VILKAN_TITLE_EXE             the title executable (default: the single
#                                .exe found in GAME_DIR)
#   VILKAN_DATA                  prefix/cache/data root (default: ../vilkan-data,
#                                OUTSIDE the repo)
#   PROTON_DIR                   Proton install (default: Proton - Experimental)
#   VILKAN_LOG                   log filter, e.g. vilkan_loader=trace,info
#
# Proton swallows the program's stdout, so the launch goes through a .bat
# that redirects into $VILKAN_DATA/vilkan-out.log; we tail it live.
# Wine/VKD3D debug output: $VILKAN_DATA/steam-0.log (PROTON_LOG=1).
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$(dirname "$HERE")"

PROTON_DIR="${PROTON_DIR:-$HOME/.local/share/Steam/steamapps/common/Proton - Experimental}"
VILKAN_DATA="${VILKAN_DATA:-$ROOT/vilkan-data}"
PROFILE="${PROFILE:-release}"
EXE="$HERE/target/x86_64-pc-windows-msvc/$PROFILE/vilkan.exe"

# The title folder: from the environment, else from victory.toml ([paths].game_dir).
GAME_DIR="${GAME_DIR:-${VILKAN_GAME_DIR:-}}"
if [[ -z "$GAME_DIR" && -f "$HERE/victory.toml" ]]; then
  GAME_DIR="$(sed -nE "s/^[[:space:]]*game_dir[[:space:]]*=[[:space:]]*['\"]?([^'\"]+)['\"]?.*/\1/p" "$HERE/victory.toml" | head -1)"
fi
[[ -n "$GAME_DIR" ]] || { echo "set GAME_DIR (or VILKAN_GAME_DIR, or [paths].game_dir in victory.toml) to the title's folder" >&2; exit 1; }
[[ -d "$GAME_DIR" ]] || { echo "GAME_DIR is not a directory: $GAME_DIR" >&2; exit 1; }

# The title executable: explicit override, else the single .exe in GAME_DIR.
TITLE_EXE="${VILKAN_TITLE_EXE:-}"
if [[ -z "$TITLE_EXE" ]]; then
  mapfile -t exes < <(find "$GAME_DIR" -maxdepth 1 -iname '*.exe' | sort)
  [[ ${#exes[@]} -eq 1 ]] || { echo "expected one .exe in $GAME_DIR (found ${#exes[@]}); set VILKAN_TITLE_EXE" >&2; exit 1; }
  TITLE_EXE="${exes[0]}"
fi

[[ -x "$PROTON_DIR/proton" ]] || { echo "no Proton at: $PROTON_DIR" >&2; exit 1; }
[[ -f "$EXE" ]] || { echo "no vilkan.exe; run scripts/build.sh first" >&2; exit 1; }

mkdir -p "$VILKAN_DATA"/{prefix,cache,data}

# Windows paths inside the Proton prefix: Z: is the Linux root.
winpath() { printf 'Z:%s' "${1//\//\\}"; }

export STEAM_COMPAT_CLIENT_INSTALL_PATH="${STEAM_COMPAT_CLIENT_INSTALL_PATH:-$HOME/.local/share/Steam}"
export STEAM_COMPAT_DATA_PATH="$VILKAN_DATA/prefix"
export PROTON_LOG="${PROTON_LOG:-1}"
export PROTON_LOG_DIR="$VILKAN_DATA"
# Proton only opens its debug log when SteamGameId is set.
export SteamGameId="${SteamGameId:-0}"
export SteamAppId="${SteamAppId:-0}"
# Our user-facing vars (VILKAN_GAME_DIR / VILKAN_DATA / VILKAN_LOG) feed the
# upstream-derived loader, which reads VICTORY_* (kept for merge-ability).
export VICTORY_GAME_DIR="$(winpath "$GAME_DIR")"
export VICTORY_CACHE_DIR="$(winpath "$VILKAN_DATA/cache")"
export VICTORY_DATA_DIR="$(winpath "$VILKAN_DATA/data")"
export VICTORY_LOG="${VILKAN_LOG:-info}"
# Backend-neutral draw capture (observes only; the D3D12 path is unchanged).
export VILKAN_DRAW_CAPTURE="${VILKAN_DRAW_CAPTURE:-}"
# Also capture a Table-root draw (the colour quad) with its descriptor-table and
# constant-buffer windows; and the first frame to capture from.
export VILKAN_DRAW_CAPTURE_TABLE="${VILKAN_DRAW_CAPTURE_TABLE:-}"
export VILKAN_DRAW_CAPTURE_FRAME="${VILKAN_DRAW_CAPTURE_FRAME:-}"
export VKD3D_DEBUG="${VKD3D_DEBUG:-warn}"
export DXVK_LOG_LEVEL="${DXVK_LOG_LEVEL:-warn}"

if [[ $# -eq 0 ]]; then
  set -- run "$(winpath "$TITLE_EXE")" --execute
fi

LOG="$VILKAN_DATA/vilkan-out.log"
BAT="$VILKAN_DATA/launch.bat"
{
  printf '@echo off\r\n'
  printf '"%s"' "$(winpath "$EXE")"
  for a in "$@"; do printf ' "%s"' "$a"; done
  printf ' > "%s" 2>&1\r\n' "$(winpath "$LOG")"
} > "$BAT"
: > "$LOG"

cd "$VILKAN_DATA"   # game-derived output must stay outside the repo
"$PROTON_DIR/proton" run cmd /c "$(winpath "$BAT")" &
pid=$!
tail -n +1 -f "$LOG" &
tpid=$!
trap 'kill $tpid 2>/dev/null || true' EXIT
wait $pid
rc=$?
sleep 0.3
kill $tpid 2>/dev/null || true
exit $rc