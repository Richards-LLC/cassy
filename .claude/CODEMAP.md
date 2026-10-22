# Cassy - Codemap

> Structural map regenerated from the merged 3.71/v57 tree: 18 Rust workspace members and TypeScript clients. Regenerate with `/codemap` when modules move.

## Top-level layout

- `.cargo/`, `.config/` - workspace compilation settings and nextest profiles.
- `.claude/`, `.codex/` - agent instructions, canonical map, harness configuration and hook projections.
- `.githooks/`, `.github/` - local Git gates, CI workflows and issue templates.
- `cas-cli/` - `cas` binary, library composition, CLI, MCP, hooks and factory TUI.
- `crates/` - shared Rust packages listed below.
- `contrib/` - installable shell helpers and their harness.
- `docs/` - architecture, design, QA journeys, operations and release records.
- `fixtures/` - retrieval-parity query sets and baselines.
- `homebrew/` - formula and package update helpers.
- `hub-web/` - Commander Vite/TypeScript client and browser journeys.
- `slack-bridge/` - TypeScript Slack router and per-user daemon.
- `migration/` - historical cloud-move scripts and manifests.
- `ops/` - service units and launch wrappers; `scripts/` - build, proof, installation and release tools.
- `site/` - static project site and system PDF.
- `Cargo.toml`, `package.json`, `rust-toolchain.toml` - workspace membership, repository tooling and pinned Rust toolchain.
- `README.md`, `AGENTS.md`, `CLAUDE.md`, `CONTRIBUTING.md` - onboarding and agent/contributor policy.
- `DESIGN.md`, `CODING_STANDARDS.md`, `CHANGELOG.md`, `CAS-DEEP-DIVE.md` - design rules, coding standards, release history and system reference.

## Workspace / packages

- `cas-cli/` - executable `cas` and service composition.
- `crates/cas-types/` - shared task, agent, delivery, event and wire types.
- `crates/cas-store/` - SQLite stores, queues, verification, knowledge and vector persistence.
- `crates/cas-search/` - Tantivy text indexes, LMDB vectors and retrieval primitives.
- `crates/cas-core/` - hooks, memory hygiene, extraction, temporal search and synchronization.
- `crates/cas-code/` - tree-sitter parsing, symbol extraction and code indexing.
- `crates/cas-mcp/` - protocol request/response schemas and daemon support.
- `crates/cas-mcp-proxy/` - upstream MCP proxy, caller policy and health tracking.
- `crates/cas-factory/` - worker/provider lifecycle, sessions, probes, routing and spec resolution.
- `crates/cas-factory-protocol/` - factory client/server transport and wire protocol.
- `crates/cas-mux/` - terminal panes, harness backends, input routing and rendering.
- `crates/cas-pty/` - PTY setup and Claude, Codex, Grok and OpenCode adapters.
- `crates/cas-recording/` - terminal recording format, readers and exports.
- `crates/cas-diffs/` - diff parsing, inline widgets and highlighting.
- `crates/cas-operator-crypto/` - HPKE/JWS envelopes shared with Cloud.
- `crates/cas-tui-test/` - PTY-backed TUI runner and screen assertions.
- `crates/ghostty_vt/`, `crates/ghostty_vt_sys/` - safe terminal-parser wrapper, Ghostty FFI and portable-target build support.

## CLI and service composition

- `cas-cli/src/main.rs`, `cas-cli/src/lib.rs` - executable entry and library module composition.
- `cas-cli/src/test_env_guard.rs` - canonical test environment, private HOME/XDG scopes and ambient credential isolation.
- `cas-cli/src/cli/mod.rs` - clap command definitions and dispatch.
- `cas-cli/src/cli/doctor.rs`, `cas-cli/src/cli/doctor/` - health producers, grouped rendering and communication checks.
- `cas-cli/src/cli/update.rs`, `cas-cli/src/cli/update/` - update phases, binding checks, discovery and receipts.
- `cas-cli/src/cli/project_files.rs`, `cas-cli/src/cli/project_files/` - managed-file inventory, Git reconciliation and locking.
- `cas-cli/src/cli/config/`, `cas-cli/src/config/` - TOML commands, key registry and operator policy.
- `cas-cli/src/capability.rs`, `cas-cli/src/cli/hub_service.rs` - account/keychain probes, bounded auth cache and readiness.
- `cas-cli/src/cli/codemap_cmd.rs`, `cas-cli/src/hooks/handlers/handlers_events/codemap.rs` - map freshness commands and session gates.
- `cas-cli/src/hooks/handlers/handlers_events/docs_freshness.rs` - immutable checkout/target measurements for documentation upkeep.
- `cas-cli/src/cli/factory/`, `cas-cli/src/cli/integrate/` - factory operations and integration commands.
- `cas-cli/src/hooks/handlers/`, `cas-cli/src/cli/hook/` - session/tool policy, native dispatch and generated hook configuration.
- `cas-cli/src/cloud/`, `cas-cli/src/hub/` - sync, identity reconciliation, pairing, operator inbox and presence.
- `cas-cli/src/cloud/file_leases.rs` - Cloud-backed cross-session file ownership advisories and lease caching.
- `cas-cli/src/worktree/`, `cas-cli/src/worktree/git/` - worktree lifecycle and safe Git branch operations.
- `cas-cli/src/store/`, `cas-cli/src/migration/migrations/` - store decorators and numbered SQLite migrations.
- `cas-cli/src/coordination/`, `cas-cli/src/notifications/` - binding intake, ownership repair, reconciliation and notification queues.
- `cas-cli/src/artifacts/`, `cas-cli/src/task_simulators.rs` - portable proofs, retention and owned iOS simulator cleanup.
- `cas-cli/src/task_ownership.rs`, `cas-cli/src/task_assignment.rs` - project ownership proof and assignment identity.
- `cas-cli/src/knowledge/`, `cas-cli/src/daemon/` - wiki distillation and background maintenance/indexing.
- `cas-cli/src/extraction/`, `cas-cli/src/consolidation/`, `cas-cli/src/sync/` - observation extraction, memory decay and harness synchronization.
- `cas-cli/src/builtins/` - canonical agents, skills, jobs, harness mirrors and sync manifest.

