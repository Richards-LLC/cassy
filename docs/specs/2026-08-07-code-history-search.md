# Cassy Code-History Search — Design Spec

- **Task:** cas-7ad6 (design-first; implementation split out after sign-off)
- **Evidence dependency:** cas-9d92 (Phase 1 + Phase 2, merged to `main` as `f710fc3d`, report `docs/analysis/2026-08-07-comm-efficiency-mining.md`)
- **Status:** DRAFT, independently verified — awaiting supervisor/operator sign-off (AC6)
- **Date:** drafted 2026-08-07; claims re-verified and corrected 2026-08-08
- **Provenance:** drafted by `proud-gazelle-50`; independently re-measured, corrected and landed by `wise-merlin-89`. The verification pass re-ran every DB count and resolved every §1 `file:line` against the tree; §5.2 and the items marked **[CORRECTED 2026-08-08]** are where the draft and reality diverged.

---

## 0. Executive summary, and the one thing that changes the shape

The goal is to let Cassy answer, as a query, *"when and why did this change, and is it still a problem?"* — by continuously indexing repository history and joining it to what Cassy already owns.

**The survey found that the join Cassy "already owns" is a schema with no data behind it.** Every table the brief names as the differentiator is empty or near-empty on the live database. This is the single most important input to the design, and it is measured, not inferred:

| Table | Purpose in the brief | Live rows |
|---|---|---|
| `commit_links` | the commit ↔ session ↔ prompt spine | **0** |
| `prompts` | session provenance | **1** |
| `file_changes.commit_hash` | ties an edit to the commit that shipped it | **0 of 7,799** |
| `file_changes.prompt_id` | ties an edit to the prompt that caused it | **1 of 7,799** |
| `code_files` / `code_symbols` | the tree-sitter symbol index to "reuse" | **0 / 0** |

Method: read-only `sqlite3 'file:/home/pippenz/Petrastella/cas-src/.cas/cas.db?immutable=1'`. Zero writes to live `.cas/`, per the cas-9d92 discipline.

> **Independent re-verification, 2026-08-08 (cas-7ad6, second worker).** Every row count in this
> table was re-measured against the live DB and every `file:line` in §1 was resolved against the
> current tree. All findings above **hold**; two claims elsewhere in the draft did **not** and are
> corrected in place — see §5.2, which was overstating the strength of the fallback provenance
> edge by roughly 10×. Corrections are marked **[CORRECTED 2026-08-08]** so the delta is auditable
> rather than silently absorbed. Counts throughout are refreshed to the 2026-08-08 measurement;
> none of the refreshes change a conclusion.

So the honest framing is: **this feature is two features, and the second one is worthless without the first.** Structural git/PR/issue indexing (§4) is straightforward and cheap. The provenance join (§5) requires first *repairing* capture paths that exist in code but do not fire. A spec that quietly assumed those tables were populated would ship a search surface that returns `is_ai_generated: false` for every line and calls it an answer — which is exactly the cas-9d92 Phase-1 failure mode ("inferring a missing mechanism from missing data without reading the code"), inverted.

Second-order consequence, and the reason this is worth doing anyway: the cost model (§7) shows backfill for this repo's entire history is **~66 embedding requests, under two minutes, ~11 MB of vectors**. The expensive part of this feature is not the indexing. It is the plumbing repair. Budget accordingly.

---

## 1. Existing-surface survey (AC2 — reuse points, not duplication)

All citations are `file:line` against this worktree.

### 1.1 Code symbol index — REUSE, but it is not running

- Parser: `crates/cas-code/src/parser/mod.rs:53` `MultiLanguageParser`; 5 grammars / 6 `Language` variants (Rust, TypeScript, JavaScript, Python, Go, Elixir) dispatched at `parser/mod.rs:74-83`.
- Store: `crates/cas-store/src/code_store.rs:15` `CodeStore` trait; `SqliteCodeStore` impl in `crates/cas-store/src/sqlite_code_store/trait_impl.rs` (batch insert `:535`, batch get `:604`).
- Tables: `code_files` (`m131`), `code_symbols` (`m132`), `code_relationships` (`m133`), `code_memory_links` (`m134`), indexes `m135`–`m140`.
- Incremental marker today is a **SHA-256 content hash only** — `cas-cli/src/daemon/indexing.rs:108-114` skips a file when `existing.content_hash == content_hash`. There is no mtime and no git-sha watermark.
- `code_files.commit_hash` and `code_symbols.commit_hash` columns **exist and are always written `None`** (`cas-cli/src/daemon/indexing.rs:131`). That is the natural place to hang a git watermark and costs no migration.

**Defects found in this surface that the spec must route around or fix:**
1. Nothing writes the Tantivy code index. `SearchIndex::index_code_symbol` / `index_code_symbols` (`cas-cli/src/hybrid_search/search_index_impl.rs:421`, `:457`) have **zero callers of any kind — not even tests** (`grep -rn index_code_symbol --include=*.rs` returns only the two definitions), so `.cas/index/code` is never created, so `code_search_available` (`cas-cli/src/hybrid_search/code.rs:70-73`) always fails and `code_search` returns a stub telling the user to run a command that does not exist (`agent_search_system/code.rs:58` advertises `cas index code`; nothing in `cas-cli/src/cli/` registers it).
2. `repository` is derived from the file's **parent directory name** (`indexing.rs:102-106`), not the repo root, which defeats the intent of `UNIQUE(repository, path)`.
3. `CodeConfig.enabled` defaults **false** — declared `#[serde(default)]` on a `bool` at `cas-cli/src/config/settings.rs:282-283` — and the daemon tick only fires when `self.activity.is_idle()` (`cas-cli/src/mcp/daemon.rs:552`), so on a busy factory it may never run.

### 1.2 Hybrid search — EXTEND, do not clone

`cas-cli/src/hybrid_search/hybrid.rs` (1,168 lines) already implements six channels: BM25 (`:445`), semantic (`:454`), temporal (`:469`), graph (`:487`), code (`:505`), knowledge (`:526`). Fusion is weighted-sum by default with an RRF mode (`:539-569`), `rrf_k = 60.0`.

The reusable jewel is **capability-honest weighting**: `ChannelCapabilities` (`cas-cli/src/hybrid_search/scorer.rs:146-187`) plus `SearchWeights::for_capabilities` (`scorer.rs:258-279`), which zeroes dead channels and redistributes their mass so live weights always sum to 1.0, falling back to BM25-only. A history channel that cannot embed (no cloud login) therefore degrades correctly *for free* — provided it is registered as a channel rather than implemented as a parallel ranker.

**Hard constraint from M6 (cas-7909).** `docs/migration/cas-b129-m6-legacy-decommission-survey.md` records that M6 **deleted** `crates/cas-search/src/hybrid.rs` and its integration test specifically because it was a *second, unrelated `HybridSearch`* whose `semantic_score` was hardcoded `0.0`. Building a new history-specific ranker would recreate the exact artifact that task just removed. **This spec forbids a new ranker.** It also inherits M6's open items: `mcp__cas__search` never reaches knowledge pages today (chain ends at `hybrid_search/search_index_query.rs:229`, BM25-only, never constructing `HybridSearch`), and every `HybridSearch` constructor sets `knowledge_store: None` (`hybrid.rs:243,255,267,283,303`). A history channel wired the same way would be equally inert — see §6.3.

### 1.3 Embeddings — REUSE; cloud-only, and that is a design constraint

