# Cassy - Codemap

> Regenerated from the synchronized source tree: 18 Rust workspace members and TypeScript clients. Regenerate with `/codemap` when modules move.

## Top-level layout

- `cas-cli/`, `crates/` - application composition and shared Rust packages.
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
- `changelog.d/` - delivery fragments; `scripts/release_changelog.py` promotes them and verifies delivery claims.

## Workspace / packages

- `cas-cli/` - executable `cas` and composed application library.
- `crates/cas-types/` - task, agent, delivery, event and wire types.
- `crates/cas-store/` - SQLite task, agent, verification, prompt, QA and knowledge stores.
- `crates/cas-search/` - Tantivy indexes, LMDB vectors, scoring and retrieval primitives.
- `crates/cas-core/` - hooks, extraction, memory hygiene, temporal search and sync.
- `crates/cas-code/` - tree-sitter parsing, symbol extraction and code chunks.
- `crates/cas-mcp/`, `crates/cas-mcp-proxy/` - protocol/action registry and upstream MCP policy/connections.
- `crates/cas-factory/`, `crates/cas-factory-protocol/` - sessions, probes, routing, spec resolution and transport.
- `crates/cas-mux/`, `crates/cas-pty/` - panes, input routing, terminal rendering and harness/account isolation.
- `crates/cas-recording/`, `crates/cas-diffs/` - recording exports and diff parsing/highlighting.
- `crates/cas-operator-crypto/` - HPKE/JWS envelopes shared with Cloud.
- `crates/cas-tui-test/` - PTY runners, screen assertions and test artifacts.
- `crates/ghostty_vt/`, `crates/ghostty_vt_sys/` - terminal parser and Ghostty FFI/build support.

## CLI, hooks and application services

- `cas-cli/src/main.rs`, `cas-cli/src/lib.rs`, `cas-cli/src/cli/mod.rs` - executable, application modules and command dispatch.
- `cas-cli/src/cli/init.rs`, `cas-cli/src/cli/setup.rs`, `cas-cli/src/cli/project_files/` - initialization, provisioning, managed sync and merge-residue recovery.
- `cas-cli/src/accounts.rs`, `cas-cli/src/account_registry/`, `cas-cli/src/cli/accounts.rs` - account inventory, identity discovery, link planning and native usage.
- `cas-cli/src/account_selection.rs`, `cas-cli/src/account_lifecycle.rs`, `cas-cli/src/account_links.rs` - recommendations, guarded account mutations and link repair.
- `cas-cli/src/cli/claude.rs`, `cas-cli/src/cli/codex.rs`, `cas-cli/src/cli/account_picker.rs`, `cas-cli/src/agent_model.rs` - harness launches, account choice and role model observations.
- `cas-cli/src/cli/release_precheck.rs`, `scripts/release-precheck.py` - hosted exact-SHA prechecks, dispatch recovery and receipt cleanup.
- `cas-cli/src/cli/doctor/`, `cas-cli/src/cli/update/`, `cas-cli/src/factory_preflight.rs`, `cas-cli/src/gh_budget.rs` - health, update receipts and GitHub request budgets.
- `cas-cli/src/cli/config/`, `cas-cli/src/config/` - command/config registry and operator policy.
- `cas-cli/src/cli/hook/`, `cas-cli/src/hooks/handlers/`, `cas-cli/src/hooks/handoff_updates.rs` - lifecycle policy, inherited process-signal guards and Violet handoff reads.
- `cas-cli/src/hooks/handlers/handlers_events/docs_freshness.rs`, `cas-cli/src/cli/codemap_cmd.rs` - checkout-bound documentation freshness and structural-change reports.
- `cas-cli/src/cloud/`, `cas-cli/src/coordination/` - sync, presence, scoped artifacts and worker peer coordination; `cas-cli/src/cloud/peer_mailbox/transcript.rs` serves bounded redacted peer transcripts.
- `cas-cli/src/knowledge/`, `cas-cli/src/daemon/`, `cas-cli/src/hybrid_search/` - distillation, indexing and composed retrieval.
- `cas-cli/src/extraction/`, `cas-cli/src/consolidation/`, `cas-cli/src/sync/` - extraction, memory decay and harness synchronization.
- `cas-cli/src/artifacts/`, `cas-cli/src/deliverable/` - task artifacts, brand resolution, HTML/PDF finalization and immutable captures.
- `cas-cli/src/builtins/` - canonical skills, agents, jobs, harness catalogs and reference-history manifest.
- `cas-cli/src/rule_refs.rs`, `cas-cli/src/cli/rules.rs`, `docs/RULE_REFERENCES.md` - rule-marker navigation and reference contract.
- `cas-cli/src/model_eval.rs`, `cas-cli/src/phase_outcome.rs`, `fixtures/verification-plan-fit/` - dataset-bound proof, measured phase outcomes and verifier prompt replays.

