# Cassy — Codemap

> Regenerated from the integrated release train: 18 Rust workspace members and TypeScript clients. Regenerate with `/codemap` when modules move.

## Top-level layout

- `cas-cli/`, `crates/` — application composition and shared Rust packages.
- `hub-web/`, `slack-bridge/` — Commander browser client and Slack router/daemon.
- `contrib/`, `homebrew/` — shell integrations and package formulas.
- `docs/`, `site/` — architecture, design, QA, releases and public website.
- `fixtures/`, `migration/` — retrieval fixtures and historical Cloud-move scripts.
- `ops/`, `scripts/` — services, installation, proof runners and release tooling.
- `.cargo/`, `.config/`, `.claude/`, `.codex/` — build/test configuration and harness projections.
- `.githooks/`, `.github/`, `changelog.d/` — Git policy, CI and per-delivery release fragments.
- `Cargo.toml`, `package.json`, `rust-toolchain.toml` — workspace and toolchain roots.
- `AGENTS.md`, `CONTRIBUTING.md`, `DESIGN.md`, `CODING_STANDARDS.md` — repository policies.

## Workspace / packages

- `cas-cli/` — `cas` binary and composed application library.
- `crates/cas-types/` — task, agent, delivery, event and wire types.
- `crates/cas-store/` — SQLite task, agent, prompt, verification and knowledge stores.
- `crates/cas-search/` — Tantivy indexes, LMDB vectors and retrieval scoring.
- `crates/cas-core/` — hooks, extraction, memory hygiene and temporal search.
- `crates/cas-code/` — tree-sitter parsing, symbols and code chunks.
- `crates/cas-mcp/` — action registry and MCP protocol types.
- `crates/cas-mcp-proxy/` — upstream MCP connections and caller policy.
- `crates/cas-factory/` — sessions, orchestration, probes and spec resolution.
- `crates/cas-factory-protocol/` — factory transport, framing and compression.
- `crates/cas-mux/` — panes, harness backends and input routing.
- `crates/cas-pty/` — PTY management, harness adapters and account isolation.
- `crates/cas-recording/`, `crates/cas-diffs/` — terminal recording and diff parsing.
- `crates/cas-operator-crypto/` — operator HPKE/JWS envelopes.
- `crates/cas-tui-test/` — PTY runners, screen assertions and test artifacts.
- `crates/ghostty_vt/`, `crates/ghostty_vt_sys/` — terminal parser and FFI/build support.

## CLI and application services

- `cas-cli/src/main.rs`, `cas-cli/src/lib.rs`, `cas-cli/src/cli/mod.rs` — executable, library composition and command dispatch.
- `cas-cli/src/cli/init.rs`, `cas-cli/src/cli/setup.rs`, `cas-cli/src/cli/project_files/` — initialization and managed project files.
- `cas-cli/src/cli/config/`, `cas-cli/src/config/` — configuration registry and operator policy.
- `cas-cli/src/accounts.rs`, `cas-cli/src/account_registry/`, `cas-cli/src/cli/accounts.rs` — account inventory and native usage snapshots.
- `cas-cli/src/account_selection.rs`, `cas-cli/src/account_lifecycle.rs`, `cas-cli/src/account_links.rs` — selection and authentication-scope link management.
- `cas-cli/src/cli/claude.rs`, `cas-cli/src/cli/codex.rs`, `cas-cli/src/agent_model.rs` — harness launch and model/effort observations.
- `cas-cli/src/cli/doctor/`, `cas-cli/src/cli/update/`, `cas-cli/src/factory_preflight.rs` — diagnostics, updates and factory readiness.
- `cas-cli/src/github_repo.rs`, `cas-cli/src/gh_budget.rs`, `scripts/github_queue_health.py` — repository identity, GitHub budgets and queue health.
- `cas-cli/src/model_eval.rs`, `cas-cli/src/phase_outcome.rs` — measured task/phase targets and evaluation receipts.
- `cas-cli/src/artifacts/`, `cas-cli/src/deliverable/` — artifact publication and report finalization.
- `cas-cli/src/extraction/`, `cas-cli/src/consolidation/`, `cas-cli/src/sync/` — memory extraction, decay and filesystem projections.

