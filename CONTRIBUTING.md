# Contributing to VIlkan

**The way in is the Discord, not a pull request.**

➤ **Join the server: <https://discord.gg/K7EGDqvxpF>**

We keep a **small, invite-only core team**. Unsolicited pull requests are **not**
the entry point and will generally not be reviewed. Access, task assignment, and
coordination all happen in Discord:

1. **Join the Discord** and read the instructions pinned there.
2. **Get coordinated through the organization** — introduce yourself, see what
   the team is working on, and agree on a task before writing any code.
3. **Get access and a task assigned in Discord.** Work is handed out and tracked
   there; once you have a task and the go-ahead, you contribute against it.

Please do not open drive-by PRs or start large changes without talking to the
team first. The Discord is where scope, priorities, and access are decided.

VIlkan is built on [VIctory](https://github.com/raffclar/victory); see
[NOTICE](NOTICE) for attribution and what is ours.

## Technical requirements (once you are working with the team)

### The no-game-assets rule

Nothing from the game is ever tracked: no assets, executable bytes, shaders,
archives, nor anything derived (converted data, disassembly, dumps,
screenshots), and no title/publisher/scene-identifying strings. `cargo xtask
policy` enforces this with **zero exceptions**. Game-derived data lives outside
the tree (`../vilkan-data`), and the data-dependent tests read it from env paths
and **skip cleanly** when absent — a clean clone with no game data builds and
passes the full suite.

### The CSM naming rule

The supported title is referred to only as **Cuck Simulator Miami (CSM)**. The
real game name, publisher, release/scene tags, and any title-identifying string
are kept out of the tree, commit messages, and documentation.

### The scrub mapping is local-only and required for syncs

The game-name scrub (`scripts/scrub.py`) reads its identifying-string mapping
from `scripts/scrub-map.local.py`, which is **gitignored and local-only**. It is
**required for upstream syncs**: the ingestion gate depends on it, and the scrub
**fails loudly** if it is absent — it never silently no-ops, so a clean clone
without the mapping cannot appear to pass. Restore the mapping before syncing.

### The ingestion gate

Every change passes the asset policy and the identifying-string sweep before it
is committed. `cargo xtask policy` runs with **zero exceptions**; the scrub
(`scripts/scrub.py --check`) must report nothing to change. Nothing game-derived
or title-identifying may end up tracked at any point.

### Upstream sync workflow

See [docs/sync.md](docs/sync.md): pull upstream into the `victory-*` library
crates, merge (our functional changes are few and listed there), re-run
`scripts/scrub.py`, then verify `cargo xtask policy` and the suite. Keep our
branding (`vilkan.exe`, `vilkan-vk`, `exchange.rs`, scripts, docs) and the
upstream identifiers (`victory-*`, `VICTORY_*`, `victory.toml`) intact.

### Before you commit

```bash
cargo test -p victory-shader -p victory-formats -p victory-core \
           -p victory-game -p victory-pipeline -p vilkan-vk --lib
cargo xtask policy
python3 scripts/scrub.py --check     # needs the local mapping
```

Do not push the old fork history; this tree is the clean origin. Pushing is
restricted to the core team and only on explicit authorization.