## Factory runtime and terminals

- `cas-cli/src/ui/factory/app/`, `cas-cli/src/ui/factory/director/` - TUI state, worker dispatch/readiness, provisioning and delivery evidence.
- `cas-cli/src/ui/factory/daemon/runtime/` - lifecycle, events, probes, scheduling and queued work; `lifecycle.rs` owns expiry, merge recovery and shutdown.
- `cas-cli/src/ui/factory/daemon/runtime/delivery.rs`, `cas-cli/src/ui/factory/daemon/runtime/prompt_queue_jobs.rs` - attributed envelopes, delivery-tip context and recipient receipts.
- `cas-cli/src/ui/factory/daemon/runtime/violet_intake.rs`, `cas-cli/src/ui/factory/daemon/runtime/violet_reader.rs`, `cas-cli/src/ui/factory/daemon/runtime/violet_consumer.rs` - Slack intake and ordered session wakes.
- `cas-cli/src/ui/factory/daemon/runtime/gui_client.rs`, `crates/cas-pty/src/input_queue.rs` - attach input draining and PTY byte diagnostics.
- `cas-cli/src/ui/factory/client.rs`, `cas-cli/src/ui/factory/session/`, `cas-cli/src/ui/factory/server_registry.rs` - attach guards, socket cleanup and owned-server checks.
- `cas-cli/src/ui/factory/supervisor_commands.rs`, `cas-cli/src/ui/factory/daemon/runtime/supervisor_commands.rs` - allowlists, Finish queues and native execution/reconnect evidence.
- `cas-cli/src/ui/factory/daemon/runtime/progress_watchdog.rs`, `cas-cli/src/ui/factory/daemon/runtime/loop_watchdog.rs` - progress and wedged-loop monitoring.
- `cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs`, `cas-cli/src/mcp/tools/service/factory_remind.rs` - reminder deadlines, queue admission and supervisor wake tests.
- `cas-cli/src/ui/factory/daemon/runtime/commander_mirror.rs`, `cas-cli/src/ui/factory/daemon/runtime/quality_turns.rs` - transcript mirrors, redaction and typed turn/cost accounting.
- `crates/cas-pty/src/pty.rs`, `crates/cas-pty/src/claude_binary.rs` - startup briefs, harness argv, native Claude selection and version probes.
- `cas-cli/src/task_assignment.rs`, `cas-cli/src/task_ownership.rs`, `cas-cli/src/worker_task_queue.rs` - binding and ordered backlogs, including pre-registration assignment admission.
- `cas-cli/src/factory_auth_health.rs`, `cas-cli/src/mcp/tools/service/factory_ops/spawn_names.rs` - authentication/model failure evidence and collision-free worker reservation.
- `cas-cli/src/factory_candidate_queue.rs`, `cas-cli/src/mcp/tools/service/candidate_ops.rs` - runner ownership and bounded candidate builds.
- `cas-cli/src/factory_worker_check.rs`, `cas-cli/src/factory_build_guard.rs`, `cas-cli/src/factory_capacity_wait.rs`, `cas-cli/src/factory_scratch.rs` - capped proof admission, builder guards and owned scratch.
- `cas-cli/src/factory_capacity_wait.rs` - typed queue-silence retries and aggregated disk-pressure notices.
- `cas-cli/src/factory_review_scratch.rs` - owned review-checkout creation, reuse and lifecycle cleanup.
- `cas-cli/src/factory_worker_runs.rs`, `cas-cli/src/factory_build_routing.rs` - detached command records, completion wakes and same-account capacity routing.
- `cas-cli/src/epic_design_review.rs`, `cas-cli/src/cli/factory/queries.rs` - stakeholder defect staffing pauses and explicit message routing.
- `cas-cli/src/train_guardrails.rs`, `cas-cli/src/train_ci.rs`, `cas-cli/src/factory_ci_retry.rs` - release-train guards, full-set proof and exact-tip retry notices.
- `crates/cas-core/src/memory/handoff.rs` - per-project/role machine selection and supersession; owner/session stamps are audit metadata.
- `cas-cli/src/supervisor_auto_clear.rs`, `cas-cli/src/supervisor_continuity.rs` - context pressure, succession handoffs and startup recovery.
- `cas-cli/src/supervisor_message_wakes.rs`, `cas-cli/src/peer_ask_gate.rs`, `cas-cli/src/cloud/peer_mailbox/outbox.rs` - typed wake limits and explicit-ask merge holds.
- `cas-cli/src/worktree/git/rescue.rs`, `cas-cli/src/worktree/retained.rs` - rescue refs and bounded retained-work inventories.
- `cas-cli/src/cli/factory/hold.rs`, `crates/cas-store/src/project_hold.rs` - operator project holds, audited state and relay admission.

