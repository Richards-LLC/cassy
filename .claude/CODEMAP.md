# Cassy - Codemap

> Structural navigation for the Rust workspace and TypeScript clients. Regenerate with `/codemap` when modules move.

## Top-level layout

- `.cargo/` - workspace build configuration and target settings.
- `.claude/` - managed Claude instructions, skills, hooks, and this map.
- `.codex/` - generated Codex instructions, hook projections, and configuration.
- `.config/` - shared test-runner configuration.
- `.githooks/` - tracked main-checkout Git hooks and fast-row delegation.
- `.github/` - CI workflows, reusable actions, and issue templates.
- `cas-cli/` - `cas` executable, command handlers, services, hooks, and factory TUI.
- `contrib/` - installable shell helpers and their harness.
- `crates/` - shared Rust libraries listed below.
- `docs/` - design, operations, QA journeys, release records, and project notes.
- `fixtures/` - retrieval-parity baselines and query sets.
- `homebrew/` - formula and package-update helper.
- `hub-web/` - TypeScript/Vite Commander client and browser journeys.
- `migration/` - historical cloud-move and deployment material.
- `ops/` - systemd units and runtime launch wrappers.
- `scripts/` - install, release, proof, scoped-test, and build-cache tools.
- `site/` - static project site and system PDF.
- `slack-bridge/` - TypeScript Slack router and per-user daemon.
- `Cargo.toml`, `package.json` - workspace membership and pinned repository tooling.
- `README.md`, `AGENTS.md`, `CONTRIBUTING.md` - onboarding, agent rules, and contribution pointers.

## Workspace / packages

- `cas-cli/` - binary `cas` and service composition (Rust).
- `crates/cas-types/` - shared task, agent, event, delivery, and wire types.
- `crates/cas-store/` - SQLite stores, queues, knowledge, verification, and vectors.
- `crates/cas-search/` - text/code indexes, grep, retrieval, and score combination.
- `crates/cas-core/` - hooks, memory hygiene, extraction, temporal search, and sync.
- `crates/cas-code/` - tree-sitter parsing and code indexing.
- `crates/cas-mcp/` - MCP protocol types and daemon support.
- `crates/cas-mcp-proxy/` - upstream MCP proxy, caller policy, and health tracking.
- `crates/cas-factory/` - worker lifecycle, provider routing, probes, and sessions.
- `crates/cas-factory-protocol/` - factory client/server protocol and transport.
- `crates/cas-mux/` - terminal panes, harness backends, input routing, and rendering.
- `crates/cas-pty/` - PTY setup and Claude, Codex, Grok, and OpenCode adapters.
- `crates/cas-recording/` - terminal recording format, readers, and exports.
- `crates/cas-diffs/` - diff parsing, inline widgets, and syntax highlighting.
- `crates/cas-operator-crypto/` - HPKE/JWS envelopes shared with Cloud.
- `crates/cas-tui-test/` - PTY-backed TUI runner and screen assertions.
- `crates/ghostty_vt/` - safe Rust wrapper for terminal parsing.
- `crates/ghostty_vt_sys/` - Ghostty FFI, build support, and portable-target checks.

## CLI and service composition

- `cas-cli/src/main.rs`, `cas-cli/src/lib.rs` - executable entry, module exports, and test environment boundary.
- `cas-cli/src/cli/mod.rs` - clap commands and command dispatch.
- `cas-cli/src/cli/doctor.rs`, `cas-cli/src/cli/doctor/` - doctor producers, grouped rendering, and communication checks.
- `cas-cli/src/cli/update.rs`, `cas-cli/src/cli/update/` - update phases, findings, progress, and receipts.
- `cas-cli/src/cli/project_files.rs`, `cas-cli/src/cli/project_files/` - managed-file inventory, Git reconciliation, and locking.
- `cas-cli/src/cli/config/`, `cas-cli/src/config/` - config commands, accessors, metadata registry, and operator policy.
- `cas-cli/src/cli/factory/`, `cas-cli/src/cli/integrate/` - factory operations and integration handlers.
- `cas-cli/src/hooks/handlers/`, `cas-cli/src/cli/hook/` - session/tool policy and native hook dispatch.
- `cas-cli/src/cloud/`, `cas-cli/src/cloud/syncer/` - peer operations, store sync, and identity reconciliation.
- `cas-cli/src/hub/`, `cas-cli/src/hub/operator_inbox/` - Commander pairing, inbox, machine presence, and idle sync.
- `cas-cli/src/worktree/`, `cas-cli/src/worktree/git/` - worktree lifecycle and safe Git branch operations.
- `cas-cli/src/store/`, `cas-cli/src/migration/migrations/` - layered stores and numbered SQLite migrations.
- `cas-cli/src/coordination/`, `cas-cli/src/notifications/` - external-work journals and notification plumbing.
- `cas-cli/src/qa_pass.rs`, `cas-cli/src/qa_evidence.rs`, `cas-cli/src/qa_batch.rs` - QA rounds, evidence gates, and batch receipts.
- `cas-cli/src/builtins/` - canonical agents, skills, jobs, harness mirrors, and sync manifest.

## Factory runtime and terminals

