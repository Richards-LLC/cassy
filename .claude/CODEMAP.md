# Cassy - Codemap

> Structural map of the 3.81.0 release tree (its report set is the newest in `docs/release-reports/`): 18 Rust packages and TypeScript clients. Regenerate with `/codemap` when modules move.

## Top-level layout

- `.cargo/`, `.config/` - Cargo settings and nextest profiles.
- `.claude/`, `.codex/`, `.context/` - agent instructions, harness projections and working context.
- `.githooks/`, `.github/` - Git policy hooks, CI workflows and issue templates.
- `cas-cli/` - executable composition, CLI, MCP server, hooks and factory terminal UI.
- `crates/` - shared Rust libraries and terminal bindings.
- `contrib/` - installable shell helpers and their tests.
- `docs/` - architecture, design briefs, QA journeys, operations and release records.
- `fixtures/`, `homebrew/` - retrieval parity baselines and Homebrew packaging helpers.
- `hub-web/` - Commander Vite/TypeScript application and browser journeys.
- `slack-bridge/` - TypeScript Slack routing and per-user daemon.
- `migration/`, `ops/` - historical cloud-move scripts, service units and launch wrappers.
- `scripts/` - installation, build admission, proof, journey and release tooling.
- `site/` - static project website and system PDF.
- `Cargo.toml`, `package.json`, `rust-toolchain.toml` - workspace membership, repository tooling and toolchain.
- `README.md`, `AGENTS.md`, `CLAUDE.md`, `CONTRIBUTING.md` - onboarding and agent/contributor policy.
- `DESIGN.md`, `CODING_STANDARDS.md`, `CHANGELOG.md`, `CAS-DEEP-DIVE.md` - design standards, release history and system reference.

## Workspace / packages

- `cas-cli/` - Rust executable `cas` and composed application library.
- `crates/cas-types/` - task, agent, delivery, event, provenance and wire types.
- `crates/cas-store/` - SQLite stores, queues, knowledge, verification and vector persistence.
- `crates/cas-search/` - Tantivy indexes, LMDB vectors, scoring and retrieval primitives.
- `crates/cas-core/` - hooks, extraction, memory hygiene, temporal search and sync.
- `crates/cas-code/` - tree-sitter parsing, symbol extraction and code chunks.
- `crates/cas-mcp/` - action registry, protocol types and daemon support.
- `crates/cas-mcp-proxy/` - upstream MCP connections, caller policy and health.
- `crates/cas-factory/` - worker lifecycle, sessions, probes, routing and spec resolution.
- `crates/cas-factory-protocol/` - factory transport, framing, compression and messages.
- `crates/cas-mux/` - terminal panes, harness backends, input routing and rendering.
- `crates/cas-pty/` - PTY setup, harness adapters, input queues and credential-store isolation.
- `crates/cas-recording/` - recording format, readers, writers and exports.
- `crates/cas-diffs/` - diff parsing, highlighting and terminal widgets.
- `crates/cas-operator-crypto/` - HPKE/JWS envelopes shared with Cloud.
- `crates/cas-tui-test/` - PTY runners, screen assertions and test artifacts.
- `crates/ghostty_vt/`, `crates/ghostty_vt_sys/` - safe terminal parser and Ghostty FFI/build support.

## CLI, hooks and application services