- `cas-cli/src/cloud/embeddings.rs`: `cas-embed-v1`, **1024 dims** (`:58`), batch **32** (`:62`), `POST {endpoint}/api/embeddings` via `ureq`, 30s timeout (`:169-216`).
- **Gated on cloud login, not a cargo feature**: `KnowledgeEmbedder::from_config` returns `None` unless `config.is_logged_in()` (`:118-130`). `None` = semantic channel absent, no LMDB env, no HTTP.
- **There is no local embedding model and reintroducing one is against the grain**: local artifacts were deleted and a janitor still removes `vectors.hnsw` / `models/` on every startup (`cas-cli/src/main.rs:269-296`).
- Vectors live in **LMDB**, not SQLite (`<cas_root>/index/knowledge-vectors/`, `embeddings.rs:308`), over `crates/cas-search/src/lmdb_store.rs:48`. Similarity is **brute-force cosine** (`embeddings.rs:260-276`, `:435-456`); `LmdbVectorStore::search` deliberately errors (`lmdb_store.rs:343-352`).
- Rate limits, recorded from the cloud team: **120 req/60s** on `/api/embeddings`, max 32 inputs (`docs/requests/completed/RESPONSE-cloud-knowledge-sync-and-embeddings.md:162-166`, `:266`). No client-side retry; failures leave `pending_embedding = 1` for the next sync (`:172-176`), which is what makes a 429 safe.
- Model-swap safety already solved: `EmbeddingMeta{provider,model,dims}` mismatch wipes the cache and marks everything pending (`embeddings.rs:322-365`).

**Reuse decision:** the history index adopts the `pending_embedding` flag pattern verbatim. It does **not** get its own embedder, its own LMDB env, or its own retry logic.

### 1.4 Blame / attribution — REUSE the parser, distrust the data

`blame_impl` (`cas-cli/src/mcp/tools/service/agent_search_system/code.rs:231`) shells `git blame --porcelain` (`:6`), parses it (`:272`), then joins each SHA through `commit_link_store.get(hash)` (`:296`, with prefix fallback `:317`) → `link.prompt_ids.first()` → `prompt_store.get(...)` (`:337`). The porcelain parser is good and should be reused.

The join behind it is dead: `commit_links` = 0 rows, so **every line returns `is_ai_generated: false` today**.

### 1.5 Git shell-out — REUSE the existing query patterns

There is no `git2`/libgit2 dependency; everything is `std::process::Command::new("git")`. Two existing sites already do exactly the "changes since a watermark" query this feature needs, and both should be factored rather than rewritten:

- `cas-cli/src/hooks/handlers/handlers_events/codemap.rs:539-553` — `git log <range> --diff-filter=ADR --name-status --format= --no-renames -z` with a NUL-safe parser at `:561`. Already generic.
- `cas-cli/src/hooks/handlers/handlers_events/project_overview.rs:615-620` — `git log -1 --format=%cI -- <path>` as a last-commit watermark, with an explicit in-code rationale at `:605-614` for **rejecting mtime in favour of commit time**. That rationale is precedent for this spec's watermark choice (§4.2).

### 1.6 GitHub — one cache to extend, no tables

- **No GitHub tables exist.** Grepping all 220 migrations for github/issue/pr yields only verification-token vocabulary (`m210`, `m215`).
- Config key `issues.repo` already exists (`cas-cli/src/config/meta/seed/issues.rs:6`).
- `cas-cli/src/hooks/handlers/issue_triage.rs:119` `fetch_issues()` already runs `gh api graphql` against `issues.repo`, caches to JSON in `cas_root` with a **5-minute TTL** (`:113`, `:159`), writes atomically via temp file (`:187`), and sanitizes titles against line injection (`:194`). This is the acquisition path to extend — §8.
- Commit `1ecd9250` added a SessionStart detector that fires when `issues.repo` is unset (`session_hygiene.rs:~630`) and deliberately proposes no value (`:653`). Good precedent for honest degradation.

### 1.7 Doctor — extend, no registry to register with

`cas-cli/src/cli/doctor.rs:44` is one linear `execute()` pushing `Check { name, status, message }` (`:32`) onto a Vec, rendered by `output_checks()` (`:1430`). The canonical modern pattern to copy is the **delivery-retries block at `:259-306`**: three-arm match where empty → `Ok` with a positive statement, non-empty → `Warning` with a bounded top-N (`.take(3)`) plus remediation, `Err` → `Warning "cannot check …: {e}"` — never a silent skip. New tables should also be added to `expected_tables` at `:316`.

### 1.8 Daemon tick — one arm to add

`run_background_loop` at `cas-cli/src/mcp/daemon.rs:366`; intervals constructed `:429-444`; `tokio::select!` arms `:500-566`. A new periodic job is a new arm beside the `code_index_interval` arm at `:552-559`, with the body following `run_code_index_cycle` at `:630` (`spawn_blocking`, errors folded into `status.last_error`).

**There is no post-commit or post-merge git hook.** The only git hook Cassy installs is a `pre-commit` guard (`cas-cli/src/ui/factory/daemon/runtime/teams.rs:1269-1530`). All commit-time capture today is harness-hook-based (PostToolUse Bash → `attribution.rs:169`). The closest template for deferred work is the codemap pending-file drain (`codemap.rs:37`, writing `.cas/codemap-pending.json` JSONL).

### 1.9 Duplication verdict (AC2)

| Capability | Existing owner | This feature |
|---|---|---|
| Symbol extraction | `cas-code::MultiLanguageParser` | reuse unchanged |
| Symbol storage / ids | `CodeStore`, `generate_symbol_id_for` | reuse unchanged |
| Lexical ranking | Tantivy `SearchIndex` | reuse; add a doc type |
| Vector ranking | `KnowledgeEmbedder` + `LmdbVectorStore` | reuse; add a key namespace |
| Channel fusion | `hybrid_search/hybrid.rs` | **extend as a 7th channel — new ranker forbidden** |
| Blame porcelain parse | `agent_search_system/code.rs:272` | reuse |
| git-log-since-watermark | `codemap.rs:539` | factor out, reuse |
| GitHub acquisition | `issue_triage.rs:119` | extend TTL cache |
| Health reporting | `doctor.rs:259-306` pattern | extend |

Nothing in this feature re-implements an existing capability. The only genuinely new machinery is the history tables (§4.1), the epoch ledger (§9), and the query surface (§6).

---

## 2. Evidence base — measured corpus (AC3 inputs)

Measured on this worktree. Values re-measured `2026-08-08`; where the figure moved since the
`2026-08-07` draft the original is shown in parentheses. **No refresh changes a conclusion** — the
cost model (§7) is unaffected at this granularity.

