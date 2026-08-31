# cas — Codemap
> Auto-generated structural map. Regenerate with `/codemap` when the layout drifts (modules added, removed, or renamed).

Rust workspace for the CAS coding-agent system. Product/domain material belongs in `docs/PRODUCT_OVERVIEW.md`; this file is a navigational index.

## Top-level layout
- `cas-cli/` — binary crate `cas`: CLI, MCP hub/server, hooks, factory TUI/daemon, bridge, and application services.
- `crates/` — 16 workspace libraries for shared types, storage, search, factory, terminal, MCP, and Ghostty bindings.
- `hub-web/` — TypeScript/Vite Commander SPA; checked-in `dist/` is the offline Cargo web-asset input.
- `slack-bridge/` — standalone Node/TypeScript service routing Slack traffic to CAS daemons.
- `contrib/shell-helpers/` — installable `cas-update` wrapper and shell-test harness.
- `docs/` — plans, specs, reports, release notes, research, guides, and operational records.
- `fixtures/retrieval-parity/` — checked-in retrieval benchmark baseline and query set.
- `migration/` — historical cloud-move logs, reports, and systemd material; not the active database migration tree.
- `ops/systemd/` — deployable Cassy Actions Runner service units and launch wrappers.
- `scripts/` — install, release, migration, scoped-test, CI, portable-ISA, worktree, and worker build-cache helpers.
- `homebrew/` — Homebrew formula, documentation, and formula update helper.
- `site/` — static project landing page and system PDF.
- `vendor/` — vendored Ghostty/libghostty-vt and pinned blake3 source.
- `.claude/` — tracked Codemap plus CAS-managed Claude agents, skills, workflows, and settings.
- `.codex/` — Codex mirror of managed agent, skill, and hook configuration.
- `.cargo/` — workspace Cargo configuration.
- `.github/` — CI/release workflows, reusable actions, and public issue templates.
- `.config/` — local development and test-runner configuration.
- Root docs — `README.md`, `CONTRIBUTING.md`, `CLAUDE.md`, `AGENTS.md`, `CAS-DEEP-DIVE.md`, `CHANGELOG.md`, and `investigation-mcp-worktree.md`.
- Root config/assets — `Cargo.toml`, `.mcp.json`, `.env.worktree.template`, `casdemo.png`, and licensing files.

## Workspace / packages
- `cas-cli` — binary `cas`; composes all service crates and owns user-facing commands.
- `crates/cas-types` — shared domain, wire, provenance, task, agent, memory, hook, search, and verification types.
- `crates/cas-store` — SQLite stores, migrations support, queues, history, knowledge, archives, and vector persistence.
- `crates/cas-search` — hybrid BM25/semantic retrieval, code search, indexes, and scoring.
- `crates/cas-core` — business logic, hooks, memory/extraction/search, and shared orchestration.
- `crates/cas-code` — multi-language code analysis, parsing, chunking, and indexing support.
- `crates/cas-mcp` — MCP daemon configuration, protocol types, and server support.
- `crates/cas-mcp-proxy` — policy-aware upstream MCP proxy engine and health tracking.
- `crates/cas-factory` — worker spawning, lane routing, configuration, director detection, and probes.
- `crates/cas-factory-protocol` — factory client/server wire protocol, codecs, compression, and transport.
- `crates/cas-mux` — in-process terminal multiplexer, pane routing, harness backends, and injection.
- `crates/cas-pty` — PTY creation/configuration plus Claude, Codex, Grok, and OpenCode conformance.
- `crates/cas-recording` — asciinema-style terminal recording format, readers, writers, and export.
- `crates/cas-diffs` — diff parsing, inline rendering, widgets, and syntax highlighting.
- `crates/cas-tui-test` — PTY-backed TUI runner, screen assertions, input sequences, and artifacts.
- `crates/ghostty_vt` — safe Rust wrapper for libghostty-vt terminal emulation.
- `crates/ghostty_vt_sys` — low-level Ghostty FFI bindings, build support, and portable-target tests.