- `cas-cli/src/main.rs`, `cas-cli/src/lib.rs`, `cas-cli/src/cli/mod.rs` - executable entry, module composition, command definitions and dispatch.
- `cas-cli/src/cli/init.rs` - initialization, recovery advice and MCP provisioning.
- `cas-cli/src/cli/claude.rs`, `cas-cli/src/cli/account_picker.rs` - account selection, profile setup and login commands.
- `cas-cli/src/cli/doctor.rs`, `cas-cli/src/cli/doctor/` - health producers, grouped rendering and communication checks.
- `cas-cli/src/cli/update.rs`, `cas-cli/src/cli/update/` - update phases, binding checks, discovery and receipts.
- `cas-cli/src/cli/project_files.rs`, `cas-cli/src/cli/project_files/` - managed files, Git reconciliation and locking.
- `cas-cli/src/cli/config/`, `cas-cli/src/config/` - configuration commands, key registry and operator policy; `config/qa_capture.rs` owns project capture intent and credential-file pointers.
- `cas-cli/src/capability.rs`, `cas-cli/src/cli/hub_service.rs` - account probes, bounded auth cache and readiness.
- `cas-cli/src/cli/codemap_cmd.rs`, `cas-cli/src/hooks/handlers/handlers_events/docs_freshness.rs` - map commands, immutable documentation measurements and calling-checkout document reads.
- `cas-cli/src/cli/factory/`, `cas-cli/src/cli/integrate/` - factory operations, cache GC and integration commands.
- `cas-cli/src/cli/factory/lifecycle.rs`, `cas-cli/src/cli/factory/daemon.rs` - `cas kill` start-time-pinned escalation, `--force-stale`, fingerprint-checked worker-group reaping and bounded daemon runtime exit.
- `cas-cli/src/cli/violet_cmd/channels.rs` - configured/Violet/Cloud channel inventory, binding checks, bounded doctor probes and offline session-start evidence.
- `cas-cli/src/hooks/handlers/`, `cas-cli/src/cli/hook/` - session/tool policy, native dispatch and hook generation.
- `cas-cli/src/hooks/handlers/handlers_events/worker_denied_tools.rs`, `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs` - worker-only operator-store guard and protected-read exemptions.
- `cas-cli/src/cloud/`, `cas-cli/src/hub/` - synchronization, identity, pairing, inbox and presence.
- `cas-cli/src/cloud/syncer/pull.rs`, `cas-cli/src/cli/update/findings.rs` - persistent sync conflicts and update cause/remedy extraction.
- `cas-cli/src/cloud/epic_split.rs`, `cas-cli/src/supervisor_notices.rs` - cross-host split reconciliation, completion notices and live-recipient routing.
- `cas-cli/src/worktree/`, `cas-cli/src/worktree/git/` - worktree lifecycle and guarded Git operations.
- `cas-cli/src/store/`, `cas-cli/src/migration/migrations/` - store decorators and numbered SQLite migrations.
- `cas-cli/src/coordination/`, `cas-cli/src/notifications/` - binding intake, ownership repair and notification queues.
- `cas-cli/src/coordination/binding_runtime.rs` - project-scoped runtime inventory, socket inactivity checks and binding drain protocol.
- `cas-cli/src/task_ownership.rs`, `cas-cli/src/task_assignment.rs` - project proof and task assignment identity.
- `cas-cli/src/artifacts/`, `cas-cli/src/task_simulators.rs` - portable proofs, retention and owned simulator cleanup.
- `cas-cli/src/knowledge/`, `cas-cli/src/daemon/` - knowledge distillation and background maintenance/indexing.
- `cas-cli/src/extraction/`, `cas-cli/src/consolidation/`, `cas-cli/src/sync/`, `cas-cli/src/test_env_guard.rs` - extraction, memory decay, harness sync and isolated test environments.
- `cas-cli/src/deliverable/`, `cas-cli/src/cli/deliverable.rs` - `cas deliverable` core: embedded asset workspace, `brand` (DESIGN.md resolution in `assets/brand.py`), `finalize` (fill, inline budget, audience hygiene, print, check-pdf in `assets/`) `capture` (immutable merge-base/head QA, catalog or journey goals), `pairs` (digest/framing validation and range provenance) and the shared receipt/exit contract.
- `cas-cli/src/builtins/` - canonical agents, skills, jobs, harness mirrors and sync manifest.
- `cas-cli/src/builtins/skills/finish/`, `cas-cli/src/builtins/reference-history.json` - shared finish command and per-reference provenance.

