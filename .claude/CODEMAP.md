# cas — Codemap
> Auto-generated structural map. Regenerate with `/codemap` when the layout drifts (modules added, removed, or renamed).

Rust workspace for the CAS coding-agent system. Product/domain material belongs in `docs/PRODUCT_OVERVIEW.md`; this file is a navigational index.

## Top-level layout
- `.cargo/` — workspace Cargo configuration and platform build settings.
- `.claude/` — rendered Claude agents, settings, workflows, and this codemap.
- `.codex/` — rendered Codex agents, hooks, and local configuration.
- `.config/` — checked-in test-runner configuration (`nextest.toml`).
- `.github/` — CI/release workflows, reusable actions, and public issue templates.
- `cas-cli/` — binary crate composing the CLI, MCP server, hooks, cloud sync, and factory.
- `contrib/` — installable shell helpers and their test harness.
- `crates/` — shared Rust libraries for storage, search, factory, terminal, MCP, and types.
- `docs/` — architecture, design, operations, research, release, and project records.
- `fixtures/` — checked-in retrieval-parity baselines and query sets.
- `homebrew/` — Homebrew formula and formula update helper.
- `hub-web/` — TypeScript/Vite Commander web client and browser tests.
- `migration/` — historical cloud-move logs, reports, and systemd material.
- `ops/` — deployable systemd units and launch wrappers.
- `scripts/` — install, release, CI policy, scoped-test, worktree, and build-cache helpers.
- `site/` — static project landing page and system PDF.
- `slack-bridge/` — standalone Node/TypeScript Slack router and per-user daemon.
- Root config/docs — `Cargo.toml`, `README.md`, `CLAUDE.md`, `AGENTS.md`, `CONTRIBUTING.md`, and `.mcp.json`.

## Workspace / packages
- `cas-cli` — binary `cas`; composes all service crates and owns user-facing commands.
- `crates/cas-types` — shared domain, wire, provenance, task, agent, memory, hook, and verification types.
- `crates/cas-store` — SQLite stores, queues, history, knowledge, archives, verification, and vector persistence.
- `crates/cas-search` — hybrid BM25/semantic retrieval, code search, LMDB persistence, grep, and scoring.
- `crates/cas-core` — hook contexts/transcripts, memory hygiene, temporal search, extraction, dedup, and sync.
- `crates/cas-code` — multi-language code analysis, parsing, chunking, and indexing support.
- `crates/cas-mcp` — MCP daemon configuration, protocol types, and server support.
- `crates/cas-mcp-proxy` — policy-aware upstream MCP proxy engine and health tracking (`code-mode-mcp`).
- `crates/cas-factory` — worker spawning, provider/lane routing, spec resolution, directors, probes, and sessions.
- `crates/cas-factory-protocol` — factory client/server wire protocol, codecs, compression, and transport.
- `crates/cas-mux` — in-process terminal multiplexer, pane routing, harness backends, and injection.
- `crates/cas-pty` — PTY creation/configuration and Claude, Codex, Grok, and OpenCode conformance.
- `crates/cas-recording` — asciinema-style terminal recording format, readers, writers, and export.
- `crates/cas-diffs` — diff parsing, inline rendering, widgets, and syntax highlighting.
- `crates/cas-tui-test` — PTY-backed TUI runner, screen assertions, input sequences, and artifacts.
- `crates/ghostty_vt` and `crates/ghostty_vt_sys` — safe Rust terminal wrapper and low-level Ghostty FFI.

