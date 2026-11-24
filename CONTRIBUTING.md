# Contributing to VIlkan


We keep a **small, invite-only core team**. Unsolicited pull requests are **not**
the entry point and will generally not be reviewed. Access, task assignment, and
coordination all happen in Discord

Please do not open drive-by PRs or start large changes without talking to the
team first.

VIlkan is built on [VIctory](https://github.com/Marvisak/VIctory); see
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

### The MCS naming rule

The supported title is referred to only as **Miami Chores Simulator (MCS)**. The
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

### Before you commit

```bash
cargo test -p victory-shader -p victory-formats -p victory-core \
           -p victory-game -p victory-pipeline -p vilkan-vk --lib
cargo xtask policy
python3 scripts/scrub.py --check     # needs the local mapping
```

Do not push the old fork history; this tree is the clean origin. Pushing is
restricted to the core team and only on explicit authorization.
