# Cassy - Codemap

> Structural map of the 3.70 merge (`0e9f5b4e3`); regenerate with `/codemap` when modules move.

## Top-level layout

- `.cargo/` - workspace build configuration and target settings.
- `.claude/` - Claude instructions, agents, hook settings, and this map.
- `.codex/` - generated Codex instructions, hook projections, and configuration.
- `.config/` - shared nextest runner configuration.
- `.githooks/` - tracked Git hooks and fast-row delegation.
- `.github/` - CI workflows, reusable actions, and issue templates.
- `cas-cli/` - `cas` executable, command handlers, services, hooks, and factory TUI.
- `contrib/` - installable shell helpers and their harness.
- `crates/` - shared Rust libraries listed below.
- `docs/` - design, operations, QA journeys, release records, and project notes.
- `fixtures/` - retrieval-parity baselines and query sets.
- `homebrew/` - formula and package-update helper.
- `hub-web/` - TypeScript/Vite Commander client and browser journeys.
- `migration/` - historical cloud-move scripts, manifests, and deployment records.
- `ops/` - systemd units and runtime launch wrappers.
- `scripts/` - installation, release, proof, scoped-test, and build-cache tools.
- `site/` - static project site and system PDF.
- `slack-bridge/` - TypeScript Slack router and per-user daemon.
- `Cargo.toml`, `package.json`, `rust-toolchain.toml` - workspace members, pinned repository tooling, and Rust toolchain.
- `README.md`, `AGENTS.md`, `CLAUDE.md`, `CONTRIBUTING.md` - onboarding, agent rules, and contribution pointers.
- `DESIGN.md`, `CODING_STANDARDS.md`, `CHANGELOG.md`, `CAS-DEEP-DIVE.md` - UI conventions, standards, version history, and system reference.

## Workspace / packages

The Rust workspace has 18 members; `hub-web/` and `slack-bridge/` are separate TypeScript packages.

- `cas-cli/` - binary `cas` and service composition (Rust).
- `crates/cas-types/` - shared task, agent, event, delivery, and wire types.
- `crates/cas-store/` - SQLite stores, queues, knowledge, verification, and vector persistence.
- `crates/cas-search/` - Tantivy text indexes, LMDB vector storage, and retrieval primitives.
- `crates/cas-core/` - hooks, memory hygiene, extraction, temporal search, and sync.
- `crates/cas-code/` - tree-sitter parsing, symbol extraction, and code indexing.
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

- `cas-cli/src/main.rs`, `cas-cli/src/lib.rs` - executable entry, exports, and shared test-environment boundary.
- `cas-cli/src/cli/mod.rs` - clap commands and command dispatch.
- `cas-cli/src/cli/doctor.rs`, `cas-cli/src/cli/doctor/` - health-check producers, grouped rendering, and communication checks.
- `cas-cli/src/cli/update.rs`, `cas-cli/src/cli/update/` - update phases, binding checks, bounded discovery, and receipts.
- `cas-cli/src/cli/project_files.rs`, `cas-cli/src/cli/project_files/` - managed-file inventory, Git reconciliation, and locking.
- `cas-cli/src/cli/config/`, `cas-cli/src/config/` - config commands, metadata registry, and operator policy.
- `cas-cli/src/cli/codemap_cmd.rs`, `cas-cli/src/hooks/handlers/handlers_events/codemap.rs` - codemap freshness CLI and session banner/gate.
- `cas-cli/src/hooks/handlers/handlers_events/docs_freshness.rs` - shared read-only cwd tree and task/integration-ref selection for documentation freshness.
- `cas-cli/src/cli/factory/`, `cas-cli/src/cli/integrate/` - factory operations and integration handlers.
- `cas-cli/src/hooks/handlers/`, `cas-cli/src/cli/hook/` - session/tool policy, native hook dispatch, and generated hook configuration.
- `cas-cli/src/cloud/`, `cas-cli/src/hub/` - store sync, identity reconciliation, Commander pairing, operator inbox, and machine presence.
- `cas-cli/src/worktree/`, `cas-cli/src/worktree/git/` - worktree lifecycle and safe Git branch operations.
- `cas-cli/src/store/`, `cas-cli/src/migration/migrations/` - layered stores and numbered SQLite migrations.
- `cas-cli/src/coordination/`, `cas-cli/src/notifications/` - Slack binding intake, ownership repair/reconciliation, pause journals, and notifications.
- `cas-cli/src/artifacts/`, `cas-cli/src/task_simulators.rs` - portable artifact proofs, retention policy, and owned iOS simulator cleanup.
- `cas-cli/src/task_ownership.rs`, `cas-cli/src/task_assignment.rs` - project ownership proof and assignment identity.
- `cas-cli/src/knowledge/`, `cas-cli/src/daemon/` - wiki distillation and background indexing/maintenance.
- `cas-cli/src/extraction/`, `cas-cli/src/consolidation/`, `cas-cli/src/sync/` - observation extraction, memory decay, and harness-file synchronization.
- `cas-cli/src/builtins/` - canonical agents, skills, jobs, harness mirrors, and sync manifest.