## cas-cli/src — application hub
`main.rs` starts the CLI; `lib.rs` exports application modules, the `panic = "unwind"` guard, and test-environment boundaries.
- `cli/` — clap dispatch for auth, cloud, config, factory, hub, knowledge, memory, provider, status, update, and worktree flows.
- `cli/factory/` — factory lifecycle, daemon attach, probes, parity checks, queries, worktrees, and wedged-worker recovery.
- `cli/hook/` — hook event dispatch and generated hook configuration; `cli/sync/` renders managed agent files.
- `cli/{codemap_cmd,project_overview_cmd,knowledge_cmd}.rs` — documentation freshness gates and knowledge operations.
- `cli/{history_cmd,index_cmd,retrieval_parity}.rs` — Git history search, code indexes, and retrieval parity commands.
- `cloud/` — cloud sync, devices, teams, proposals, embeddings, aliases, and queued push/pull coordination.
- `config/` — settings, runtime hooks, access policy, metadata registry, and seeded coordination/daemon/history/QA sections.
- `daemon/` — background maintenance, decay, observation, source watching, indexing, and bounded relevance evaluation.
- `history/` — incremental Git history index, changelog/refs, FTS search, provenance, symbols, and epochs.
- `hooks/handlers/` — session-start context/budget/hygiene, PreToolUse/PostToolUse, stop handling, and issue triage.
- `hybrid_search/` and `knowledge/` — lexical/semantic/code composition, caches, knowledge-source selection, and distillation.
- `store/` and `migration/` — layered stores/sync wrappers plus numbered SQLite migrations and migration orchestration.
- `worktree/` — discovery, Git operations, target locking, external links, salvage, sweep, and cleanup.
- `bridge/server/` — HTTP/SSE bridge handlers used by `cas bridge serve`; `hub/` owns Commander pairing and fleet runtime.
- `mcp/` — always-available daemon, Unix socket, MCP request routing, prompts, resources, and tool handlers.
- `orchestration/`, `notifications/`, `telemetry/`, `tracing/`, `otel.rs`, `sentry.rs` — coordination and observability plumbing.
- `agent_id.rs`, `capability.rs`, `harness_policy.rs` — identity, capability, and provider-harness policy boundaries.
- `factory_{context_reset,isolation,preflight,target_cache}.rs` plus `factory_*` guards — factory safety and host/build checks.
- `retrieval_eval.rs` and `retrieval_parity/` — labeled scoring against committed retrieval fixtures and diffs.

## Factory coordination surfaces
- `cas-cli/src/ui/factory/app/` — bare-`cas` TUI state, panels, selection, rendering, worker/epic views, and worktree actions.
- `cas-cli/src/ui/factory/director/` — mission/task/worker coordination, prompts, reminders, events, radar, and supervisor-stall tests.
- `cas-cli/src/ui/factory/daemon/` — PTY-owning daemon; `runtime/` covers lifecycle, delivery, CI watch, relay, teams, queue/events, and merge sweep.
- `cas-cli/src/ui/factory/{boot,client,protocol,server_registry,session}.rs` — startup, client transport, protocol, server lifecycle, and session state.
- `crates/cas-factory/src/{routing,spec_resolver,probe,director,config}.rs` — provider/lane registry, worker specs, probes, directors, and configuration.
- `crates/cas-factory/src/session/` — worker session lifecycle, resume state, and focused session tests.
- `crates/cas-factory/policy/lane-registry.toml` — checked-in provider lanes and capability registry consumed by routing.

## MCP service and tool tree
- `cas-cli/src/mcp/{daemon,socket,server}/` — daemon lifecycle, Unix transport, runtime, parent watchdog, prompts, and resources.
- `cas-cli/src/mcp/tools/core/` — task, memory, knowledge, search, rules, skills, workflow, system, opinion, maintenance, and coordination handlers.
- `cas-cli/src/mcp/tools/core/task/` — task queries, notes, proposals, dependencies, updates, and lifecycle proof/close gates.
- `cas-cli/src/mcp/tools/service/` — factory/reminder ops, liveness, orphan recovery, external verification, server/worktree ops, patterns/specs, and panic containment.
- `cas-cli/src/mcp/tools/service/agent_search_system/` — agent search, code/history/context retrieval, messaging, and supervisor queue operations.
- `cas-cli/src/mcp/tools/service/worker_liveness/` — worker receipt parsing and liveness tests; `opencode_liveness.rs` covers OpenCode probes.
- `cas-cli/src/mcp/tools/types/` — shared request/response schemas for task, search, system, looping, rules/skills, verification, and worktrees.
- `cas-cli/src/mcp/tools/{mod.rs,mod_tests.rs,traffic_limits.rs}` — tool registration, action-surface tests, and dispatch limits.

## Builtins, tests, and supporting clients
- `cas-cli/src/builtins/` — embedded managed prompts, agents, skills, harness mirrors, and `reference-history.json` sync manifest.
- `cas-cli/src/builtins/agents/` — canonical duplicate, learning, rule, session, and task-verifier agent prompts.
- `cas-cli/src/builtins/{codex,grok}/{agents,skills}/` — provider-specific prompt trees; `codex/` also carries factory-supervisor prompts.
- `cas-cli/src/builtins/skills/` — canonical shared skills, references, examples, scripts, and design/release assets rendered to harness mirrors.
- `cas-cli/tests/` — integration targets for CLI, hooks, cloud, factory/MCP, hub, search, verification, e2e, and multi-agent behavior.
- `cas-cli/tests/mcp_tools_test/` — MCP action coverage; `task_tools/` holds lifecycle, dependency, close-gate, and verification suites.
- `cas-cli/tests/e2e/` — factory, hooks, memory/rules, multi-agent, tasks, teams, verification, and worktree flows.
- `cas-cli/tests/{factory_parity,factory_codex_skill_guardrails,factory_mcp_ops}_test.rs` — factory parity and worker-facing contract gates.
- `cas-cli/tests/{builtin_archive_portability,builtin_doc_hygiene,agent_definition_contract,skill_hygiene}_test.rs` — managed prompt/skill hygiene gates.
- `cas-cli/tests/{retrieval_eval,retrieval_parity,project_identity_parity}_test.rs` — retrieval and cross-surface parity checks.
- `cas-cli/tests/{common,e2e,fixtures,support,snapshots,proptest}/` — shared fixtures, helpers, snapshots, and property tests.
- `hub-web/src/` — Commander SPA state, pairing, sessions, panes, attention, messaging, terminal adapters, and Vitest tests.
- `slack-bridge/src/` — Slack router/daemon entrypoints, commands, sessions, filtering, formatting, and tests.