## MCP, storage and knowledge

- `cas-cli/src/mcp/daemon.rs`, `cas-cli/src/mcp/server/` — stdio framing, eager startup, immutable caller identity and backend replacement.
- `cas-cli/src/mcp/tools/core/`, `cas-cli/src/mcp/tools/service/` — core operations, dispatch and bounded tool futures.
- `cas-cli/src/mcp/tools/service/core.rs`, `cas-cli/src/mcp/tools/core/memory.rs` — public memory dispatch and changed-field update receipts.
- `cas-cli/src/mcp/tools/service/agent_search_system/message.rs` — directed messages, blocker answers and owner-revalidated batch-notice inbox reads.
- `cas-cli/src/mcp/tools/core/agent_coordination/agent_management.rs`, `cas-cli/src/mcp/tools/core/agent_coordination/agent_session.rs` — registered identity inspection and native-child caller boundaries.
- `cas-cli/src/mcp/tools/service/factory_ops.rs`, `cas-cli/src/mcp/tools/service/server_handler.rs` — factory operations and harness/context accounting.
- `cas-cli/src/store/`, `cas-cli/src/migration/migrations/` — store decorators and forward schema migrations.
- `cas-cli/src/store/syncing.rs`, `cas-cli/src/store/detect.rs` — rule projections, active-rule cut and store discovery.
- `cas-cli/src/knowledge/`, `cas-cli/src/daemon/`, `cas-cli/src/hybrid_search/` — knowledge distillation, indexing and retrieval.
- `cas-cli/src/cloud/`, `cas-cli/src/coordination/` — optional Cloud synchronization and presence.
- `cas-cli/src/cloud/peer_mailbox/`, `cas-cli/src/cloud/peer_claims.rs` — peer mail, same-task source handoff and source-heartbeat retirement.
- `cas-cli/src/mcp/tools/core/task/source_claim.rs` — source-claim admission for creation, staffing, queues and spawning.
- `cas-cli/src/mcp/tools/core/rules.rs`, `cas-cli/src/mcp/tools/core/rule_drift.rs`, `cas-cli/src/rule_refs.rs` — rule provenance, drift and source-reference navigation.
- `crates/cas-core/src/memory/handoff.rs`, `crates/cas-core/src/hooks/context/build_start.rs` — project/role handoff succession and startup ownership filtering.
- `crates/cas-store/src/history_store.rs`, `crates/cas-store/src/agent_store/continuity.rs` — provenance repair and retirement snapshots.

## Factory runtime and terminals

