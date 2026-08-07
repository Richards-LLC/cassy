# cas-b129 M6 — Legacy decommission survey and flat-file end-state decision

Task: cas-7909 (M6 of EPIC cas-b129). Written after M5 cutover (cas-edee) closed
with the migration applied to the real stores.

Every claim below is cited to `file:line` in the tree at the commit this document
lands on, or to a query run against the live post-cutover stores. Nothing here is
inferred from the epic description; where the epic description turned out to be
wrong, this document says so and the code wins.

---

## 0. The headline, before the detail

**"Legacy decommission" cannot mean retiring the legacy memory store, and this
task does not attempt it.** Three independent facts, each verified, rule it out:

1. **The mapping spec deliberately keeps most rows as entries, permanently.**
   `Disposition::StayEntry` is defined as "Remains in `entries`, untouched"
   (`cas-cli/src/memory_migration/routing.rs:21`), and the migration performs no
   deletes (no `DELETE` in `cas-cli/src/memory_migration/apply.rs`). The M5 audit
   routed 438 rows to `stay-entry` and 1117 to `deliberately-leave` out of 1701
   (`.cas/migration/cas-b129/audit.json`). Legacy memory is a by-design permanent
   co-resident of the knowledge store, not a transitional staging area.

2. **Live counts confirm the asymmetry.** Post-cutover, read-only:

   | store | `knowledge_pages` | `entries` (archived=0) |
   |---|---|---|
   | project `.cas/cas.db` | 107 | 1252 |
   | global `~/.cas/cas.db` | 39 | 450 |

   146 pages against 1702 live entries. Decommissioning the legacy read path
   today would strand ~91% of retrievable rows.

3. **The knowledge read path is not yet a substitute** — see §2. Knowledge pages
   are unreachable from search, and the SessionStart knowledge index shows on the
   order of 10–12 of 146 pages.

So M6's removals are confined to **code that is provably superseded and provably
unreachable**, and the substantive M6 deliverable is this survey plus the four
recorded decisions in §5.

---

## 1. Correction: the removal candidate named in the task description is wrong

The task description asks to remove "the dead local semantic-search stub at
`hybrid.rs:599` and its scorer-weight allocation". **Do not do this.** Verified
against the code:

- `cas-cli/src/hybrid_search/hybrid.rs:599` is not a semantic stub. It is the
  code-channel multiplicative boost. Line numbers drifted after the knowledge
  channel landed.
- The semantic channel is not a dead stub. `semantic_search`
  (`hybrid.rs:831`) dispatches to a cloud-backed `SemanticChannel` when one is
  attached; local embeddings were removed earlier and the channel is cloud-only
  by design (`hybrid.rs:27`, `:219`). The `SemanticChannel` is defined over
  `KnowledgePage`, not `Entry` (`docs/migration/cas-b129-legacy-memory-inventory.md`
  §3.2) — it is *destination-system* machinery, the opposite of legacy.
- The "scorer-weight allocation" is the fix, not the bug. `channel_capabilities`
  → `SearchWeights::for_capabilities` (`hybrid.rs:559-565`) renormalizes weights
  over channels that can actually fire; the rationale comment at
  `cas-cli/src/hybrid_search/scorer.rs:138` records that without it a Conceptual
  query handed 0.60 to a channel returning nothing. Removing it reintroduces that
  bug.

**Classification: KEEP.**

---

## 2. Read-path state after cutover

### 2.1 The `search` tool does not reach knowledge pages

Call chain for `mcp__cas__search action=search`, hop by hop:

`mcp/tools/service/mod.rs:717` → `:725` `search_impl`
→ `mcp/tools/service/agent_search_system/search_context.rs:4`
→ `mcp/tools/core/search.rs:117` `cas_search_with_provenance`
→ `:136` `open_search_index()` (`mcp/server/mod.rs:309`, Tantivy at
`<cas_root>/index/tantivy`)
→ `:191` `search_unified` (`hybrid_search/search_index_query.rs:229`, BM25 only)
→ `:220` `store.get(&result.id)` against the **legacy entry store**.

It never constructs `HybridSearch`, and it never opens the knowledge store:
`open_knowledge_store` (`mcp/server/mod.rs:236`) is called only from the
`knowledge` tool (`mcp/tools/core/knowledge.rs:106, 146, 221, 391, 425`).
`DocType::KnowledgePage` exists (`hybrid_search/mod.rs:111`) but is never written
into or resolved out of the Tantivy index.

### 2.2 `HybridSearch`'s knowledge channel is unreachable in production

- The only production construction sites of `HybridSearch` anywhere are
  `cas-cli/src/hooks/scorer.rs:32` and `:38` (the SessionStart context scorer).