## MCP, storage and delivery gates

- `cas-cli/src/mcp/daemon.rs`, `cas-cli/src/mcp/server/` - stdio framing, startup, in-flight accounting and compatible backend replacement.
- `cas-cli/src/mcp/tools/core/`, `cas-cli/src/mcp/tools/service/` - operations, service dispatch and bounded tool futures.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs` - first-park pushed-tip attribution and existing parked-anchor protections.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/` - attribution, receipt scope, proof/upkeep gates, rule coverage and reviewed corrections.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/verified_delivery.rs`, `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/merge_queue_proof.rs` - reviewed landing, typed filesystem refusals and queue-proof validation.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/task_attribution.rs` - authenticated supervisor import grants and linked-checkout WIP identity; empty checkpoints require explicit task attribution.
- `cas-cli/src/mcp/tools/core/task/lifecycle.rs` - recorded-start graph reconciliation for resumed worker branches and linked-checkout identity.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/proof_claims.rs` - bounded close-reason proof assertions and clause-specific receipt checks.
- `cas-cli/src/mcp/tools/core/task/lifecycle/qa_dispatch.rs`, `cas-cli/src/mcp/tools/core/task/lifecycle/merge_close.rs`, `cas-cli/src/mcp/tools/core/task/lifecycle/supervisor_push.rs` - review dispatch, read-only backfill admission, target-only parked-anchor recovery, ordinary close and lifecycle outbox.
- `cas-cli/src/mcp/tools/core/workflow/premerge_verification.rs` - immutable candidates, review scratch, plan/evidence binding, overview validation, target-move rebinding, late bound-child verdict admission and landed proof promotion.
- `cas-cli/src/mcp/tools/core/workflow/premerge_verification/automatic.rs` - account-aware automatic verifier launch, sealed child authority, exact-child crash recovery, late verdict submission and separate attempt diagnostics.
- `cas-cli/src/mcp/tools/core/agent_coordination/agent_session.rs`, `cas-cli/src/mcp/tools/core/agent_coordination/agent_management.rs`, `cas-cli/src/mcp/server/mod.rs` - connection-bound identity and child registration admission; native Codex reviewer guidance in `cas-cli/src/prompt_revalidation.rs`.
- `cas-cli/src/mcp/tools/core/workflow/premerge_verification/peer.rs`, `crates/cas-types/src/verification.rs`, `crates/cas-store/src/verification_store.rs` - peer approvals and durable portable proof boundaries.
- `cas-cli/src/mcp/tools/core/workflow/worktree_ops.rs`, `cas-cli/src/mcp/tools/core/workflow/protected_pr.rs` - guarded delivery publication, unpublished-merge restoration and exact-source protected default-branch PR routing.
- `cas-cli/src/mcp/tools/core/workflow/train_merge.rs`, `cas-cli/src/mcp/tools/core/workflow/release_freeze.rs`, `cas-cli/src/mcp/tools/core/workflow/release_auto_cut.rs` - ordered train merges, freeze and automatic cut admission.
- `cas-cli/src/mcp/tools/core/task/rebind.rs`, `cas-cli/src/mcp/tools/core/task/recover_delivery.rs`, `cas-cli/src/mcp/tools/core/task/block_hold.rs` - audited branch/delivery recovery, empty retargeted-lane replacement and task holds.
- `cas-cli/src/mcp/tools/core/task/source_claim.rs`, `cas-cli/src/cloud/peer_claims.rs` - stakeholder source admission, same-task peer staffing transfer, heartbeat retirement and fenced release.
- `cas-cli/src/finding_triage.rs`, `cas-cli/src/mcp/tools/core/task/update.rs`, `cas-cli/src/mcp/tools/core/task/release_train.rs` - journey severity labels and default release-train assignment.
- `cas-cli/src/store/`, `cas-cli/src/migration/migrations/`, `cas-cli/src/worktree/` - store decorators, numbered migrations and Git/worktree lifecycle.
- `crates/cas-store/src/agent_store/continuity.rs`, `crates/cas-store/src/worker_task_queue_store.rs` - retirement snapshots and persistent queues.
- `crates/cas-store/src/history_store.rs`, `crates/cas-store/src/prompt_queue_store.rs`, `crates/cas-store/src/qa_pass_store.rs` - provenance repair, durable prompts and QA maintenance.
- `crates/cas-core/src/hooks/`, `crates/cas-core/src/memory/`, `crates/cas-core/src/search/temporal/` - hook schemas and memory/search primitives.
- `cas-cli/src/mcp/tools/service/core.rs`, `cas-cli/src/mcp/tools/core/memory.rs`, `cas-cli/src/mcp/tools/core/rules.rs`, `cas-cli/src/mcp/tools/core/rule_drift.rs` - memory/rule dispatch, partial updates, provenance and drift.

## QA, Commander and services

- `cas-cli/src/qa_registry.rs`, `cas-cli/src/qa_impact.rs`, `cas-cli/src/qa_impact_io.rs` - journey registry validation and diff selection.
- `cas-cli/src/qa_ledger.rs`, `cas-cli/src/qa_evidence.rs`, `cas-cli/src/qa_deployed_origins.rs`, `cas-cli/src/qa_walk_progress.rs` - ledger grammar, captures, deployed origins and walk progress.
- `cas-cli/src/qa_pass/`, `cas-cli/src/qa_batch.rs`, `cas-cli/src/qa_author_override.rs`, `cas-cli/src/qa_post_release.rs` - independent QA, release verdicts, owner-scoped receipt offers and live notice revalidation at queue/inbox boundaries.
- `cas-cli/src/fast_lane.rs`, `cas-cli/src/fast_lane_walk.rs`, `docs/qa/fast-lane.md` - hotfix eligibility, post-merge proof and revert choices.
- `cas-cli/src/quality_ledger.rs`, `cas-cli/src/quality_ledger/estimates.rs`, `crates/cas-store/src/quality_event_store.rs`, `docs/ESTIMATES.md` - append-only quality events and measured estimates.
- `cas-cli/src/builtins/skills/cas-test-audit/` - read-only test-value auditing and CI timing recipes.
- `cas-cli/src/hub/`, `cas-cli/src/bridge/server/`, `cas-cli/src/cli/hub.rs` - Commander service, HTTP/SSE bridge, pairing and durable operator input.
- `cas-cli/src/cli/hub_preview.rs`, `cas-cli/src/hub/artifact_proxy.rs`, `cas-cli/src/hub/local_image.rs` - time-boxed candidate previews and authorized media.
- `hub-web/src/main.ts`, `hub-web/src/conversation-store.ts`, `hub-web/src/inbox/` - browser bootstrap, selected conversations and reply grants.
- `hub-web/src/connection.ts`, `hub-web/src/connection-lifecycle.ts`, `hub-web/src/status-refresh.ts` - authenticated streams and ordered publication.
- `hub-web/src/conversation-history.ts`, `hub-web/src/thread-window.ts`, `hub-web/src/early-thread.ts` - transcript paging and bounded windows.
- `hub-web/src/project-navigation.ts`, `hub-web/src/epic-progress.ts`, `hub-web/src/team-view.ts` - project cards, train/task progress and team status.
- `hub-web/src/work-events.ts`, `hub-web/src/work-picture.ts`, `hub-web/src/worker-timeline.ts` - operation disclosures and worker activity.
- `hub-web/src/markdown-renderer.ts`, `hub-web/src/rich-diagram.ts`, `hub-web/src/rich-attachment.ts` - safe Markdown, diagrams and evidence rendering.
- `hub-web/src/composer-uploads.ts`, `hub-web/src/sent-attachments.ts`, `cas-cli/src/hub/composer_upload.rs` - upload identity, progress and stored attachments.
- `hub-web/src/pairing-dialog.ts`, `hub-web/src/write-grant.ts`, `hub-web/src/accounts-panel.ts`, `hub-web/src/account-usage-check.ts` - pairing, input grants and account controls.
- `hub-web/e2e/journeys/`, `hub-web/preview/` - catalog journeys, protocol doubles and real-hub fixtures.
- `slack-bridge/src/router-main.ts`, `slack-bridge/src/daemon-main.ts` - Slack router and user-daemon entrypoints.

## Cross-cutting

- **Tests:** inline Rust modules, `crates/cas-store/tests/`, `cas-cli/tests/`; process isolation in `cas-cli/src/test_env_guard.rs`. Workers use capped named nextest filters; the merge queue owns the full Rust suite.
- **Browser proof:** Vitest in `hub-web/src/`, Playwright in `hub-web/e2e/`, affected selection via `scripts/journey-eval.sh` and `scripts/journeys-for-diff.py`.
- **Docs:** `cas-cli/docs/ARCHITECTURE.md`, `cas-cli/docs/CONTRIBUTING.md`, `docs/design/`, `docs/qa/`, `docs/status-words.md`.
- `cas-cli/src/cli/release_automation.rs` - train context, historical backfill admission and close outcomes, frozen-successor guards and later-tag reconciliation.
- **Release/CI:** `scripts/release.sh`, `scripts/train-ci.py`, `scripts/release-integration-gates.py`, `.github/workflows/ci.yml`, `.github/workflows/full-validation.yml` and `.github/actions/full-gate-job/`.
- **Proof/scripts:** `scripts/run-verified-tests.sh`, `scripts/fast-rows-pre-push.sh`, `scripts/check-test-shape.py`, `scripts/check-changed-markdown.py`; host tooling in `ops/systemd/` and `scripts/qa-rig/`.
- **GitHub health:** `cas-cli/src/github_repo.rs`, `scripts/github_queue_health.py`, `scripts/github-queue-health.graphql` - bounded queue health and canonical repository identity.
- `scripts/resolve-ci-base.sh`, `scripts/ci-queue-flakes.py` - exhaustive suite bases and individual script/web recovery.
- **Config:** `.mcp.json`, `.claude/settings.json`, `.codex/hooks.json`, `cas-cli/src/config/`, `hub-web/DESIGN.md`.

## Entrypoints

- **CLI/library:** `cas-cli/src/main.rs` (`cas`), `cas-cli/src/lib.rs`; MCP daemon `cas-cli/src/mcp/daemon.rs` (`cas serve`).
- **Factory:** `cas-cli/src/ui/factory/app/mod.rs`, `cas-cli/src/ui/factory/daemon/mod.rs`; Commander `hub-web/src/main.ts`.
- **In-place daemon updates:** `crates/cas-pty/src/adopt.rs`, `crates/cas-mux/src/pane/handoff.rs`, `crates/cas-mux/src/mux/handoff.rs`, `cas-cli/src/ui/factory/daemon/reexec.rs`, `cas-cli/src/cli/factory/reexec.rs`; real-hub journey HUB-J11.