## Factory runtime and terminals

- `cas-cli/src/ui/factory/app/mod.rs`, `cas-cli/src/ui/factory/app/init.rs` - TUI state, startup wiring and warm target seeding (copied build outputs, orphan seed-stage recovery).
- `cas-cli/src/ui/factory/client.rs`, `cas-cli/src/ui/factory/session.rs`, `cas-cli/src/ui/factory/session/socket_cleanup*.rs` - attach client signal guard, session records and locked Linux/macOS kernel socket-holder proof before orphan reaping.
- `cas-cli/src/ui/factory/app/provisioning.rs`, `cas-cli/src/ui/factory/app/render_and_ops/` - provisioning, rendering and worker/worktree actions.
- `cas-cli/src/ui/factory/daemon/runtime/` - lifecycle, startup delivery, events, probes, relays and render scheduling.
- `cas-cli/src/ui/factory/daemon/runtime/loop_watchdog.rs`, `cas-cli/src/ui/factory/daemon/runtime/client_input.rs` - wedged-loop reporting and shutdown exit; input ownership and layout resize scheduling.
- `cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs`, `cas-cli/src/mcp/tools/service/factory_remind.rs` - persisted-task reminder reconciliation, per-row expiry and task-wait defaults.
- `cas-cli/src/ui/factory/daemon/runtime/violet_intake.rs` - lifecycle-owned Slack intake and pause control.
- `cas-cli/src/ui/factory/daemon/runtime/violet_reader.rs` - managed delivery decoding, recovery ordering and idempotent wake intake.
- `cas-cli/src/ui/factory/daemon/runtime/violet_activity.rs` - managed wake presentation, source age and addressing.
- `cas-cli/src/ui/factory/daemon/runtime/violet_consumer.rs` - per-session daemon consumer receipt; running/stale/unknown consumer status for factory status and doctor.
- `cas-cli/src/ui/factory/director/`, `cas-cli/src/ui/factory/server_registry/` - dispatch, reminders, stalls and owned servers.
- `cas-cli/src/supervisor_continuity.rs`, `cas-cli/src/supervisor_branch_recovery.rs` - durable succession and authenticated worker-branch recovery.
- `cas-cli/src/factory_session_scope.rs`, `cas-cli/src/mcp/tools/service/factory_ops.rs` - session ownership, adopted-orphan worker routing, reset and fingerprinted shutdown.
- `cas-cli/src/ui/factory/app/assignment_evidence.rs`, `cas-cli/src/supervisor_message_wakes.rs` - assignment receipts and wake batching.
- `cas-cli/src/ui/factory/app/merge_alert.rs`, `cas-cli/src/ui/factory/app/director_refresh.rs` - pushed-green signals, shared external probes, readiness cache and durable per-tip admission.
- `cas-cli/src/factory_capacity_wait.rs` - lease-bound capacity episodes, retry visibility and assistance deadlines.
- `cas-cli/src/factory_binary_skew.rs`, `cas-cli/src/history/epochs.rs` - live binary/version identity and runtime epoch history.
- `cas-cli/src/factory_build_guard.rs`, `cas-cli/src/factory_review_scratch.rs` - build-pressure admission, review checkouts, inherited flags, target reuse and freeable-inode accounting.
- `cas-cli/src/factory_worker_check.rs` - capped checks/tests, receipt rebinding and supervisor-recorded evidence.
- `cas-cli/src/factory_target_cache.rs`, `cas-cli/src/factory_target_cache/`, `cas-cli/src/factory_scratch.rs` - budgeted cache inventory, removed-checkout ownership, measured-capacity GC and retirement.
- `crates/cas-mux/src/backend/`, `crates/cas-mux/src/pane/` - harness launch, readiness and terminal snapshots.
- `crates/cas-mux/src/mux.rs`, `crates/cas-mux/src/submit_gate.rs` - multiplexer events, input injection, deadline-bounded worker teardown and submit gating.
- `crates/cas-pty/src/store_isolation.rs`, `crates/cas-pty/conformance/` - credential-store masking and harness conformance proofs.