| Quantity | Value | How measured |
|---|---|---|
| Commits (all) | **2,444** (was 2,440) | `git rev-list --count HEAD` |
| — non-merge | 1,651 (was 1,649) | `git rev-list --count --no-merges HEAD` |
| — merge | 793 (was 791) | `git rev-list --count --merges HEAD` |
| History span | 2026-03-11 → 2026-08-08 (~150 days) | first/last commit date |
| Commit message text (all) | **1,671,393 bytes** (was 1,666,723) | `git log --format='%s%n%b' \| wc -c` |
| — non-merge only | 1,486,733 bytes (was 1,482,142) | as above with `--no-merges` |
| Mean subject length | 71.8 chars | `awk` over `%s` |
| (commit, file) pairs, non-merge | **9,489** (was 9,479) | `git log --no-merges --name-only` |
| Distinct files ever touched | 3,727 | `git log --name-only \| sort -u` |
| Tags | 80 | `git tag \| wc -l` |
| CHANGELOG | 158,882 bytes / 969 lines (was 151,352 / 948) | `wc -lc CHANGELOG.md` |
| GitHub issues (all states) | **116** (259,219 bytes title+body) | `gh issue list --state all` |
| Issue comments | 198 | `gh issue list --json comments` |
| Pull requests (all states) | **57** | `gh pr list --state all` |
| Recent velocity | ~80 commits/day (883 over 11 active days, 2026-07-27..08-07) | `git log --since=14.days` |

Live-DB context for the storage decision (§7.3): `cas.db` is **456 MB** (was 435 MB), dominated by
`events` at **978,013 rows** (`supervisor_injected` alone = 748,094). Also present: `file_changes`
7,799, `task_lease_history` 2,235, `sessions` 617, `tasks` 1,484.

The `cas.db` growth of ~21 MB in under a day is itself a data point for §7.3: this feature's entire
proposed footprint (~8 MB) is smaller than one day of ambient `events` growth, which is the
strongest available argument that the sidecar-vs-same-db question is not worth agonising over.

---

## 3. Non-goals

- No diff-hunk embeddings. Diffs are indexed **structurally** (touched files + symbols), per the brief. Hunk text is high-volume, low-signal, and would multiply embedding cost by ~50× for worse recall than the commit message.
- No cross-project or global index. Project-local only (§10).
- No new ranker (§1.2).
- No local embedding model (§1.3).
- No rewriting of history already captured by `events` — the epoch ledger (§9) reads it, it does not replace it.
- **Not a replacement for CODEMAP.md** — see §3.1, which answers a direct operator question.

### 3.1 Codemap coexistence — can git vectorization replace the codemap?

*(Added 2026-08-08 to answer the operator's question of 2026-08-07: whether git vectorization can
replace the codemap.)*

**No, and the reason is architectural rather than a maturity gap: they are opposite retrieval
modes.** Building this feature should not put CODEMAP.md on a deprecation path.

| | CODEMAP.md | Code-history search |
|---|---|---|
| Retrieval mode | **push** — injected at SessionStart, unprompted | **pull** — answers a question you already have |
| Precondition | none; it fires before you know what to ask | you must know what to ask |
| Temporal domain | the repo **as it is now** | how the repo **got this way** |
| Unit | the whole repo's structure, one document | individual commits/issues/PRs |
| Failure when absent | you don't know the codebase's shape and can't form a good question | you can't answer a specific question |

Measured, this worktree: `.claude/CODEMAP.md` is **126 lines / 18,346 bytes** — a deliberately
budget-shaped orientation document, with a compact rendering path explicitly for the SessionStart
size budget (`codemap.rs:245`) and a git-commit-based freshness gate
(`get_structural_changes_after_codemap`, `codemap.rs:493`) that diffs structural adds/deletes/renames
since the commit that last touched the map.

The decisive argument is that **a vector index cannot do the codemap's job at all**, in either
direction:

1. **Push requires no query.** The codemap's entire value is being present *before* the agent knows
   what to ask. A search surface with no query returns nothing. You cannot SessionStart-inject "the
   answer" to an unasked question; the codemap is what makes the first question well-formed.
2. **Orientation is a summary, not a retrieval.** "16 library crates, binary in `cas-cli`, schema
   migrations live in `cas-cli/src/migration/`" is a *synthesis* over the whole tree. Top-k
   retrieval over commit messages returns k commits — it cannot emit a statement about the repo's
   overall shape, because that statement exists in no single indexed document.
3. **The temporal domains do not overlap.** The codemap describes current structure. This index
   describes change. A commit-message index cannot answer "where do schema migrations live" except
   by accident, via whichever commit last mentioned it — which is precisely the stale-answer failure
   the codemap's freshness gate exists to prevent.

**Where they genuinely compose**, and this is the useful part of the operator's question: this
feature can make the codemap's *freshness gate* smarter. The gate today counts structural
adds/deletes/renames since the map's baseline commit (`codemap.rs:493`) — a file-count heuristic
that cannot distinguish a rename sweep from a new subsystem. Once `history_commit_files` and
`history_commit_symbols` exist (M1/M3), the gate could weight drift by whether the changes touch
structure the map actually describes. That is a post-M3 enhancement, deliberately **not** in this
spec's scope, and it is an argument for the two surfaces coexisting rather than one replacing the
other.

**One correction to the framing this section was handed.** The task steer described CODEMAP.md as
"already a view over the knowledge store (per cas-ee3d)". I could not confirm that against the
current tree and am recording the divergence rather than repeating it: `.claude/CODEMAP.md` is a
**tracked markdown file** regenerated by the `codemap` skill (`.claude/skills/codemap`), and
`grep -rn codemap --include=*.rs cas-cli/src/` filtered for knowledge/page/store returns **no
hits** — the handler reads and writes the file and a `codemap-pending.json` sidecar
(`codemap.rs:27`), with no knowledge-store path. If cas-ee3d changed this, it is not visible in the
code I read; if it is planned rather than landed, the distinction matters here because a
knowledge-store-backed codemap *would* share storage with this feature and change the coexistence
analysis. **Flagged for the operator/supervisor to confirm** — it does not change this section's
conclusion, since the push/pull argument is independent of where the map is stored.

---

## 4. Design — incremental structural indexing

### 4.1 Data model

Five new tables, `Subsystem::Code` (registration pattern: `cas-cli/src/migration/migrations/mod.rs:107-120`, `:296-305`).

**`history_commits`** — one row per commit.
`sha TEXT PK` (full 40), `short_sha TEXT NOT NULL` (indexed, for the abbreviated-SHA joins §5.2), `parent_shas TEXT` (JSON array), `is_merge INTEGER NOT NULL`, `author_name`, `author_email`, `authored_at TEXT`, `committed_at TEXT NOT NULL`, `subject TEXT NOT NULL`, `body TEXT`, `branch_hint TEXT`, `repository TEXT NOT NULL`, `pending_embedding INTEGER NOT NULL DEFAULT 1`, `indexed_at TEXT NOT NULL`, `scope TEXT NOT NULL DEFAULT 'project'`.
Indexes: `committed_at DESC`, `short_sha`, partial `(committed_at) WHERE pending_embedding = 1` — mirroring `knowledge_pages` (`crates/cas-store/src/knowledge_store.rs:76-77`).

**`history_commit_files`** — the structural diff mapping.
`sha TEXT NOT NULL` (FK → `history_commits` CASCADE), `file_path TEXT NOT NULL`, `change_type TEXT NOT NULL` (A/M/D/R), `old_path TEXT`, `insertions INTEGER`, `deletions INTEGER`, `PRIMARY KEY (sha, file_path)`. Index on `file_path`.

**`history_commit_symbols`** — the symbol overlap, populated only when the symbol index is live (§11 M2).
`sha TEXT NOT NULL`, `symbol_id TEXT NOT NULL`, `qualified_name TEXT NOT NULL`, `file_path TEXT NOT NULL`, `PRIMARY KEY (sha, symbol_id)`. Index on `qualified_name`.
Derivation: for each changed file, intersect the commit's changed **line ranges** (from `git log --numstat` plus a `-U0` diff for ranges) with `code_symbols.line_start..line_end` for that file at that revision. Degradation is explicit: if `code_symbols` has no rows for the file, write no symbol rows and record the commit as `symbol_mapping = absent` rather than `none` (§10 honesty rule).

