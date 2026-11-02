# Cassy - Codemap

> Auto-generated structural map of the 3.98 integration tree. Regenerate with `/codemap` when modules move.

## Top-level layout

- `cas-cli/`, `crates/` - application composition and shared Rust workspace packages.
- `hub-web/`, `slack-bridge/` - Commander browser client and TypeScript Slack routing services.
- `contrib/`, `homebrew/` - installable shell integrations and package formulas.
- `docs/`, `site/` - architecture, design, QA and release documents; website and system PDF.
- `fixtures/`, `migration/` - retrieval parity fixtures and historical Cloud-move scripts.
- `ops/`, `scripts/` - service units, host installation, proof runners and release tooling.
- `.cargo/`, `.config/` - Cargo configuration and nextest profiles.
- `.claude/`, `.codex/` - harness instructions, projections and repository navigation.
- `.githooks/`, `.github/` - Git policy hooks, CI workflows and issue templates.
- `Cargo.toml`, `package.json`, `rust-toolchain.toml` - workspace, script and toolchain roots.
- `README.md`, `AGENTS.md`, `CLAUDE.md`, `CONTRIBUTING.md` - onboarding and contributor policies.
- `DESIGN.md`, `CODING_STANDARDS.md`, `CHANGELOG.md`, `CAS-DEEP-DIVE.md` - design, standards and system references.

## Workspace / packages

- `cas-cli/` - Rust executable `cas` and composed application library.
- `crates/cas-types/` - task, agent, delivery, event and wire types.
- `crates/cas-store/` - SQLite task, agent, verification, prompt, QA and knowledge stores.
- `crates/cas-search/` - Tantivy indexes, LMDB vectors, scoring and retrieval primitives.
- `crates/cas-core/` - hooks, extraction, memory hygiene, temporal search and sync.
- `crates/cas-code/` - tree-sitter parsing, symbol extraction and code chunks.
- `crates/cas-mcp/` - action registry, protocol types and daemon support.
- `crates/cas-mcp-proxy/` - upstream MCP connections, caller policy and health.
- `crates/cas-factory/` - lifecycle, sessions, probes, routing and spec resolution.
- `crates/cas-factory-protocol/` - factory transport, framing, compression and messages.
- `crates/cas-mux/` - panes, harness backends, input routing and terminal rendering.
- `crates/cas-pty/` - PTY setup, harness adapters, input queues and account isolation.
- `crates/cas-recording/`, `crates/cas-diffs/` - recording exports and diff parsing/highlighting.
- `crates/cas-operator-crypto/` - HPKE/JWS envelopes shared with Cloud.
- `crates/cas-tui-test/` - PTY runners, screen assertions and test artifacts.
- `crates/ghostty_vt/`, `crates/ghostty_vt_sys/` - terminal parser and Ghostty FFI/build support.

## CLI, hooks and application services