## MCP and transport

- `cas-cli/src/mcp/daemon.rs`, `cas-cli/src/mcp/socket.rs`, `cas-cli/src/mcp/server/` - server lifecycle, Unix transport, prompts and watchdog.
- `cas-cli/src/mcp/tools/core/` - task, memory, knowledge, artifact, search, rule and workflow handlers.
- `cas-cli/src/mcp/tools/core/task/lifecycle/` - start/close, evidence gates, epic lanes and merged-delivery closure.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs`, `cas-cli/src/mcp/tools/core/task/lifecycle/repository_proof.rs` - delivery anchors, worktree-keyed receipts and immutable introducing-merge review boundaries.
- `cas-cli/src/mcp/tools/core/task/lifecycle/merge_close.rs`, `cas-cli/src/mcp/tools/core/workflow/verification_tools.rs` - green-merge parking, verdict-driven close/reopen and held-close restoration.
- `cas-cli/src/mcp/tools/core/task/block_hold.rs`, `cas-cli/src/mcp/tools/core/task/lifecycle/supervisor_push.rs` - sole-owned verification-wait classification, quiet lifecycle receipts and delivery notices.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/task_attribution.rs` - task-owned commit/diff attribution.
- `cas-cli/src/mcp/tools/core/task/rebind.rs` - supervisor approval and audited rebinding of a worker's replacement delivery branch.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/receipt_scope.rs` - receipt scope and delivery evolution checks.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/delivery_evolution.rs`, `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/evidence_only.rs` - surviving content, reference-history proof and net evidence-only diffs.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/registry_upkeep.rs`, `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/docs_upkeep.rs` - journey registry and documentation gates.
- `cas-cli/src/mcp/tools/core/task/repo_context/candidate_probe.rs` - bounded repository discovery for lifecycle operations.
- `cas-cli/src/mcp/tools/core/agent_coordination/branch_adoption.rs` - guarded parked-branch transfers and target-contained receiver HEAD adoption.
- `cas-cli/src/mcp/tools/service/`, `cas-cli/src/mcp/tools/types/` - factory/reminder/server dispatch and request schemas; composition/future-size guards live in `cas-cli/src/mcp/tools/service/mod.rs`.
- `cas-cli/src/bridge/server/`, `crates/cas-mcp/src/types/` - HTTP/SSE handlers and protocol definitions.

## QA, storage, retrieval and clients

- `cas-cli/src/qa_registry.rs`, `cas-cli/src/qa_seed.rs`, `cas-cli/src/qa_impact.rs`, `cas-cli/src/qa_impact_io.rs` - journey safety, path filters, impact snapshots and selection.
- `cas-cli/src/qa_traffic.rs`, `cas-cli/src/hub/usage.rs` - `cas qa journeys --traffic` shares from the hub's Commander usage ledger (`POST /v1/usage`).
- `cas-cli/src/qa_post_release.rs`, `cas-cli/src/cli/qa_post_release.rs`, `cas-cli/src/qa_pass/`, `cas-cli/src/qa_evidence.rs`, `cas-cli/src/config/upkeep.rs` - release observations, independent QA, trace rebinds and upkeep gates.
- `crates/cas-store/src/agent_store/continuity.rs` - retirement snapshots, process evidence, end causes and successor claims.
- `crates/cas-store/src/prompt_queue_store.rs`, `crates/cas-store/src/accepted_proposal_store.rs` - durable prompts and proposal ownership.
- `crates/cas-core/src/hooks/`, `crates/cas-core/src/memory/`, `crates/cas-core/src/search/temporal/` - hook schemas, memory maintenance and temporal retrieval.
- `cas-cli/src/hybrid_search/`, `cas-cli/src/history/` - composed retrieval and Git-history search.
- `hub-web/src/main.ts`, `hub-web/src/conversation-store.ts`, `hub-web/src/inbox/`, `hub-web/src/connection.ts` - Commander startup, conversations, reply grants and connections.
- `hub-web/src/usage.ts` - per-journey usage counter (action to HUB journey map), flushed to every paired machine.
- `hub-web/e2e/journeys/`, `hub-web/preview/` - browser journeys and isolated previews.
- `slack-bridge/src/router-main.ts`, `slack-bridge/src/daemon-main.ts` - Slack routing and daemon entrypoints.

