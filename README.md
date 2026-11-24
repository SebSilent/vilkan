# VIlkan — GTA VI Xbox, native on Linux

**VIlkan is built on [VIctory](https://github.com/raffclar/victory)** (the Xbox
Series X|S → Windows compatibility layer), with our own Linux runtime, wave64
work, and a native Vulkan translation path. The title's x86-64 code runs
unchanged inside our process; VIlkan supplies the platform underneath it. The
supported title is referred to only as **Cuck Simulator Miami (CSM)**. See
[NOTICE](NOTICE) for attribution and what is ours.

Two backends, one loader/HLE:

- **Proton bootstrap (temporary):** the Windows binary (`vilkan.exe`, host
  D3D12) runs under Proton → VKD3D-Proton → Vulkan. This is how we boot today.
- **Native Vulkan (the target):** our own RDNA2 → SPIR-V translation and a
  Linux-native Vulkan runtime (`vilkan-vk`), no Proton and no D3D12. Compute
  dispatch works; graphics is not implemented yet.

## Status — honest

**What works today**

- **Proton bootstrap boots and runs.** Menus, first-boot screens and the
  loading→world transition run at the title's 30 fps on an NVIDIA RTX 2070
  through Proton, with 0 shader refusals and 0 crashes in a verified boot.
- **wave64-on-wave32 exchange, with numeric proof.** Console wave64 compute
  runs on a 32-lane host via a backend-neutral round-scheduler exchange
  (`crates/victory-shader/src/emit/exchange.rs`). A CPU oracle checks slot
  indices and parity against console semantics, and native GPU tests check
  cross-half readlane/writelane numerically.
- **Native Vulkan compute dispatch on NVIDIA.** `vilkan-vk` (ash) dispatches
  sidecar-compiled SPIR-V directly on Linux. The two captured blur shaders run
  against a crafted bindless descriptor set and pass numeric box-filter checks
  (constant, impulse, gradient) on the RTX 2070 (subgroup 32).

**What does not work**

- **The world is dark.** Most scene geometry is drawn indirectly through mesh
  shaders we do not yet translate; those draws are dropped. This is the
  untranslated-mesh/indirect gap, not a crash.
- **No native graphics path yet.** `vilkan-vk` does compute only; there is no
  native Vulkan vertex/pixel/mesh pipeline or presentation. Graphics still goes
  through the Proton/D3D12 bootstrap.

**What you must supply yourself** (none of it is in this repo)

- **The title** — your own dump of Cuck Simulator Miami (Xbox Series X|S).
  Point `VILKAN_GAME_DIR` (or `GAME_DIR`) at its folder.
- **The external translator** — a clean-room RDNA2 → SPIR-V translator
  (GPL-3.0) for the native path, invoked as a sidecar. Set `TRANSLATOR_DIR`.
  We do not redistribute it; see [NOTICE](NOTICE) and `LOCAL-NOTES.md`.
- **Fixture paths** for the data-dependent tests: `VILKAN_VK_FIXTURES` (SPIR-V),
  `VILKAN_SHADER_FIXTURES` (captured containers), `VILKAN_BCPACK_TABLES`
  (BCPack tables). **Every step that needs these skips cleanly when they are
  absent**, so a clean clone with no game data builds and passes the suite.

## Build and run from a clean clone

Requirements: Rust (pinned in `rust-toolchain.toml`), a CPU with AVX2. For the
Proton path: Steam with a Proton runtime, and a Vulkan GPU. For the native path:
a Vulkan 1.3 driver, `llvm-mc` (AMDGPU target) and `spirv-tools`
(`spirv-as`/`spirv-val`), and the external translator.

```bash
# 1. Build and test the platform code. Needs NO game data and NO translator.
cargo test -p victory-shader -p victory-formats -p victory-core \
           -p victory-game -p victory-pipeline -p vilkan-vk --lib
cargo xtask policy        # the no-game-assets rule, zero exceptions
```

The fixture-dependent tests (captured shaders, BCPack tables, native dispatch)
**skip** here because their env paths are unset — that is expected, not a failure.

