# VIlkan

VIlkan — Cuck Simulator Miami (Xbox), native on Linux.

**Discord: <https://discord.gg/K7EGDqvxpF>**

Cuck Simulator Miami (CSM), the Xbox Series X|S build, running **natively on
Linux** from files the user supplies. The title's x86-64 code runs unchanged
inside our process; we supply the console platform underneath it and convert
the title's data, ahead of time where it can and on first use where it must
(shaders). See [docs/architecture.md](docs/architecture.md).

## Backends: bootstrap now, native Vulkan next

The goal is a **native Linux process with our own Vulkan backend** — no
Proton, no D3D12. We get there in two stages:

1. **Bootstrap (today): D3D12 → VKD3D-Proton.** We cross-compile a Windows
   binary (`vilkan.exe`, MSVC target) and run it under **Proton**, so the host
   D3D12 it emits is translated to Vulkan by VKD3D-Proton. This reuses the
   loader, HLE, shader translation and formats, and changes only the host
   underneath. It is a **temporary bootstrap backend**, not the destination.
2. **Target: our own RDNA2 → SPIR-V Vulkan backend.** A native Linux
   device proxy behind the console D3D12 ABI that records the console's
   commands and replays them on Vulkan, with the console GPU code recompiled
   straight to SPIR-V. This drops Proton entirely.

The **wave64 exchange** (`crates/vilkan-shader/src/emit/exchange.rs`) is
**backend-neutral**: it emits no subgroup operations and derives lanes from
the workgroup-local index, so the same planner, slot layout and protocol
lower to DXIL today and will be **reused unchanged by the SPIR-V emitter**.
See [docs/wave64-exchange.md](docs/wave64-exchange.md) for the technique and
[docs/linux-proton.md](docs/linux-proton.md) for what must become true to
switch backends.

### First native execution (no Proton, no D3D12)

The `vilkan-vk` crate (ash) runs sidecar-compiled SPIR-V **natively on Linux**,
built with plain `cargo`. The shader compiler is a clean-room RDNA2 translator
reused offline as a sidecar — not ported, not reimplemented (its provenance and
license position are in a local, untracked note). The cross-half wave64
fixtures dispatch on the RTX 2070 (subgroup 32) with `--wave64-emulate` and
match the console reference numerically; both captured blur shaders compile to
validated wave64-emulated SPIR-V from the real header metadata. See
[docs/native-vulkan.md](docs/native-vulkan.md) for the contract and the
remaining blur-dispatch step (the bindless descriptor heap).

## Naming and merge-ability

VIlkan began as a fork of an upstream Xbox compatibility layer. To keep the
tree free of title and upstream branding, **everything we own is named
VIlkan**: the library crates (`vilkan-core`, `vilkan-shader`, `vilkan-gpu`,
…), the app crate/binary (`vilkan.exe`), the `VILKAN_*` environment variables,
the `vilkan.toml` config file and the `.vilkan-root` asset-policy marker.
Merges from upstream are reconciled by hand against this naming.

## Publishing

This tree **is the clean origin**: it was initialized fresh from the scrubbed
working tree with a single commit, no remotes, no tags and no prior history.
The earlier fork history (which named the title and upstream projects) was
moved aside to a private backup outside the tree and is never published. The
asset policy ([docs/asset-policy.md](docs/asset-policy.md)) keeps the tree
free of game-derived and identifying material; commit messages here carry no
title, publisher or upstream project names. Do not add a remote that points
at the old fork or push its history.

## Linux quick start (bootstrap)

```sh
cargo install cargo-xwin --locked   # once; also needs clang
scripts/build.sh                    # cross-compiles vilkan.exe (MSVC target)
# point at YOUR title dump (no path is hardcoded):
export GAME_DIR=/path/to/your/title/dump
scripts/run.sh                      # boots the title under Proton Experimental
```

The title path and data dir come from config/env: `GAME_DIR` (or
`VILKAN_GAME_DIR`, or `[paths].game_dir` in `vilkan.toml`), and `VILKAN_DATA`
for the prefix/cache/data root (default `../vilkan-data`, **outside** the
repo). `scripts/run.sh` finds the title executable in `GAME_DIR` (or takes
`VILKAN_TITLE_EXE`). Game-derived data never lives in the tree.