## Cross-cutting

- **Tests:** inline Rust modules, `crates/cas-store/tests/`, explicit `cas-cli/tests/` harnesses and `cas-cli/tests/integration/` modules.
- **Browser proof:** `hub-web/src/` Vitest, `hub-web/e2e/` Playwright and `scripts/journey-eval.sh` affected selection.
- **Contributor policy / Rust proof:** `cas-cli/docs/CONTRIBUTING.md`, `cas-cli/docs/ARCHITECTURE.md`, and capped named `cargo nextest` / `cargo check` commands.
- **QA contracts:** `docs/qa/`, `scripts/terminal-qa.mjs` (ANSI capture parser and checks); **release records:** `docs/release-notes/` Slack drafts, `docs/release-reports/` per-version Markdown, brief, HTML, PDF and visual-QA reports.
- **Design records:** `docs/design/` tokens and briefs; `docs/design/cli/` terminal briefs (e.g. `violet-channel-diagnostics.brief.md`), locale/width captures and `docs/design/cli/captures/`.
- **Release tooling:** `scripts/release.sh`, `scripts/release-gate.sh`, `scripts/release-train-announce.py`, `scripts/find-release-prebuild.sh`, `scripts/release-report-post.py`, `cas-cli/src/cli/release_report.rs`, `cas-cli/src/builtins/skills/cas-release-report/scripts/check-pdf.py`.
- **Proof tooling:** `scripts/qa-candidate-build.py`, `scripts/release_env.py`, `scripts/release_scratch.py`, `scripts/fast-rows-pre-push.sh`.
- **Terminal regression routing:** `scripts/terminal-qa.test.mjs`, `scripts/ci-script-tests-for-diff.py`, `.github/workflows/lane.yml` - changed-input Node suite selection in normal fast rows and slim CI.
- **Runner setup:** `scripts/install-cassy-actions-runner.sh`, `scripts/cassy-actions-runner-registration.sh`, `scripts/test-cassy-actions-runner-registration.sh`, and `ops/systemd/` runner units.
- **Process-proof safety:** `scripts/run-verified-tests.sh`, `scripts/test-factory-client-terminal.py` - bounded discovery/process groups and PTY exit fixtures.
- **Doc validation:** `scripts/check-changed-markdown.py`, `scripts/check-design-paths.py`, `scripts/release-train-announce.py` - Markdown, portable paths and four-body draft checks.
- **Runbooks/config:** `ops/systemd/`, `docs/branch-protection/`, `docs/ci/`, `.mcp.json`, `.env.worktree.template`, `.claude/settings.json`, `.codex/hooks.json`.

## Entrypoints

- CLI/library: `cas-cli/src/main.rs`, `cas-cli/src/lib.rs` (`cas`); TUI/daemon: `cas-cli/src/ui/factory/app/mod.rs`, `cas-cli/src/ui/factory/daemon/mod.rs`.
- MCP: `cas-cli/src/mcp/daemon.rs` (`cas serve`); HTTP: `cas-cli/src/bridge/server/` (`cas bridge serve`).
- Commander: `hub-web/src/main.ts` (Vite client served by `cas hub`); setup/hooks: `cas-cli/src/cli/setup.rs`, `cas-cli/src/cli/hook.rs`.
- Web proof: `hub-web/package.json` Vitest/typecheck and `scripts/journey-eval.sh` affected journeys.