## QA registry, impact and upkeep

- `cas-cli/src/qa_registry.rs`, `cas-cli/src/qa_seed.rs` - typed journey registry, criteria, path filters, debt and bootstrap coverage.
- `cas-cli/src/qa_impact.rs`, `cas-cli/src/qa_impact_io.rs` - versioned impact analysis, Git snapshots and journey selection.
- `cas-cli/src/qa_post_release.rs` - persisted release observations, scheduling and verdict state.
- `cas-cli/src/cli/qa.rs`, `cas-cli/src/cli/qa_impact.rs`, `cas-cli/src/cli/qa_post_release.rs` - QA commands and adapters.
- `cas-cli/src/cli/doctor/journeys.rs` - registry health row using shared coverage/debt measurements.
- `cas-cli/src/config/upkeep.rs` - registry, CODEMAP and overview adoption stages.
- `cas-cli/src/qa_pass.rs`, `cas-cli/src/qa_evidence.rs`, `cas-cli/src/qa_batch.rs` - independent QA eligibility, proof gates and batch receipts.

## Factory runtime and terminals

- `cas-cli/src/ui/factory/app/mod.rs` - TUI state, launch wiring and startup envelopes.
- `cas-cli/src/ui/factory/app/init.rs`, `cas-cli/src/ui/factory/app/provisioning.rs` - app initialization and worker provisioning.
- `cas-cli/src/ui/factory/app/render_and_ops/` - rendering, epic-worker actions and worktree operations.
- `cas-cli/src/ui/factory/daemon/runtime/` - startup delivery, event processing, probes, relays and render scheduling.
- `cas-cli/src/ui/factory/daemon/runtime/violet_intake.rs` - lifecycle-owned Slack drain and pause control.
- `cas-cli/src/ui/factory/director/` - task dispatch, worker coordination, reminders and stall handling.
- `cas-cli/src/ui/factory/app/assignment_evidence.rs`, `cas-cli/src/supervisor_message_wakes.rs` - assignment receipts and supervisor wake batching.
- `cas-cli/src/ui/factory/server_registry/` - owned server records and lifecycle.
- `cas-cli/src/factory_capacity_wait.rs` - lease-bound capacity episodes, retry visibility and deadline/assistance notices.
- `cas-cli/src/factory_scratch.rs`, `cas-cli/src/factory_build_guard.rs` - scratch paths, build pressure admission and recovery.
- `cas-cli/src/factory_worker_check.rs` - capped worker checks/tests, receipt rebinding and authenticated supervisor evidence.
- `cas-cli/src/factory_target_cache.rs`, `cas-cli/src/factory_target_cache/` - cache inventory/GC, ownership, baselines, parked targets and retirement.
- `crates/cas-mux/src/backend/`, `crates/cas-mux/src/pane/` - harness launch, pane readiness and terminal snapshots.
- `crates/cas-mux/src/mux.rs`, `crates/cas-mux/src/submit_gate.rs` - multiplexer events, input injection and submit gating.
- `crates/cas-pty/src/`, `crates/cas-pty/conformance/` - PTY adapters, queued input and harness conformance proofs.
- `crates/cas-pty/src/store_isolation.rs` - worker credential-store masking through Linux bubblewrap namespaces.

## MCP and transport