- `set_knowledge_store` (`hybrid.rs:356`) has one caller, the unit test at
  `hybrid.rs:1002`. `set_knowledge_store_from_path` (`:361`),
  `has_knowledge_store` (`:367`) and `set_semantic_channel_from_config` (`:382`)
  have zero callers of any kind.
- Every constructor sets `knowledge_store: None` (`hybrid.rs:242, 252, 264, 278,
  304`), so `knowledge_scores` (`:756`) returns empty on every production call.
- `HybridSearchOptions::enable_knowledge` defaults `false` (`hybrid.rs:115`) and
  the sole production caller does not set it (`hooks/scorer.rs:49-62`).

So the channel is doubly disabled, and `knowledge_weight: 0.25` (`hybrid.rs:121`)
is inert.

### 2.3 The SessionStart knowledge index is a truncated prefix

- `KNOWLEDGE_SECTION_TOKEN_BUDGET = 600`
  (`crates/cas-core/src/hooks/context/build_start.rs:22`), and the call site
  passes `min(remaining, 600) - 50` (`:763-773`) — at most 550 tokens.
- `render_knowledge_index` (`:42`) lists pages sorted deterministically and
  hard-`break`s when the next line would exceed the budget (`:70-72`). At roughly
  45–55 tokens per line (`KNOWLEDGE_SNIPPET_CHARS = 120`, `:25`), that admits
  about 10–12 pages of the 146 that exist.
- The outer byte budget is `SESSION_START_BUDGET_BYTES = 9 * 1024`
  (`cas-cli/src/hooks/handlers/session_budget.rs:38`).

### 2.4 The legacy memory read surfaces that remain load-bearing

Not exhaustive of the M1 inventory, but these are the ones that would break:

- `store.list_pinned()` — `build_start.rs:289` and
  `crates/cas-core/src/hooks/context/plan_mode.rs:52`. Rendered full-body,
  budget-exempt (`build_start.rs:287`), survives `minimal_start`, and is not in
  `DEGRADABLE_BASE_SECTIONS` (`session_budget.rs:50-57`) so it also survives the
  byte budget. The single most privileged read path in the system.
- `merge_entries` → `store.list()` (`crates/cas-core/src/hooks/context/mod.rs:526`)
  feeding `## Helpful Memories` (`build_start.rs:653-757`) and
  `## Related to Current Work` (`:784-800`).
- `HybridContextScorer` (`cas-cli/src/hooks/scorer.rs`) — the sole production
  consumer of `HybridSearch`.
- `memory` MCP actions `get` / `list` / `recent` / `remember`
  (`mcp/tools/core/memory.rs:583, 786, 705, 323`), MCP resources
  (`mcp/server/resources.rs:14-16`), MCP prompts (`mcp/server/prompts.rs:167,
  195, 291, 358`), statusline (`cli/statusline/data_and_format.rs`).
- `DocType::Entry` hydration in `core/search.rs:220` and
  `core/system.rs:386-388` (`context_for_subagent`).

**All of the above: STILL LOAD-BEARING. Untouched by this task.**

---

## 3. Flat-file end-state — CODEMAP.md and PRODUCT_OVERVIEW.md

### 3.1 Reader/writer enumeration (the evidence AC2 asks for)

**No code anywhere reads the contents of either file.** Every production touch is
metadata-only — `Path::exists()`, `fs::metadata().modified()`, or
`git log -- <path>`. There are exactly seven non-test path constructions:

| file | site |
|---|---|
| `.claude/CODEMAP.md` | `hooks/handlers/handlers_events/codemap.rs:361`, `:504`, `:671`; `cli/codemap_cmd.rs:36`, `:90` |
| `docs/PRODUCT_OVERVIEW.md` | `hooks/handlers/handlers_events/project_overview.rs:515` (const `DOC_PATH` at `:35`); `cli/project_overview_cmd.rs:43` |

The only `read_to_string` calls in those four modules read the **sidecar ledgers**
`.cas/codemap-pending.json` and `.cas/project-overview-pending.json`
(`codemap.rs:654`; `codemap_cmd.rs:97`, `:165`; `project_overview.rs:355`;
`project_overview_cmd.rs:94`), never the markdown.

**No code writes them either.** Generation is LLM-driven through two builtin
skills registered at `cas-cli/src/builtins.rs:314-323`; the instruction "Write to
`.claude/CODEMAP.md`" lives at `builtins/skills/codemap/SKILL.md:48` and "Write to
`docs/PRODUCT_OVERVIEW.md`" at `builtins/skills/project-overview/SKILL.md:53`
(each mirrored for codex and grok). The "view over the knowledge store" property
asserted by `cas-cli/docs/ARCHITECTURE.md:68` is **prose instruction inside the
skill body, not code** — there is no export function.

