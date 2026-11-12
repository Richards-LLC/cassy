# Cassy — Codemap

> Structural map of eighteen Rust workspace members, Commander and the Slack bridge. Regenerate with `/codemap` after module moves.

## Top-level layout

- `cas-cli/`, `crates/` — application composition and shared Rust packages.
- `hub-web/`, `slack-bridge/` — Commander browser client and TypeScript Slack routing services.
- `contrib/`, `homebrew/` — shell integrations and package formulas.
- `docs/`, `site/` — architecture, design, QA and release documents; website and system PDF.
- `fixtures/`, `migration/` — retrieval fixtures and historical Cloud migration scripts.
- `ops/`, `scripts/` — service units, host installation, proof runners and release tooling.
- `.cargo/`, `.config/` — Cargo settings and nextest profiles.
- `.claude/`, `.codex/` — harness instructions, projections and repository navigation.
- `.githooks/`, `.github/` — Git policy hooks, CI workflows and issue templates.
- `Cargo.toml`, `package.json`, `rust-toolchain.toml` — workspace, repository tooling and toolchain roots.
- `README.md`, `AGENTS.md`, `CLAUDE.md`, `CONTRIBUTING.md` — onboarding and contributor policies.
- `DESIGN.md`, `CODING_STANDARDS.md`, `CHANGELOG.md`, `CAS-DEEP-DIVE.md` — design, standards and system references.
- `changelog.d/` — delivery fragments compiled by `scripts/release_changelog.py`.

## Workspace / packages

- `cas-cli/` — executable `cas` and composed application library.
- `crates/cas-types/` — task, agent, delivery, event and wire types.
- `crates/cas-store/` — SQLite task, agent, verification, prompt, QA and knowledge stores.
- `crates/cas-search/` — Tantivy indexes, LMDB vectors, scoring and retrieval primitives.
- `crates/cas-core/` — hooks, extraction, memory hygiene, temporal search and sync.
- `crates/cas-code/` — tree-sitter parsing, symbol extraction and code chunks.
- `crates/cas-mcp/`, `crates/cas-mcp-proxy/` — protocol registry and upstream MCP policy/connections.
- `crates/cas-factory/`, `crates/cas-factory-protocol/` — sessions, probes, routing, spec resolution and transport.
- `crates/cas-mux/`, `crates/cas-pty/` — panes, input routing, terminal rendering and harness isolation.
- `crates/cas-recording/`, `crates/cas-diffs/` — recording exports and diff parsing/highlighting.
- `crates/cas-operator-crypto/` — HPKE/JWS envelopes shared with Cloud.
- `crates/cas-tui-test/` — PTY runners, screen assertions and test artifacts.
- `crates/ghostty_vt/`, `crates/ghostty_vt_sys/` — terminal parser and Ghostty FFI/build support.

## CLI, hooks and application services

- `cas-cli/src/main.rs`, `cas-cli/src/lib.rs`, `cas-cli/src/cli/mod.rs` — executable, library modules and command dispatch.
- `cas-cli/src/cli/init.rs`, `cas-cli/src/cli/setup.rs`, `cas-cli/src/cli/project_files/` — initialization, provisioning and managed-file sync.
- `cas-cli/src/accounts.rs`, `cas-cli/src/account_registry/`, `cas-cli/src/account_selection.rs` — account inventory, discovery and recommendations.
- `cas-cli/src/account_lifecycle.rs`, `cas-cli/src/account_links.rs`, `cas-cli/src/cli/account_picker.rs` — account mutations, links and selection.
- `cas-cli/src/cli/claude.rs`, `cas-cli/src/cli/codex.rs`, `cas-cli/src/agent_model.rs` — harness launches and model observations.
- `cas-cli/src/cli/doctor/`, `cas-cli/src/cli/update.rs`, `cas-cli/src/ops/` — diagnostics, upgrades and operational commands.
- `cas-cli/src/cli/release_precheck.rs`, `scripts/release-precheck.py` — exact-SHA prechecks and dispatch recovery.
- `cas-cli/src/cli/release_automation.rs`, `cas-cli/src/cli/release_adapters.rs` — release inventory, promotion and reconciliation adapters.
- `cas-cli/src/cli/release_notice_failure.rs`, `cas-cli/src/issue_reconcile.rs` — exhausted release notices and issue reconciliation.
- `cas-cli/src/hooks/handlers/`, `crates/cas-core/src/hooks/` — hook dispatch, handlers and shared schemas.
- `cas-cli/src/config/`, `cas-cli/src/harness_policy.rs` — runtime settings and harness policy projections.
- `cas-cli/src/hybrid_search/`, `cas-cli/src/knowledge/`, `cas-cli/src/extraction/` — composed retrieval, distillation and extraction.
- `cas-cli/src/consolidation/`, `cas-cli/src/sync/`, `cas-cli/src/memory_migration/` — memory maintenance and harness synchronization.
- `cas-cli/src/artifacts/`, `cas-cli/src/deliverable/` — artifact publication, proof capture and report finalization.
- `cas-cli/src/builtins/` — canonical skills, agents, harness catalogs and reference-history manifest.
- `cas-cli/src/rule_refs.rs`, `cas-cli/src/cli/rules.rs`, `docs/RULE_REFERENCES.md` — rule-marker navigation.
- `cas-cli/src/model_eval.rs`, `cas-cli/src/phase_outcome.rs`, `fixtures/verification-plan-fit/` — dataset proofs and verifier replay fixtures.