- `cas-cli/src/ui/factory/app/`, `cas-cli/src/ui/factory/daemon/runtime/` — TUI state, provisioning, loop scheduling and queued work.
- `cas-cli/src/ui/factory/director/`, `cas-cli/src/ui/factory/server_registry.rs` — readiness attribution and owned-process checks.
- `cas-cli/src/task_assignment.rs`, `cas-cli/src/task_ownership.rs`, `cas-cli/src/worker_task_queue.rs` — task binding, pre-registration queue admission and ordered continuation.
- `crates/cas-store/src/worker_task_queue_store.rs` — persistent worker backlogs.
- `cas-cli/src/factory_auth_health.rs`, `cas-cli/src/ui/factory/app/assignment_evidence.rs` — account/model admission and staffing evidence.
- `cas-cli/src/mcp/tools/service/factory_ops/spawn_names.rs` — isolated worker-name reservation.
- `cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs`, `cas-cli/src/ui/factory/daemon/runtime/delivery.rs` — durable queue selection, wake gates and delivery receipts.
- `cas-cli/src/ui/factory/daemon/runtime/director_refresh.rs`, `cas-cli/src/ui/factory/daemon/runtime/director_refresh_tests.rs` — fresh director reads and lifecycle incident replays.
- `cas-cli/src/ui/factory/daemon/runtime/lifecycle.rs`, `cas-cli/src/ui/factory/daemon/runtime/progress_watchdog.rs` — lifecycle recovery, expiry and progress escalation.
- `cas-cli/src/ui/factory/daemon/runtime/commander_mirror.rs`, `cas-cli/src/ui/factory/daemon/runtime/quality_turns.rs` — redacted transcripts and incremental turn accounting.
- `cas-cli/src/ui/factory/daemon/runtime/violet_intake.rs`, `cas-cli/src/ui/factory/daemon/runtime/violet_reader.rs` — Slack intake and recipient ownership.
- `cas-cli/src/ui/factory/supervisor_commands.rs`, `cas-cli/src/ui/factory/daemon/runtime/supervisor_commands.rs` — command admission, Finish cancellation and native execution evidence.
- `cas-cli/src/factory_worker_check.rs`, `cas-cli/src/factory_build_guard.rs`, `cas-cli/src/factory_capacity_wait.rs`, `cas-cli/src/factory_scratch.rs` — capped proof, command-stable capacity waits and owned scratch paths.
- `cas-cli/src/factory_worker_runs.rs`, `cas-cli/src/factory_build_routing.rs` — detached-run receipts and peer capacity routing.
- `cas-cli/src/factory_candidate_queue.rs`, `cas-cli/src/mcp/tools/service/candidate_ops.rs` — exact-revision candidate requests and builds.
- `cas-cli/src/factory_ci_retry.rs`, `cas-cli/src/train_ci.rs`, `cas-cli/src/train_guardrails.rs` — lane receipts, merge retries and train guardrails.
- `cas-cli/src/supervisor_continuity.rs`, `cas-cli/src/supervisor_auto_clear.rs` — succession and context handoffs.
- `cas-cli/src/worktree/`, `cas-cli/src/worktree/retained.rs`, `cas-cli/src/worktree/git/rescue.rs` — Git lifecycle, retained-work inventory and rescue refs.
- `crates/cas-pty/src/claude_binary.rs`, `crates/cas-pty/src/pty.rs` — native Claude resolution and worker startup briefs.
- `crates/cas-pty/src/adopt.rs`, `crates/cas-mux/src/pane/handoff.rs`, `cas-cli/src/ui/factory/daemon/reexec.rs` — in-place daemon handoff and PTY adoption.

## Delivery gates and hooks

- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs` — fresh pushed-tip selection from recorded task starts and parked-delivery attribution fences.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/` — proof scope, inspected changelog/delivery ranges, upkeep, reports and verified-delivery binding.
- `cas-cli/src/mcp/tools/core/task/lifecycle/supervisor_push.rs`, `cas-cli/src/mcp/tools/core/task/block_hold.rs`, `cas-cli/src/mcp/tools/core/task/notes.rs` — lifecycle outbox, task-scoped blocker answers, supervisor decisions and fresh-question deadlines.
- `cas-cli/src/mcp/tools/core/workflow/premerge_verification.rs`, `cas-cli/src/mcp/tools/core/workflow/premerge_verification/` — immutable candidates, local/peer review and verdict rebinding.
- `cas-cli/src/mcp/tools/core/task/lifecycle/merge_close.rs`, `cas-cli/src/mcp/tools/core/workflow/worktree_ops.rs` — reviewed merge operations and close reconciliation.
- `cas-cli/src/mcp/tools/core/workflow/train_merge.rs`, `cas-cli/src/mcp/tools/core/workflow/release_freeze.rs`, `cas-cli/src/mcp/tools/core/workflow/release_auto_cut.rs` — ordered integration and release admission.
- `cas-cli/src/mcp/tools/core/task/rebind.rs`, `cas-cli/src/mcp/tools/core/task/recover_delivery.rs`, `cas-cli/src/mcp/tools/core/task/release_train.rs` — branch identity, recovery and train assignment.
- `cas-cli/src/epic_design_review.rs`, `cas-cli/src/finding_triage.rs`, `crates/cas-types/src/verification.rs` — design holds, finding severity and review types.
- `cas-cli/src/hooks/handlers/`, `cas-cli/src/cli/hook/`, `cas-cli/src/hooks/handoff_updates.rs` — native hooks, policy and handoff reads.
- `cas-cli/src/builtins/` — canonical skills, agents, harness catalogs and reference history.

## QA, Commander and services

