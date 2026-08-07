# cas-b129 M2 — Mapping spec: disposition for every legacy memory surface

Task: **cas-e311** (Phase 2 of epic **cas-b129**). Normative. M3 implements this
document literally; where it says MUST, a migration that does otherwise is wrong.

Input: [`cas-b129-legacy-memory-inventory.md`](./cas-b129-legacy-memory-inventory.md) (M1, cas-13aa).
All counts below are read-only observations from `~/.cas/cas.db` (**GLOBAL**) and
`/home/pippenz/Petrastella/cas-src/.cas/cas.db` (**PROJECT**).

## Disposition vocabulary

| Disposition | Meaning |
|---|---|
| `migrate-to-page` | Row becomes a `knowledge_pages` row + markdown body; distillable. |
| `carry-verbatim` | Row becomes a **locked** page whose body is byte-identical to the legacy content; distillation may never rewrite it. |
| `stay-entry` | Row remains in `entries`, untouched. Not a loss — the entries store survives the migration. |
| `merge-into` | Field is folded into another destination field rather than getting its own. |
| `deliberately-leave` | Field/row is intentionally not carried. Every use states why and what is thereby lost. |

Rows get exactly one row-level disposition (§4). Fields get exactly one
field-level disposition (§5). No blanks, no TBDs.

---

## 1. Motivating constraint — the destination cannot natively hold entry state

`knowledge_pages` is, in full
(`crates/cas-store/src/knowledge_store.rs:304-323`, DDL verified against both live DBs):

```
row_id, id, page_type, title, rel_path, snippet,
locked, sources_json, created_at, updated_at, pending_embedding
```

The page **body is a markdown file** at `rel_path`, not a column.

There is **no column** for: `importance`, `stability`, `memory_tier`,
`helpful_count`, `harmful_count`, `access_count`, `last_accessed`, `type`
(entry type), `observation_type`, `belief_type`, `confidence`, `tags`, `domain`,
`branch`, `session_id`, `source_tool`, `team_id`, `share`, `valid_from`,
`valid_until`, `review_after`, `last_reviewed`, `archived`, `scope`,
`pending_extraction`, `raw_content`, `compressed`.

Only `created_at` / `updated_at` and a file-path provenance list (`sources_json`)
have native destinations. **`migrate-to-page` is therefore not a lossless
disposition on its own.** The epic's zero-loss requirement needs a carrier.

## 2. The carrier — reserved `cas_legacy_*` frontmatter (APPROVED direction)

`cas-cli/src/knowledge/merge.rs` defines exactly five keys CAS owns and
regenerates — `OWNED_KEYS = ["title", "type", "sources", "locked", "updated"]`
(`merge.rs:92`). Every other frontmatter key is captured into
`Frontmatter.passthrough` (`merge.rs:85-88`, `:155-162`) and re-emitted verbatim
by `render_frontmatter` (`merge.rs:~185`). The doc comment states the intent
outright: *"Lines CAS does not own, carried through a round trip verbatim.
Without this, a `tags:` or `owner:` key a human added to a page would be silently
destroyed by the next distillation pass."*

**Rule C1.** Legacy entry state MUST ride as `cas_legacy_*` frontmatter keys.
The prefix is reserved; nothing outside migration may write it. It cannot
collide with `OWNED_KEYS`, so it is immune to re-distillation by construction.

**Rule C2 — reserved key names** (the complete set; M3 emits no others):