## cas-cli/src — application hub
`main.rs` starts the CLI; `lib.rs` exports application modules and the canonical `TestEnvGuard`.
- `cli/mod.rs` — clap command dispatch; command modules cover auth, cloud, config, factory, hub, knowledge, memory, provider, status, update, and worktree flows.
- `cli/factory/` — `cas factory` launch/configuration, lifecycle, worktree, liveness, parity, communication probes, and diagnostics.
- `cli/{hub,hub_service,hub_reverse_pairing}.rs` — `cas hub` fleet-control CLI, managed service installation, and reverse pairing.
- `cli/{codemap_cmd,project_overview_cmd,knowledge_cmd}.rs` — documentation freshness gates and knowledge commands.
- `cli/{history_cmd,index_cmd,retrieval_parity}.rs` — history indexing/search, code index operations, and retrieval parity commands.
- `cli/{hook,init,integrate,sync,update}.rs` — hook dispatch, setup/integration, managed-file sync, and update transactions.
- `cli/{claude,codex,claude_md,account_picker,provider_default,viktor}.rs` — harness profiles, login material, provider defaults, and Viktor access.
- `cli/{doctor,status,statusline,limits,queue,sweep,worktree}.rs` — diagnostics, usage, queues, cleanup, and worktree operations.
- `bridge/server/` — HTTP bridge used by `cas bridge serve`, including factory and MCP-facing handlers.
- `cloud/` — cloud sync, devices, teams, task proposals, embeddings, code vectors, and sync queue coordination.
- `config/` — settings, runtime hooks, metadata, access policy, and generated configuration seed data.
- `daemon/` — background maintenance, decay, observation, queue processing, filesystem watching, and indexing.
- `builtins/` — embedded Claude/Codex/Grok skills, agents, workflows, and generated builtin reference history.
- `mcp/{daemon,socket,server}/` — always-available MCP daemon, Unix socket, and request routing.
- `mcp/tools/core/` — task, memory, knowledge, search, rules, skills, workflow, system, and agent-coordination handlers.
- `mcp/tools/service/` — factory, supervisor queue, messaging, liveness, external verification, pattern/spec, and worktree handlers.
- `hooks/` — SessionStart, PreToolUse, PostToolUse, session-stop, transcript, scoring, and delivery-provenance behavior.
- `hub/` — machine-local Commander hub: server/runtime, discovery, pairing auth, identity, attention, events, and death detection.
- `history/` — incremental Git history index, changelog/refs, FTS search, provenance, symbols, and epochs.
- `hybrid_search/` — lexical/semantic/code/knowledge search composition, caches, filters, graph, and scoring.
- `knowledge/` — source selection, chunking, prompts, merge policy, and distillation pipeline.
- `memory_migration/` — legacy memory import, audit, preconditions, routing, rollback, and reindex operations.
- `migration/migrations/` — numbered SQLite migrations; current tip `m243_surfaced_artifacts_create_table.rs`.
- `store/` — layered project/global stores, syncing/notifying wrappers, sharing policy, detection, and test mocks.
- `sync/` — managed artifact rendering from builtins into `.claude/`, `.codex/`, and harness mirrors.
- `ui/factory/` — bare-`cas` TUI, daemon runtime, director events/prompts, session summarization, app state, and rendering.
- `worktree/` — discovery, Git operations, external-link handling, salvage, sweep, target locking, and cleanup.
- `ai_enrichment.rs`, `prompt_revalidation.rs`, `retrieval_parity/` — bounded model enrichment, prompt checks, and retrieval fixtures/diffs.
- `retrieval_eval.rs` — labeled retrieval evaluation: replays the committed prompt-context fixture through the SessionStart and ambient selectors and scores precision@5 / recall@5 against a committed baseline.
- `agent_id.rs`, `capability.rs`, `factory_{context_reset,isolation,preflight,target_cache}.rs` — identity, capability, factory safety, preflight, and cache boundaries.
- `ambient_recall.rs`, `consolidation/`, `extraction/`, `notifications/`, `orchestration/`, `telemetry/`, `tracing/` — recall, extraction, coordination, observability, and trace plumbing.

## cas-cli/tests and benchmarks
- `cas-cli/tests/` — integration coverage for CLI, hooks, factory/MCP, cloud, search, verification, e2e, snapshots, and multi-agent behavior.
- `cas-cli/tests/mcp_tools_test/` — MCP handler suites, including task lifecycle gates/proposals and server protocol coverage.
- `cas-cli/tests/common/`, `e2e/`, `fixtures/`, and `support/` — shared fixtures, end-to-end helpers, data, and test utilities.
- `cas-cli/benches/code_indexing.rs` — Criterion benchmark for code indexing.
- `hub-web/src/*.test.ts` and `hub-web/e2e/` — Commander unit tests and browser-facing test harness.
- Inline `#[cfg(test)]` modules plus crate-specific `crates/*/tests/` — lower-level unit and integration coverage.

