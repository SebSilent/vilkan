#!/usr/bin/env bash
# Cross-compile the Windows build of our loader from Linux.
#
# The title runs inside vilkan.exe (Windows PE); under Proton its host
# D3D12 calls go through VKD3D-Proton to Vulkan. A native Linux/Vulkan
# backend replaces this path later (docs/linux-proton.md).
#
# Needs: rustup (toolchain pinned by rust-toolchain.toml), cargo-xwin
# (`cargo install cargo-xwin --locked`), clang. The MSVC CRT + Windows SDK
# are downloaded by cargo-xwin on first use into ~/.cache/cargo-xwin.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

PROFILE="${PROFILE:-release}"
# The diagnostics feature carries the scripted-input injector (VICTORY_KEYS),
# the SC_CLOSE provenance and the event-relative input holds the acceptance
# harness drives the title with. Without it the title sits at the press-start
# menu forever (no input is injected). Default it ON; override with FEATURES=.
FEATURES="${FEATURES:-diagnostics}"
args=(xwin build --profile "$PROFILE" -p vilkan --target x86_64-pc-windows-msvc)
if [[ -n "$FEATURES" ]]; then args+=(--features "$FEATURES"); fi
cargo "${args[@]}"

echo
echo "built: target/x86_64-pc-windows-msvc/$PROFILE/vilkan.exe"