| Key | Source column | Format |
|---|---|---|
| `cas_legacy_id` | `entries.id` | scalar |
| `cas_legacy_db` | which DB file the row came from | `global` \| `project` |
| `cas_legacy_scope` | derived (§5.12) | `global` \| `project` |
| `cas_legacy_type` | `type` | scalar |
| `cas_legacy_observation_type` | `observation_type` | scalar |
| `cas_legacy_belief_type` | `belief_type` | scalar |
| `cas_legacy_confidence` | `confidence` | float |
| `cas_legacy_memory_tier` | `memory_tier` | scalar |
| `cas_legacy_archived` | `archived` | bool |
| `cas_legacy_importance` | `importance` | float |
| `cas_legacy_stability` | `stability` | float |
| `cas_legacy_access_count` | `access_count` | int |
| `cas_legacy_last_accessed` | `last_accessed` | RFC3339 |
| `cas_legacy_helpful_count` | `helpful_count` | int |
| `cas_legacy_harmful_count` | `harmful_count` | int |
| `cas_legacy_created` | `created` | RFC3339 |
| `cas_legacy_updated_at` | `updated_at` | RFC3339 |
| `cas_legacy_tags` | `tags` | YAML list block |
| `cas_legacy_team_id` | `team_id` | scalar |
| `cas_legacy_valid_from` | `valid_from` | RFC3339 |
| `cas_legacy_valid_until` | `valid_until` | RFC3339 |
| `cas_legacy_review_after` | `review_after` | RFC3339 |
| `cas_legacy_last_reviewed` | `last_reviewed` | RFC3339 |
| `cas_legacy_domain` | `domain` | scalar |
| `cas_legacy_branch` | `branch` | scalar |
| `cas_legacy_session_id` | `session_id` | scalar |
| `cas_legacy_source_tool` | `source_tool` | scalar |

**Rule C3 — omit-when-default.** A key is emitted only when its column is
non-NULL and differs from the schema default. Absence means "schema default",
which is lossless because the defaults are fixed by DDL: `type='learning'`,
`archived=0`, `stability=0.5`, `importance=0.5`, `access_count=0`,
`memory_tier='working'`, `helpful_count=0`, `harmful_count=0`,
`belief_type='fact'`, `confidence=1.0`, `pending_embedding=1`,
`scope='project'`. M3 MUST record this default table in the migration ledger so
the omission is decodable without reading this spec.

**Rule C4 — parser safety.** `parse_frontmatter` is a hand-rolled line reader,
not a YAML engine (`merge.rs:117-120`). List blocks survive passthrough (a
`- item` line has no colon, so it falls to the passthrough branch at
`merge.rs:139-144`), but only `sources` gets list *parsing*. M3 MUST therefore
emit `cas_legacy_tags` as a simple `- item` block and every other key as a flat
scalar. No nested maps, no multi-line strings, no anchors.

## 3. Corpus reality — what is actually being migrated

Row counts are **not** the corpus. Exactly five literal content strings account
for **994 of 1696 rows (58.6%)** — integration-test fixtures written by the test
suite into the real databases:

| Fixture content | GLOBAL | PROJECT |
|---|---:|---:|
| `Test memory from MCP protocol test` | 46 | 160 |
| `Rust programming language with ownership and borrowing` | 46 | 160 |
| `Context test memory entry` | 46 | 160 |
| `Consolidated memory test entry` | 45 | 159 |
| `Test entry for notification test` | 29 | 143 |
| **fixture subtotal** | **212** | **782** |
| **real rows** | **238** | **464** |

PROJECT holds only **469 distinct contents across 1246 rows**. The genuine
migration corpus is **702 rows**, not 1695.

Real-row composition: GLOBAL 162 learning / 70 context / 3 preference /
3 observation; PROJECT 314 learning / 123 context / 18 preference / 9 observation.

**Second corpus hazard — cross-project contamination.** The PROJECT (cas-src) DB
contains another project's records, concentrated in exactly the band a naive
importance rule would promote: **41 of 210** high-importance real rows are
Accounting-domain (`QBO`, `1040`, `TNTAP`, `FONCE`, `FAE 183`, `1065`). Observed
titles include *"FINAL QBO State — 44 JEs, All 1098s Received"*, *"Richards 1040
— All three years complete"*, *"Roark 2023 FAE 183 FONCE Submitted TNTAP"*. The
corpus even contains a memory titled *"Cross-project task contamination via cloud
sync — root cause traced"*. **`importance >= 0.8` is not a durability signal for
cas-src facts; it is partly a contamination signal.** Any rule keyed on
importance alone would enshrine a third party's tax filings as cas-src knowledge
pages.

**Rule D0 — the DB is live.** PROJECT moved 1245 → 1246 rows during authoring of
the M1 inventory. M3 MUST snapshot (or assert a stable count across) the source
tables before extraction, and record the observed count in the ledger.

---

## 4. Row-level routing — ordered decision procedure

Applied top to bottom; **first match wins**. This is total: every row in either
DB reaches exactly one disposition, so no judgement call remains at migration
time (the M2 demo criterion).