- `cas-cli/src/qa_registry.rs`, `cas-cli/src/qa_impact.rs`, `cas-cli/src/qa_ledger.rs`, `cas-cli/src/qa_evidence.rs` — journey selection and evidence grammar/bundles.
- `cas-cli/src/qa_pass/`, `cas-cli/src/qa_batch.rs`, `cas-cli/src/qa_post_release.rs`, `cas-cli/src/qa_walk_progress.rs` — review rounds, release receipts, owner-scoped batch notices and walk progress.
- `cas-cli/src/fast_lane.rs`, `cas-cli/src/fast_lane_walk.rs`, `docs/qa/fast-lane.md` — hotfix admission and failed-walk recovery.
- `cas-cli/src/quality_ledger.rs`, `cas-cli/src/quality_ledger/estimates.rs`, `docs/ESTIMATES.md` — friction events and measured estimates.
- `cas-cli/src/cli/hub.rs`, `cas-cli/src/cli/hub_service.rs`, `cas-cli/src/hub/` — hub lifecycle, pairing, operator inbox, artifacts and media routes.
- `cas-cli/src/bridge/server/`, `crates/cas-mcp/src/types/` — HTTP/SSE bridge and wire schemas.
- `hub-web/src/main.ts`, `hub-web/src/connection.ts`, `hub-web/src/connection-lifecycle.ts` — browser bootstrap, authenticated streams and reconnection.
- `hub-web/src/conversation-store.ts`, `hub-web/src/conversation-history.ts`, `hub-web/src/inbox/` — conversations, history windows and reply grants.
- `hub-web/src/project-navigation.ts`, `hub-web/src/epic-progress.ts`, `hub-web/src/team-view.ts` — projects, tasks and worker projections.
- `hub-web/src/markdown-renderer.ts`, `hub-web/src/reply-images.ts`, `hub-web/src/rich-attachment.ts` — safe reply rendering and evidence viewers.
- `hub-web/src/composer-uploads.ts`, `hub-web/src/pairing-dialog.ts`, `hub-web/src/accounts-panel.ts` — uploads, pairing and account controls.
- `hub-web/e2e/journeys/`, `hub-web/preview/`, `slack-bridge/src/router-main.ts`, `slack-bridge/src/daemon-main.ts` — browser journeys, preview fixtures and Slack services.

## Cross-cutting

- **Tests:** inline Rust modules and `cas-cli/tests/` harnesses; environment isolation in `cas-cli/src/test_env_guard.rs`.
- **Browser proof:** Vitest in `hub-web/src/`, Playwright in `hub-web/e2e/`; selectors in `scripts/journeys-for-diff.py`.
- **Docs:** `cas-cli/docs/ARCHITECTURE.md`, `cas-cli/docs/CONTRIBUTING.md`, `docs/design/`, `docs/qa/` and `docs/status-words.md`.
- **Release:** `cas-cli/src/cli/release_automation.rs` (train assignment, historical backfill and close outcomes), `scripts/release.sh`, `scripts/release-gate.sh`, `scripts/release_changelog.py`, `docs/release-notes/`, `docs/release-reports/`.
- **CI:** `.github/workflows/`, `.github/actions/full-gate-job/`, `scripts/train-ci.py` and `scripts/resolve-ci-base.sh`.
- **Proof tooling:** `scripts/check-test-shape.py`, `scripts/run-verified-tests.sh`, `scripts/fast-rows-pre-push.sh`, `scripts/check-changed-markdown.py`.
- **Config:** `.mcp.json`, `.claude/settings.json`, `.codex/hooks.json`, `cas-cli/src/config/` and `hub-web/DESIGN.md`.

## Entrypoints

- CLI/library: `cas-cli/src/main.rs`, `cas-cli/src/lib.rs`; MCP: `cas-cli/src/mcp/daemon.rs` (`cas serve`).
- Factory TUI/daemon: `cas-cli/src/ui/factory/app/mod.rs`, `cas-cli/src/ui/factory/daemon/mod.rs`.
- Commander: `hub-web/src/main.ts`; Slack: `slack-bridge/src/router-main.ts`, `slack-bridge/src/daemon-main.ts`.
- Tests: capped named nextest filters for workers; Vitest/typecheck for touched web inputs; merge queue owns the full Rust suite.