**Four gates depend on existence/mtime:**

| gate | site | behavior when the file is missing |
|---|---|---|
| SessionStart codemap banner | `handlers_session.rs:216-226` → `codemap.rs:348`, `:411-416` | `Missing` → `severity="high"`, prepended, fires every session until regenerated |
| SessionStart project-overview banner | `handlers_session.rs:229-243` → `project_overview.rs:509`, `:515-518` | `Missing` → `severity="high"` banner |
| PreToolUse hard deny on task-create / spawn-worker | `pre_tool.rs:291-328` | **Does not fire.** The arm at `:311-317` matches only `SignificantlyStale`; locked by tests at `:1917`, `:1946` |
| Stop reminder | `stop_flow.rs:622-662` → `codemap.rs:645-674` | Silent (`:671-674` early-returns `None`) |

Both CLI status commands degrade gracefully: `codemap_cmd.rs:40-43` and
`project_overview_cmd.rs:45-49` print "not found" and exit 0.

**Human/agent readers only:** `CLAUDE.md:51` and `README.md:253` — plain markdown
links; nothing parses them.

**Two empirical facts that settle the decision:**

- `docs/PRODUCT_OVERVIEW.md` **does not exist in this repo today**, and the system
  has been running with its high-severity banner active. The missing-file path is
  demonstrably non-fatal.
- `.gitignore:23` gitignores `/.claude/CODEMAP.md`, so
  `get_codemap_last_commit` (`codemap.rs:467-475`) returns `None` on every ref and
  freshness always falls through to the mtime branch (`:419-424` → `:503-509`).
  The "commit the codemap to reset staleness" procedure documented in the skill
  bodies at `SKILL.md:155-170` is inoperative here.

### 3.2 Decision: (a) thin auto-export — but **not yet**; keep the flat files, retire nothing

Between the two options the epic posed, the evidence chooses **(a) keep them as
generated views**, and explicitly rejects (b) retire-and-point-at-store *for now*.
Reasoning, from the enumeration above:

1. **(b) is not cheap, it is a rewrite.** Because nothing reads the contents, the
   flat files themselves cost nothing structurally — but four gates, two CLI
   subcommands, six SKILL.md bodies and ~18 code sites are wired to their
   *existence*. Retiring the files means deleting the gates too; leaving the gates
   means a permanent high-severity banner in every session.
2. **The store cannot carry the load today.** Pointing agents at the store instead
   of a file presumes the store is reachable. Per §2.1 and §2.3 it is reachable
   only through the `knowledge` MCP tool and a ~10-of-146-page truncated index.
   Retiring a file that agents actually read, in favour of a surface they can
   barely enumerate, is precisely the loss this epic promises not to incur.
3. **Nothing is currently broken.** The flat files are gitignored, cheap,
   regenerable, and already behave as views by convention.

**Recorded preconditions for revisiting (b):** knowledge pages reachable from
`mcp__cas__search`; a knowledge index that can enumerate all pages (or a paged
pull); and a measured comparison showing store retrieval is at least as good as
opening the file. Until all three hold, (b) stays closed.

**No flat file is removed by this task**, satisfying AC2's "no flat file removed
while any gate/harness still depends on it".

---

## 4. Inventory: classification of every candidate examined

Legend: **R** = remove-now, **L** = still load-bearing during transition,
**K** = keep (not legacy — destination-system or working code),
**D** = deferred, real finding but out of this task's scope.