| # | Predicate | Disposition | Rationale | Rows (G/P) |
|---|---|---|---|---|
| **R1** | `content` exactly equals one of the five fixture strings in §3 (exact match, **never** a `LIKE`) | `deliberately-leave` | Test-suite artifacts, not memories. Migrating them would create 994 junk pages and poison retrieval. Lossless in substance: the five strings are enumerated here and in the ledger, so the set is fully reconstructible. | 212 / 782 |
| **R2** | `memory_tier = 'in_context'` (pinned) | `carry-verbatim` (locked) | Epic requirement: pinned content survives untouched. Highest-privilege read path. | 0 / 0 |
| **R3** | `type = 'preference'` | `carry-verbatim` (locked) | Persona / user-sovereign content ("Naming taste", "Claude-only machine — prefer .claude", "User workflow preferences"). Human-authored intent must never be re-worded by distillation. | 3 / 18 |
| **R4** | `belief_type IN ('opinion', 'hypothesis')` | `stay-entry` | Epistemic state with live mutators (`reinforce`/`weaken`/`contradict`) that have no destination equivalent. Pages have no `confidence` to decay. | 0 / 1 |
| **R5** | `type = 'observation'` | `stay-entry` | Session ephemera. Already second-class on the read path — filtered out of SessionStart unless `feedback_score() > 0` (`build_start.rs:668-672`). | 3 / 9 |
| **R6** | Contamination quarantine: row matches the foreign-domain predicate (§4.1) | `deliberately-leave`, **quarantine-logged** | Belongs to another project; migrating it into cas-src knowledge is the bug, not the fix. Not deleted — see §4.1. | 37 / 47 |
| **R7** | `type = 'context'` | `migrate-to-page` | Durable project facts — the epic's stated page candidates. | 70 / ~123 |
| **R8** | `type = 'learning'` AND `helpful_count > harmful_count` | `migrate-to-page` | Positively-reinforced learning is the only *human-confirmed* durability signal in the corpus (importance is 76% untouched default; stability is decay-time, not judgement). | 0 / 4 |
| **R9** | `type = 'learning'` (remainder) | `stay-entry` | Session learnings per the epic's own split. Distillation can promote them later via the normal pipeline; migration does not pre-judge. | ~162 / ~310 |

Rules are exhaustive: R7–R9 cover every remaining `type` value, and the four
values in `EntryType` (`crates/cas-types/src/entry.rs:16`) are the only ones
present in either DB.

### 4.1 Contamination quarantine (R6)

Predicate: `content` or `title` matches any of `QBO`, `TNTAP`, `FONCE`,
`FAE 183`, `1040`, `1065`, `Journal Entr` (case-sensitive; these are proper
nouns and form numbers, so false positives on cas-src prose are implausible).

**Widened after the M5 run-1 cutover** with nine property/loan/settlement proper
nouns — `Roark`, `Realty`, `JRPW`, `Renovo`, `Leake`, `Moultrie`, `Radnor`,
`Old Hickory`, `HUD-1`. The original list is accounting/tax vocabulary only, so
29 of the 181 pages run 1 produced were the same project's *real-estate* records,
two of which won the SessionStart index budget and were injected into every
cas-src session. The widening raises the quarantine from 84 to 123 rows and is
monotone (39 newly quarantined, none released).

The tokens are **proper nouns only, never generic English** — measured against
the real 1700-row corpus, `Property` matches `Object.getOwnPropertyNames(...)`,
`Lease` matches the `LeaseNotFound` error type, and `Richards` matches the
`Richards-LLC` GitHub org and Vercel team used throughout genuine cas-src
infrastructure memories. All three are excluded for that reason. Sixteen further
candidates (`Escrow`, `Mortgage`, `Loan`, `Settlement Statement`, `1098`, `K-1`,
`CSL`, `Ingram`, `Rearden`, …) were measured and add zero rows beyond the proper
nouns, so none was adopted. See the M5 runbook §8.2.

Quarantined rows are **`stay-entry` in place** — they are *not* deleted and *not*
paged. M3 writes them to a `quarantine.jsonl` in the migration ledger with their
full row and matched token. Deleting another project's records on a
heuristic is not a call this migration gets to make.

