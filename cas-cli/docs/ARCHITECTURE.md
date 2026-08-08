# CAS Architecture

## Workspace Layout

The root `Cargo.toml` defines a workspace. `cas-cli/` is the main binary crate; `crates/` contains library crates.

**Core data flow**: CLI commands and MCP tool calls both go through the store trait abstractions in `cas-cli/src/store/`, which wraps `cas-store` (SQLite) with notification and sync layers.

### cas-cli (main crate) — `cas-cli/src/`

| Module | Purpose |
|--------|---------|
| `main.rs` / `lib.rs` | Entry point, module declarations |
| `cli/` | Clap command definitions and handlers. `mod.rs` has the `Commands` enum — add new subcommands here. |
| `mcp/` | MCP server: `server/` (CasCore with cached OnceLock stores), `tools/` — 12 action-dispatched meta-tools (`memory`, `task`, `rule`, `skill`, `coordination`, `search`, `system`, `verification`, `knowledge`, `team`, `pattern`, `spec`; plus `mcp_search`/`mcp_execute` when a proxy is configured), each fanning out to handlers in `core/` and `service/` — `daemon.rs` (embedded background maintenance), `socket.rs` (notification socket) |
| `store/` | Re-exports from `cas-store` + wrappers: `notifying_*.rs` (emit change notifications), `syncing_*.rs` (sync to `.claude/` filesystem), `layered.rs` (project + global store composition), `detect.rs` (find `.cas/` root) |
| `hooks/` | Claude Code hook event handlers (SessionStart, Stop, PostToolUse, etc.). `handlers/` has session, state, event, and middleware handlers. `scorer.rs` ranks context items for injection. |
| `migration/` | Forward-only schema migrations. `migrations/` has individual migration files (m001–m218). `detector.rs` introspects existing schema. |
| `knowledge/` | LLM distillation pipeline for the project wiki: `sources.rs` (what is distillable), `chunk.rs`, `prompt.rs` (role-isolation armor), `llm.rs` (provider-CLI runner + `ScriptedLlm` mock), `merge.rs` (cost-tiered merge), `pipeline.rs` (`run_distillation`) |
| `ui/` | Ratatui TUI components for factory view: `factory/`, `components/`, `widgets/`, `theme/`, `markdown/` |
| `config/` | Configuration loading from `.cas/config.toml` (TOML is the only format written; a legacy `.cas/config.yaml` is migrated once and renamed to `config.yaml.bak`) |
| `orchestration/` | Agent name generation and orchestration logic |
| `worktree/` | Git worktree management for factory workers |
| `consolidation/` | Memory consolidation and decay |
| `extraction/` | AI-powered extraction of observations into structured memory |
| `bridge/` | Local helper server for external tool integration |
| `cloud/` | CAS Cloud sync (optional) |
| `sync/` | Filesystem sync to `.claude/rules/` and `.claude/skills/` |

### Workspace Crates — `crates/`

| Crate | Purpose |
|-------|---------|
| `cas-types` | Shared data types (Entry, Task, Rule, Skill, Agent, etc.) |
| `cas-store` | SQLite storage layer — trait definitions (`Store`, `TaskStore`, `RuleStore`, `KnowledgeStore`, etc.) and their SQLite implementations. `knowledge_store.rs` is the odd one out: it keeps page *index rows* in SQLite and page *bodies* as markdown on disk. |
| `cas-search` | Search infrastructure: `Bm25Index` (Tantivy), `LmdbVectorStore` (heed) and score-combination helpers. Local search is **BM25-only** — the vector store and `HybridSearch` are wired but have no local embedder, so semantic ranking is cloud-gated. |
| `cas-core` | Core business logic, hooks framework, search index abstraction, skill/rule syncing |
| `cas-mcp` | MCP protocol types and request/response models |
| `cas-factory` | Factory session lifecycle: `FactoryCore`, config, director, recording, notifications |
| `cas-factory-protocol` | WebSocket message protocol between supervisor and worker agents |
| `cas-mux` | Terminal multiplexer layout and rendering (side-by-side/tabbed agent views) |
| `cas-pty` | PTY management for agent terminal sessions |
| `cas-recording` | Terminal session recording and playback |
| `cas-code` | Code analysis via tree-sitter |
| `cas-diffs` | Diff parsing, rendering, syntax highlighting |
| `cas-tui-test` | TUI testing framework |
| `ghostty_vt` / `ghostty_vt_sys` | Virtual terminal parser (based on Ghostty) |

### The memory surfaces — one map

