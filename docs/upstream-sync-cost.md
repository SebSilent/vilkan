# Upstream sync cost: the world-rendering commits

Scope: the upstream commits past our base `d222b87` that touch mesh shaders,
indirect draws, execute-indirect, or the world-rendering path. This is a
cost/conflict assessment only — **no fix is implemented here**.

Upstream is 180 commits past our base. The world-path subset:

| commit | subject (paraphrased) | files |
| --- | --- | --- |
| `35b054e` | pipelines off the render thread + cached; indirect execution default-on | 13 files, +1018/−101 |
| `3c81420` | drop draws whose fresh descriptor tables cannot be made; 2^18 dynamic slots | draw.rs, descriptor/host.rs |
| `15b0091` | shader loop-limit knob to diagnose slow world frames | shader_cache.rs, emit/lower.rs, emit/mod.rs |
| `0eca412` | diagnostics: skip pipelines by shader hash | draw.rs, pipeline.rs |
| `38ae55d` | keep the diagnostic skip outside indirect argument validation | draw.rs |
| `787f665` | pipeline staging cleanup on cache publication failure | victory-pipeline/cache.rs |

## Files we have functionally modified (vs base `d222b87`)

Measured by diffing our tree against the base blob of each file:

| file | our changed lines | what we changed |
| --- | --- | --- |
| gpu/draw.rs | 107 | draw-capture fn + per-draw oracle trigger (insertion at base 795–800) |
| gpu/capture.rs | 48 | per-draw readback + RT-state barrier fix |
| gpu/host.rs | 26 | wave64 feature query (base 187) + window-class brand renames |
| gpu/pipeline/host.rs | 24 | `dump_refused` fn (base 251) + 2 call sites (base 403, 477) |
| gpu/list.rs | 18 | comment-only alias renames |
| gpu/pipeline.rs | 9 | `container_bytes` accessor (base 104) + comment renames |
| shader/emit/mod.rs | ~30 | wave64→wave32 exchange (insertion at base 97) |
| shader/emit/lower.rs | ~150 | wave64 exchange lowering (base 201, 382, 2171, …) |
| gpu/indirect.rs, gpu/shadow.rs, gpu/descriptor/host.rs | 0 | untouched |

## Concrete conflict points

A conflict exists only where an upstream hunk's base-line range contains one
of our insertions/edits. Everything else auto-merges.

| file | our edit (base line) | upstream hunk (base range) | verdict |
| --- | --- | --- | --- |
| gpu/draw.rs | capture insert 795–800 | 35b054e nearest 818; 3c81420 nearest 696/988 | **clean** (≥18 lines clear) |
| gpu/host.rs | wave64 query 187; renames 1303/1680/1703 | 35b054e 254, 274, 412, 677, 790, 1280–1296 | **clean** |
| gpu/pipeline.rs | accessor 104; comment 457 | 35b054e 159+; 0eca412 207, 458–464 | **clean** (457 adjacent to 458, comment-only) |
| gpu/pipeline/host.rs | `dump_refused` call **403** | 35b054e **396–404** | **CONFLICT** (call inside rewritten hunk) |
| gpu/pipeline/host.rs | `dump_refused` call **477** | 35b054e **470–478** | **CONFLICT** (call inside rewritten hunk) |
| gpu/list.rs | alias comment 1132 | 35b054e rewrites the same execute-indirect comment 1129–1143 | **comment-vs-comment**, trivial |
| shader/emit/mod.rs | wave64 insert **97** | 15b0091 **95–101** | **CONFLICT** (insert inside hunk) |
| shader/emit/lower.rs | wave64 fields **201** | 15b0091 **201–214** | **CONFLICT** (insert inside hunk) |
| gpu/indirect.rs, shadow.rs, descriptor/host.rs | none | 35b054e / 3c81420 modify them | **clean take** (we have no edits) |

So the real, code-level conflicts are **four localized hunks in three files**:
`pipeline/host.rs` (2 call sites), `emit/mod.rs` (1), `emit/lower.rs` (1).
Each is our small addition landing inside a region upstream rewrote; resolution
is re-inserting our addition into upstream's new structure, not reconciling
divergent logic. The ~10 alias comment renames are re-applied mechanically.

The bulk of `35b054e` is new files we do not have (`pipeline/library.rs` +254,
`pipeline/make.rs` +238) and new code in `host.rs`/`shadow.rs` we never touched
— all clean takes.

## Merge effort estimate

- **Textual:** small. ~4 conflict hunks + ~10 comment renames. A focused
  half-day of careful merging.
- **Semantic (the real cost):** our capture/oracle and `dump_refused` assume
  *synchronous* pipeline creation. `35b054e` makes pipelines async and **drops
  a pipeline's draws until it is ready** (≤0.5 s wait per frame). After the
  merge the oracle trigger can fire on a dropped/empty draw, and `dump_refused`
  must hook the new worker-thread path (`pipeline/make.rs`) rather than the old
  inline creation. Re-verifying the wave64 dump and the capture against the
  async flow is the bulk of the work: **~1 day total** including re-test.
- `3c81420` changes `bind()` to return `bool` and drops draws whose tables fail;
  our capture reads `self.targets` after the draw, so a dropped draw yields an
  empty oracle. This is a semantic interaction to handle, not a textual conflict.

## Will indirect execution turning on change our dark-world outcome?

**No — unlikely, on primary evidence.** `35b054e`'s own commit message states,
verbatim in spirit: *"Not working yet: the world shows no geometry, and runs
end on a GPU page fault after at most about 55 s."* Upstream turned indirect
execution on (the console's type-105 single-pass dispatch now runs as a plain
dispatch instead of being dropped), moved pipeline creation off-thread, and
cached pipelines — and **still reports no world geometry**. The remaining
blocker (no geometry + GPU page fault) is downstream of indirect execution.

This matches our own observation: the menu/HUD renders (frame-256 carries
content) while the 3D world stays dark. Indirect execution is necessary to
submit the world's draws but is not sufficient to light them; merging it would
change *which* draws run, not fix the geometry/page-fault blocker. The evidence
is the upstream author's own statement in the commit message plus the
structural fact that the page fault persists after the change.

**Recommendation:** do not merge `35b054e` expecting a lit world. If we sync,
do it for the async-pipeline/caching infrastructure and the type-105 dispatch,
budgeting ~1 day and re-testing the capture path; treat the dark world as a
separate, still-open blocker.