```bash
# 2. Proton bootstrap (needs the title + Proton). Cross-compile the Windows
#    binary, then run it under Proton.
cargo install cargo-xwin
rustup target add x86_64-pc-windows-msvc
scripts/build.sh                       # -> target/.../release/vilkan.exe
VILKAN_GAME_DIR=/path/to/title scripts/run.sh
```

`scripts/run.sh` takes the title path from `GAME_DIR`/`VILKAN_GAME_DIR` (or
`[paths].game_dir` in `victory.toml`); there is no hardcoded title path. It
bridges our `VILKAN_*` vars to the upstream `VICTORY_*` config the binary reads.
Without a title it exits with a clear message; nothing is bundled.

```bash
# 3. Native Vulkan compute (needs a Vulkan driver + the translator + fixtures).
cargo build -p vilkan-vk
# Compile a captured shader to SPIR-V with the external translator:
TRANSLATOR_DIR=/path/to/translator python3 scripts/compile-native.py <blob> <out>
# Run the numeric dispatch tests against compiled fixtures:
VILKAN_VK_FIXTURES=/path/to/spirv cargo test -p vilkan-vk
```

Without `TRANSLATOR_DIR` the compile step exits with a clear message; without
`VILKAN_VK_FIXTURES` the dispatch tests skip. The platform suite (step 1) always
passes regardless.

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

Logging: `VICTORY_LOG=victory_loader=trace,info` (a release build logs info and
above). Saves, settings and `crashes.log` live under the data folder; derived
data under the cache folder — both outside the repo (`../vilkan-data` by default).

## Input

XInput pads. With none connected the keyboard is a pad while the title's window
has focus:

| Keys | Pad |
|------|-----|
| W A S D / I J K L | left / right stick |
| arrows | D-pad |
| Enter, Backspace, X, Y | A, B, X, Y |
| Q / E, Z / C | shoulders, triggers |
| F / H | stick clicks |
| Esc, Tab | Menu, View |

## Debugging

A release build has no debugging aids. Build them in with

```powershell
cargo build --profile diag -p vilkan --features diagnostics
```

| Variable | Effect |
|----------|--------|
| `VICTORY_LOG=<target>=debug,...` | debug and trace output |
| `VICTORY_STACKS=20,60` | every thread's stack at those seconds |
| `VICTORY_WATCH=<rva>,...` | logs a title u32 when it changes |
| `VICTORY_KEYS=30:Enter,35:X` | presses keys at those seconds |
| `VICTORY_CRASH_SAVE=<rva>:<len>,...` | saves title memory on a crash |
| `VICTORY_CAPTURE=<dir>` | saves presented frames |
| `VICTORY_DUMP_TARGETS=1` | with capture, saves every render target |
| `VICTORY_D3D12_DEBUG=1` | host debug layer |
| `VICTORY_D3D12_WARP=1` | host device on the software adapter |

## Naming and merge-ability

The **library crates are upstream's `victory-*`**, the config/marker files are
`victory.toml`/`.victory-root`, and the code reads the upstream `VICTORY_*` env
vars — kept identical to upstream so updates merge cleanly. **VIlkan branding is
ours:** the app/binary `vilkan.exe`, the window/adapter strings, `vilkan-vk`,
`exchange.rs`, the scripts, docs, and our user-facing env vars (`VILKAN_DATA`,
`VILKAN_GAME_DIR`, `VILKAN_TITLE_EXE`, `VILKAN_SHADER_FIXTURES`,
`VILKAN_BCPACK_TABLES`, `VILKAN_VK_FIXTURES`). See [docs/sync.md](docs/sync.md)
for the upstream-sync workflow and [CONTRIBUTING.md](CONTRIBUTING.md).

## Publishing

This tree **is the clean origin**: initialized fresh from the scrubbed working
tree, no remotes, no tags, no prior fork history. The asset policy
([docs/asset-policy.md](docs/asset-policy.md)) keeps the tree free of
game-derived and identifying material; commit messages carry no title,
publisher or upstream-project names beyond the credited attribution. Do not add
a remote that points at the old fork or push its history.