# Syncing with upstream VIctory

VIlkan is built on VIctory. The library crates keep their upstream names
(`victory-*`) so upstream updates merge cleanly. This is the workflow.

## Layout

- **Upstream-derived** (`crates/victory-*`, `victory.toml`, `.victory-root`,
  `tests/fixtures/hle-probe`): upstream code. Most files are byte-identical to
  our fork base plus the game-name scrub. A few carry our functional changes
  (see [NOTICE](../NOTICE)): the wave64 exchange wiring in `victory-shader`
  (`emit/lower.rs`, `emit/mod.rs`), the host wave-width query and refused-shader
  dump in `victory-gpu` (`host.rs`, `pipeline/host.rs`), external BCPack tables
  in `victory-formats/bcpack.rs`, and the zero-exception policy in `xtask`.
- **Ours** (`apps/vilkan`, `crates/vilkan-vk`, `emit/exchange.rs`, `scripts/`,
  `docs/`, `README.md`, `NOTICE`): branded VIlkan, never overwritten by a sync.

## Workflow

1. **Pull upstream.** Fetch the upstream VIctory clone and merge its changes
   into the `victory-*` crates. Our functional changes are localized to the
   files listed in NOTICE; resolve conflicts there by keeping our functional
   edits and taking upstream's surrounding code.
2. **Re-run the scrub.** `python3 scripts/scrub.py` strips the title,
   publisher, release/scene tags and title-identifying strings that upstream
   prose reintroduces. It is idempotent and safe to re-run; `--check` reports
   what would change without writing. **The identifying-string mapping is
   local-only** (`scripts/scrub-map.local.py`, gitignored) and **required**: the
   scrub fails loudly if it is absent, never silently no-ops, so a clean clone
   without the mapping cannot appear to pass the ingestion gate. Restore the
   mapping before syncing.
3. **Verify the policy.** `cargo xtask policy` must pass with **zero
   exceptions** — nothing game-derived may be tracked at any point. If a sync
   brings in a binary asset or a title-identifying string, remove it (move
   game-derived data to the external data dir) before committing.
4. **Build and test.** Both targets: `cargo xwin build --release -p vilkan
   --target x86_64-pc-windows-msvc` and the Linux crates
   (`cargo test -p victory-shader -p victory-formats -p victory-core
   -p victory-game -p victory-pipeline -p vilkan-vk`). The suite passes
   without fixtures; captured-shader tests skip when their env path is unset.
5. **Boot check.** One Proton boot (`scripts/run.sh`) must show 0 refusals and
   0 crashes.

## Rules

- Nothing game-derived is ever tracked. The asset policy
  ([asset-policy.md](asset-policy.md)) runs with zero exceptions.
- The game-name scrub keeps the supported title referred to only as "Cuck
  Simulator Miami" (CSM). Project names (VIctory) are credited in NOTICE, not
  scrubbed.
- Do not push this fork's history to a public remote; the public repo is
  initialized fresh from the scrubbed tree (see README, Publishing).