CAS stores agent-facing knowledge in seven distinct surfaces. They are not tiers of one thing; each has its own store, its own write path, and its own retrieval channel. The recurring confusion this table exists to end is "which one do I write to, and who will ever read it back?"

| Surface | What it holds | Stored where | Written by | Read back by |
|---------|---------------|--------------|------------|--------------|
| **Entries** (memories) | Free-form learnings, preferences, observations, opinions. Belief-typed (`Fact` / `Opinion` / `Hypothesis`) with a confidence score. | `entries` table in `.cas/cas.db` | `memory` MCP tool, `cas add`, extraction from sessions | `search` MCP tool, SessionStart injection via `hooks/scorer.rs` |
| **Rules** | Normative constraints ("always…", "never…") with optional path globs and auto-approve grants. | `rules` table; proven rules also synced to `.claude/rules/cas/` | `rule` MCP tool, `cas rule` | Claude Code reads the synced markdown directly; also BM25-searchable |
| **Skills** | Procedural playbooks — a `SKILL.md` body plus references. | `skills` table; synced to `.claude/skills/`. Builtins ship from `cas-cli/src/builtins/skills/` in three harness flavors (claude / codex / grok). | `skill` MCP tool, `cas skill`, builtin sync on `cas init` | The harness loads them as Agent Skills; also BM25-searchable |
| **Entities** | Extracted proper nouns (person, project, technology, file, concept, …) and their mentions — the join layer between prose and code. | `entities`, `entity_mentions`, `relationships` | `search action=entity_extract`, background extraction | `search action=entity_list` / `entity_show` |
| **Code index** | Symbols and files parsed by tree-sitter, plus code↔memory links. | `code_symbols`, `code_files`, `code_relationships`, `code_memory_links` | Daemon code-index cycle (60s), `cas index` | `search action=code_search` / `code_show` / `grep` |
| **Knowledge pages** | The distilled project wiki: LLM-written prose about *this repo*, with source provenance and a user-sovereignty lock. | Index rows in `knowledge_pages` + `knowledge_sources`; **bodies are markdown files on disk** under `.cas/knowledge/<type>/<title>.md` | `cas knowledge build` (distillation), `knowledge action=write` (hand-authored, always `locked=1`) | `knowledge` MCP tool (`search`/`read`/`list`), `cas knowledge search|read` |
| **Patterns** | Cross-project personal/team conventions. | **Not local** — CAS Cloud, reached over the `/api/patterns` HTTP surface | `pattern` MCP tool | `pattern` MCP tool (requires login) |

Two properties of the knowledge surface are load-bearing and easy to get wrong:

- **Bodies never enter SQLite.** `knowledge_pages_fts` is a *contentless* FTS5 table (`content=''`, `contentless_delete=1`) over title + snippet + body: the inverted index lives in the DB, the prose only ever lives on disk. That is what makes the pages greppable with ordinary tools and keeps the database small.
- **`locked` is a one-way promise to the user.** `commit_ingest` never sets or clears `locked` and its upsert is guarded by `WHERE knowledge_pages.locked = 0`, so distillation can neither overwrite a locked page nor lock one the user didn't. `set_locked` is the only way the bit moves; the `knowledge action=write` MCP handler goes unlock → write → lock precisely because that guard is real.

`.claude/CODEMAP.md` and `docs/PRODUCT_OVERVIEW.md` are **views over this surface, not a parallel one**: both are ordinary distillable sources, so the `codemap` and `project-overview` skills query `cas knowledge search` before regenerating and run `cas knowledge build` after writing, which turns each doc into a page plus a source-ledger entry.

**Search reality check:** every "search" above except the knowledge surface goes through the Tantivy BM25 index (`cas-core/src/search/`, doc types `entry`/`task`/`rule`/`skill`/`spec`/`code_symbol`/`code_file`). The knowledge surface has its own SQLite FTS5 index instead. **Neither is semantic on its own.** There is no local embedder; the semantic channel exists only when the cloud does — see below.

### The local/cloud boundary for project knowledge

**Local is the source of truth. The cloud is an optional enhancement, never a dependency.** Concretely:

| Concern | Owner |
|---|---|
| Pages, bodies, provenance, the `locked` bit | **Local** — SQLite + markdown on disk. Fully functional with no account, no network, no cloud build. |
| Embedding vectors | **Cloud computes, local caches.** `cloud/embeddings.rs` posts page text to `/api/embeddings` and stores the vectors in an LMDB cache under `.cas/index/knowledge-vectors/`. |
| Team distribution of pages | **Cloud transports.** `cloud/syncer/knowledge.rs` pushes/pulls pages over the existing `/api/sync` endpoints. |