- `cas-cli/src/main.rs`, `cas-cli/src/lib.rs`, `cas-cli/src/cli/mod.rs` - executable, application modules and command dispatch.
- `cas-cli/src/cli/release_precheck.rs`, `scripts/release-precheck.py` - hosted exact-SHA prechecks, dispatch recovery, receipts and ref cleanup.
- `cas-cli/src/cli/init.rs`, `cas-cli/src/cli/setup.rs`, `cas-cli/src/cli/project_files/` - initialization, provisioning and managed-file reconciliation.
- `cas-cli/src/accounts.rs`, `cas-cli/src/cli/accounts.rs`, `cas-cli/src/account_registry/` - account inventory, identity discovery and native usage snapshots.
- `cas-cli/src/account_selection.rs`, `cas-cli/src/account_lifecycle.rs`, `cas-cli/src/account_links.rs` - recommendations, guarded mutations and shared-link repair.
- `cas-cli/src/cli/claude.rs`, `cas-cli/src/cli/codex.rs`, `cas-cli/src/cli/account_picker.rs`, `cas-cli/src/cli/limits.rs` - harness launch, login, account choice and capacity reports.
- `cas-cli/src/agent_model.rs` - role-scoped model/effort observations used by hooks, Commander and status.
- `cas-cli/src/cli/doctor/`, `cas-cli/src/cli/update/`, `cas-cli/src/factory_preflight.rs` - health, update receipts and account/root readiness.
- `cas-cli/src/cli/config/`, `cas-cli/src/config/` - command/config registry and operator policy.
- `cas-cli/src/cli/hook/`, `cas-cli/src/hooks/handlers/` - hook dispatch, native lifecycle handlers and policy.
- `cas-cli/src/hooks/handlers/handlers_events/docs_freshness.rs`, `cas-cli/src/cli/codemap_cmd.rs` - checkout-bound documentation freshness and structural-change reports.
- `cas-cli/src/cloud/`, `cas-cli/src/coordination/` - sync/presence, peer mailboxes and local agent binding.
- `cas-cli/src/knowledge/`, `cas-cli/src/daemon/`, `cas-cli/src/hybrid_search/` - knowledge distillation, indexing and composed retrieval.
- `cas-cli/src/extraction/`, `cas-cli/src/consolidation/`, `cas-cli/src/sync/` - extraction, memory decay and harness synchronization.
- `cas-cli/src/artifacts/`, `cas-cli/src/deliverable/` - task artifacts, brand resolution, HTML/PDF finalization and immutable captures.
- `cas-cli/src/builtins/` - canonical skills, agents, job definitions, harness catalogs and reference-history manifest.

## Factory runtime and terminals

- `cas-cli/src/ui/factory/app/` - TUI state, provisioning, worker actions and delivery/assignment evidence.
- `cas-cli/src/ui/factory/client.rs`, `cas-cli/src/ui/factory/session/` - attach signal guards and fingerprinted socket cleanup.
- `cas-cli/src/ui/factory/daemon/runtime/` - startup, lifecycle, events, probes, queued work and render scheduling.
- `cas-cli/src/ui/factory/daemon/runtime/lifecycle.rs` - attention relays, verification expiry sweeps and shutdown cleanup.
- `cas-cli/src/ui/factory/daemon/runtime/commander_mirror.rs` - transcript ownership, answer mirrors and bounded historical watches.
- `cas-cli/src/ui/factory/daemon/runtime/delivery.rs`, `cas-cli/src/ui/factory/daemon/runtime/prompt_queue_jobs.rs` - prompt readiness, queued input and recipient receipts.
- `cas-cli/src/ui/factory/daemon/runtime/loop_watchdog.rs`, `cas-cli/src/ui/factory/daemon/runtime/client_input.rs` - wedged-loop exit and input ownership.
- `cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs`, `cas-cli/src/mcp/tools/service/factory_remind.rs` - durable reminder delivery and deadline/admission timer validation.
- `cas-cli/src/ui/factory/daemon/runtime/violet_intake.rs`, `cas-cli/src/ui/factory/daemon/runtime/violet_reader.rs`, `cas-cli/src/ui/factory/daemon/runtime/violet_consumer.rs` - Slack intake, ordered wakes and per-session receipts.
- `cas-cli/src/ui/factory/director/`, `cas-cli/src/ui/factory/server_registry.rs` - dispatch/readiness attribution and owned-server process checks.
- `cas-cli/src/ui/factory/supervisor_commands.rs`, `cas-cli/src/ui/factory/daemon/runtime/supervisor_commands.rs` - command allowlists, native mappings and execution evidence.
- `cas-cli/src/task_assignment.rs`, `cas-cli/src/task_ownership.rs`, `cas-cli/src/worker_task_queue.rs` - task binding, ordered worker backlogs and durable continuation.
- `cas-cli/src/factory_candidate_queue.rs`, `cas-cli/src/factory_candidate_queue_tests.rs`, `cas-cli/src/mcp/tools/service/candidate_ops.rs` - typed requests, runner ownership, bounded sandbox probes and daemon-managed candidate builds.
- `cas-cli/src/factory_ci_retry.rs` - exact-SHA merge retries, terminal CI notices, expiry and corrupt-intent quarantine.
- `cas-cli/src/factory_worker_check.rs`, `cas-cli/src/factory_build_guard.rs`, `cas-cli/src/factory_capacity_wait.rs` - capped proof admission, builder guards and durable capacity waits.