## Factory runtime and terminals

- `cas-cli/src/ui/factory/app/`, `cas-cli/src/ui/factory/director/` — TUI state, dispatch, readiness and provisioning.
- `cas-cli/src/ui/factory/daemon/runtime/` — lifecycle jobs, worker events, polling and queues.
- `cas-cli/src/ui/factory/daemon/runtime/delivery.rs`, `cas-cli/src/ui/factory/daemon/runtime/prompt_queue_jobs.rs` — delivery envelopes and prompt receipts.
- `cas-cli/src/ui/factory/daemon/runtime/violet_intake.rs`, `cas-cli/src/ui/factory/daemon/runtime/violet_reader.rs` — Slack intake and managed claim reads.
- `cas-cli/src/ui/factory/daemon/runtime/progress_watchdog.rs`, `cas-cli/src/ui/factory/daemon/runtime/loop_watchdog.rs` — progress and stalled-loop detection.
- `cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs`, `cas-cli/src/ui/factory/daemon/runtime/delivery_probe_jobs.rs` — reminders and delivery/completion probes.
- `cas-cli/src/ui/factory/client.rs`, `cas-cli/src/ui/factory/session/`, `cas-cli/src/ui/factory/server_registry.rs` — attach and server ownership.
- `cas-cli/src/ui/factory/supervisor_commands.rs`, `cas-cli/src/ui/factory/daemon/runtime/supervisor_commands.rs` — command admission and execution evidence.
- `cas-cli/src/ui/factory/daemon/runtime/commander_mirror.rs`, `cas-cli/src/ui/factory/daemon/runtime/quality_turns.rs` — transcript mirrors and turn accounting.
- `cas-cli/src/task_assignment.rs`, `cas-cli/src/task_ownership.rs`, `cas-cli/src/worker_task_queue.rs` — task binding and ordered backlogs.
- `cas-cli/src/factory_auth_health.rs`, `cas-cli/src/factory_candidate_queue.rs`, `cas-cli/src/factory_review_scratch.rs` — authentication, candidate and review checkout ownership.
- `cas-cli/src/factory_worker_check.rs`, `cas-cli/src/factory_worker_runs.rs` — capped proof commands, detached jobs and completion records.
- `cas-cli/src/factory_build_guard.rs`, `cas-cli/src/factory_capacity_wait.rs`, `cas-cli/src/factory_target_cache/` — build admission, capacity and target ownership.
- `cas-cli/src/supervisor_auto_clear.rs`, `cas-cli/src/supervisor_continuity.rs`, `cas-cli/src/supervisor_message_wakes.rs` — continuity and typed wakes.
- `cas-cli/src/supervisor_notices.rs`, `cas-cli/src/peer_ask_gate.rs`, `cas-cli/src/cloud/peer_mailbox/` — durable notices and peer asks.
- `cas-cli/src/train_guardrails.rs`, `cas-cli/src/train_ci.rs`, `cas-cli/src/factory_ci_retry.rs` — train guards, proof and retries.
- `cas-cli/src/worktree/git/`, `cas-cli/src/worktree/retained.rs`, `scripts/cas-merge-drivers.py` — Git operations, rescue refs and shared merge drivers.
- `cas-cli/src/cli/factory/hold.rs`, `crates/cas-store/src/project_hold.rs` — operator project holds.
- `crates/cas-pty/src/adopt.rs`, `crates/cas-mux/src/pane/handoff.rs`, `cas-cli/src/ui/factory/daemon/reexec.rs` — daemon replacement and terminal adoption.