Rules that keep that boundary honest:

- **No auth ⇒ no calls, no files, no channel.** `KnowledgeEmbedder::from_config` returns `None` when logged out, and every caller treats `None` as "this installation has no semantic channel" rather than a degraded mode. No LMDB environment is created. This is the same shape as the `dims = 0` provider-absent pattern: unconfigured storage is never materialised.
- **`has_semantic()` tells the truth.** `HybridSearch::has_semantic` is true only when a channel is attached *and* vectors are actually cached. A configured-but-empty channel still reports false, so `SearchWeights::for_capabilities` keeps redistributing that weight to the live channels instead of allocating mass to a channel that can only return nothing.
- **Never cache a zero vector.** A provider that fails soft would otherwise poison the cache with a vector equidistant from every query. Zero vectors are rejected and the page keeps `pending_embedding = 1`, so the next run retries it.
- **A model change forces a reindex.** The cache is tagged with `{provider, model, dims}`; on mismatch it is wiped and `mark_all_pending_embedding` re-arms every page. Vectors from two models are not comparable, and mixing them corrupts ranking silently.
- **The `locked` bit rides the wire and is honoured on arrival.** Incoming pages are applied through `commit_ingest`, whose `WHERE knowledge_pages.locked = 0` guard means a teammate's copy can no more overwrite a page you locked than distillation can. A page locked upstream arrives locked.
- **Pulled pages always arrive `pending_embedding = 1`.** A teammate's vector lives in a teammate's cache; this machine embeds the page itself or it is semantically invisible here.
- **Embedding requests chunk at 32 inputs, and that cap is a constant, not a literal.** `MAX_EMBED_INPUTS_PER_REQUEST` is the endpoint's hard cap; `embed_pending_pages` splits its page budget into chunks of it and `embed_batch` refuses a longer input list outright. `DEFAULT_EMBED_BATCH` is a *page* budget per invocation, a different number with a different job. They were once both `32` at two call sites, which is exactly how "one request with every page in it" survived: the cap looked enforced and wasn't.
- **An embedding run that did not do its job says so.** A request failure is reported through `EmbedReport::request_errors` with the unattempted pages counted as `deferred`, never downgraded to a `tracing::warn!`. `cas cloud sync` prints the problem and the store-wide `pending_after` count, so "0 embedded" can never read as "nothing to do". A `404`/`501` is classified as `capability_absent` — a boundary of the installation to state plainly, not an error to alarm about.
- **One response shape.** `/api/embeddings` returns flat `{"embeddings": [[..]]}`. The client accepts that and nothing else: an OpenAI-style `data[].embedding` fallback used to make an unrelated `data` array parse as a list of empty vectors rather than fail.
- **The knowledge pull carries `team_id` when there is one.** Project scope alone does not partition teams: a user in two teams that share one `project_canonical_id` would otherwise pull the union of both teams' pages. A teamless install sends no `team_id` and is unaffected.

- **Every pulled row is re-checked against the local project before it is written.** The knowledge pull runs the same `entity_matches_project` guard as every other entity type — one shared definition of "is this row mine", not a second implementation that can drift. Foreign and unscoped rows are refused *at ingest* and counted in `KnowledgePullReport::refused_foreign` with their ids; they are never written and never silently dropped. This matters more for pages than for anything else: pages merge on `rel_path` into both `cas.db` and disk, so a foreign page with a colliding path would overwrite the local body, and `knowledge_pages` has no project column to audit afterwards. Detection after the fact cannot undo that write.
- **A push the server refused does not advance the watermark.** The knowledge push has no per-row queue to leave un-marked, so the watermark is its only retry lever: advancing it past a page the server rejected under the locked guard would mean that page is never offered again until a human happens to edit it.
- **The pull watermark is the server's `pulled_at`, never the local clock.** Client wall-clock skew would silently widen or narrow the next `since` window, and rows created in the gap would never be pulled again. When the server sends no `pulled_at` the mark is left alone, so the next pull re-requests the same window rather than skipping it.

**Canonical-id equality is byte-exact, on both sides of the wire.** This is a protocol invariant, not an implementation detail, and it is pinned by `canonical_id_equality_is_byte_exact_by_protocol` in `pull.rs`. Two things follow:

- Normalizing (lowercasing, trimming, stripping a trailing `/`) would not be a convenience — it would **merge two distinct projects** permanently and unattributably. The server deliberately refused to normalize for the same reason.
- The client-side check is the **second** line of defence. The server filters on the id the client *sends* and echoes the stored column, so an id divergence never presents as a rejected row: the client gets an **empty envelope, indefinitely, with no warning on either side**. Silent starvation, not contamination. When sync "returns nothing", suspect an id mismatch upstream rather than assuming the row filter is eating data.

**What page sync does NOT cover** (boundaries by construction, not gaps to paper over):

- **There is no page delete over sync.** The generic `DELETE /api/sync/{type}/{id}` route accepts `knowledge_page`, but the server keeps no tombstone for it: it removes only the caller's own row, records nothing, and cross-row dedupe can re-deliver the same page on the very next pull. Building delete on that route would produce a resurrection loop that looks like a client bug. Deletion stays local until the server has tombstones.
- **Account- and global-scope pages have no wire identity.** `project_id` is `NOT NULL` server-side and the client fails closed without a canonical id, so a page outside a project simply does not sync.

### Key Patterns

**Store trait hierarchy**: `cas-store` defines traits (`Store`, `TaskStore`, `RuleStore`, `SkillStore`, `EntityStore`, `AgentStore`, `VerificationStore`, `WorktreeStore`). `SqliteStore` implements all of them. `cas-cli/src/store/` wraps these with notification and sync decorators.

**CasCore (MCP server)**: Lives in `cas-cli/src/mcp/server/mod.rs`. Caches all store instances in `OnceLock` fields — each store type opened exactly once per server lifetime. Has an embedded daemon for background maintenance: code re-index every 60s, agent heartbeat every 30s, full maintenance every 30min, cloud sync on its own interval. It does **not** generate embeddings — `daemon::indexing::run_embedding_cycle` is a no-op stub kept for signature compatibility.

**`cas serve` project-root resolution** (`cas-cli/src/mcp/server/runtime.rs::resolve_mcp_serve_root`): Priority order: (1) `CLAUDE_PROJECT_DIR` env var — Claude Code 2.1.139+ sets this when spawning a stdio MCP server, eliminating cwd-mismatch failures; (2) `CAS_ROOT` env var (explicit override); (3) git-worktree detection; (4) directory walk from cwd. Falls back silently to (2)–(4) when `CLAUDE_PROJECT_DIR` is unset or points at a non-existent path.

**CasContext**: In `cas-cli/src/store/mod.rs`. Resolves the `.cas/` directory once at CLI entry points and passes it through — enables deterministic test behavior.

**Hook scoring**: `cas-cli/src/hooks/scorer.rs` ranks context items (memories, tasks, rules, skills) by relevance for injection into SessionStart context, staying within a token budget.

**Factory worker commit guard** (`cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs`, `check_worker_git_commit_scope`): Fires for ALL factory workers (`CAS_AGENT_ROLE=worker` + `CAS_FACTORY_MODE`) on every `git commit` / `git merge` Bash command. Denies commits to protected branches (`main`, `master`, `staging`, detached HEAD) regardless of whether the worker has an isolated worktree (`CAS_CLONE_PATH`). Isolated workers also get a cwd-outside-worktree guard. Non-isolated (standalone-task) workers that run in the shared primary checkout are therefore prevented from committing to `main` (cas-ba04 fix). The only bypass is switching to a non-protected branch — `--no-verify` does NOT bypass this guard (it only skips git hooks, not the Claude Code PreToolUse harness).

**Team scope resolution chain** (`cas-cli/src/cloud/config.rs::active_team_id`, cas-ea2f5): When a write is dual-enqueued to the team push queue, the team UUID is resolved at `open_store` time via a four-step chain. (0) Kill-switch: if `team_auto_promote = Some(false)` in the project `.cas/cloud.json`, the result is always `None` — no team dual-enqueue regardless of other config. (1) Project-level explicit override: `team_id` in the project `.cas/cloud.json` wins unconditionally; set via `cas cloud team set <uuid>`. (2) User default: `default_team_id` in `~/.cas/cloud.json`, populated by `cas cloud team default <slug>` or automatically by `fetch_and_cache_teams` (`cloud/me.rs`) on `cas login`. (3) Implicit single-team auto-pick: if `teams[]` has exactly one entry and no `default_team_id` is set, that team is used automatically — no configuration needed. (4) `None` — ambiguous (0 or ≥2 teams without a nominated default) or not logged in. The testable inner `active_team_id_with_user_config(user_cfg: Option<&CloudConfig>)` accepts an injected user config for unit tests without disk I/O; the production `active_team_id()` reads from `user_level_cloud_json_path()` (honours the `CAS_USER_CLOUD_JSON` test-seam env var).