**`history_docs`** — GitHub issues/PRs/comments and CHANGELOG entries, one row per embeddable text unit.
`id TEXT PK` (`gh:issue:116`, `gh:pr:57`, `gh:comment:<id>`, `changelog:v2.49.0`), `doc_kind TEXT NOT NULL`, `number INTEGER`, `title TEXT`, `body TEXT`, `state TEXT`, `author TEXT`, `created_at`, `updated_at`, `closed_at`, `url TEXT`, `refs_json TEXT` (extracted SHAs / issue numbers / task ids), `pending_embedding INTEGER NOT NULL DEFAULT 1`, `fetched_at TEXT NOT NULL`, `scope TEXT NOT NULL DEFAULT 'project'`.

**`history_index_state`** — the watermark and honesty ledger (one row per `(repository, source)`).
`repository TEXT NOT NULL`, `source TEXT NOT NULL` (`git` | `github` | `changelog`), `last_indexed_sha TEXT`, `last_indexed_at TEXT`, `last_attempt_at TEXT`, `last_error TEXT`, `backfill_complete INTEGER NOT NULL DEFAULT 0`, `items_indexed INTEGER NOT NULL DEFAULT 0`, `PRIMARY KEY (repository, source)`.

**`history_epochs`** — §9.

### 4.2 Watermark and incrementality (brief item 1)

`history_index_state.last_indexed_sha` is the watermark. A delta pass is:

```
git rev-list --reverse <last_indexed_sha>..HEAD          # new commits, oldest first
git log --no-renames -z --name-status --numstat <range>  # structural diff, NUL-safe
```

reusing the NUL-safe parser at `codemap.rs:561`.

Rules, each with its reason:

1. **Commit SHA, not mtime.** Precedent and rationale already in-tree at `project_overview.rs:605-614`.
2. **Watermark advances only after the whole batch commits transactionally.** A partial batch must re-run, not silently skip; the cas-9d92 root cause was precisely a state pair (`transport_delivered_at` vs `acked_at`) that no path reconciled, leaving rows both "done" and "not done". One watermark, advanced once, avoids reproducing that shape.
3. **If `last_indexed_sha` is not an ancestor of `HEAD`** (force-push, branch switch, rebase), do **not** attempt a delta. Set `backfill_complete = 0` and re-run backfill. Detected with `git merge-base --is-ancestor`.
4. **Backfill is one-time, chunked, and resumable** — 500 commits per transaction, watermark advanced per chunk, so an interrupted backfill resumes rather than restarting.
5. **Session load does a freshness check only** — a single `git rev-parse HEAD` compared against the watermark, reported as a lag number. It never indexes. (Brief item 1.)

### 4.3 Scheduling (brief item 1)

- **Primary: daemon tick.** New `tokio::select!` arm beside `daemon.rs:552-559`, default interval **300s**.
- **Not gated on `is_idle()`.** The existing code-index arm is (`daemon.rs:552`), and the measured consequence is `code_files = 0` on a repo with 2,444 commits — on a busy factory the daemon is never idle, so the job never runs. A delta pass over ~80 commits/day is bounded work; it should be rate-limited, not idleness-gated. This is a deliberate divergence from the neighbouring code path, made because the neighbouring code path demonstrably never fires.
- **Secondary: opportunistic drain.** The PostToolUse `git commit` detector (`attribution.rs:169`) appends the new SHA to a pending file, following the codemap template (`codemap.rs:37`). The daemon drains it. This is an optimisation for freshness; correctness never depends on it, because §4.2's `rev-list` from the watermark catches anything the hook missed.
- **No git hook is installed.** Cassy installs only a `pre-commit` guard today; adding `post-commit`/`post-merge` would collide with per-worktree private hooks dirs (`teams.rs:1510`) and with the factory's shared-hooks path (`:1406`). Rejected as not worth the blast radius.

### 4.4 Embedding what, exactly (brief item 2)

Embedded: commit `subject + "\n" + body` (one vector per commit); issue/PR `title + body`; each issue/PR comment; each CHANGELOG release section.
Not embedded: diffs, file paths, symbol names — those are the **precision** side and are matched structurally (§6.2).

Vector keys namespaced `history:commit:{sha}` and `history:doc:{id}`, stored in the existing LMDB env via `KnowledgeVectorCache` (`embeddings.rs:284`). No new env: `OPEN_ENVS` (`embeddings.rs:292-304`) exists because LMDB refuses a double-open in one process, and a second env keyed to a second path would be a new failure mode for no benefit.

---

## 5. The differentiator — joining to the task/provenance graph (brief item 3)

### 5.1 The intended spine, and its measured state

The brief's join is `commit → task commit_receipt → close/decision notes → session → prompt → blamed line`. Measured:

- `commit_links` **0 rows** — the commit↔session↔prompt edge does not exist.
- `prompts` **1 row**.
- `file_changes` 7,799 rows, **0** with `commit_hash`, **1** with `prompt_id`.
- `commit_receipt` **is not a column**. It is a request-only field (`cas-cli/src/mcp/tools/types/task.rs:229`) resolved at `close_ops.rs:7292` and persisted **as free text** in `tasks.notes` via `append_close_decision_note` (`close_ops.rs:7470`). Only **18** tasks carry the `"resolved to full commit"` string.

Why `commit_links` is empty is worth stating because it constrains the fix: `detect_and_link_git_commit` (`attribution.rs:169`) fires only from the **PostToolUse Bash hook** on a recognised `git commit` command (`is_git_commit_command`, `:606`), and takes an early return for non-Claude harnesses whose `tool_response` is a bare string. In a factory running mixed harnesses, most commits never reach it.

### 5.2 What is actually populated, and the join this spec uses instead

| Edge | Usable rows | Caveat |
|---|---|---|
| `tasks.deliverables` → `factory_branch_anchor` | **229** full SHAs, all distinct | written at commit time by `attribution.rs:515`; cleared on reopen (`task_store.rs:590`) |
| `events` where `event_type='worker_git_commit'` | **984** of 10,298 rows — see below | `session_id` 100% populated, but the SHA is usually absent and is a **variable-width** prefix when present |
| `tasks.notes` close decisions | 19 | free text, substring-matched |
| `task_lease_history` | 2,235 | who held the task when |
| `sessions` | 617 | `cwd`, `branch`, `worktree_id`, `outcome` |

**[CORRECTED 2026-08-08] The `worker_git_commit` edge is ~10× weaker than the 2026-08-07 draft
claimed, and its key is not the shape the draft assumed.** The draft presented it as "10,279 rows,
`head_sha` abbreviated to 8 chars (`crates/cas-factory/src/director.rs:343`)". Re-measurement:

| `worker_git_commit` row class | Count |
|---|---|
| `metadata` **entirely NULL** — no SHA at all | **9,268** (90.0%) |
| usable, `head_sha` **7 chars** | 594 |
| usable, `head_sha` **8 chars** | 390 |
| `head_sha = '?'` (documented degradation stub, `factory_ops.rs:6185`) | 46 |
| **total** | **10,298** |

Usable: **984 rows / 474 distinct SHA prefixes.** Both of the draft's specifics were wrong:

- **The cited emission site is not one.** `director.rs:343` is `EventType::WorkerGitCommit` appearing
  inside a `worker_activity_types` array used to pick a worker's latest activity for the TUI — it
  reads the event, it does not write it. The actual writer is `emit_worker_final_git_state`
  (`cas-cli/src/hooks/handlers/handlers_middle/session_stop/mod.rs:528`, emitting at `:589`), whose
  payload comes from `collect_worker_git_status` (`cas-cli/src/mcp/tools/service/factory_ops.rs:6015`).
- **The width is not fixed at 8.** That function computes the SHA as
  `run_git(worktree_path, &["rev-parse", "--short", "HEAD"])` (`factory_ops.rs:6021`). `--short` uses
  git's *dynamic* abbreviation length, which grows with object count — it returns 8 in this repo
  today and returned 7 earlier in its history, which is exactly the 594/390 split above. Any
  implementation that slices `sha[0..8]` will silently fail to match the 594 seven-char rows.

Three consequences the implementation must carry:

1. **Match by variable-length prefix, not a fixed slice.** The join predicate is
   `history_commits.sha LIKE event_head_sha || '%'` with the event's own length, never
   `substr(sha,1,8) = head_sha`.
2. **Exclude the `'?'` stub explicitly.** It is a legitimate "git status unavailable" sentinel, not a
   SHA; treating it as one would match nothing and look like a coverage gap rather than a
   degradation signal.
3. **The collision guard needs recomputing for 28 bits, not 32.** A 7-char prefix is 28 bits: for
   2,444 commits the any-collision probability is `1 - exp(-2444² / 2·2²⁸)` ≈ **1.1%**, roughly
   **16× the 0.07%** the draft computed for the 8-char case (that 8-char figure is itself correct).
   This does not change the design — §5.2's rule was already "return all matches with
   `ambiguous: true`, never silently pick the first" — but it moves ambiguity from a theoretical
   footnote to something a test must actually cover, and it means the guard is load-bearing rather
   than defensive.

**This strengthens rather than weakens the spec's thesis.** The draft's §0 argument is that the
provenance join is a schema without data; the fallback edge it proposed instead turns out to be 90%
empty too. The `factory_branch_anchor` edge (229 full, exact, unambiguous SHAs) is therefore the
*only* high-confidence provenance edge that exists today, and §10.1's `provenance_coverage_pct`
(~9%) is close to the true ceiling until M5 lands — not a number that the `worker_git_commit`
fallback was ever going to rescue. M5 (§5.3) is correspondingly more important, not less.

**Design decision: the join is resolved at query time over these populated edges, into a `history_commit_provenance` view, with per-edge confidence — not by assuming `commit_links`.**

```
commit.sha
  ├── exact  → tasks.deliverables.factory_branch_anchor = sha            (confidence: high)
  ├── prefix → events.worker_git_commit metadata.head_sha = sha[0..8]    (confidence: high; see below)
  ├── text   → tasks.notes LIKE '%<sha-prefix>%'                          (confidence: medium)
  └── none   → provenance: null, reason: "no populated edge"              (never a silent empty)
```

The prefix join needs a guard, and **[CORRECTED 2026-08-08]** the guard must be sized for the
*shortest* prefix in the corpus, not the longest. 2,444 commits over an 8-hex-char (32-bit) space
gives a birthday collision probability of roughly `1 - exp(-2444² / 2·2³²)` ≈ **0.07%**; over the
7-char (28-bit) space that 594 of the 984 usable rows actually use, it is `1 - exp(-2444² / 2·2²⁸)`
≈ **1.1%**. Both grow with the square of history size. Rule: the prefix join must `SELECT` all
matches and **return them all with `ambiguous: true`** when more than one full SHA matches, never
silently pick the first — and it must match on the event's own prefix length rather than a fixed
slice. `history_commits.short_sha` is indexed precisely to make this cheap; because the stored
width is dynamic, the index is on the prefix column with `LIKE prefix || '%'` semantics rather than
an equality join against a hardcoded `substr(sha,1,8)`.

### 5.3 The repair, stated as a dependency not a wish

Two of this feature's headline queries (§6.4 Q4, Q6) are only as good as the provenance edges. Milestone **M5** (§11) repairs `commit_links` population by moving the commit→session link off the harness-specific Bash-hook path and onto the daemon indexer itself: when the indexer ingests a commit, it resolves the session by joining `committed_at` + `branch` against `sessions` and `events.worker_git_commit`, and writes a `commit_links` row with an explicit `link_method` so a reconstructed link is never confused with an observed one.

M5 is **optional for shipping M1–M4** and mandatory before any claim that Cassy can answer "which prompt caused this line". Until M5 lands, the query surface must report `provenance_coverage` as a measured percentage (§10), not omit the field.

**[CORRECTED 2026-08-08] M5's priority is raised by §5.2's re-measurement.** The draft could argue M5 was deferrable because a 10,279-row fallback edge would carry most queries in the meantime. That edge is 984 usable rows. There is no interim substitute for the repair: without M5, provenance answers rest on 229 exact anchors, and Q4 in particular should be regarded as *not yet supported* rather than merely degraded. Recommendation to sign-off (§12 Q1) is unchanged — keep M5 in the epic — but it should be sequenced as early as its dependencies allow rather than treated as a tail milestone.

---

## 6. Query surface (brief item 5, AC4)

### 6.1 Shape

- **MCP:** `mcp__cas__search action=history`. Dispatch is a new arm in the existing match at `cas-cli/src/mcp/tools/service/mod.rs:734-735`; request fields on the existing `SearchContextRequest`.
- **CLI:** `cas history search <query>` plus `cas history status` (watermark/lag) and `cas history backfill`. This also finally gives `cas index code` a home — §11 M2 registers it, closing the "command advertised at `code.rs:58` that does not exist" defect.

Parameters: `query`, `path` (filter/boost), `symbol`, `since`/`until`, `kind` (`commit|issue|pr|changelog`), `task_id`, `session_id`, `limit`, `include_provenance`.

### 6.2 Ranking — hybrid recall + structural precision

Registered as a **7th channel in the existing `HybridSearch`** (`hybrid.rs`), not a new ranker (§1.2):

- **Recall (embedding):** cosine over `history:*` vectors, exactly as the semantic channel does today.
- **Recall (lexical):** Tantivy BM25 over commit subject/body and doc title/body, via a new `DocType::HistoryCommit` / `DocType::HistoryDoc`, written through the **batch** `index_code_symbols`-style path — never the singular per-item form, which commits per item (`search_index_impl.rs:421` vs `:457`).
- **Precision (structural):** path-overlap and symbol-overlap boosts from `history_commit_files` / `history_commit_symbols`, applied multiplicatively in the style of the existing graph/code boosts (`hybrid.rs:572-607`): `score *= 1 + w · overlap`.
- **Recency:** the existing temporal channel's `0.5^(days/30)` decay (`hybrid.rs:712-736`) applies unchanged.

Because the channel registers through `ChannelCapabilities` (`scorer.rs:146`), a machine with no cloud login loses the embedding half and the remaining weights renormalize to sum 1.0 (`scorer.rs:258-279`) instead of silently scoring everything 0.0 — the exact bug M6 deleted a whole module for.

### 6.3 Wiring risk, called out explicitly