## crates — key module roots
- `cas-store/src/{agent_store,delegation_receipt_store,external_task_dependency_store}.rs` — leases, delegation receipts, and external task dependencies.
- `cas-store/src/{knowledge_store,history_store,code_vector_store,trace_archive}.rs` — durable knowledge, history, vectors, and bounded trace archives.
- `cas-store/src/{surfaced_artifact_store,version_store,viktor_*_store}.rs` — surfaced artifacts, rule/skill versions, and Viktor gateway state.
- `cas-store/src/{prompt_queue_store,supervisor_queue_store,task_store}.rs` — durable prompts, supervisor review queues, and task lifecycle.
- `cas-core/src/{hooks,memory,search,sync}/` — hook contexts, memory logic, retrieval, and managed artifact synchronization.
- `cas-search/src/{bm25,code_search,parallel,scorer,traits}.rs` — text/code indexes, parallel retrieval, scoring, and search traits.
- `cas-mcp/src/{daemon,types}/` — embedded daemon lifecycle and MCP operation/type definitions.
- `cas-factory/src/{routing,spec_resolver,probe,session}/` — capability-aware lane routing, worker specs, probes, and sessions.
- `cas-factory/policy/lane-registry.toml` — checked-in provider lane and capability policy.
- `cas-mux/src/{backend,mux,pane,pty,render,spec}.rs` — provider backends, terminal panes, injection, rendering, and worker specs.
- `cas-pty/{src,conformance}/` — PTY runtime adapters and pinned harness contract receipts.
- `cas-types/src/provenance.rs` — source lineage helpers for durable domain records.
- `ghostty_vt_sys/{build_support.rs,tests/portable_target.rs}` — FFI build and portable target enforcement.

## Supporting services and assets
- `hub-web/src/main.ts` — Commander SPA entrypoint; pairing, sessions, panes, attention, messaging, and terminal adapters sit beside it.
- `hub-web/{package.json,vite.config.ts,tsconfig.json}` — Vite build, TypeScript checks, and Vitest scripts for the web client.
- `hub-web/dist/` — checked-in browser bundle and Ghostty WASM consumed by the Rust hub; rebuild from `hub-web/`, never hand-edit.
- `slack-bridge/src/{router-main,daemon-main}.ts` — Slack event/router and per-user bridge daemon entrypoints.
- `contrib/shell-helpers/{cas-update,install.sh,tests/}` — user update command, installer, and shell tests.
- `fixtures/retrieval-parity/{baseline-soundwave.json,queryset.toml}` — retrieval parity fixture inputs.
- `homebrew/cas.rb` — package formula; `scripts/release.sh` and `scripts/check-release-preflight.sh` drive release checks.

## Cross-cutting
- **Tests:** colocated Rust unit tests, crate `tests/`, `cas-cli/tests/`, and Vitest suites under `hub-web/src/`.
- **Test isolation:** use `TestEnvGuard` from `cas-cli/src/lib.rs`; do not introduce a second HOME/environment helper.
- **Docs:** `cas-cli/docs/` holds architecture, contributing, migrations, proxy, capabilities, TUI, and worktree design; `docs/` holds specs, guides, reports, reviews, and release records.
- **Release/CI:** `.github/workflows/`, `scripts/`, `CHANGELOG.md`, `docs/release-notes/`, and `docs/RELEASE_SLACK_RUBRIC.md` contain lanes, preflights, and publication policy.
- **Research/operations:** `docs/{analysis,research,reports,spikes}/`, `docs/{branch-protection,ci,migration}/`, `migration/`, and `ops/systemd/` hold durable investigations and host operations.
- **Config:** `.claude/settings.json`, `.codex/config.toml`, `.codex/hooks.json`, `.mcp.json`, root `Cargo.toml`, and `.env.worktree.template`.
- **Generated/local state:** `target/`, `node_modules/`, `hub-web/node_modules/`, and worktree-local CAS state are intentionally not source-map entries.

## Entrypoints
- CLI: `cas-cli/src/main.rs` → `cas`.
- Library: `cas-cli/src/lib.rs` → crate `cas`.
- Factory TUI: `cas-cli/src/ui/factory/app/mod.rs` → bare `cas`.
- Factory daemon: `cas-cli/src/ui/factory/daemon/` → `cas factory` runtime.
- MCP hub: `cas-cli/src/mcp/daemon.rs` → `cas serve`.
- HTTP bridge: `cas-cli/src/bridge/server/` → `cas bridge serve`.
- Commander web: `hub-web/src/main.ts` → Vite bundle served by `cas hub`.
- Slack bridge: `slack-bridge/src/{router-main,daemon-main}.ts` → npm `start:*` scripts.
- Hooks: `cas-cli/src/cli/hook.rs` → `cas hook <event>`.
- Tests: `cargo test -p cas --lib`, `npm test` in `hub-web/`, or the workspace gate owned by integration.