## MCP, storage and delivery gates

- `cas-cli/src/mcp/daemon.rs`, `cas-cli/src/mcp/server/` - MCP transport and server composition.
- `cas-cli/src/mcp/tools/core/`, `cas-cli/src/mcp/tools/service/` - core operations, service dispatch and bounded tool futures.
- `cas-cli/src/mcp/tools/core/task/lifecycle/` - task start/close, repository proof, evidence gates and delivery closure.
- `cas-cli/src/mcp/tools/core/task/lifecycle/supervisor_push.rs` - lifecycle outbox, dispatch expiry ownership, old-wait fallback and approved-close grace.
- `cas-cli/src/mcp/tools/service/worker_liveness.rs`, `cas-cli/src/cli/factory/wedged.rs` - bounded transcript/process evidence, background-job completion and CPU attribution across user-work descendants.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/` - delivery attribution, proof/upkeep gates and reviewed corrections; `gate_preflight.rs` snapshots park requirements.
- `cas-cli/src/mcp/tools/core/task/lifecycle/merge_close.rs`, `cas-cli/src/mcp/tools/core/workflow/worktree_ops.rs` - merge/close reconciliation and guarded Git delivery operations.
- `cas-cli/src/mcp/tools/core/task/rebind.rs` - audited supervisor-approved branch replacement.
- `cas-cli/src/store/`, `cas-cli/src/migration/migrations/`, `cas-cli/src/worktree/` - store decorators, numbered migrations and Git/worktree lifecycle.
- `crates/cas-store/src/agent_store/continuity.rs`, `crates/cas-store/src/worker_task_queue_store.rs` - agent retirement snapshots and persistent worker queues.
- `crates/cas-store/src/prompt_queue_store/`, `crates/cas-store/src/qa_pass_store.rs` - durable prompts, recipient surfacing and QA maintenance.
- `crates/cas-core/src/hooks/`, `crates/cas-core/src/memory/`, `crates/cas-core/src/search/temporal/` - hook schemas and memory/search primitives.

## QA and Commander

- `cas-cli/src/qa_registry.rs`, `cas-cli/src/qa_impact.rs`, `cas-cli/src/qa_impact_io.rs` - registry validation and diff-scoped journey selection.
- `cas-cli/src/qa_ledger.rs`, `cas-cli/src/cli/qa_ledger.rs`, `cas-cli/src/qa_evidence.rs` - shared evidence grammar, scaffolds, validation and bundles.
- `cas-cli/src/qa_pass/`, `cas-cli/src/qa_batch.rs`, `cas-cli/src/qa_author_override.rs`, `cas-cli/src/mcp/tools/service/qa_pass_ops.rs` - independent QA, release verdicts and audited author overrides.
- `cas-cli/src/quality_ledger.rs`, `crates/cas-store/src/quality_event_store.rs`, `cas-cli/src/cli/ledger.rs` - append-only quality events and friction-ledger commands.
- `cas-cli/src/cli/hub.rs`, `cas-cli/src/cli/hub_service.rs`, `cas-cli/src/hub/tailscale.rs` - hub service lifecycle and Serve target ownership.
- `cas-cli/src/bridge/server/`, `crates/cas-mcp/src/types/` - HTTP/SSE routes and MCP action schemas.
- `cas-cli/src/hub/auth/`, `cas-cli/src/hub/operator_inbox/`, `cas-cli/src/hub/accounts.rs` - pairing, durable operator input and account projections.
- `cas-cli/src/hub/dismissed_sessions.rs` - authenticated legacy session tombstones.
- `hub-web/src/project-navigation.ts` - running-session project cards, machine filters and device-local collapse.
- `hub-web/src/main.ts`, `hub-web/src/conversation-store.ts`, `hub-web/src/inbox/` - browser bootstrap, selected conversation and reply grants.
- `hub-web/src/connection.ts`, `hub-web/src/connection-lifecycle.ts`, `hub-web/src/status-refresh.ts` - authenticated streams, subscriptions and ordered status publication.
- `hub-web/src/conversation-history.ts`, `hub-web/src/early-thread.ts`, `hub-web/src/thread-window.ts` - history paging and bounded transcript windows.
- `hub-web/src/provider-mark.ts`, `hub-web/src/live-phase.ts`, `hub-web/src/team-view.ts`, `hub-web/src/progress-model.ts` - harness marks, live phase, Team state and release/task projections.
- `hub-web/src/work-events.ts`, `hub-web/src/work-picture.ts`, `hub-web/src/work-rows-view.ts`, `hub-web/src/worker-timeline.ts` - operation disclosures and worker activity views.
- `hub-web/src/markdown-renderer.ts`, `hub-web/src/code-frame.ts`, `hub-web/src/rich-diagram.ts`, `hub-web/src/rich-attachment.ts` - safe Markdown, source controls and lazy evidence renderers.
- `hub-web/src/composer-uploads.ts`, `hub-web/src/sent-attachments.ts`, `cas-cli/src/hub/composer_upload.rs` - upload identity, progress and stored attachment routes.
- `cas-cli/src/hub/artifact_proxy.rs`, `cas-cli/src/hub/chunk_assets.rs`, `cas-cli/src/hub/visual_pages.rs` - authorized media, content chunks and passive page previews.
- `hub-web/src/pairing-dialog.ts`, `hub-web/src/write-grant.ts`, `hub-web/src/accounts-panel.ts` - pairing, scoped input grants and account controls.
- `hub-web/e2e/journeys/`, `hub-web/preview/` - catalog journeys, protocol doubles and real-hub fixtures.
- `slack-bridge/src/router-main.ts`, `slack-bridge/src/daemon-main.ts` - Slack router and per-user daemon entrypoints.

## Cross-cutting

- **Tests:** inline Rust modules, `crates/cas-store/tests/`, explicit `cas-cli/tests/` harnesses and `cas-cli/tests/integration/`; process environment isolation in `cas-cli/src/test_env_guard.rs`.
- **Browser proof:** Vitest in `hub-web/src/`, Playwright in `hub-web/e2e/`, affected selection via `scripts/journey-eval.sh` and `scripts/journeys-for-diff.py`.
- **Docs:** `cas-cli/docs/ARCHITECTURE.md`, `cas-cli/docs/CONTRIBUTING.md`, `docs/design/`, `docs/qa/`, `docs/analysis/` and `docs/qa/bug-families/`.
- **Release:** `scripts/release.sh`, `scripts/release-gate.sh`, `scripts/release-integration-gates.py`, `scripts/deploy-hub-release.py`, `docs/release-notes/`, `docs/release-reports/`.
- **CI:** `.github/workflows/ci.yml`, `.github/actions/run-script-suite-tests/`, `scripts/resolve-ci-base.sh`, `scripts/ci-queue-flakes.py` and script regression tests.
- **Host tooling:** `scripts/install-on-publish.sh`, `scripts/qa-rig/commander-two-machine/`, `ops/systemd/` and `docs/ci/`.
- **Proof tooling:** `scripts/qa-candidate-build.py`, `scripts/run-verified-tests.sh`, `scripts/fast-rows-pre-push.sh`, `scripts/check-changed-markdown.py`.
- **Config:** `.mcp.json`, `.claude/settings.json`, `.codex/hooks.json`, `cas-cli/src/config/` and `hub-web/DESIGN.md`.

## Entrypoints

- CLI: `cas-cli/src/main.rs` (`cas`); library composition: `cas-cli/src/lib.rs`.
- Factory TUI/daemon: `cas-cli/src/ui/factory/app/mod.rs`, `cas-cli/src/ui/factory/daemon/mod.rs`.
- MCP: `cas-cli/src/mcp/daemon.rs` (`cas serve`); bridge: `cas-cli/src/bridge/server/` (`cas bridge serve`).
- Commander: `hub-web/src/main.ts`; Slack services: `slack-bridge/src/router-main.ts`, `slack-bridge/src/daemon-main.ts`.
- Tests: capped named nextest filters for workers; Vitest/typecheck and affected journeys for web changes; merge queue owns the full Rust suite.