M6's survey records that the knowledge channel is **inert in production**: every `HybridSearch` constructor passes `knowledge_store: None` (`hybrid.rs:243,255,267,283,303`), `enable_knowledge` defaults `false` (`:115`), and the sole production construction site (`cas-cli/src/hooks/scorer.rs:32,:38`) sets neither — so `knowledge_weight: 0.25` does nothing. A history channel added the same way would be equally dead.

**Acceptance gate for M4:** an integration test must assert that the *production* construction path returns a history result for a known commit — not merely that the channel returns results when hand-constructed in a unit test. Without that gate this feature ships inert and looks fine.

### 6.4 Example queries (AC4 — ≥6, with the mechanism each exercises)

**Q1 — "Why does `blame_impl` shell out to git instead of using libgit2?"**
Embedding recall over commit bodies + PR descriptions; symbol-overlap precision boost on `blame_impl`. Returns the commits touching that symbol with their messages and the PR that introduced it.

**Q2 — "What changed in the delivery state machine in the last two weeks?"**
`since=14d` + path filter on the delivery modules; temporal channel; returns commits grouped by touched file with subjects. Exercises the structural index with no embedding dependence.

**Q3 — "Show me every commit that touched `prompt_queue_store.rs` and the task each belonged to."**
Path filter → `history_commit_files` → §5.2 provenance join. Output includes `link_method` and `confidence` per row; commits with no populated edge appear with `provenance: null, reason: "no populated edge"` rather than being dropped.

**Q4 — "Which session and prompt produced the `acked_via='hook_surfaced'` stamping?"**
Symbol/text match → commit → provenance join → session → prompt. **This is the query that is degraded until M5** (§5.3); until then it answers at whatever `provenance_coverage` reports and says so in the response.