> **ESCALATION E1 (§10)** — this predicate is a heuristic. It is deliberately
> conservative (quarantine, never delete), but supervisor should confirm the
> token list before M3 runs.

### 4.2 Page shape for migrated rows

- `page_type` — from entry type: `context` → `context`, `learning` → `learning`,
  `preference` → `persona`. Drives `canonical_rel_path` (`knowledge_store.rs:327-335`).
- `title` — `entries.title` when non-NULL (641 of 1696 rows), else `preview(60)`
  (`crates/cas-types/src/entry/behavior.rs:309`), matching what SessionStart
  already displays (`build_start.rs:293`).
- `snippet` — first sentence of content, capped to the index-inject budget.
- `body` — legacy `content`, byte-identical, beneath the frontmatter block.
- `sources_json` — see §7.

---

## 5. Field-level disposition — every inventory surface

Every surface in the M1 inventory (§1 epic-named, §2 NEW-FOUND) appears exactly
once. G/P = GLOBAL/PROJECT non-default row counts.

### 5.1 Epic-named surfaces

| M1 § | Surface | Disposition | Rule |
|---|---|---|---|
| 1.1 | `type` (entry types) | `merge-into` `page_type` + `carry-verbatim` as `cas_legacy_type` | Drives R3/R5/R7/R8/R9 routing and the page type; also preserved verbatim so the original is recoverable. |
| 1.2 | `memory_tier` | `carry-verbatim` as `cas_legacy_memory_tier`; **does not gate migration** | See §5.1a. |
| 1.3 | `importance` / `stability` / `access_count` / `last_accessed` | `carry-verbatim` (four `cas_legacy_*` keys) | No destination column and no destination decay engine. Preserved as provenance only; MUST NOT be used as a routing signal (§3 contamination). |
| 1.4 | Opinions: `belief_type`, `confidence` | `stay-entry` (R4); scalars `carry-verbatim` if a row reaches a page by another rule | Scalars-only, per M1: there is no history. |
| 1.4 | Opinion reinforce/weaken/contradict **evidence** | `deliberately-leave` | **Nothing to carry.** `opinion.rs:54/102/155` echo the evidence into the response string and never persist it. No history table exists in either DB. Loss is zero because the data never existed. Re-adding history is net-new work, not migration — **ESCALATION E2**. |
| 1.5 | Pins (`memory_tier='in_context'`) | `carry-verbatim` → locked page (R2) | 0 rows today; rule is specified so the migration is correct if a pin exists at run time. |
| 1.6 | `team_id` | `carry-verbatim` as `cas_legacy_team_id`; **authoritative** | See §8. |
| 1.6 | `share` | `deliberately-leave` | Dead column: 0 non-NULL rows in either DB, never written by any code path. Carrying it would migrate 1696 NULLs while implying a semantic that was never exercised. §8. |
| 1.7 | `valid_from` / `valid_until` / `review_after` | `carry-verbatim` (three keys) | 0/0/1/0 rows. Preserved, but note the destination has no expiry enforcement either — parity, not regression (§9). |

#### 5.1a Which archive concept means "cold"? (supervisor question)

`memory_tier` is authoritative; the `archived` flag is not.

Evidence: `archived = 1` on **0 of 1696** rows in both DBs, while
`memory_tier = 'archive'` covers **1512**. But every read path filters
`archived = 0` and **none** filters tier (`store_list`,
`crates/cas-store/src/sqlite/store_entry_crud.rs:267`), so archive-tier rows are
behaviourally *live*: they flow into SessionStart scoring today.

Therefore:
- `memory_tier` = the real cold/warm signal → `carry-verbatim`.
- `archived` = the real delete-ish signal, currently unused → `carry-verbatim` as
  `cas_legacy_archived` for completeness, but it selects nothing.
- **Neither gates migration.** Routing (§4) ignores both. Excluding archive-tier
  rows would drop 89% of the corpus while those same rows are being injected into
  live sessions — that would be data loss disguised as tidiness.
- The destination has no tier column, so cold-ness survives as frontmatter only.
  This is an accepted capability reduction, recorded in §9.

### 5.2 NEW-FOUND surfaces (all 20 resolved)