| # | Path | Evidence | Class |
|---|---|---|---|
| 1 | `cas-cli/src/store/layered.rs` (609 lines) | Orphan: no `mod layered;` exists anywhere in `cas-cli` (only `crates/cas-store/src/lib.rs:47`). `crate::store::layered` resolves to the **re-exported** `cas_store::layered` (`cas-cli/src/store/mod.rs:132`), so this file is never in the module tree and never compiled. Superseded duplicate of `crates/cas-store/src/layered.rs` (32-line diff). | **R** |
| 2 | `cas-cli/src/store/markdown.rs` (586 lines) | Same orphan proof: `store/mod.rs` re-exports `cas_store::{… markdown}` at `:134` and declares no `mod markdown`. Header: "Legacy Markdown storage backend … the original file-based storage format with YAML frontmatter" — the pre-SQLite memory backend. Superseded duplicate of `crates/cas-store/src/markdown.rs`. | **R** |
| 3 | `crates/cas-search/src/hybrid.rs` + `crates/cas-search/tests/hybrid_integration.rs` + the `lib.rs:75-77` re-export | A second, unrelated `HybridSearch` (different fields, `semantic_score` hardcoded 0.0 at `:33`, `enable_semantic` documented as ignored at `:48`). `cas-cli` imports `cas_search` at four sites and none imports `HybridSearch`; the only consumers are its own tests. Superseded by `cas-cli/src/hybrid_search/hybrid.rs`. | **R** |
| 4 | `hybrid.rs:599` "dead local semantic stub" + weight allocation | §1 — misidentified; the line is the code boost, the semantic channel is live cloud machinery, the weight renormalization is a bug fix. | **K** |
| 5 | `HybridSearch::set_knowledge_store` `:356`, `set_knowledge_store_from_path` `:361`, `has_knowledge_store` `:367`, the `knowledge_scores` channel `:527, 551, 613, 646-664, 756` | Zero/production-zero callers (§2.2) — but this is **destination-system scaffolding**, the exact wiring the single-read flip needs. Deleting it removes the thing §5.2 is about. | **K** |
| 6 | `set_semantic_channel` `:376`, `set_semantic_channel_from_config` `:382`, `semantic::open_semantic_channel` | Same: unreached cloud wiring for the knowledge-side semantic channel, not superseded legacy. | **K** |
| 7 | `HybridSearch::open_full` `:323`, `has_reranker()` `:873` | Zero callers; `open_full`'s own doc says it is "now equivalent to `open_with_graph`". Genuinely dead, but they are search-API vestiges rather than memory paths superseded by knowledge — removing them belongs to a search-cleanup task with its own test pass. | **D** |
| 8 | `crates/cas-core/src/search/{index_ops,query_ops,scorer,metrics}.rs` | `cas-cli` defines its own `SearchIndex` and `search_unified` and imports only `cas_core::search::temporal` (`hybrid_search/mod.rs:64`). Likely a dead duplicate of the same shape as #3, but it has internal cross-references inside `cas-core` that this task did not exhaustively verify. Not removed on an unverified theory. | **D** |
| 9 | `cleanup_vector_files` `cas-cli/src/main.rs:269-296`, called `:263` | A one-shot janitor for removed local-embedding artifacts (`vectors.hnsw`, `models/`) that runs on **every** startup. Real cleanup candidate after a release window; unrelated to the knowledge migration. | **D** |
| 10 | Deprecated no-op embedding params `mcp/tools/types/system.rs:44, 51`; `crates/cas-mcp/src/types/ops_secondary.rs:217, 224` | Documented as ignored; removing them is an MCP schema change with client-compat implications. | **D** |
| 11 | 14 orphan migration files under `cas-cli/src/migration/migrations/` (`m035_entries_add_visibility`, `m036_entries_add_owner_id`, `m037_entries_idx_team_id`, `m061`–`m063`, `m082`–`m084`, `m125`–`m128`) | Never declared in `migration/migrations/mod.rs` (which declares only `m034_entries_add_team_id:42` and `m038_entries_add_last_reviewed:43` in that range) and never registered in the `MIGRATION` list at `:236-237`. Confirmed against the live schema: `entries` has `team_id` and `share` but **no `visibility`, no `owner_id`**. This is the root cause of finding #6 in the M1 inventory ("`share` vs `team_id` split-brain"). Dead, but a team-sharing concern, not a memory→knowledge one. | **D** |
| 12 | `store.list_pinned()`, `merge_entries`/`store.list()`, `HybridContextScorer`, `memory` MCP get/list/recent/remember, MCP resources & prompts, statusline, `DocType::Entry` hydration | §2.4 | **L** |
| 13 | `cas memory share`/`unshare`, `cas memory-migrate`, `cas retrieval-parity` | Team sharing is orthogonal to retrieval; the other two are the migration's own instrumentation and its rollback path. | **K** |
| 14 | `cas-cli/src/consolidation/` | Live: `daemon/decay.rs:82` calls `consolidate_all`, scheduled from `daemon/maintenance.rs`. Operates on legacy entries, which remain. | **L** |
| 15 | `.claude/CODEMAP.md`, `docs/PRODUCT_OVERVIEW.md` and their four gates | §3 | **K** |

Everything in **R** is *proposed* for removal and awaits an explicit supervisor
ruling — see §6; nothing has been removed. Nothing in **L** is touched.
Everything in **D** is written up as a follow-up rather than acted on, per the
repo's "don't ship a fix on a plausible-but-unconfirmed theory" rule.

---

## 5. The four decisions M5 carried forward

### 5.1 M4 parity harness is blind to the global store — CONFIRMED, fix specified