**Q5 — "Has anyone tried to fix the redelivery hot-loop before?"**
Embedding recall across commit messages *and* `history_docs` (issues #166 and its comments, plus CHANGELOG entries). Cross-source recall is the thing SQL/grep cannot do — grep for "hot-loop" misses a commit that says "stop re-emitting per poll tick".

**Q6 — "Is the idle-gate message loss still a problem?"** *(the binary-epoch query — see §9)*
Resolves to: (a) the commits/PR that claim the fix (issue #167 → linked PR → merge commit), (b) the epoch in which the running binary containing that commit started serving, (c) whether any occurrence of the symptom postdates that epoch boundary. Answer is one of **STILL-LIVE / FIXED-VERIFIED / FIXED-UNVERIFIED / INSUFFICIENT-POST-FIX-DATA**, with the boundary timestamp and the post-boundary sample size stated.

**Q7 — "What files does a change to `SearchWeights::for_capabilities` usually come with?"**
Co-change analysis over `history_commit_files`: files most frequently appearing in the same commit. Pure SQL over the structural index, no embeddings.

**Q8 — "Which of my closed tasks shipped code that has since been reverted or rewritten?"**
Task → `factory_branch_anchor` → commit → files/symbols → later commits touching the same symbols with a larger deletion count.

### 6.5 Response contract

Every response carries an `index_status` block: `{last_indexed_sha, lag_commits, lag_seconds, backfill_complete, provenance_coverage_pct, semantic_available, last_error}`. This is not optional metadata — see §10.

---

## 7. Cost model (AC3)

### 7.1 Backfill — one-time, for this repo

Embeddable units: 1,651 non-merge commits + 116 issues + 198 comments + 57 PRs + ~80 CHANGELOG release sections ≈ **2,100 units**. (Merge commits are indexed structurally but not embedded — their messages are `Merge branch 'x'`, which is noise. This drops 791 units, 32% of commits, for zero recall loss.)

- **Text volume:** ~1.48 MB commit messages + 0.26 MB issue bodies + ~0.15 MB CHANGELOG + comments ≈ **~1.9 MB ≈ ~480 K tokens**.
- **Requests:** 2,100 ÷ 32 per batch = **~66 requests**.
- **Wall clock at the 120 req/60s limit:** floor of **~33 seconds**; realistically **60–120 s** including HTTP round-trips at a 30 s timeout ceiling. Structural git parsing of 2,444 commits (`git log --numstat`, one pass) adds **~5–15 s**.
- **Conclusion: full backfill of this repo's entire history is a sub-two-minute, ~66-request operation.** This is the finding that makes the feature viable; it is also why chunked resumability (§4.2 rule 4) is cheap insurance rather than a burden.

### 7.2 Steady-state delta — per day

At the measured ~80 commits/day (~54 non-merge after the 32% merge share), plus ~2 issues/PRs and their comments: **~60 units/day ≈ 2 requests/day ≈ under 2 seconds**. A 300 s tick handles ~11 new commits per pass at current velocity — comfortably one batch.

Cost is therefore **negligible in steady state** and dominated entirely by the one-time backfill. No sampling, no retention pressure, no need for a cost cap in v1.

### 7.3 Storage

- **Vectors:** 2,100 × 1024 dims × 4 bytes = **8.6 MB**, plus LMDB overhead ≈ **~11 MB**. Grows ~0.25 MB/day. The LMDB env is already sized at 10 GB (`lmdb_store.rs:39`).
- **SQLite rows:** `history_commits` 2,444 (with bodies ≈ 1.7 MB) + `history_commit_files` 9,489 + `history_commit_symbols` ~15–20 K (estimated at ~2 symbols per changed file) + `history_docs` ~450 (~0.4 MB) ≈ **~5–8 MB including indexes**.
- **Decision: same `cas.db`, not a sidecar.** `cas.db` is 435 MB and 977,847 of its rows are `events` (748,002 of them a single event type). Adding ~30 K rows and ~8 MB is **under 2% growth** and well inside the noise of the existing events table. A sidecar DB would buy nothing and would cost cross-database joins on exactly the task/session joins that are this feature's entire point (§5). If `cas.db` size becomes a problem, the correct fix is `events` retention, not this table set.
- **Retention:** none in v1. Commit history is bounded by the repo and does not grow without bound relative to the repo itself; at ~0.25 MB/day of vectors, a year is ~90 MB. Revisit if a consuming repo has >50 K commits — recorded as an open question (§12).

---

## 8. GitHub data acquisition (brief, spec section)

- **Source:** extend `issue_triage.rs:119`'s `gh api graphql` call rather than adding a second GitHub client. `issues.repo` (`config/meta/seed/issues.rs:6`) is the single source of the owner/name.
- **Cadence:** the daemon history tick fetches GitHub at most every **15 minutes** (a separate, longer interval than the 300 s git tick — GitHub data changes slower and is rate-limited by a third party). The existing 5-minute TTL cache (`issue_triage.rs:113`) is left alone for the SessionStart banner; the indexer reads the same cache when fresh and refetches when not.
- **Incrementality:** GraphQL query filtered by `updated_at > last_indexed_at` from `history_index_state('github')`. Only changed issues/PRs are re-embedded (their `pending_embedding` reset to 1). Closed-and-unchanged issues cost nothing.
- **Offline / unauthenticated behaviour:** if `gh` is absent, unauthenticated, or `issues.repo` is unset, the git half of the index runs normally and `history_index_state('github')` records `last_error`. The response contract (§6.5) surfaces it. **Never a silent partial index** — this is the same discipline the new SessionStart detector uses when `issues.repo` is unset (`session_hygiene.rs:653`: report, propose nothing, do not guess).
- **Rate limits:** `gh api graphql` is subject to GitHub's 5,000 points/hour; a filtered incremental query at 15-minute cadence is ~96 queries/day. Non-issue.
- **PR ↔ commit linkage:** GraphQL returns each PR's merge commit SHA and its commits; these populate `history_docs.refs_json` and give Q6 its "which PR shipped this commit" edge without heuristics.

---

## 9. Binary-epoch awareness (brief item 4 — the cas-9d92 AC7 lesson)

This is the part that makes "is symptom X still a problem" answerable, and it is the part most likely to be got wrong, because cas-9d92 got it wrong once and had to retract a headline finding.

**The lesson, quoted from the evidence:** a fix's *tag date* is not when the fix started running. cas-9d92 Phase 1 read a 33.5% → 19.3% undelivered-rate drop as v2.49.0's fix working; the retraction (task note, 2026-08-07 21:14) established the binary was installed at 21:02:26Z while **pre-install daemons kept heartbeating until 21:36:37Z**. The window 21:02:26–21:36:35 is a **MIXED** epoch serving both binaries and must not be read as post-fix. The supervisor ruling (task note 21:59) adopted **21:36:35Z as the canonical clean-post boundary**, superseding an earlier 21:31:50Z factory-restart stamp.

**`history_epochs` table:**
`id INTEGER PK`, `epoch_kind TEXT` (`binary_install` | `daemon_start` | `daemon_last_heartbeat`), `binary_path TEXT`, `binary_mtime TEXT`, `version TEXT`, `started_at TEXT`, `ended_at TEXT`, `pid INTEGER`, `exe_deleted INTEGER`, `recorded_at TEXT`.

**Population:** on every daemon start, record `daemon_start` with the binary's mtime, reported version, and whether `/proc/<pid>/exe` still resolves to a live inode. Historical rows are backfilled from `daemon_instances` and `events`.

**Epoch classification, three-valued not two:**
```
CLEAN-PRE   : t < first_install_of(version)
MIXED       : first_install_of(version) <= t < last_heartbeat_of_any_older_binary
CLEAN-POST  : t >= last_heartbeat_of_any_older_binary
```
The MIXED window is **never** counted as post-fix. This is a hard rule in the query layer, not a convention.

**Verdict logic for Q6:**

| Condition | Verdict |
|---|---|
| symptom occurs in CLEAN-POST | **STILL-LIVE** |
| no CLEAN-POST occurrence, and CLEAN-POST sample ≥ threshold | **FIXED-VERIFIED** |
| no CLEAN-POST occurrence, sample < threshold | **INSUFFICIENT-POST-FIX-DATA** |
| fix commit exists, no CLEAN-POST epoch yet | **FIXED-UNVERIFIED** |
| symptom absent CLEAN-POST, present CLEAN-PRE, only MIXED data between | **INSUFFICIENT-POST-FIX-DATA** |

The `INSUFFICIENT-POST-FIX-DATA` verdict is not a hedge; it is the direct encoding of cas-9d92's own stated limit ("clean-post epoch is 17 rows / ~45 min: decisive for the unreconciled pair, too small to certify anything RESOLVED"). A system that collapses that case into FIXED-VERIFIED will reproduce the retracted finding automatically, at scale.

**Threshold:** configurable, default **100 post-boundary observations of the relevant class**. Both the verdict and the sample size are always returned; the caller is never handed a bare "fixed".

---

## 10. Staleness, failure honesty, privacy

### 10.1 Never silently stale

- Every query response carries `index_status` (§6.5). A stale index answers *and says how stale*, in commits and seconds.
- **New `cas doctor` check, "code history index"**, following the `doctor.rs:259-306` pattern exactly: `Ok` when lag is under one tick interval and `backfill_complete = 1`; `Warning` with the top-3 offending sources when lag exceeds it, when `last_error` is set, or when backfill is incomplete; `Warning "cannot check code history index: {e}"` on store-open failure — never a silent skip.
- New tables added to `expected_tables` (`doctor.rs:316`).
- `provenance_coverage_pct` is computed and reported, not assumed. Today it would read **~9%** (229 anchored SHAs of 2,444 commits) on the high-confidence edge alone. **[CORRECTED 2026-08-08]** The draft implied the `worker_git_commit` edge would lift this substantially; §5.2's re-measurement shows it contributes at most 474 distinct prefixes, 90% of that event class carrying no SHA at all — so realistic total coverage is bounded well under 30% even counting medium-confidence edges, and the *high*-confidence figure stays ~9% until M5. Publishing both numbers, split by confidence, is the point: it makes §5.3's repair a visible, measurable debt rather than an invisible one.

### 10.2 Failure modes and their declared behaviour

| Failure | Behaviour |
|---|---|
| No cloud login | Embedding channel absent; capability renormalization (`scorer.rs:258`) keeps lexical + structural live; `semantic_available: false` in every response |
| `gh` missing / unauthenticated / `issues.repo` unset | Git half indexes normally; `history_index_state('github').last_error` set and surfaced |
| Watermark not an ancestor of HEAD | Backfill re-run (§4.2 rule 3); never a silent gap |
| Partial batch failure | Watermark not advanced; batch retried (§4.2 rule 2) |
| `code_symbols` empty | `symbol_mapping = absent` recorded per commit; symbol-overlap boost contributes 0 rather than silently matching nothing |
| Ambiguous 8-char SHA prefix | All matches returned with `ambiguous: true` (§5.2) |

### 10.3 Privacy and scope

- **Project-local only.** All tables carry `scope TEXT NOT NULL DEFAULT 'project'`, matching every existing code/knowledge table. No global-tier (`~/.cas/cas.db`) write path.
- Commit messages, issue bodies, and PR descriptions are sent to the cloud embedding endpoint. Per the vendor record, the server persists **no vectors and no input text**, logging only counts/model/duration (`docs/requests/completed/RESPONSE-cloud-knowledge-sync-and-embeddings.md:302-305`). This must be stated in user-facing docs; it is a behaviour change for anyone who assumed history stayed local.
- **Diffs are never sent off-machine** — a direct consequence of the structural-not-textual diff decision (§3), which therefore doubles as a privacy property.
- Embedding is skipped entirely when not logged in, so an offline/air-gapped user gets a fully functional lexical + structural index with nothing leaving the machine.

---

## 11. Implementation plan (AC5)

Each milestone is independently landable and independently useful. Estimates are worker-sessions.

| # | Milestone | Scope | Est. | Depends |
|---|---|---|---|---|
| **M1** | Structural git index | `history_commits`, `history_commit_files`, `history_index_state` + migrations; backfill + delta walker; factor the NUL-safe git-log parser out of `codemap.rs:539-561`; daemon tick arm; `cas history backfill` / `cas history status` | 2 | — |
| **M2** | Revive the symbol index | Register `cas index code` (closes the advertised-but-absent command, `code.rs:58`); wire `SearchIndex::index_code_symbols` (batch form) from the indexer so `.cas/index/code` exists; fix `repository` derivation (`indexing.rs:102-106`); populate `commit_hash`; decide `CodeConfig.enabled` default and the `is_idle()` gate | 2 | — (parallel to M1) |
| **M3** | Symbol mapping | `history_commit_symbols`; changed-line-range ↔ symbol-range intersection; `symbol_mapping = absent` degradation | 1.5 | M1, M2 |
| **M4** | Query surface | `action=history` + `cas history search`; 7th channel in `hybrid_search/hybrid.rs`; `DocType::HistoryCommit/HistoryDoc`; `index_status` contract; **production-path integration test (§6.3 gate)** | 2 | M1 |
| **M5** | Provenance join + repair | `history_commit_provenance` resolution over the populated edges (§5.2) with `link_method`/`confidence`; **variable-width** prefix matching (`LIKE prefix \|\| '%'`, never `sha[0..8]`) with the `'?'` stub excluded and an ambiguity test at 7 chars; repair `commit_links` population from the daemon indexer with explicit `link_method` | 2.5 | M1, M4 |
| **M6** | GitHub + CHANGELOG docs | `history_docs`; extend `issue_triage.rs:119` GraphQL incrementally; CHANGELOG release-section parser; PR↔commit refs | 1.5 | M1 |
| **M7** | Embeddings | `pending_embedding` drain reusing `KnowledgeEmbedder`; `history:*` LMDB key namespace; capability registration | 1.5 | M1, M4, M6 |
| **M8** | Binary epochs + verdicts | `history_epochs`; daemon-start recording; `daemon_instances`/`events` backfill; three-valued classifier; Q6 verdict logic incl. `INSUFFICIENT-POST-FIX-DATA` | 2 | M1, M4 |
| **M9** | Honesty surfaces | `cas doctor` check; `expected_tables`; `provenance_coverage_pct`; failure-mode table (§10.2) implemented and tested | 1 | M1, M4, M5 |

**Total ≈ 16 worker-sessions.** Suggested order: M1 ∥ M2 → M4 → (M3, M6) → M5 → M7 → M8 → M9.

**Minimum useful slice:** M1 + M4 alone answers Q2, Q3 (structurally), and Q7 with no cloud dependency and no provenance repair. That is a real, shippable surface and a good first epic boundary.

---

## 12. Open questions for sign-off (AC6)

1. **Is M5 (provenance repair) in scope for this feature, or a separate epic?** It is the brief's stated differentiator, but it is a repair of existing broken capture, not new search machinery. Recommendation: keep M5 in the epic, land it after M4, and gate any "which prompt caused this" claim on it.
2. **`CodeConfig.enabled` default and the `is_idle()` gate (M2).** Flipping the default to `true` changes behaviour for every existing install. Removing the idleness gate changes daemon load. Both are needed for this feature to function; both are operator calls.
3. **Cloud embedding of commit messages and issue bodies is a data-egress change.** Explicitly acceptable, or should the embedding half be opt-in behind a config key?
4. **Retention beyond this repo's scale.** No retention is proposed for ≤50 K commits. A consuming repo with 500 K commits would need ~2 GB of vectors. Defer, or design the cap now?
5. **Merge-commit exclusion from embedding** (32% of commits, §7.1). Squash-merge workflows put real content in merge commits; this repo does not. Should the exclusion be heuristic (skip only `^Merge (branch|pull request)`) rather than structural?

6. **[ADDED 2026-08-08] Is ~9% high-confidence provenance coverage enough to ship the provenance half at all?** §5.2's correction removes the fallback edge that made a pre-M5 interim story plausible. Two honest options, and this is a product call rather than a technical one:
   **(a)** Ship M1+M4 as a pure code-history surface, present provenance as explicitly unsupported until M5, and file M5 as prerequisite work — the query surface never advertises a capability it cannot deliver.
   **(b)** Make M5 a blocking phase-0 so the feature launches with the differentiator working.
   Recommendation: **(a)**. It matches the "minimum useful slice" already identified in §11 (M1+M4 answers Q2, Q3, Q7 with no provenance dependency), it gets a working surface in front of users sooner, and it keeps the honesty contract intact via `provenance_coverage_pct`. (b) front-loads 2.5 sessions of repair before anything is demonstrable, which is the shape that tends not to land.

   **Trigger condition that flips (a) → (b).** Recording this in advance so the decision is
   falsifiable rather than defended after the fact, in the same discipline cas-9d92 used for its
   F3 fold. Adopt **(b)** if, once M1+M4 are landed and measurable, **either**:
   - **the query evidence fails** — of the eight §6.4 example queries, more than two cannot be
     answered acceptably on the substitutes alone. Q4 is *expected* to fail and does not count
     toward the threshold; it is already declared unsupported pre-M5. The test is whether failure
     spreads beyond Q4 into queries this spec claims are provenance-independent (Q2, Q3, Q7);
   - **or the coverage evidence collapses** — measured `provenance_coverage_pct` on the
     high-confidence edge falls below ~5%, or the `factory_branch_anchor` edge stops growing
     (it is cleared on task reopen, `task_store.rs:590`, so it can shrink), indicating the
     substitutes decay faster than history accumulates.

   Conversely, the observation that would **confirm (a)** and let M5 be deferred further: Q2/Q3/Q7
   answer well in practice and users do not ask provenance questions often enough for the ~9%
   ceiling to bite. That is measurable from query logs once M4 ships, and it should be measured
   rather than assumed — this spec's own §5.2 correction is a reminder of what happens when an edge's
   strength is asserted from a row count nobody re-checked.

7. **[ADDED 2026-08-08] Should `emit_worker_final_git_state` record the full 40-char SHA?** 90% of `worker_git_commit` rows carry no SHA at all and the rest carry a dynamic-width abbreviation (§5.2). Writing the full SHA at `factory_ops.rs:6021` (`rev-parse HEAD` rather than `rev-parse --short HEAD`) is a roughly one-line change that would make every *future* row an exact-match edge and retire the collision guard for new data. It does not fix the 90%-NULL problem or backfill history, so it is not a substitute for M5 — but it is cheap enough that it may be worth doing independently of this epic, and the decision belongs with whoever owns that emitter.

---

## 13. Acceptance-criteria trace

| AC | Where |
|---|---|
| 1 — spec covering all sections, evidence per choice | this document; every design choice carries a `file:line`, a measured number, or a cas-9d92 citation |
| 2 — existing-surface survey, no duplication, reuse named | §1, table §1.9 |
| 3 — cost model with real numbers | §7 (backfill ~66 requests / <2 min / ~11 MB; delta ~2 requests/day), inputs measured in §2 |
| 4 — query surface, ≥6 examples incl. binary-epoch | §6.4 (eight queries; Q6 is the binary-epoch one, logic in §9) |
| 5 — implementation plan, independently landable, estimated | §11 (9 milestones, ~16 sessions, dependency order + minimum useful slice) |
| 6 — supervisor sign-off before implementation tasks are filed | **PENDING** — §12 lists the seven decisions needed |
| operator Q — can git vectorization replace the codemap? | §3.1 (no; push vs pull orientation, with the composition opportunity named) |
| supervisor steer — default to (a), state the other option's trigger | §12 Q6 (recommendation (a) + explicit flip conditions and the confirming observation) |

**Verification trace (2026-08-08).** AC1 requires each design choice to carry evidence. That
obligation was tested rather than assumed: all live-DB counts were re-measured read-only, and 18
`file:line` citations were sampled and resolved against the tree, of which 17 matched exactly.
Corrections applied: `settings.rs:286`→`:282-283`; `hybrid.rs` `knowledge_store: None` line list;
`index_code_symbol[s]` has zero callers including tests (draft understated); §2 counts refreshed;
and the two substantive §5.2 errors (fallback-edge volume and SHA width/emission site). The
`[CORRECTED 2026-08-08]` markers exist so a reviewer can audit the delta instead of re-deriving it.
