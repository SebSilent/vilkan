# Contributing to VIlkan

VIlkan is built on [VIctory](https://github.com/raffclar/victory); see
[NOTICE](NOTICE) for attribution and what is ours.

## The no-game-assets rule

Nothing from the game is ever tracked: no assets, executable bytes, shaders,
archives, nor anything derived (converted data, disassembly, dumps,
screenshots), and no title/publisher/scene-identifying strings. `cargo xtask
policy` enforces this with **zero exceptions**. Game-derived data lives outside
the tree (`../vilkan-data`), and the data-dependent tests read it from env paths
and **skip cleanly** when absent — a clean clone with no game data builds and
passes the full suite.

## The scrub mapping is local-only and required for syncs

The game-name scrub (`scripts/scrub.py`) reads its identifying-string mapping
from `scripts/scrub-map.local.py`, which is **gitignored and local-only**. It is
**required for upstream syncs**: the ingestion gate depends on it, and the scrub
**fails loudly** if it is absent — it never silently no-ops, so a clean clone
without the mapping cannot appear to pass. Restore the mapping before syncing.

## Upstream sync workflow

See [docs/sync.md](docs/sync.md): pull upstream into the `victory-*` library
crates, merge (our functional changes are few and listed there), re-run
`scripts/scrub.py`, then verify `cargo xtask policy` and the suite. Keep our
branding (`vilkan.exe`, `vilkan-vk`, `exchange.rs`, scripts, docs) and the
upstream identifiers (`victory-*`, `VICTORY_*`, `victory.toml`) intact.

## Before you commit

```bash
cargo test -p victory-shader -p victory-formats -p victory-core \
           -p victory-game -p victory-pipeline -p vilkan-vk --lib
cargo xtask policy
python3 scripts/scrub.py --check     # needs the local mapping
```

Do not push the old fork history; this tree is the clean origin.