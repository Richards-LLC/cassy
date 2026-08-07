# TencentDB-Agent-Memory review — what CAS should steal

Date: 2026-08-06. Source: repo zip extracted to
`~/Petrastella/research/tencentdb-agent-memory/TencentDB-Agent-Memory-feat-server_team/`
(Tencent's open-source agent-memory system, feat-server_team branch, ~840 files, TS/Node).
Five read-only spike agents reviewed MemoryCore, MemoryKnowledge, MemoryProxy, the
SDK/plugin integration surface, and mapped the CAS baseline for comparison. No code was
installed or run; all claims below were verified at file:line by the spikes.

## System shape

Four services: **MemoryCore** (L0–L3 memory engine + skill engine, Node `node:sqlite`),
**MemoryKnowledge** (Wiki + CodeGraph, better-sqlite3 + Drizzle), **MemoryProxy**
(transparent Anthropic/OpenAI proxy — the actual Claude Code integration; agents just
repoint `ANTHROPIC_BASE_URL`), **MemoryPanel** (human console). Memory hierarchy:
L0 raw conversation → L1 structured atoms (both SQLite) → L2 scenario blocks → L3 persona
(both **markdown files written by sandboxed tool-using LLM agents**, not SQLite).

## The "novel SQLite" verdict

Less vector wizardry than the marketing implies, but real techniques:

- **sqlite-vec `vec0`** for cosine KNN (MemoryCore only), with four production lessons:
  an auxiliary `updated_time` column *inside* the vec0 table so TTL expiry is a direct
  `DELETE ... WHERE updated_time < ?` (no ID join); a hard "never insert zero vectors"
  invariant (vec0 yields NULL distance, unfilterable in-query — they over-fetch +10 to
  absorb legacy rows); an `embedding_meta` `{provider, model, dims}` table that
  auto-drops + reindexes on model change; and **`dimensions = 0` as a first-class
  no-embeddings mode** — vec0 tables never created, extension never loaded, store runs
  FTS-only. Default shipped config is `provider: "none"`, i.e. BM25-only.
- **Write-side tokenization + dumb FTS5**: run jieba (MemoryCore) or a JS CJK tokenizer
  (MemoryKnowledge) at write time, store space-joined tokens in an `unicode61` FTS5
  column, keep raw text in a parallel `UNINDEXED` column, apply the same tokenizer to
  queries. Full custom tokenization with zero C extensions.
- **All tenancy columns mirrored into FTS5 as `UNINDEXED`** so post-recall filtering
  needs no join-back; 5× over-fetch when filtered (neither vec0 nor FTS MATCH supports
  predicate pushdown).
- **Hybrid = RRF k=60**, FTS + vector in parallel, each leg independently degradable.
  **No re-ranker, no recency/decay in ranking** — their `priority` field and timestamps
  are stored and rendered but never scored. (CAS's decay/stability model is ahead here.)
- **DB-per-namespace + LRU connection pool** (MemoryKnowledge): one `index.db` per wiki
  next to its markdown; RAM bounded by open connections × page cache, not corpus size
  (replaced an in-heap index that OOM'd at 20GB). Pooled readers; ephemeral
  single-transaction writers ending in `wal_checkpoint(TRUNCATE)`.
- **Content-hash source ledger** (`source(filename PK, sha256, status, ...)`) + a pure
  classifier deciding what re-hits the LLM; index rebuilt wholesale in one transaction
  together with per-source status, so "what's indexed" and "what we believe was
  ingested" can never diverge. Failed extractions retry free on next run.
- Partial unique indexes for soft-delete idempotency (`UNIQUE(...) WHERE deleted_at IS
  NULL`) and single-table skill versioning (`WHERE is_head=1 AND status='active'`).
- FTS5 schema versioning by **marker-column probing** (`pragma_table_info` for one
  sentinel column per version; any gap → DROP + rebuild from base tables). No migration
  ledger for derived data.
- **Ratio circuit-breaker on TTL deletion**: refuse any expiry pass that would delete
  >80% of rows; cutoff must be in the past and ≥24h back.

## The organizing principle: prompt-cache economics

The system went through three generations visible side by side in the repo:
(1) per-turn auto-recall injected before each user message → (2) same, plus a stable
system-prompt appendix → (3) **MemoryProxy: per-turn L0/L1 injection removed entirely**.
Gen 3 injects only byte-stable content at session init (L3 persona verbatim, L2 as a
path + ≤200-char summary *index*, tool guides), frozen in a "hook cache" so the system
prompt prefix is byte-identical every turn and never busts Anthropic KV caching. L0/L1
become pull-only read tools (curl recipes against an auth-injecting bridge; the proxy
overwrites identity fields server-side so the model can't forge them and tokens never
enter the prompt). Their own doc flags gen 2 as unresolved; new systems should start at
gen 3: **inject the index, toolize the body**.

## Capture discipline (the other big lesson)

- **Round-level, not turn-level write-back**: only flush when the assistant reply has
  zero tool calls → "1 human Q&A = 1 memory append"; documented failure mode of the
  naive version is 30+ spurious archives per session.
- **Structural harness-call classification**: main/fork/sidequery derived from the
  position of Claude Code's `cache_control` marker; all memory writes and injection
  gated on it, so title-generation/compaction calls never pollute memory.
- **Anti-feedback-loop rule, enforced in code**: never persist a user message still
  containing the injected `<relevant-memories>` block — a `before_message_write` hook
  strips it and capture restores the cached clean text. Otherwise next turn's search
  runs against your own injection and memory self-amplifies into mush.
- **Role-isolation armor for the extractor**: transcripts fed to the skill-review LLM
  are wrapped in `<<past-user>>`/`<<past-assistant>>`/... markers with explicit
  instructions to ignore system-reminder/rules/memories blocks inside, plus a
  "you are being role-captured — STOP" self-catch.
- **Convergence over accumulation**: the extractor's input is a growing snapshot, so
  "Nothing to save." is a first-class success output; saving with nothing new is
  defined as degrading the library. Candidates pass a 5-way classification gate
  (Skill/Memory/Wiki/CodeGraph/Temporary) and a numeric rubric (30/25/20/25, total ≥72,
  no dimension <12) before any write.
- **Retrieval-augmented dedup**: batch-embed new memories → KNN each against existing
  (vector → FTS → skip degradation) → one batched LLM call returns
  `store|update|merge|skip` per record; every failure path defaults to `store`.
- **Trigger engineering**: exponential warm-up (extract after 1, 2, 4, 8, … convs — new
  sessions get memories immediately, cost decays) and a **downward-only timer** for
  consolidation (fire time only moves earlier; encodes max-interval, post-activity
  delay, min-interval floor in one primitive; cold sessions auto-cancel).
- **asset_reflection**: opt-in injected block asking the model to end its answer with an
  honest per-tool "did this memory actually help" self-report — a near-free memory-ROI
  attribution signal.

## Verified caveats (don't cargo-cult)

README benchmark (PersonaMem 48%→76%) has no harness/config/data in-repo. Governance
states (`candidate`/`approved`, reviewer role) exist in types with zero enforcement.
Two drifting RRF implementations. Dead `communities` API + unused Louvain dep. No
persistent job queue in MemoryKnowledge (in-memory only; restart = mark failed).
`priority`/recency dead weight in ranking. Deployment doc is a major version stale.

## CAS gap map → ranked adoption list

CAS baseline (verified in cas-src): local semantic search is a stub returning empty
(`cas-cli/src/hybrid_search/hybrid.rs:599`) yet the scorer still allocates Conceptual
queries 60% to it; dedup/consolidation purely lexical (tag-grouped consolidation);
auto-pipelines (session-learn, learning/rule review, dup detection) all default off;
retrieval feedback recorded but never feeds ranking; global-scope injection disabled.
What CAS is ahead on: per-entry decay/stability/spaced-repetition, tiering, entity-graph
channel, rules/skills promotion + filesystem sync.

1. **Make no-embeddings a first-class mode** (small, immediate). Stop advertising a
   6-channel hybrid with a dead channel; renormalize Conceptual/semantic weights to live
   channels, gate on capability like their `provider:"none"` mode. If we later revive
   local vectors, do it with sqlite-vec + their four tricks (aux TTL column, zero-vector
   invariant, embedding_meta auto-reindex, dims-0 mode).
2. **Inject the index, toolize the body** (SessionStart). Replace top-N full-content
   memory injection with pinned/persona content verbatim + a compact index (title + hook
   line per memory) + an explicit pull instruction; agent fetches bodies via
   `mcp__cas__memory get` / `search`. More recall per token, and the injected block is
   naturally session-stable.
3. **Upgrade dedup to retrieval-augmented + batched LLM judge**. Keep the BM25 candidate
   recall we have; replace the 5-dimension token scorer verdict with one batched Haiku
   call returning store/update/merge/skip; fail open to store. Same pattern fixes
   consolidation's tag-only grouping (recall candidates by BM25 instead of tags).
4. **Harden and default-on session-learn**: add role-isolation transcript armor (our
   transcripts are full of system-reminders — real prompt-injection surface for the
   extractor), the 5-way classification gate + numeric rubric, and "Nothing to save" as
   success. Convergence discipline is what makes default-on safe.
5. **Round-level capture gating**: buffer PostToolUse observations but only synthesize
   at zero-tool-call assistant boundaries; classify and skip harness-internal
   sessions/calls (our subagent + sidechain traffic today writes observations like any
   other).
6. **Wire feedback into ranking**: retrieval_feedback currently feeds nothing; an
   asset-reflection-style Stop-hook self-report ("which injected/recalled memories
   actually helped") mapped onto helpful/harmful counters closes the loop cheaply.
7. **Trigger polish**: exponential warm-up for extraction thresholds; downward-only
   timer for daemon consolidation; ratio circuit-breaker (+past-cutoff sanity) on
   `apply_decay`/`auto_prune` before they can mass-archive on a clock skew.
8. **(Speculative, bigger)** FTS5-with-host-tokenization as a Tantivy replacement:
   tokenize in Rust (tantivy analyzers or unicode-segmentation), store space-joined
   tokens in FTS5, mirror filter columns UNINDEXED. Would drop the 50MB writer budget,
   the separate index directory, and the swap dance for one DB with transactional
   index+data writes. Needs a benchmark spike before committing.

Zeitgeist takeaway: the current wave is *not* vectors-everywhere — Tencent ships
BM25-only by default and spends its engineering on prompt-cache-shaped injection,
LLM-in-the-loop distillation/dedup with strict acceptance gates, and capture hygiene.
CAS's cloud-only semantic split isn't behind the times; our real gaps are injection
shape, dedup quality, extractor safety, and the feedback loop.