## Status

**Not playable yet.** On an NVIDIA RTX 2070 (wave32) under Proton: the title
boots through its first-boot screens and menus at 30 fps, loads the world and
stays up; the world picture is black (indirect draws and untranslated mesh
shaders — the next blocker). The wave64 blur computes that a wave32 host
refuses are lowered by the backend-neutral exchange and accepted (0 refusals,
0 crashes). See [docs/linux-proton.md](docs/linux-proton.md).

## Getting started

Requirements: a CPU with AVX2, a Vulkan GPU (the bootstrap runs the Windows
binary under Proton; Shader Model 6.6 and mesh shaders for the world), Rust
(pinned in `rust-toolchain.toml`), clang + cargo-xwin to cross-compile.

```sh
cargo xtask install-hooks                # asset-policy check on every commit
cp vilkan.example.toml vilkan.toml       # then set your paths (outside the repo)
cargo test --workspace                   # needs no game data
cargo run -p vilkan -- config            # check your paths
```

The full test suite passes on a clean clone with no game data: tests that
need real title files read their paths from environment variables and skip
when unset.

Run the title (bootstrap):

```sh
scripts/build.sh
GAME_DIR=/path/to/your/dump scripts/run.sh
```

The window caption shows the framerate. Saves, settings and `crashes.log`
are kept under the data folder, derived data under the cache folder (both
outside the repo).

Input: XInput pads. With none connected the keyboard is a pad while the
title's window has focus:

| Keys | Pad |
|------|-----|
| W A S D / I J K L | left / right stick |
| arrows | D-pad |
| Enter, Backspace, X, Y | A, B, X, Y |
| Q / E, Z / C | shoulders, triggers |
| F / H | stick clicks |
| Esc, Tab | Menu, View |

## Commands

```
vilkan inspect <exe|dll> [--imports] [--exports]
vilkan census [paths...] [--missing] [--strict]
vilkan lib-exports <import.lib> [--list]
vilkan shader-info [paths...] [--verbose] [--disasm]
vilkan translate [paths...] [--validate] [--pipelines] [--list] [--jobs N]
vilkan run <exe> [--execute] [--unimplemented trap|zero] [--strict]
            [--console series-x|series-s] [-- <title arguments>]
vilkan config
```

Logging: `VILKAN_LOG=vilkan_loader=trace,info` (a release build logs info and
above).

## Debugging

A release build has no debugging aids. Build them in with:

```sh
cargo build --profile diag -p vilkan --features diagnostics
```

| Variable | Effect |
|----------|--------|
| `VILKAN_LOG=<target>=debug,...` | debug and trace output |
| `VILKAN_STACKS=20,60` | every thread's stack at those seconds |
| `VILKAN_WATCH=<rva>,...` | logs a title u32 when it changes |
| `VILKAN_KEYS=30:Enter,35:X` | presses keys at those seconds, focus or not |
| `VILKAN_CRASH_SAVE=<rva>:<len>,...` | saves title memory to the cache folder on a crash |
| `VILKAN_CAPTURE=<dir>` | saves presented frames |
| `VILKAN_DUMP_TARGETS=1` | with the above, saves every render target of those frames |
| `VILKAN_D3D12_DEBUG=1` | host debug layer |
| `VILKAN_INDIRECT=1` | runs the title's indirect draws and dispatches |
| `VILKAN_D3D12_GBV=1` | with the above, GPU-based validation |
| `VILKAN_D3D12_DRED=1` | on a GPU failure, the allocation hit and commands in flight |
| `VILKAN_D3D12_WARP=1` | host device on the software adapter |
| `VILKAN_AUDIO_CAPTURE=<file.wav>` | records what the title plays |
| `VILKAN_XMA_DUMP=<dir>` | saves the first XMA voices' context and input |

Every build appends crashes and host GPU failures to `crashes.log` in the
title's data folder.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).