## MCP, storage and delivery gates

- `cas-cli/src/gates/`, `cas-cli/build.rs` — generated stable gate codes, classifications and registry stamps.
- `cas-cli/src/mcp/daemon.rs`, `cas-cli/src/mcp/server/` — stdio transport, caller identity and backend replacement.
- `cas-cli/src/mcp/tools/core/`, `cas-cli/src/mcp/tools/service/` — operations, registrations and bounded tool futures.
- `cas-cli/src/mcp/tools/service/server_handler.rs`, `cas-cli/src/mcp/tools/service/mutation_receipt.rs` — response budgets and mutation receipts.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs` — park/close orchestration, ownership and timeout recovery.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/` — attribution (including cancelled-to-successor delivery receipts), receipts, report quality, proof claims and reviewed corrections.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/registry_upkeep.rs` — shared delivery snapshots and finding-bound decisions used by report quality; registry enforcement is lane lint.
- `cas-cli/src/mcp/tools/core/task/lifecycle/merge_close.rs`, `cas-cli/src/mcp/tools/core/task/lifecycle/supervisor_push.rs` — post-merge close and recovery-owner notices.
- `cas-cli/src/mcp/tools/core/task/close_refusal.rs`, `cas-cli/src/mcp/tools/core/task/notes.rs` — close refusal facts and trusted note markers.
- `cas-cli/src/mcp/tools/core/task/rebind.rs`, `cas-cli/src/mcp/tools/core/task/recover_delivery.rs` — branch and delivery recovery.
- `cas-cli/src/mcp/tools/core/task/lifecycle/qa_dispatch.rs`, `cas-cli/src/mcp/tools/core/workflow/premerge_verification/` — review dispatch, candidate verification and cycle-bound approval stamps (`premerge_verification/cycle.rs`).
- `cas-cli/src/mcp/tools/core/workflow/worktree_ops.rs`, `cas-cli/src/mcp/tools/core/workflow/protected_pr.rs` — candidate merges and protected publication.
- `cas-cli/src/mcp/tools/core/workflow/train_merge.rs`, `cas-cli/src/mcp/tools/core/workflow/release_freeze.rs` — ordered train merges and release freeze admission.
- `cas-cli/src/mcp/tools/core/task/source_claim.rs`, `cas-cli/src/cloud/peer_claims.rs`, `cas-cli/src/cloud/epic_split.rs` — source claims and cross-machine work shares.
- `cas-cli/src/store/`, `cas-cli/src/migration/migrations/`, `cas-cli/src/worktree/` — store decorators, migrations and worktree lifecycle.
- `crates/cas-store/src/delivery_cycle.rs`, `crates/cas-types/src/delivery.rs` — delivery cycles, anchor lineage and typed delivery data.
- `crates/cas-store/src/prompt_queue_store.rs`, `crates/cas-store/src/prompt_queue_store/` — prompt forwarding, receipt retries and expiry.
- `crates/cas-store/src/task_store.rs`, `cas-cli/src/cloud/syncer/pull.rs` — conditional task storage and synced-cycle reconciliation.
- `crates/cas-core/src/memory/`, `crates/cas-core/src/search/temporal/` — memory and temporal search primitives.

## QA, Commander and services

- `cas-cli/src/qa_registry.rs`, `cas-cli/src/qa_impact.rs`, `cas-cli/src/qa_upkeep.rs` — journey validation, diff impact and lane upkeep.
- `cas-cli/src/qa_ledger.rs`, `cas-cli/src/qa_evidence.rs`, `cas-cli/src/qa_pass/` — ledger parsing, proof manifests and independent QA dispatch.
- `cas-cli/src/qa_batch.rs`, `cas-cli/src/qa_batch/`, `cas-cli/src/qa_post_release.rs`, `cas-cli/src/release_reconcile.rs` — release membership, external-store replay identity, batch verdicts and durable reconciliation.
- `cas-cli/src/fast_lane.rs`, `cas-cli/src/fast_lane_walk.rs` — fast-lane eligibility and post-merge proof.
- `cas-cli/src/quality_ledger/`, `crates/cas-store/src/quality_event_store.rs`, `docs/ESTIMATES.md` — quality events, delivery metrics and measured estimates.
- `cas-cli/src/hub/`, `cas-cli/src/bridge/server/`, `cas-cli/src/cli/hub.rs` — Commander service, bridge, pairing and durable input.
- `hub-web/src/main.ts`, `hub-web/src/styles.css` — browser bootstrap, worker cards and desktop/phone presentation.
- `hub-web/src/connection.ts`, `hub-web/src/status-refresh.ts`, `hub-web/src/conversation-store.ts` — authenticated connections and conversation state.
- `hub-web/src/conversation-history.ts`, `hub-web/src/thread-window.ts`, `hub-web/src/early-thread.ts` — transcript paging and bounded windows.
- `hub-web/src/project-navigation.ts`, `hub-web/src/epic-progress.ts`, `hub-web/src/team-view.ts` — project, train and team views.
- `hub-web/src/work-events.ts`, `hub-web/src/work-picture.ts`, `hub-web/src/worker-timeline.ts` — activity and operation disclosures.
- `hub-web/src/markdown-renderer.ts`, `hub-web/src/rich-diagram.ts`, `hub-web/src/rich-attachment.ts` — Markdown, diagrams and attachment rendering.
- `hub-web/src/composer-uploads.ts`, `cas-cli/src/hub/composer_upload.rs` — upload identity and progress.
- `hub-web/src/pairing-dialog.ts`, `hub-web/src/write-grant.ts`, `hub-web/src/accounts-panel.ts` — pairing, grants and accounts.
- `hub-web/e2e/journeys/`, `hub-web/preview/` — browser journeys, protocol doubles and real-hub fixtures.
- `slack-bridge/src/router-main.ts`, `slack-bridge/src/daemon-main.ts` — Slack router and user-daemon entrypoints.

## Cross-cutting

- **Tests:** inline Rust modules and `cas-cli/tests/`; process isolation uses `cas-cli/src/test_env_guard.rs`. Workers run capped named nextest filters; the merge queue runs the Rust suite.
- **Browser proof:** Vitest under `hub-web/src/`, Playwright under `hub-web/e2e/`; affected selection in `scripts/journeys-for-diff.py`.
- **Docs:** `cas-cli/docs/ARCHITECTURE.md`, `cas-cli/docs/CONTRIBUTING.md`, `docs/design/`, `docs/qa/`, `docs/release-reports/`.
- **Generated docs:** `docs/product-overview.d/`, `docs/qa/bug-families/`, `scripts/doc-fragments.py` — fragments, lane lint and release compilation.
- **Release/CI:** `scripts/release.sh`, `scripts/train-ci.py`, `.github/workflows/ci.yml`, `.github/workflows/full-validation.yml`.
- **Proof/scripts:** `scripts/run-verified-tests.sh`, `scripts/fast-rows-pre-push.sh`, `scripts/check-test-shape.py`, `scripts/check-changed-markdown.py`.
- **Config:** `.mcp.json`, `.claude/settings.json`, `.codex/hooks.json`, `cas-cli/src/config/`, `hub-web/DESIGN.md`.

## Entrypoints

- **CLI/library:** `cas-cli/src/main.rs` (`cas`), `cas-cli/src/lib.rs`; MCP daemon `cas-cli/src/mcp/daemon.rs` (`cas serve`).
- **Factory:** `cas-cli/src/ui/factory/app/mod.rs`, `cas-cli/src/ui/factory/daemon/mod.rs`; Commander `hub-web/src/main.ts`.