`cas retrieval-parity` resolves the global store with
`crate::config::global_cas_dir()` (`cas-cli/src/cli/retrieval_parity.rs:70`),
which is `dirs::config_dir()/cas` = `~/.config/cas`
(`cas-cli/src/config/access/global.rs:3`). On this host `~/.config/cas/cas.db`
**does not exist**; the live global store is `~/.cas/cas.db` (89 MB).
`ParityContext::with_global` filters any path lacking `cas.db`
(`cas-cli/src/retrieval_parity/mod.rs:161`) — **silently** — so the global tier
never participated and every green parity run to date covered the project store
only.

The correct resolver already exists and documents this exact inconsistency:
`host_cas_dir()` (`cas-cli/src/store/known_repos.rs:36`), whose comment at
`:17-23` states that `global_cas_dir()` "is **not** where the live host CAS state
actually lives" and that reconciling it was deferred.

**Decision: fix the harness to resolve the global tier via `host_cas_dir()`, and
make `with_global` loud rather than silent when handed a path with no `cas.db`.**
Not landed here: it changes what a parity run measures, so it needs its own
capture/replay pair rather than riding along with a removal commit. Filed as a
follow-up.

### 5.2 The single-read flip — REJECTED FOR NOW, with named preconditions

Re-verified: there is no `[knowledge]` config section and no read-path key
anywhere. The only memory-related config is `MemoryConfig`
(`cas-cli/src/config/settings.rs:911-919`) holding one field,
`session_learn_auto`, which is a **write**-path Stop-hook flag.

**Decision: do not build the flip, and do not treat it as pending plumbing.** A
flip presupposes two interchangeable read paths. Per §2 there is one read path
(legacy entries, BM25 + SessionStart injection) and one partial path (knowledge,
reachable only through its own MCP tool and a truncated index). Building a switch
now would let an operator turn off retrieval for ~91% of rows in exchange for a
surface that search cannot even reach.

**Preconditions before the flip is worth designing:** (1) knowledge pages indexed
into and resolvable from the unified search index, so `mcp__cas__search` returns
them; (2) `HybridSearch` actually given a knowledge store on the production path
(the `set_knowledge_store` wiring in **K** #5 exists precisely for this);
(3) the §5.3 measurement showing knowledge retrieval is at least at parity.

### 5.3 Knowledge-vs-memory retrieval quality is unmeasured — measurement is OWED

The parity harness answers "is the same knowledge still retrievable at the same
rank *from the legacy surfaces*" (`retrieval_parity/mod.rs:17-25`), and all nine
of its channels are legacy-memory channels — `Search`, `Recent`, `List`,
`Pinned`, `Helpful`, `ByType`, `ByTier`, `ByTag`, `SessionMerge`
(`retrieval_parity/queryset.rs:18-43`). A green replay proves the migration left
the legacy paths **undisturbed**. It says nothing about whether the knowledge
system retrieves as well.

**Decision: the measurement is owed, and it is a hard gate on any future removal
of a legacy read path.** It is not a gate on this task, because this task removes
no read path. Filed as a follow-up.

### 5.4 Flat-file end-state

See §3.2 — option (a), keep as views, with (b) gated on three named
preconditions.

---

## 6. Proposed Stage B removal set — NOT YET EXECUTED

**Nothing has been removed.** Stage B is gated on an explicit supervisor
remove-now ruling; this document is the Stage A deliverable that the ruling
should be made against.

Proposed remove-now set, each traceable to a row in §4:

- `cas-cli/src/store/layered.rs` — §4 #1
- `cas-cli/src/store/markdown.rs` — §4 #2
- `crates/cas-search/src/hybrid.rs`, `crates/cas-search/tests/hybrid_integration.rs`,
  and the corresponding `crates/cas-search/src/lib.rs` re-export — §4 #3

Nothing else is proposed. No flat file, no live read path, no config, no gate.

The set was executed once against a scratch commit purely to establish that it
compiles and tests clean, then reverted pending the ruling. Result on that
scratch run: `cargo check -p cas-search --all-targets` exit 0,
`cargo check -p cas --lib` exit 0 (4 pre-existing unrelated warnings),
`cargo test -p cas-search` exit 0, `cargo test -p cas --lib hybrid_search::`
exit 0 with 102 passed / 0 failed. So the removal is known-safe; only the
authorization is outstanding.

Follow-up task specs proposed: §5.1 parity-harness global resolution; §5.3
knowledge retrieval-quality measurement; §4 #7–#11 deferred dead code, of which
#11 (unregistered `visibility`/`owner_id` migrations) also explains a standing
finding in the M1 inventory.