## QA registry, impact, and upkeep

- `cas-cli/src/qa_registry.rs` - journey YAML schema, validation, debt, and shared path-filter semantics.
- `cas-cli/src/qa_seed.rs` - bootstrap/merge registry entries from tracked routes and project context; coverage status.
- `cas-cli/src/qa_impact.rs` - pure, versioned diff-to-journey impact analysis and unowned-area detection.
- `cas-cli/src/qa_impact_io.rs` - read-only Git snapshots, diff paths, and legacy selector boundary for impact analysis.
- `cas-cli/src/qa_post_release.rs` - persisted read-only release observations and scheduling/verdict state.
- `cas-cli/src/cli/qa.rs` - `cas qa journeys --seed|--check|--status` and QA subcommand dispatch.
- `cas-cli/src/cli/qa_impact.rs` - `cas qa impact` adapters for diff, files, stdin, and task-owned changes.
- `cas-cli/src/cli/qa_post_release.rs` - `cas qa post-release start|status` and persisted jobs for the MCP daemon runner.
- `cas-cli/src/cli/doctor/journeys.rs` - doctor journey-registry row using the shared coverage/debt report.
- `cas-cli/src/config/upkeep.rs` - registry/CODEMAP/overview stages and one-release observe-to-enforce adoption.
- `cas-cli/src/qa_pass.rs`, `cas-cli/src/qa_evidence.rs`, `cas-cli/src/qa_batch.rs` - independent QA eligibility, evidence gates, and batch receipt validation.

## Factory runtime and terminals

- `cas-cli/src/ui/factory/app/mod.rs` - TUI state, launch wiring, and supervisor/worker startup envelopes.
- `cas-cli/src/ui/factory/app/init.rs`, `cas-cli/src/ui/factory/app/provisioning.rs` - app initialization and worker provisioning.
- `cas-cli/src/ui/factory/app/render_and_ops/` - rendering, epic-worker actions, and worktree operations.
- `cas-cli/src/ui/factory/daemon/runtime/` - queued startup delivery, event processing, probes, relay jobs, and render scheduling.
- `cas-cli/src/ui/factory/daemon/runtime/violet_intake.rs` - lifecycle-owned legacy Slack drain and pause control.
- `cas-cli/src/ui/factory/director/` - task dispatch, worker coordination, reminders, and stall handling.
- `cas-cli/src/ui/factory/app/assignment_evidence.rs`, `cas-cli/src/supervisor_message_wakes.rs` - assignment evidence and directed supervisor wake batching.
- `cas-cli/src/ui/factory/server_registry/` - owned server records and lifecycle.
- `cas-cli/src/factory_scratch.rs`, `cas-cli/src/factory_worker_check.rs` - target-path resolution, capped worker proof admission, and authenticated supervisor-recorded exact-tip test evidence.
- `cas-cli/src/factory_target_cache/` - target ownership, baseline seeding, parked caches, and retirement.
- `crates/cas-mux/src/backend/`, `crates/cas-mux/src/pane/` - harness launch/environment projections, pane readiness, and terminal snapshots.
- `crates/cas-mux/src/mux.rs`, `crates/cas-mux/src/submit_gate.rs` - multiplexer events, queued injection, and submit gating.
- `crates/cas-pty/src/`, `crates/cas-pty/conformance/` - PTY adapters, queued input, and harness conformance receipts.
- `crates/cas-pty/src/store_isolation.rs` - Linux bubblewrap namespaces masking operator credential stores from workers.

## MCP and transport