- `cas-cli/src/ui/factory/app/mod.rs` - TUI state, launch wiring, and supervisor/worker startup envelopes.
- `cas-cli/src/ui/factory/app/init.rs`, `cas-cli/src/ui/factory/app/provisioning.rs` - app initialization and worker provisioning.
- `cas-cli/src/ui/factory/app/render_and_ops/` - rendering, epic-worker actions, and worktree operations.
- `cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs` - queued startup delivery and daemon event processing.
- `cas-cli/src/ui/factory/daemon/runtime/` - queue workers, delivery probes, relay jobs, notices, and render scheduling.
- `cas-cli/src/ui/factory/director/` - task dispatch, worker coordination, reminders, and stall handling.
- `cas-cli/src/ui/factory/server_registry/` - owned server records and lifecycle.
- `cas-cli/src/factory_scratch.rs`, `cas-cli/src/factory_worker_check.rs` - target-path resolution and capped worker proof admission.
- `cas-cli/src/factory_target_cache/` - target ownership, baseline seeding, parked caches, and retirement.
- `crates/cas-mux/src/backend/` - harness launch configuration and environment projections.
- `crates/cas-mux/src/pane/mod.rs`, `crates/cas-mux/src/pane/snapshot.rs` - pane readiness, startup-screen state, and terminal snapshots.
- `crates/cas-mux/src/mux.rs`, `crates/cas-mux/src/submit_gate.rs` - multiplexer events, queued injection, and submit gating.
- `crates/cas-pty/src/`, `crates/cas-pty/conformance/` - PTY adapters, queued input, and harness conformance receipts.

## MCP and transport

- `cas-cli/src/mcp/daemon.rs`, `cas-cli/src/mcp/socket.rs` - MCP lifecycle and Unix transport.
- `cas-cli/src/mcp/server/` - runtime, prompts, resources, watchdog, and allocator controls.
- `cas-cli/src/mcp/tools/core/` - task, memory, knowledge, artifact, search, rule, and workflow handlers.
- `cas-cli/src/mcp/tools/core/task/lifecycle/` - task start/close, evidence validation, epic lanes, and merged-delivery closure.
- `cas-cli/src/mcp/tools/service/` - factory/reminder actions, liveness, servers, and external verification.
- `cas-cli/src/mcp/tools/types/` - request/response schemas shared by MCP actions.
- `cas-cli/src/bridge/server/` - HTTP/SSE bridge handlers.
- `crates/cas-mcp/src/types/`, `crates/cas-mcp-proxy/src/` - protocol definitions and upstream proxy implementation.

## Storage, retrieval, and clients

- `crates/cas-store/src/prompt_queue_store.rs` - durable prompt queue and delivery lifecycle.
- `crates/cas-store/src/communication_slo/` - communication metrics and fixture-backed SQL queries.
- `crates/cas-core/src/hooks/`, `crates/cas-core/src/memory/` - hook schemas/dispatch and memory maintenance.
- `crates/cas-core/src/search/temporal/`, `crates/cas-core/src/sync/` - temporal retrieval and synchronization.
- `cas-cli/src/hybrid_search/`, `cas-cli/src/history/` - composed retrieval and Git-history search.
- `hub-web/src/main.ts`, `hub-web/src/conversation-store.ts` - Commander entry and conversation state.
- `hub-web/src/inbox/`, `hub-web/src/connection.ts` - inbox presence/reply grants and connection lifecycle.
- `hub-web/e2e/journeys/`, `hub-web/preview/` - browser journeys and isolated previews.
- `slack-bridge/src/router-main.ts`, `slack-bridge/src/daemon-main.ts` - Slack routing and daemon entrypoints.

## Cross-cutting

- **Tests:** inline Rust modules, `crates/cas-store/tests/`, `cas-cli/tests/`, `hub-web/src/` Vitest, and `hub-web/e2e/` Playwright.
- **Contributor policy:** `cas-cli/docs/CONTRIBUTING.md`, `cas-cli/docs/ARCHITECTURE.md`.
- **QA contracts:** `docs/qa/`, `scripts/journey-eval.sh`, `scripts/terminal-qa.mjs`.
- **Release records:** `docs/release-notes/`, `docs/release-reports/`.
- **Release tooling:** `scripts/release-gate.sh`, `scripts/release_env.py`, `scripts/proof_target.py`.
- **Worker hooks/cache:** `.githooks/`, `scripts/fast-rows-pre-push.sh`, `scripts/refresh-worker-build-cache.sh`.
- **Markdown tooling:** `scripts/markdownlint.py`, `.markdownlint-cli2.jsonc`.
- **Deploy/runbooks:** `ops/systemd/`, `docs/branch-protection/`, `docs/ci/`.

## Entrypoints

- CLI/library: `cas-cli/src/main.rs`, `cas-cli/src/lib.rs` (`cas`).
- Factory TUI/daemon: `cas-cli/src/ui/factory/app/mod.rs`, `cas-cli/src/ui/factory/daemon/mod.rs`.
- MCP server: `cas-cli/src/mcp/daemon.rs` (`cas serve`).
- HTTP bridge: `cas-cli/src/bridge/server/` (`cas bridge serve`).
- Commander web: `hub-web/src/main.ts` (Vite client served by `cas hub`).
- Slack router/daemon: `slack-bridge/src/router-main.ts`, `slack-bridge/src/daemon-main.ts`.
- Hooks/setup: `cas-cli/src/cli/hook.rs`, `cas-cli/src/cli/setup.rs`.
- Scoped Rust proof: `scripts/run-scoped-tests.sh`; full assembly belongs to the supervisor/merge queue.