| M1 § | Surface | Disposition | Rationale |
|---|---|---|---|
| 2.1 | `belief_type` / `confidence` as general fields | `carry-verbatim` | Two keys; routing use is R4 only. |
| 2.2 | `archived` flag vs archive tier | `carry-verbatim` (both) | Ambiguity resolved in §5.1a. |
| 2.3 | `observation_type` | `carry-verbatim` | 1 row (`general`). Observations `stay-entry` (R5), so the key appears only if such a row is paged by another rule. |
| 2.4 | `helpful_count` / `harmful_count` | `carry-verbatim` + **routing signal** (R8) | The corpus's only human-confirmed durability signal. |
| 2.5 | `access_count` / `last_accessed` | `carry-verbatim` | Telemetry; no destination engine consumes it. |
| 2.6 | `title` | `merge-into` `knowledge_pages.title` | Falls back to `preview(60)` for the ~⅔ of rows without one (§4.2). |
| 2.7 | `tags` | `carry-verbatim` as `cas_legacy_tags` list | 1085 non-empty rows. Note `opinion`/`hypothesis` literal tags written by the constructors (`behavior.rs:59`, `:97`) are redundant with `belief_type` but carried anyway — verbatim means verbatim. |
| 2.8 | `raw_content` / `compressed` | `deliberately-leave` | 0 rows in either DB; feature never exercised. Nothing to lose. If a compressed row appears at run time, M3 MUST abort rather than silently drop `raw_content` — **hard assert**. |
| 2.9 | `session_id` / `source_tool` | `carry-verbatim` | `session_id` is NULL on 1696/1696 (so C3 omits it universally); `source_tool` is `mcp` on 1691. |
| 2.10 | `domain` | `carry-verbatim` | 0 rows; key never emitted under C3. |
| 2.11 | `branch` | `carry-verbatim` | 0 rows; key never emitted under C3. |
| 2.12 | `scope` column | `deliberately-leave`; **derive instead** | The column is wrong: all 450 GLOBAL rows claim `scope='project'`. M3 MUST derive scope from (a) which DB file the row came from and (b) the `g-`/`p-` id prefix that `merge_entries` strips (`crates/cas-core/src/hooks/context/mod.rs:528`, `:539`), and record it as `cas_legacy_scope` + `cas_legacy_db`. Carrying the column would propagate a known-false value. |
| 2.13 | `last_reviewed` / `review_after` | `carry-verbatim` | 4 / 0 rows. |
| 2.14 | `pending_extraction`, `pending_embedding`, `updated_at`, `indexed_at` | `updated_at` → `carry-verbatim`; the other three → `deliberately-leave` | Pipeline state, not content. `pending_embedding` is 1 on 1696/1696 and the destination re-arms its own flag on insert (`knowledge_pages.pending_embedding DEFAULT 1`), so carrying it is meaningless. `indexed_at` refers to a Tantivy index that migration invalidates (§6). |
| 2.15 | 11 stranded `sync_queue` entry rows | **drain → else invalidate**, ledgered | See §5.3. |
| 2.16 | `code_memory_links` | `deliberately-leave` | 0 rows in both DBs. **Hard assert**: if non-zero at run time, M3 aborts — a memory↔code-symbol edge has no destination and must not be silently dropped. |
| 2.17 | `entities` / `entity_mentions` / `relationships` | `deliberately-leave` | 0 rows in both DBs; the graph channel is wired but has never held data. Same hard assert as 2.16. |
| 2.18 | Legacy `MarkdownStore` backend | `deliberately-leave` | No `entries/` or `archive/` directory exists on this machine. **Scoped out**: this migration is defined for the SQLite backend only. M3 MUST check for the directories and abort if present rather than pretend the backend does not exist. |
| 2.19 | Tantivy BM25 index dir | `deliberately-leave` (rebuild) | Derived artifact, not a source of truth. §6. |
| 2.20 | `retrieval_queries` / `_query_results` / `_outcomes` | `deliberately-leave` | 0 rows in both DBs. No relevance history exists to preserve; M4 (cas-90fd) must generate its query set — already relayed to swift-owl-81. |

### 5.3 The 11 stranded `sync_queue` rows (supervisor question)

11 rows with `entity_type='entry'` sit in GLOBAL's `sync_queue`; PROJECT has none.
Deterministic procedure, in order:

1. **Drain.** M3 attempts a normal sync flush and re-reads the count.
2. If the queue reaches 0 → proceed. Record `drained: 11`.
3. If drain is impossible (cloud unauthenticated / offline), **invalidate**:
   delete the 11 rows *after* writing their full payloads to the migration
   ledger. Record `invalidated: 11` with payloads.
4. **Never preserve them across the migration.** They reference legacy entry ids
   that R1–R9 may retire or re-home; replaying them post-migration would push
   rows the local store no longer has in that form, re-creating exactly the
   cross-project contamination already visible in this corpus (§3).

M3 MUST assert the entry-row count is 0 before extraction begins.

---

## 6. Derived artifacts

The Tantivy index (`~/.cas/index`) and the `entries` FTS state are derived. After
migration M3 MUST (a) reindex `knowledge_pages`, and (b) leave the entries index
consistent with whatever rows remain under `stay-entry`. `SearchIndex::open`
deletes and recreates the index directory on a field-count mismatch, so this must
be an explicit, logged step rather than an incidental side effect.

---

## 7. Provenance carry rules, per disposition (AC3)

| Disposition | Provenance carried | Mechanism |
|---|---|---|
| `migrate-to-page` | `created` → `knowledge_pages.created_at`; `updated_at` → `updated_at`; **and** both re-stated as `cas_legacy_created` / `cas_legacy_updated_at`; `cas_legacy_id`, `cas_legacy_db`, `cas_legacy_scope`, `cas_legacy_source_tool`, `cas_legacy_session_id`; feedback (`helpful`/`harmful`), telemetry (`access_count`, `last_accessed`), scoring state (`importance`, `stability`) | native columns + `cas_legacy_*` frontmatter |
| `carry-verbatim` | Identical to `migrate-to-page`, **plus** the body is byte-identical to legacy `content` and the page is locked | as above + §8 lock |
| `stay-entry` | Untouched — the row keeps every column | no-op |
| `merge-into` | Value reaches its destination field; the original is additionally preserved under its `cas_legacy_*` key so the merge is reversible | both |
| `deliberately-leave` | None. Each use in §5 states what is lost and why it is zero-substance | ledger note |

**Rule P1 — dual-writing native + `cas_legacy_*` timestamps is deliberate.**
`created_at` is a CAS-owned page column that later processes may touch;
`cas_legacy_created` is passthrough and cannot be rewritten. Belt and braces on
the one provenance fact the epic names explicitly.

**Rule P2 — `sources_json`.** `sources` is CAS-owned (`OWNED_KEYS`) and is a list
of *file paths* that distillation regenerates. A legacy memory has no source
file, so M3 MUST write `sources_json = []` and record origin in
`cas_legacy_id` / `cas_legacy_db` instead. Writing a synthetic path into
`sources` would be overwritten by the next distillation pass and would corrupt
the source ledger.

---

## 8. Locked-bit semantics (AC2) and share semantics

### 8.1 Locked bit — already implemented; this is the specification of it

- **Field.** `knowledge_pages.locked INTEGER NOT NULL DEFAULT 0`
  (`crates/cas-store/src/knowledge_store.rs:316`, DDL verified in both DBs),
  mirrored in the body as `locked:` in frontmatter (parsed `merge.rs:146`,
  predicate `is_locked_body` `merge.rs:167`, rendered by `render_frontmatter`).
- **Enforcement point.** `commit_ingest`'s update is guarded by
  `WHERE knowledge_pages.locked = 0` (`crates/cas-store/src/knowledge_store.rs:938`).
  A locked page is never overwritten by distillation. This is the single
  enforcement site.