- `cas-cli/src/mcp/daemon.rs`, `cas-cli/src/mcp/socket.rs` - MCP lifecycle and Unix transport.
- `cas-cli/src/mcp/server/` - runtime, prompts, resources, watchdog and allocator controls.
- `cas-cli/src/mcp/tools/core/` - task, memory, knowledge, artifact, search, rule and workflow handlers.
- `cas-cli/src/mcp/tools/core/task/lifecycle/` - start/close, evidence validation, epic lanes and merged-delivery closure.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/registry_upkeep.rs` - registry and bug-case gates over immutable delivery snapshots.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/docs_upkeep.rs` - CODEMAP/overview upkeep, escapes and observe accounting.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/merged_verification.rs` - verification dispatch after merged-delivery refusals.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/task_attribution.rs` - task-owned commit/diff attribution and shared hunk parsing.
- `cas-cli/src/mcp/tools/core/task/repo_context/candidate_probe.rs` - bounded lifecycle repository discovery.
- `cas-cli/src/mcp/tools/core/agent_coordination/branch_adoption.rs` - guarded parked-branch transfer between worktrees.
- `cas-cli/src/mcp/tools/service/`, `cas-cli/src/mcp/tools/types/` - action dispatch, factory operations and per-action request schemas.
- `cas-cli/src/bridge/server/`, `crates/cas-mcp/src/types/` - HTTP/SSE handlers and shared protocol definitions.

## Storage, retrieval and clients

- `crates/cas-store/src/prompt_queue_store.rs`, `crates/cas-store/src/accepted_proposal_store.rs` - prompt delivery and proposal ownership.
- `crates/cas-store/src/task_note_markers.rs` - trusted artifact/proposal append markers.
- `crates/cas-store/src/rule_candidate_store.rs`, `crates/cas-store/src/communication_slo/` - rule review and communication metrics/SQL.
- `crates/cas-core/src/hooks/`, `crates/cas-core/src/memory/` - hook schemas/dispatch and memory maintenance.
- `crates/cas-core/src/search/temporal/`, `crates/cas-core/src/sync/` - temporal retrieval and synchronization.
- `cas-cli/src/hybrid_search/`, `cas-cli/src/history/` - composed retrieval and Git-history search.
- `hub-web/src/main.ts`, `hub-web/src/conversation-store.ts`, `hub-web/src/inbox/`, `hub-web/src/connection.ts` - Commander startup, conversations, reply grants and connections.
- `hub-web/e2e/journeys/`, `hub-web/preview/` - browser journeys and isolated previews.
- `slack-bridge/src/router-main.ts`, `slack-bridge/src/daemon-main.ts` - Slack routing and daemon entrypoints.

## Cross-cutting

- **Tests:** inline Rust modules, `crates/cas-store/tests/`, and explicit `cas-cli/tests/integration/` harnesses; canonical environment guard in `cas-cli/src/test_env_guard.rs`.
- **Browser proof:** `hub-web/src/` Vitest, `hub-web/e2e/` Playwright and `scripts/journey-eval.sh` affected journeys.
- **Contributor policy:** `cas-cli/docs/CONTRIBUTING.md`, `cas-cli/docs/ARCHITECTURE.md`.
- **QA contracts:** `docs/qa/`, `scripts/terminal-qa.mjs`; **release records:** `docs/release-notes/`, `docs/release-reports/`.
- **Release tooling:** `scripts/release.sh`, `scripts/release-gate.sh`, `scripts/release-train-announce.py`, `cas-cli/src/cli/release_report.rs`.
- **Proof tooling:** `scripts/qa-candidate-build.py`, `scripts/release_env.py`, `scripts/release_scratch.py`, `scripts/fast-rows-pre-push.sh`.
- **Markdown/runbooks:** `scripts/markdownlint.py`, `.markdownlint-cli2.jsonc`, `ops/systemd/`, `docs/branch-protection/`, `docs/ci/`.
- **Config:** `.mcp.json`, `.env.worktree.template`, `.claude/settings.json`, `.codex/hooks.json` - MCP, worktree and harness projections.

## Entrypoints

- CLI/library: `cas-cli/src/main.rs`, `cas-cli/src/lib.rs` (`cas`); factory TUI/daemon: `cas-cli/src/ui/factory/app/mod.rs`, `cas-cli/src/ui/factory/daemon/mod.rs`.
- MCP: `cas-cli/src/mcp/daemon.rs` (`cas serve`); HTTP: `cas-cli/src/bridge/server/` (`cas bridge serve`).
- Commander: `hub-web/src/main.ts` (Vite client served by `cas hub`); hooks/setup: `cas-cli/src/cli/hook.rs`, `cas-cli/src/cli/setup.rs`.
- Worker Rust proof: capped `cargo nextest run -p <crate> --lib -E 'test(name)'`, `cargo check -p <crate> --lib` or `--tests`; full assembly belongs to the merge queue.
- Web checks: `hub-web/package.json` Vitest/typecheck and `scripts/journey-eval.sh` affected selection.
