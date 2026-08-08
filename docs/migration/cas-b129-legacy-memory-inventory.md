# cas-b129 M1 — Legacy memory surface inventory

Task: **cas-13aa** (Phase 1 of epic **cas-b129**, Memory → knowledge migration).
Produced: 2026-08-07. All database access was **read-only** (`file:<path>?mode=ro`);
every query is recorded in the `search_manifest` on task close and reproduced in
[Appendix A](#appendix-a--queries-run).

## Scope of this document

Enumerates every surface of the legacy memory store (`entries` table + its
satellites, its read paths, and its non-SQL backends) so that M2's mapping spec is
inventory-driven. Each surface carries:

- **owning schema + code** (`file:line`)
- **live row counts** from the two real databases on this machine
- **read paths** that consume it
- **disposition** — an empty placeholder for M2 to fill

Databases inventoried:

| Label | Path |
|---|---|
| **GLOBAL** | `~/.cas/cas.db` |
| **PROJECT** | `/home/pippenz/Petrastella/cas-src/.cas/cas.db` (451 MB) |

## Headline totals

| | GLOBAL | PROJECT |
|---|---:|---:|
| `entries` rows (all) | **450** | **1245** |
| `archived = 1` | 0 | 0 |
| `created` range | 2026-03-16 → 2026-04-24 | 2026-03-16 → 2026-08-07 |
| `knowledge_pages` (destination system) | 0 | 0 |

**Total legacy rows on this machine: 1695.** The destination (`knowledge_pages`)
is empty in both DBs, so M3 starts from a clean target.

> **Revised by M2 (cas-e311).** Row count is not corpus size. M2 found that
> **994 of these rows (58.6%) are integration-test fixtures** — exactly five
> literal content strings duplicated 143–160× each — leaving a genuine migration
> corpus of **702 rows**. M2 also found cross-project contamination concentrated
> in the high-importance band. Read
> [`cas-b129-mapping-spec.md` §3](./cas-b129-mapping-spec.md) before using any
> count in this document to size the migration. The per-surface counts below are
> unchanged and correct as raw column statistics.

---

## 1. Surfaces named in the cas-b129 epic description

### 1.1 Entry types — `entries.type`

- **Schema**: `entries.type TEXT NOT NULL DEFAULT 'learning'`
- **Code**: `crates/cas-types/src/entry.rs:16` (`pub enum EntryType`), field at `crates/cas-types/src/entry.rs:224`+ (`Entry`)
- **Counts**

  | value | GLOBAL | PROJECT |
  |---|---:|---:|
  | `learning` | 374 | 1095 |
  | `context` | 70 | 123 |
  | `preference` | 3 | 18 |
  | `observation` | 3 | 9 |

- **Read paths**
  - SessionStart filter: `crates/cas-core/src/hooks/context/build_start.rs:668-672` — `Observation` entries are dropped unless `feedback_score() > 0`; all other types pass.
  - Basic scorer type weight: `crates/cas-core/src/hooks/context/mod.rs:~160-184` (`BasicContextScorer::calculate_score`).
  - MCP `memory action=list` / `recent`: `cas-cli/src/mcp/tools/core/memory.rs:786`, `:705`.
  - Learning-review hook (type = `learning` only): `crates/cas-store/src/sqlite/store_entry_queries.rs:95` → `cas-cli/src/hooks/handlers/handlers_middle/session_stop/mod.rs:176`.
- **Disposition (M2)**: `merge-into` page_type + `carry-verbatim` as `cas_legacy_type`. Drives row routing R3/R5/R7/R8/R9 in the M2 spec; original value preserved verbatim so the mapping is reversible. See [mapping spec §5.1](./cas-b129-mapping-spec.md).

### 1.2 Memory tiers — `entries.memory_tier`

- **Schema**: `entries.memory_tier TEXT NOT NULL DEFAULT 'working'`; index `idx_entries_memory_tier`
- **Code**: `crates/cas-types/src/entry.rs:162` (`pub enum MemoryTier` — `InContext`, `Working`, `Cold`, `Archive`), field at `crates/cas-types/src/entry.rs:262`; parser accepts `pinned`/`core`/`hot`/`warm`/`archived` aliases at `crates/cas-types/src/entry.rs:198-218`.
- **Counts**

  | tier | GLOBAL | PROJECT |
  |---|---:|---:|
  | `in_context` | **0** | **0** |
  | `working` | 107 | 62 |
  | `cold` | 14 | 0 |
  | `archive` | 329 | 1183 |

  Tier × type cross-tab (PROJECT): `archive/learning` 1043, `archive/context` 114,
  `archive/preference` 17, `archive/observation` 9, `working/learning` 52,
  `working/context` 9, `working/preference` 1.

- **Read paths**
  - `store_list` (`crates/cas-store/src/sqlite/store_entry_crud.rs:267`) — **does not filter on tier**: `WHERE archived = 0 ORDER BY created DESC LIMIT 10000`. Consequence: the 95%-archive-tier population still flows into SessionStart scoring.
  - `store_list_decayable` (`.../store_entry_crud.rs:312`) — excludes `in_context` and `archive`.
  - `store_list_pinned` (`crates/cas-store/src/sqlite/store_entry_queries.rs:38`) — `in_context` only.
- **Finding — `LIMIT 10000` ceiling**: `store_list` caps at 10 000 rows. PROJECT is at 1245, so no truncation today, but the cap is a silent data-loss edge for any migration that reads via `Store::list()` instead of raw SQL. **M3 must not use `Store::list()` as its extraction read.**
- **Disposition (M2)**: `carry-verbatim` as `cas_legacy_memory_tier`; **does not gate migration**. `memory_tier` is the authoritative cold signal (the `archived` flag is 0 everywhere); but since no read path filters tier, archive-tier rows are behaviourally live and are migrated like any other. Destination has no tier column. See [mapping spec §5.1a](./cas-b129-mapping-spec.md).

### 1.3 Importance / stability / decay state

- **Schema**: `entries.stability REAL NOT NULL DEFAULT 0.5`, `entries.importance REAL NOT NULL DEFAULT 0.5`, `entries.access_count INTEGER NOT NULL DEFAULT 0`, `entries.last_accessed TEXT`
- **Code**: `crates/cas-types/src/entry.rs:304` (stability), `:313` (importance); decay math at `crates/cas-types/src/entry/behavior.rs:324` (`retrievability`), `:342` (`relevance_score`), `:359` (`reinforce`), `:374` (`apply_decay`), `:481` (`should_prune`); tier movement at `behavior.rs:265` (`demote_tier`) / `:277` (`promote_tier`).
- **Counts**

  | | GLOBAL | PROJECT |
  |---|---:|---:|
  | `importance <> 0.5` (non-default) | 130 | 304 |
  | `stability <> 0.5` (non-default) | 417 | 1192 |
  | `access_count > 0` | 32 | 91 |
  | `last_accessed NOT NULL` | 32 | 91 |

  Importance buckets (`floor(importance*10)`), PROJECT: `0.4`→1, `0.5`→942,
  `0.6`→72, `0.7`→21, `0.8`→168, `0.9`→35, `1.0`→6.
  Stability buckets, PROJECT: `0.1`→1160, `0.3`→9, `0.4`→9, `0.5`→59, `0.6`→5,
  `0.7`→2, `0.8`→1. GLOBAL: `0.1`→338, `0.2`→4, `0.3`→35, `0.4`→39, `0.5`→33, `0.7`→1.

  **Interpretation**: stability has been driven to the floor (~0.1) for 93% of PROJECT
  rows by the decay daemon. Importance is largely untouched default (76%). Any
  M2 disposition that keys on stability as a quality signal will be reading
  decay-time-elapsed, not human judgement.

- **Read paths**
  - Decay daemon: `cas-cli/src/daemon/decay.rs:14` (`list_decayable`), `:53` (`apply_decay(days/30)`), `:151` (`list_prunable(0.1)`); enabled by `cas-cli/src/daemon/types.rs:21,55` (`apply_decay: true` default), driven from `cas-cli/src/daemon/maintenance.rs:52`.
  - SessionStart scoring: `crates/cas-core/src/hooks/context/mod.rs:~160-184` multiplies `importance_boost * stability_boost * access_boost * age_decay`.
  - Hybrid blend: `cas-cli/src/hooks/scorer.rs:74-125` — 70% hybrid score, 30% normalized basic score (which carries the importance/stability signal).
  - MCP `memory action=set_tier`: `cas-cli/src/mcp/tools/service/core.rs:156` → `cas-cli/src/mcp/tools/core/system.rs:201`.
- **Disposition (M2)**: `carry-verbatim` as `cas_legacy_importance` / `_stability` / `_access_count` / `_last_accessed`. Provenance only — explicitly **not** a routing signal, because importance is 76% untouched default and its high band is contaminated by another project's rows. See [mapping spec §5.1, §3](./cas-b129-mapping-spec.md).

### 1.4 Opinions and reinforce / weaken / contradict history

- **Schema**: `entries.belief_type TEXT NOT NULL DEFAULT 'fact'`, `entries.confidence REAL NOT NULL DEFAULT 1.0`, plus `helpful_count` / `harmful_count`.
- **Code**: `crates/cas-types/src/entry.rs:34` (`pub enum BeliefType`), `:346` / `:356` (fields); constructors `crates/cas-types/src/entry/behavior.rs:54` (`new_opinion`), `:92` (`new_hypothesis`); mutators `behavior.rs:417` (`reinforce_confidence`), `:434` (`weaken_confidence`), `:451` (`contradict_confidence`), `:462` / `:470` (belief promotion).
- **Counts**

  | belief_type | GLOBAL | PROJECT |
  |---|---:|---:|
  | `fact` | 450 | 1244 |
  | `hypothesis` | 0 | **1** |
  | `opinion` | **0** | **0** |

  `confidence <> 1.0`: GLOBAL 0, PROJECT 1. `helpful_count > 0`: GLOBAL 0,
  PROJECT 4. `harmful_count > 0`: 0 in both.

- **🔴 CRITICAL FINDING — the "history" does not exist.**
  `cas_opinion_reinforce` / `_weaken` / `_contradict`
  (`cas-cli/src/mcp/tools/core/opinion.rs:9`, `:60`, `:108`) accept an `evidence`
  string, but **never persist it**. Each handler mutates only scalars —
  `entry.confidence`, `helpful_count`/`harmful_count`, and (on contradict below
  0.1 confidence) `archived` — then writes the entry back. The evidence text is
  used exactly once, to build the human-readable response message:
  `opinion.rs:54`, `opinion.rs:102`, `opinion.rs:155`
  (`msg.push_str(&format!("\nEvidence: {}", truncate_str(&req.evidence, 100)))`).
  There is **no opinion-history table** in either database, and no append to the
  entry body. So the epic's "opinions with reinforce/weaken/contradict history"
  surface reduces to three scalar columns; the audit trail was never recorded and
  cannot be recovered by migration. M2 must scope this surface as
  *scalars-only* and, if history is wanted in the destination system, treat it as
  net-new capability rather than a carry-over.
- **Secondary finding**: with 0 opinions and 1 hypothesis across 1695 rows, this
  surface is effectively unused in production. Migration risk here is
  near-zero regardless of the disposition chosen.
- **Disposition (M2)**: Rows: `stay-entry` (routing rule R4) — pages cannot represent confidence decay. Scalars `belief_type`/`confidence`: `carry-verbatim`. Reinforce/weaken/contradict **evidence**: `deliberately-leave` — it was never persisted, so there is nothing to carry (loss is zero). Escalated as E2. See [mapping spec §5.1, §10](./cas-b129-mapping-spec.md).

### 1.5 Pins (in-context tier)

- **Schema**: no dedicated column — a pin is `memory_tier = 'in_context'`.
- **Code**: `crates/cas-types/src/entry/behavior.rs:287` (`pin`), `:292` (`unpin`), `:299` (`is_pinned`); query `crates/cas-store/src/sqlite/store_entry_queries.rs:38` (`store_list_pinned`); trait `crates/cas-store/src/lib.rs:312`; wrappers `cas-cli/src/store/notifying_entry.rs:125`, `cas-cli/src/store/syncing_entry.rs:193`.
- **Counts**: **0** pinned entries in GLOBAL, **0** in PROJECT.
- **Read paths** (all currently no-ops for lack of data)
  - SessionStart "📌 Pinned Memories (Always Active)" — `crates/cas-core/src/hooks/context/build_start.rs:287-311`. Pinned entries are the **only** entries injected with full body; everything else is a preview (regression-guarded at `crates/cas-core/src/hooks/context/tests.rs:454-481`). Pins also survive `minimal_start` (`build_start.rs:313`).
  - Plan mode "📌 Critical Context (Pinned)" — `crates/cas-core/src/hooks/context/plan_mode.rs:50-56`.
  - CLI AI-selection context — `cas-cli/src/hooks/context.rs:405`.
  - Statusline — `cas-cli/src/cli/statusline/data_and_format.rs:92`.
- **Finding — pins are unreachable from the MCP surface.** The `memory` tool
  advertises `set_tier (working/cold/archive)` only
  (`cas-cli/src/mcp/tools/service/mod.rs:193`), so an agent cannot create a pin
  through the documented API even though `MemoryTier::from_str` accepts
  `pinned`/`core`/`in_context` (`crates/cas-types/src/entry.rs:198-206`). This is
  the likely cause of the 0/1695 pin count, and it means the highest-leverage
  read path in the whole legacy system has never carried data.
- **Disposition (M2)**: `carry-verbatim` → **locked** knowledge page (routing rule R2). Locked bit set via `set_locked` after `commit_ingest`; an unlocked carry-verbatim page fails the migration run. See [mapping spec §8.1](./cas-b129-mapping-spec.md).

### 1.6 Team-share flags

- **Schema**: `entries.team_id TEXT` (index `idx_entries_team_id`) and `entries.share TEXT` (added by a later ALTER — it is the trailing `, share TEXT` in the table DDL).
- **Code**: `crates/cas-types/src/entry.rs:367` (`team_id`), `:376` (`share: Option<crate::scope::ShareScope>`); scope types at `crates/cas-types/src/scope.rs:18` (`Scope`), `:90` (`ScopeFilter`).
- **Counts**

  | | GLOBAL | PROJECT |
  |---|---:|---:|
  | `team_id NOT NULL` | 0 | **626** |
  | `share NOT NULL` | **0** | **0** |

- **Finding — split-brain sharing state.** Half the PROJECT rows (626/1245, 50.3%)
  carry a `team_id`, but **not one row in either DB has a non-NULL `share`**. The
  two columns encode overlapping intent (team auto-promote vs. explicit share
  scope) and only the older one has ever been written. M2 must decide which is
  authoritative before M3 maps either; naively carrying `share` forward would
  migrate 1695 NULLs and silently drop the 626 real `team_id` associations.
- **Root cause (cas-0955, resolved).** This is not data loss and not a
  half-migrated column: the visibility half of the sharing model was written
  but never wired up. Thirteen migration files (`m035`/`m036`/`m037`,
  `m061`-`m063`, `m082`-`m084`, `m125`-`m128`) adding `visibility`, `owner_id`,
  `collaborators` and duplicate `team_id` indexes had no `mod` declaration and
  no `MIGRATIONS` entry, so they never compiled and never ran — `entries` has
  no `visibility` and no `owner_id` column in any database, and no Rust code
  has ever read one. `share` (`ShareScope`, `m195`-`m198`) is the design that
  actually shipped; it is simply unused. The orphan files were deleted and a
  guard test now fails if any migration file is unregistered.
- **Read paths**: `Scope`/`ScopeFilter` filtering in `crates/cas-store/src/sqlite/store_entry_crud.rs:285` (`store_list_by_scope_and_tag`); cloud sync queue (see §2.15).
- **Disposition (M2)**: `team_id`: `carry-verbatim` as `cas_legacy_team_id`, **authoritative**. `share`: `deliberately-leave` — dead column, 0 rows, never written. M3 must not synthesize `share` from `team_id`. Team scoping is unenforced in the destination — escalated as E3. See [mapping spec §8.2](./cas-b129-mapping-spec.md).

### 1.7 Temporal validity windows — `valid_from` / `valid_until`

- **Schema**: `entries.valid_from TEXT`, `entries.valid_until TEXT` (plus the related `review_after TEXT`).
- **Code**: `crates/cas-types/src/entry.rs:318`, `:323`, `:328`; behavior `crates/cas-types/src/entry/behavior.rs:225` (`is_temporally_valid`), `:246` (`is_expired`), `:255` (`set_validity`).
- **Counts**

  | | GLOBAL | PROJECT |
  |---|---:|---:|
  | `valid_from NOT NULL` | 0 | **0** |
  | `valid_until NOT NULL` | 0 | **1** |
  | `valid_until` already expired | 0 | 0 |
  | `review_after NOT NULL` | 0 | 0 |

- **Finding — validity is defined but not enforced on the hot read path.**
  `is_temporally_valid` / `is_expired` exist on `Entry`, but the SessionStart
  entry pull (`store_list`, `crates/cas-store/src/sqlite/store_entry_crud.rs:267`)
  filters on `archived = 0` only — no `valid_until` predicate — and
  `build_start.rs:668-672` filters only on entry type. An expired entry would
  still be injected. With 1 row using the feature this is currently harmless, but
  M2 should not assume the legacy system honored these windows.
- **Disposition (M2)**: `carry-verbatim` as `cas_legacy_valid_from` / `_valid_until` / `_review_after`. Destination has no expiry enforcement — but neither did the legacy read path, so this is parity, not regression. See [mapping spec §5.1, §9](./cas-b129-mapping-spec.md).

---

## 2. NEW-FOUND surfaces — present in code/DB, absent from the cas-b129 description

Each of these is real state on the legacy entry that the epic description does not
enumerate. Flagged per acceptance criterion (4).

| # | Surface | Schema / code | GLOBAL | PROJECT | Notes | Disposition (M2) |
|---|---|---|---:|---:|---|---|
| 2.1 | **NEW-FOUND** `belief_type` + `confidence` as general fields (beyond opinions) | `crates/cas-types/src/entry.rs:34`, `:346`, `:356` | 450 fact | 1244 fact / 1 hypothesis | Epic mentions opinions only; the enum also carries `Fact` and `Hypothesis` and every row has a `confidence` | `carry-verbatim` (`cas_legacy_belief_type`, `_confidence`); routing use is R4 only |
| 2.2 | **NEW-FOUND** `archived` flag, distinct from `memory_tier='archive'` | `entries.archived`; `crates/cas-store/src/sqlite/store_entry_crud.rs:384`, `:395`, `:403` | 0 | 0 | Two independent "archive" concepts. Every read path filters `archived = 0`; nothing filters tier. 1512/1695 rows are tier-archive but flag-live | `carry-verbatim` (both); ambiguity resolved — tier is authoritative, flag selects nothing, neither gates migration (spec §5.1a) |
| 2.3 | **NEW-FOUND** `observation_type` | `crates/cas-types/src/entry.rs:73`; `entries.observation_type`; `idx_entries_obs_type` | 0 non-NULL | 1 (`general`) | Sub-classification of `observation` entries | `carry-verbatim` as `cas_legacy_observation_type` |
| 2.4 | **NEW-FOUND** feedback counters `helpful_count` / `harmful_count` | `entries.*`; MCP `cas-cli/src/mcp/tools/core/memory.rs:623`, `:653`; index `idx_entries_helpful_score` | 0 / 0 | 4 / 0 | Drives `feedback_score()` (`behavior.rs:304`) and `list_helpful` (`store_entry_queries.rs:56`) → `cas-cli/src/hooks/context.rs:399` | `carry-verbatim` **and routing signal** — rule R8 promotes `helpful>harmful` learnings to pages; the only human-confirmed durability signal in the corpus |
| 2.5 | **NEW-FOUND** access telemetry `access_count` / `last_accessed` | `entries.*` | 32 / 32 | 91 / 91 | Feeds decay + scoring | `carry-verbatim` (`cas_legacy_access_count`, `_last_accessed`); no destination engine consumes it |
| 2.6 | **NEW-FOUND** `title` | `entries.title` | 212 | 429 | Only ~1/3 of rows have one; the rest fall back to `preview(60)` (`build_start.rs:293`) | `merge-into` `knowledge_pages.title`, falling back to `preview(60)` for the ~2/3 of rows without one |
| 2.7 | **NEW-FOUND** `tags` (JSON/CSV text) | `entries.tags`; `store_list_by_scope_and_tag` at `store_entry_crud.rs:285` | 320 non-empty | 765 non-empty | Opinion/hypothesis constructors write literal `opinion` / `hypothesis` tags (`behavior.rs:59`, `:97`) | `carry-verbatim` as a `cas_legacy_tags` YAML list block (parser-safe per spec rule C4) |
| 2.8 | **NEW-FOUND** compression pair `raw_content` / `compressed` | `entries.*` | 0 / 0 | 0 / 0 | Feature wired, never used. Zero rows compressed | `deliberately-leave` — 0 rows, feature never exercised. **Hard assert**: a compressed row at run time aborts M3 |
| 2.9 | **NEW-FOUND** provenance `session_id` / `source_tool` | `entries.*`; `idx_entries_session`; `store_list_by_session` at `store_entry_queries.rs:76` | 0 / 449 (`mcp`) | 0 / 1242 (`mcp`) | `session_id` is **universally NULL** — the per-session read path `list_by_session` is dead. `source_tool` is uniformly `mcp` | `carry-verbatim` (`cas_legacy_session_id`, `_source_tool`); `session_id` omitted universally under the omit-when-default rule |
| 2.10 | **NEW-FOUND** `domain` | `crates/cas-types/src/entry.rs:338`; `idx_entries_domain` | 0 | 0 | Context-aware knowledge field, unused | `carry-verbatim`; key never emitted (0 rows) |
| 2.11 | **NEW-FOUND** `branch` (worktree scoping) | `crates/cas-types/src/entry.rs:362`; `store_list_by_branch` at `store_entry_crud.rs:421`; consumer `cas-cli/src/mcp/server/mod.rs:849` | 0 | 0 | Read path live, zero data | `carry-verbatim`; key never emitted (0 rows) |
| 2.12 | **NEW-FOUND** `scope` (global vs project) | `crates/cas-types/src/scope.rs:18`; `entries.scope`; `idx_entries_scope` | **450 = `project`** | 1245 = `project` | ⚠️ Every row in the *global* DB is labelled `scope='project'`. Scope is carried by which DB file the row lives in, not by the column — and by the `g-`/`p-` id prefix stripped in `merge_entries` (`crates/cas-core/src/hooks/context/mod.rs:520-549`). M3 must not trust `entries.scope` | `deliberately-leave` the column, **derive instead** — scope comes from the DB file plus the `g-`/`p-` id prefix, recorded as `cas_legacy_scope` + `cas_legacy_db`. Carrying a known-false value is refused |
| 2.13 | **NEW-FOUND** learning-review state `last_reviewed` / `review_after` | `entries.*`; `idx_entries_unreviewed_learnings`; `store_entry_queries.rs:95`, `:115` | 0 / 0 | 4 / 0 | Consumed by the session-stop learning-review hook (`cas-cli/src/hooks/handlers/handlers_middle/session_stop/mod.rs:176`) | `carry-verbatim` (`cas_legacy_last_reviewed`, `_review_after`) |
| 2.14 | **NEW-FOUND** pipeline state `pending_extraction`, `pending_embedding`, `updated_at`, `indexed_at` | `entries.*`; `crates/cas-store/src/sqlite/store_entry_indexing.rs`; trait `crates/cas-store/src/lib.rs` (`list_pending_index`, `mark_indexed`) | 0 / **450** / 450 / 176 NULL | 0 / **1245** / 1245 / 1 NULL | ⚠️ `pending_embedding = 1` for **100% of rows in both DBs** — no entry has ever been embedded. Confirms §3.2: entries have no vector representation | `updated_at` → `carry-verbatim`; `pending_extraction` / `pending_embedding` / `indexed_at` → `deliberately-leave` (pipeline state; destination re-arms its own flag) |
| 2.15 | **NEW-FOUND** cloud `sync_queue` rows for entries | `sync_queue` table | **11** (`entity_type='entry'`) | 0 | 11 undelivered entry syncs sitting in the global queue; PROJECT has no entry rows queued at all | **drain → else invalidate**, ledgered. Never preserved across the migration; M3 asserts the count is 0 before extraction (spec §5.3) |
| 2.16 | **NEW-FOUND** `code_memory_links` table | `crates/cas-store/…`; migration `cas-cli/src/migration/migrations/m134_code_memory_links_create_table.rs` | 0 | 0 | Memory↔code-symbol association, no data | `deliberately-leave` — 0 rows. **Hard assert**: a non-zero count aborts M3 rather than silently dropping a memory-to-code edge |
| 2.17 | **NEW-FOUND** entity graph over memories (`entities`, `entity_mentions`, `relationships`) | `crates/cas-store/src/entity_store.rs`; search channel `cas-cli/src/hybrid_search/entity_search.rs:217`, graph expansion `cas-cli/src/hybrid_search/hybrid.rs:767-806` | 0 / 0 / 0 | 0 / 0 / 0 | Retrieval channel exists and is wired into hybrid search; the backing tables are empty in both DBs | `deliberately-leave` — 0 rows. Same hard assert as 2.16 |
| 2.18 | **NEW-FOUND** legacy **MarkdownStore** backend (file-based entries, YAML frontmatter) | `cas-cli/src/store/markdown.rs:1-18` (`//! Legacy Markdown storage backend`), impls at `:288`, `:312`, `:340`; mirrored at `crates/cas-store/src/markdown.rs:307` | **no `entries/` or `archive/` dir on disk** | no dir | A second, entirely separate persistence format implementing the same `Store` trait. Zero data on this machine, but M2 must state whether the migration is defined for it | `deliberately-leave` — migration is scoped to the SQLite backend. **Hard assert**: an `entries/` or `archive/` directory aborts M3 |
| 2.19 | **NEW-FOUND** Tantivy BM25 index directory | `~/.cas/index/*.{idx,term,store,pos,fast,fieldnorm}`; `cas-cli/src/hybrid_search/search_index_query.rs:120` | present, many segments | — | Derived artifact over entries; not a source of truth but must be rebuilt/invalidated after migration | `deliberately-leave` (rebuild) — derived artifact; M3 reindexes explicitly (spec §6) |
| 2.20 | **NEW-FOUND** retrieval-feedback tables `retrieval_queries` / `retrieval_query_results` / `retrieval_outcomes` | `crates/cas-store/src/retrieval_store.rs` | 0 / 0 | 0 / 0 | The `search action=retrieval_feedback` surface. Empty — no baseline of "which memory was helpful for which query" exists to preserve or to measure parity against (relevant to M4, cas-90fd) | `deliberately-leave` — 0 rows; no relevance history exists to preserve. M4 must generate its query set |

---

## 3. Read-path map

Every consumer of legacy `entries` data on this machine, grouped by entry point.

### 3.1 SessionStart context injection (the highest-volume reader)

Entry point: `crates/cas-core/src/hooks/context/build_start.rs:134` (`build_context_with_stores`).

1. **Merge** — `merge_entries` (`crates/cas-core/src/hooks/context/mod.rs:520`) calls `store.list()` on the project store then the global store, de-duplicating on the id with the `p-`/`g-` prefix stripped (project wins). Backed by `store_list` (`crates/cas-store/src/sqlite/store_entry_crud.rs:267`, `WHERE archived = 0 … LIMIT 10000`).
2. **Filter** — `build_start.rs:668-672`: drop `Observation` entries with non-positive feedback. No tier, validity, or scope filter.
3. **Score** — `build_start.rs:679` via `ContextScorer`:
   - `BasicContextScorer` (`crates/cas-core/src/hooks/context/mod.rs:187`): `type_weight × feedback_boost × age_decay × importance_boost × stability_boost × access_boost`.
   - `HybridContextScorer` (`cas-cli/src/hooks/scorer.rs:74`): BM25/hybrid score blended 70/30 with the normalized basic score; falls back to basic when the query is empty or hybrid returns nothing.
4. **Render** — three sections:
   - `## 📌 Pinned Memories (Always Active)` — `build_start.rs:287`, **full body**, budget-exempt, survives `minimal_start`.
   - `## Helpful Memories (n/m shown …)` — `build_start.rs:703-733`, **previews only**.
   - `## Related to Current Work` — `build_start.rs:780-810`, reuses the scored set, `score > 0.3`, top 5, previews.
   - Separately, `## <knowledge index>` — `build_start.rs:762` `render_knowledge_index` — reads **`knowledge_pages`, not entries**. This is the "index-inject / body-pull" block (`build_start.rs:105`, `KNOWLEDGE_PULL_INSTRUCTION` at `:116`), i.e. the *destination* system's read path, currently rendering 0 rows.

### 3.2 Retrieval channels (`mcp__cas__search`, hybrid scoring)

- **BM25 / Tantivy** — `cas-cli/src/hybrid_search/search_index_query.rs:120` (`search`), driven by `cas-cli/src/hybrid_search/hybrid.rs:420` / `:448`. **This is the only channel that indexes legacy entries.**
- **Semantic / vector** — `cas-cli/src/hybrid_search/semantic.rs:29-80`. 🔴 **The semantic channel is defined over `KnowledgePage`, not `Entry`** (`SemanticChannel::embed_page_text(page: &KnowledgePage, …)` at `semantic.rs:64`; `search()` returns page ids at `:72`). Corroborated by the data: `pending_embedding = 1` for 1695/1695 rows (§2.14) and `knowledge_pages = 0`. **Legacy memories have never had semantic retrieval.** Migration into `knowledge_pages` is therefore a retrieval *upgrade*, and M4's parity harness (cas-90fd) must baseline BM25-only behavior — comparing post-migration semantic hits against a "legacy semantic" baseline that never existed would be meaningless.
- **Entity / graph expansion** — `cas-cli/src/hybrid_search/entity_search.rs:217`, `cas-cli/src/hybrid_search/hybrid.rs:767-806`. Wired, backing tables empty (§2.17).
- **Code channel** — `cas-cli/src/hybrid_search/code.rs:33`; orthogonal to entries.

### 3.3 MCP `memory` tool

Router: `cas-cli/src/mcp/tools/service/mod.rs:193-240`. Actions → handlers in
`cas-cli/src/mcp/tools/core/memory.rs` unless noted:

| action | handler |
|---|---|
| `remember` | `memory.rs:323` |
| `get` | `memory.rs:583` |
| `list` | `memory.rs:786` |
| `recent` | `memory.rs:705` |
| `update` | `memory.rs:966` |
| `delete` | `memory.rs:763` |
| `archive` / `unarchive` | `memory.rs:898` / `:926` |
| `helpful` / `harmful` | `memory.rs:623` / `:653` |
| `mark_reviewed` | `memory.rs:681` |
| `set_tier` | `service/core.rs:156` → `core/system.rs:201` |
| `opinion_reinforce` / `_weaken` / `_contradict` | `core/opinion.rs:9` / `:60` / `:108` |

### 3.4 Hooks and background jobs

- **SessionStart** — §3.1, plus the CLI AI-selection variant at `cas-cli/src/hooks/context.rs:399` (`list_helpful`) and `:405` (`list_pinned`).
- **Plan mode** — `crates/cas-core/src/hooks/context/plan_mode.rs:50`.
- **Session stop / learning review** — `cas-cli/src/hooks/handlers/handlers_middle/session_stop/mod.rs:176` (`list_unreviewed_learnings`).
- **Decay daemon** — `cas-cli/src/daemon/decay.rs:14` (decay pass), `:151` (prune pass at stability < 0.1); config `cas-cli/src/daemon/types.rs:21,55`; scheduler `cas-cli/src/daemon/maintenance.rs:52`; MCP surface `cas-cli/src/mcp/tools/core/maintenance.rs:9`, `:60`.
- **Statusline** — `cas-cli/src/cli/statusline/data_and_format.rs:92`.
- **MCP prompts** — `cas-cli/src/mcp/server/prompts.rs:168`, `:291`, `:358` (`store.recent(n)`).
- **Branch-scoped listing** — `cas-cli/src/mcp/server/mod.rs:849`.
- **System info** — `cas-cli/src/mcp/tools/core/system.rs:65` (`list_archived`).

### 3.5 Store-layer decorators

Every read above passes through the decorator chain, which must be considered
if M3 reads through the trait rather than raw SQL:
`cas-cli/src/store/notifying_entry.rs:97-137` and
`cas-cli/src/store/syncing_entry.rs:150-205` both delegate the full `Store`
read surface; `syncing_entry` additionally drives the cloud `sync_queue` (§2.15).

---

## 4. Findings that change the shape of M2/M3

1. **Opinion history was never persisted** (§1.4). Not a migration problem — a
   capability gap. Do not write a mapping rule for data that does not exist.
2. **Pins are unreachable from the MCP API** (§1.5), which is why the single
   most privileged read path (full-body, budget-exempt, minimal-mode-surviving)
   carries 0 rows. If the destination system keeps a pin concept, expose it.
3. **Legacy entries have no embeddings and no semantic channel** (§3.2, §2.14).
   Migration is a retrieval upgrade; M4's parity baseline must be BM25-only.
4. **Two archive concepts** (§2.2): `archived` flag (0 rows) vs
   `memory_tier='archive'` (1512/1695 rows). Read paths filter the former and
   ignore the latter, so 89% of rows are "archived" by tier yet fully live in
   context injection. M2 must say which one means "cold" in the destination.
5. **`entries.scope` is unreliable** (§2.12): all 450 rows in the *global* DB
   claim `scope='project'`. Scope is carried by DB file + id prefix. M3 must
   derive scope from provenance, not the column.
6. **`share` vs `team_id` split-brain** (§1.6): 626 rows carry `team_id`, 0 rows
   carry `share`. Mapping only `share` silently drops all team associations.
   *Cause, resolved in cas-0955:* the capability was never built rather than
   built and lost. Thirteen migration files adding the `visibility` / `owner_id`
   / `collaborators` half of the sharing model were never declared in
   `migration/migrations/mod.rs` and never ran, so those columns exist in no
   database and are read by no code. They have been deleted and a guard test
   now fails on any unregistered migration file. `team_id` remains
   authoritative; `share` remains a dead column.
7. **`Store::list()` caps at 10 000 rows** (§1.2). Safe today (1245), unsafe as a
   general extraction primitive. M3 should read with explicit paginated SQL.
8. **11 entry rows are stuck in the global `sync_queue`** (§2.15) — decide
   whether migration drains, preserves, or invalidates them.
9. **No retrieval-outcome history exists** (§2.20): 0 rows in
   `retrieval_queries` / `retrieval_outcomes` in both DBs. There is no
   historical relevance signal to carry forward, and M4 must generate its
   baseline rather than mine one.

---

## Appendix A — queries run

All executed read-only via `sqlite3 "file:<db>?mode=ro"` against
`~/.cas/cas.db` and `/home/pippenz/Petrastella/cas-src/.cas/cas.db`.

Schema/enumeration:

- `.tables`
- `.schema entries`

Counts (each run against both DBs):

- `select count(*) from entries`
- `select archived, count(*) from entries group by archived`
- `select type, count(*) from entries group by type`
- `select memory_tier, count(*) from entries group by memory_tier`
- `select belief_type, count(*) from entries group by belief_type`
- `select scope, count(*) from entries group by scope`
- `select observation_type, count(*) from entries group by observation_type`
- `select memory_tier, type, count(*) from entries group by memory_tier, type`
- `select share, count(*) from entries group by share`
- `select source_tool, count(*) from entries group by source_tool`
- `select cast(importance*10 as int), count(*) from entries group by 1`
- `select cast(stability*10 as int), count(*) from entries group by 1`
- `select min(created), max(created) from entries`
- `count(*) where` … `valid_from is not null`, `valid_until is not null`,
  `valid_until < datetime('now')`, `review_after is not null`,
  `last_reviewed is not null`, `team_id is not null`, `share is not null`,
  `domain is not null`, `branch is not null`, `title is not null`,
  `tags not in ('','[]')`, `compressed=1`, `raw_content is not null`,
  `pending_embedding=1`, `pending_extraction=1`, `indexed_at is null`,
  `session_id is not null`, `source_tool is not null`, `helpful_count>0`,
  `harmful_count>0`, `access_count>0`, `last_accessed is not null`,
  `importance<>0.5`, `stability<>0.5`, `confidence<>1.0`,
  `updated_at is not null`
- `select count(*) from code_memory_links | entities | entity_mentions | relationships | knowledge_pages | knowledge_sources`
- `select entity_type, count(*) from sync_queue group by entity_type`
- `select count(*) from retrieval_queries | retrieval_outcomes`

Filesystem checks:

- `ls ~/.cas/index` (Tantivy segments present)
- `ls -d ~/.cas/entries ~/.cas/archive <project>/.cas/entries` → all absent
  (MarkdownStore has no data)

No `INSERT`, `UPDATE`, `DELETE`, `ALTER`, or `PRAGMA` write was issued at any
point; both handles were opened `mode=ro`.