- **Who can set/clear it.** Only `Store::set_locked`
  (`knowledge_store.rs:1144`; trait doc `:629-633`: *"the only way to lock a page
  after creation: `commit_ingest` deliberately never touches `locked`, so
  distillation can neither lock nor unlock what the user decided"*). In practice:
  a human or agent through the `knowledge` MCP write path
  (`cas-cli/src/mcp/tools/core/knowledge.rs:289-377`, which unlocks → writes →
  re-locks and restores the prior state on failure, `:325-358`). **Distillation
  can never clear it.**
- **Migration rule L1.** Every `carry-verbatim` page (R2 pins, R3 preferences)
  MUST be created with `locked = 1` and frontmatter `locked: true`, applied via
  `set_locked` *after* `commit_ingest` — mirroring the hand-written-page path.
  A `carry-verbatim` page that ends up unlocked is a migration failure and M3
  MUST fail the run, not warn.
- **Migration rule L2.** `migrate-to-page` rows are created **unlocked**
  (`locked = 0`) so distillation may later improve them. That is the whole point
  of the page/entry split.

### 8.2 Share and team semantics — `team_id` is authoritative

`team_id` has 626 non-NULL rows in PROJECT; `share` has **0** in either DB. The
two columns encode overlapping intent and only `team_id` was ever written.

- `team_id` → `carry-verbatim` as `cas_legacy_team_id`. **Authoritative.**
- `share` → `deliberately-leave` (§5.1).
- **Rule S1.** M3 MUST NOT synthesize a `share` value from `team_id`. The
  destination has no sharing enforcement at all, so any synthesized value would
  be an unenforced assertion — worse than absence.
- **Rule S2.** Because the destination cannot enforce team scoping, a page
  carrying `cas_legacy_team_id` is *not* thereby team-scoped. Team-scoped
  retrieval is a capability the destination lacks (§9) — **ESCALATION E3**.

---

## 9. Accepted capability reductions

Recorded so they are decisions, not accidents. None is data loss; each is an
enforcement/behaviour the destination does not have.

1. **No tier engine.** `memory_tier` carries as text; nothing in the destination
   decays or promotes pages. The decay daemon (`cas-cli/src/daemon/decay.rs`)
   continues to operate on whatever remains under `stay-entry`.
2. **No feedback loop.** `helpful`/`harmful` carry as text; pages have no
   feedback actions.
3. **No expiry enforcement.** `valid_until` carries as text — but note the
   *legacy* system did not enforce it either (`store_list` filters only
   `archived`), so this is parity, not regression.
4. **No team scoping.** §8.2 / E3.
5. **No epistemic state.** Opinions/hypotheses `stay-entry` precisely because
   pages cannot represent confidence decay.

**Capability gained:** migrated pages become semantically retrievable. Legacy
entries never were — `pending_embedding = 1` on 1696/1696 and `SemanticChannel`
is defined over `KnowledgePage`, not `Entry`
(`cas-cli/src/hybrid_search/semantic.rs:64`, `:72`). This is why M4's parity
baseline must be BM25-only.

---

## 10. Escalations (AC4)

Every NEW-FOUND flag is resolved in §5. Three items need a supervisor decision
before M3 rather than a worker's guess:

- **E1 — contamination predicate (§4.1).** Token list (`QBO`, `TNTAP`, `FONCE`,
  `FAE 183`, `1040`, `1065`, `Journal Entr`) is a heuristic over 84 rows.
  Conservative by construction (quarantine in place, never delete), but the list
  should be confirmed. *Recommendation: approve as written.*
- **E2 — opinion history (§5.1).** The evidence trail never existed. Confirm it
  is out of scope for cas-b129 and, if wanted, gets its own task.
  *Recommendation: out of scope; file a follow-up.*
- **E3 — team scoping (§8.2).** 626 rows carry `team_id` into a destination with
  no team enforcement. Confirm that carrying it as inert provenance is
  acceptable for now. *Recommendation: accept; note as a known gap.*

Also for the record: **the five fixture strings (§3) should be purged from the
real databases and the test suite pointed at a temp DB** — 994 junk rows in
production stores is its own bug. Out of scope here; worth a task.

---

## 11. M3 preconditions — hard asserts before extraction

M3 MUST abort (not warn) if any of these fail:

1. `sync_queue` entry-row count is 0 (§5.3).
2. `code_memory_links`, `entities`, `entity_mentions`, `relationships` are all 0 (§5.2).
3. No row has `compressed = 1` or non-NULL `raw_content` (§5.2).
4. No `entries/` or `archive/` MarkdownStore directory exists (§5.2).
5. Source row counts are stable across two reads (§3, Rule D0).
6. Extraction reads via explicit paginated SQL, **not** `Store::list()` — that
   method caps at `LIMIT 10000` (`store_entry_crud.rs:267`).
7. Every extracted row matches exactly one rule in §4; an unrouted row aborts the run.
8. Every `carry-verbatim` page ends with `locked = 1` (Rule L1).