## Crates — key module roots
- `cas-store/src/{task_store,prompt_queue_store,supervisor_queue_store,spawn_queue_store}.rs` — durable task and coordination queues.
- `cas-store/src/{agent_store,knowledge_store,history_store,code_vector_store,retrieval_store}.rs` — agents, knowledge, history, vectors, and retrieval outcomes.
- `cas-store/src/{verification_store,external_verification_gate,surfaced_artifact_store,version_store}.rs` — verification, artifacts, and rule/skill versions.
- `cas-core/src/{hooks,memory,search/temporal,sync,extraction}/` — hook input, memory hygiene, temporal search, managed sync, and extraction.
- `cas-search/src/{bm25,code_search,lmdb_store,grep,parallel,scorer,traits}.rs` — text/code indexes, persistence, grep, retrieval parallelism, and scoring.
- `cas-types/src/{provenance,task,agent,delivery,verification,spec}.rs` — lineage and core coordination records.
- `cas-mcp/src/{daemon.rs,types/}` — embedded MCP daemon lifecycle and protocol/type definitions.
- `cas-mux/src/{backend,pane}/` plus `mux.rs`, `pty.rs`, `render.rs`, and `harness.rs` — terminal backends and pane operations.
- `cas-pty/{src,conformance}/` — runtime adapters and pinned harness contract receipts.
- `cas-recording/src/`, `cas-diffs/src/`, `cas-tui-test/src/` — terminal recordings, diff widgets, and TUI test support.
- `ghostty_vt_sys/{build_support.rs,tests/portable_target.rs}` — FFI build support and portable-target enforcement.
- Crate-local `tests/`, `benches/`, and inline `#[cfg(test)]` modules provide lower-level integration and unit coverage.

## Cross-cutting
- **Managed files:** sources live in `cas-cli/src/builtins/`; sync renders `.claude/`, `.codex/`, and Grok mirrors.
- **Docs:** `cas-cli/docs/` holds architecture/contributing/migration/proxy/TUI/worktree material; `docs/` holds durable project records.
- **CI/release:** `.github/`, `scripts/`, `CHANGELOG.md`, `docs/release-notes/`, and `docs/release-reports/` hold gates and receipts.
- **Operations:** `migration/`, `ops/systemd/`, `docs/branch-protection/`, `docs/ci/`, and `docs/factory/` hold host/runbook material.
- **Tests:** colocated Rust tests, crate `tests/`, `cas-cli/tests/`, and Vitest suites under `hub-web/src/`.
- **Test isolation:** use `TestEnvGuard` from `cas-cli/src/lib.rs`; do not add a second HOME/environment helper.
- **Generated/local state:** `target/`, `node_modules/`, `hub-web/dist/`, `.cas/`, and vendored sources are not source-map entries.

## Entrypoints
- CLI: `cas-cli/src/main.rs` → `cas`.
- Library: `cas-cli/src/lib.rs` → crate `cas`.
- Factory TUI: `cas-cli/src/ui/factory/app/mod.rs` → bare `cas`.
- Factory daemon: `cas-cli/src/ui/factory/daemon/mod.rs` → `cas factory` runtime.
- MCP hub: `cas-cli/src/mcp/daemon.rs` → `cas serve`.
- HTTP bridge: `cas-cli/src/bridge/server/` → `cas bridge serve`.
- Commander web: `hub-web/src/main.ts` → Vite bundle served by `cas hub`.
- Slack bridge: `slack-bridge/src/{router-main,daemon-main}.ts` → npm `start:*` scripts.
- Hooks: `cas-cli/src/cli/hook.rs` → `cas hook <event>`; setup: `cas-cli/src/cli/setup.rs` → `cas setup`.
- Worker tests: `scripts/run-scoped-tests.sh -p cas --lib <module>`; supervisor gate: `cargo nextest run -p cas`.