- `cas-cli/src/mcp/daemon.rs`, `cas-cli/src/mcp/socket.rs` - MCP lifecycle and Unix transport.
- `cas-cli/src/mcp/server/` - runtime, prompts, resources, watchdog, and allocator controls.
- `cas-cli/src/mcp/tools/core/` - task, memory, knowledge, artifact, search, rule, and workflow handlers.
- `cas-cli/src/mcp/tools/core/task/lifecycle/` - task start/close, evidence validation, epic lanes, and merged-delivery closure.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/registry_upkeep.rs` - G1-G3 registry, user-surface, and bug-case gates over immutable snapshots.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/docs_upkeep.rs` - G5 CODEMAP structure and G6 overview upkeep, escapes, and observe accounting.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/merged_verification.rs` - verification dispatch when another close gate refuses a merged delivery.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/task_attribution.rs` - task-owned diff attribution and shared hunk parsing.
- `cas-cli/src/mcp/tools/core/task/repo_context/candidate_probe.rs` - bounded candidate-repository discovery for task lifecycle calls.
- `cas-cli/src/mcp/tools/core/agent_coordination/branch_adoption.rs` - guarded transfer of parked branches into receiver worktrees.
- `cas-cli/src/mcp/tools/service/`, `cas-cli/src/mcp/tools/types/` - factory/reminder/server actions, external verification, and request/response schemas.
- `cas-cli/src/bridge/server/`, `crates/cas-mcp/src/types/` - HTTP/SSE bridge handlers and shared protocol definitions.

## Storage, retrieval, and clients

- `crates/cas-store/src/prompt_queue_store.rs`, `crates/cas-store/src/accepted_proposal_store.rs` - durable prompt delivery and cross-project proposal ownership.
- `crates/cas-store/src/task_note_markers.rs` - reserve trusted artifact/proposal marker appends in the untrusted task timeline.
- `crates/cas-store/src/rule_candidate_store.rs`, `crates/cas-store/src/communication_slo/` - rule-candidate review state and communication metrics/SQL.
- `crates/cas-core/src/hooks/`, `crates/cas-core/src/memory/` - hook schemas/dispatch and memory maintenance.
- `crates/cas-core/src/search/temporal/`, `crates/cas-core/src/sync/` - temporal retrieval and synchronization.
- `cas-cli/src/hybrid_search/`, `cas-cli/src/history/` - composed retrieval and Git-history search.
- `hub-web/src/main.ts`, `hub-web/src/conversation-store.ts`, `hub-web/src/inbox/`, `hub-web/src/connection.ts` - Commander entry, conversations, reply grants, and connection lifecycle.
- `hub-web/e2e/journeys/`, `hub-web/preview/` - browser journeys and isolated previews.
- `slack-bridge/src/router-main.ts`, `slack-bridge/src/daemon-main.ts` - Slack routing and daemon entrypoints.

## Cross-cutting

- **Tests:** inline Rust modules, `crates/cas-store/tests/`, `cas-cli/tests/`, `hub-web/src/` Vitest, and `hub-web/e2e/` Playwright.
- **Contributor policy:** `cas-cli/docs/CONTRIBUTING.md`, `cas-cli/docs/ARCHITECTURE.md`.
- **QA contracts:** `docs/qa/`, `scripts/journey-eval.sh`, `scripts/terminal-qa.mjs`.
- **Release records:** `docs/release-notes/`, `docs/release-reports/`, `docs/release-reports/assets/` - per-version drafts, reports, evidence, and publication rubrics.
- **Release tooling:** `scripts/release.sh`, `scripts/release-gate.sh`, `scripts/release-train-announce.py`, `cas-cli/src/cli/release_report.rs`.
- **Worker hooks/cache:** `.githooks/`, `scripts/fast-rows-pre-push.sh`, `scripts/refresh-worker-build-cache.sh`.
- **Markdown:** `scripts/markdownlint.py`, `.markdownlint-cli2.jsonc`; **deploy/runbooks:** `ops/systemd/`, `docs/branch-protection/`, `docs/ci/`.
- **Config:** `.mcp.json`, `.env.worktree.template`, `.claude/settings.json`, `.codex/hooks.json` - MCP, worktree environment, and harness hook projections.

## Entrypoints

- CLI/library: `cas-cli/src/main.rs`, `cas-cli/src/lib.rs` (`cas`); factory TUI/daemon: `cas-cli/src/ui/factory/app/mod.rs`, `cas-cli/src/ui/factory/daemon/mod.rs`.
- MCP server: `cas-cli/src/mcp/daemon.rs` (`cas serve`); HTTP bridge: `cas-cli/src/bridge/server/` (`cas bridge serve`).
- Commander: `hub-web/src/main.ts` (Vite client served by `cas hub`); hooks/setup: `cas-cli/src/cli/hook.rs`, `cas-cli/src/cli/setup.rs`.
- Worker Rust proof: capped `cargo nextest run -p <crate> --lib -E 'test(name)'` and `cargo check -p <crate> --lib` (or `--tests`).
- Web checks: `hub-web/package.json` Vitest/typecheck and `scripts/journey-eval.sh` affected journeys; full assembly belongs to the merge queue.
