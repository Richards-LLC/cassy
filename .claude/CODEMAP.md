# Cassy — Codemap

> Regenerated from the merged workspace: eighteen Rust members, Commander and Slack TypeScript clients. Regenerate with `/codemap` when modules move.

## Top-level layout

- `cas-cli/`, `crates/` — composed application and shared Rust workspace packages.
- `hub-web/`, `slack-bridge/` — Commander browser client and Slack routing services.
- `contrib/`, `homebrew/` — shell integrations and package formulas.
- `docs/`, `site/` — architecture, design, QA, release documents and website.
- `fixtures/`, `migration/` — retrieval fixtures and historical Cloud migration scripts.
- `ops/`, `scripts/` — service units, host installation, proof runners and release tooling.
- `.cargo/`, `.config/` — Cargo configuration and nextest profiles.
- `.claude/`, `.codex/` — harness instructions, projections and navigation.
- `.githooks/`, `.github/` — Git policy hooks, workflows and issue templates.
- `Cargo.toml`, `package.json`, `rust-toolchain.toml` — workspace and toolchain roots.
- `README.md`, `AGENTS.md`, `CLAUDE.md`, `CONTRIBUTING.md` — onboarding and contributor policy.
- `DESIGN.md`, `CODING_STANDARDS.md`, `CAS-DEEP-DIVE.md` — design and system references.
- `changelog.d/`, `CHANGELOG.md` — task fragments and promoted release history.

## Workspace / packages

- `cas-cli/` — `cas` executable and application library.
- `crates/cas-types/` — task, agent, delivery, verification and wire types.
- `crates/cas-store/` — SQLite task, prompt, history, verification and knowledge stores.
- `crates/cas-search/` — Tantivy indexes, LMDB vectors, scoring and retrieval.
- `crates/cas-core/` — hooks, extraction, memory hygiene and temporal search.
- `crates/cas-code/` — tree-sitter parsing, symbols and code chunks.
- `crates/cas-mcp/` — action registry, protocol types and schemas.
- `crates/cas-mcp-proxy/` — upstream connections, caller policy and health.
- `crates/cas-factory/` — lifecycle, sessions, probes, routing and spec resolution.
- `crates/cas-factory-protocol/` — factory transport, framing and compression.
- `crates/cas-mux/` — panes, harness backends, input routing and rendering.
- `crates/cas-pty/` — PTYs, harness adapters, input queues and account isolation.
- `crates/cas-recording/`, `crates/cas-diffs/` — recording exports and diff parsing.
- `crates/cas-operator-crypto/` — HPKE/JWS envelopes shared with Cloud.
- `crates/cas-tui-test/` — PTY runners, screen assertions and artifacts.
- `crates/ghostty_vt/`, `crates/ghostty_vt_sys/` — terminal parser and FFI/build support.

## CLI, hooks and services

- `cas-cli/src/main.rs`, `cas-cli/src/lib.rs`, `cas-cli/src/cli/mod.rs` — binary, application modules and dispatch.
- `cas-cli/src/cli/init.rs`, `cas-cli/src/cli/setup.rs`, `cas-cli/src/cli/project_files/` — provisioning and managed-file reconciliation.
- `cas-cli/src/accounts.rs`, `cas-cli/src/account_registry/`, `cas-cli/src/account_selection.rs` — account inventory, selection and usage.
- `cas-cli/src/config/`, `cas-cli/src/cli/config/` — settings registry, policy and commands.
- `cas-cli/src/cli/doctor/`, `cas-cli/src/cli/update/`, `cas-cli/src/factory_preflight.rs` — health, installation and readiness.
- `cas-cli/src/hooks/handlers/`, `cas-cli/src/cli/hook/` — native lifecycle, tool and session policy.
- `cas-cli/src/hooks/handlers/handlers_events/message_display.rs` — shared key-shape redaction and message rendering.
- `cas-cli/src/hooks/handoff_updates.rs` — bounded handoff-channel reads through Violet.
- `cas-cli/src/cloud/`, `cas-cli/src/coordination/` — sync, presence, peer mail and local identity.
- `cas-cli/src/cloud/peer_mailbox/workers.rs` — same-epic worker authority, receive revalidation and supervisor copies.
- `cas-cli/src/cloud/peer_mailbox/outbox.rs`, `cas-cli/src/peer_ask_gate.rs` — explicit-ask merge holds and correlated-answer replay.
- `cas-cli/src/knowledge/`, `cas-cli/src/hybrid_search/`, `cas-cli/src/daemon/` — distillation, retrieval and background indexing.
- `cas-cli/src/extraction/`, `cas-cli/src/consolidation/`, `cas-cli/src/sync/` — extraction, memory decay and harness synchronization.
- `cas-cli/src/artifacts/`, `cas-cli/src/deliverable/` — durable files, branding, HTML/PDF and captures.
- `cas-cli/src/builtins/` — canonical skills, agents, harness catalogs and reference history.
- `cas-cli/src/rule_refs.rs`, `cas-cli/src/cli/rules.rs` — rule markers, stakeholder scopes and navigation.

## Factory runtime

- `cas-cli/src/ui/factory/app/` — TUI state, provisioning and task/worker actions.
- `cas-cli/src/ui/factory/daemon/runtime/` — lifecycle, events, probes, queued work and rendering.
- `cas-cli/src/ui/factory/daemon/runtime/lifecycle.rs` — recovery, merge-queue ejection, verifier deadlines and shutdown.
- `cas-cli/src/ui/factory/daemon/runtime/commander_mirror.rs` — transcript ownership, native replies and API-error mirrors.
- `cas-cli/src/ui/factory/daemon/runtime/delivery.rs`, `cas-cli/src/ui/factory/daemon/runtime/prompt_queue_jobs.rs` — clock-stamped wakes, input admission and recipient receipts.
- `cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs` — durable reminders and deadline events.
- `cas-cli/src/ui/factory/daemon/runtime/progress_watchdog.rs`, `cas-cli/src/ui/factory/daemon/runtime/loop_watchdog.rs` — progress escalation and loop exit.
- `cas-cli/src/ui/factory/daemon/runtime/quality_turns.rs` — incremental harness usage and authenticated turn provenance.
- `cas-cli/src/ui/factory/client.rs`, `cas-cli/src/ui/factory/session/` — attach guards and fingerprinted socket cleanup.
- `cas-cli/src/factory_worker_check.rs` — capped checks/tests, isolated build targets and completion receipts.
- `cas-cli/src/factory_build_routing.rs` — peer capacity routing, expiry and failed-send retirement.
- `cas-cli/src/mcp/tools/service/worker_liveness.rs`, `cas-cli/src/cli/factory/wedged.rs` — transcript/process liveness and background-job attribution.
- `cas-cli/src/train_guardrails.rs`, `cas-cli/src/train_guardrails_tests.rs` — train drop, readiness and age notices.
- `cas-cli/src/epic_design_review.rs` — stakeholder defect pauses and audited operator lift.
- `cas-cli/src/gh_budget.rs`, `cas-cli/src/gh_budget_tests.rs` — GitHub usage evidence and machine backoff.
- `cas-cli/src/worktree/git/`, `cas-cli/src/worktree/retained.rs` — Git mutation, rollback ownership, rescue refs and retained-checkout inventory.
- `cas-cli/src/cli/factory/hold.rs`, `crates/cas-store/src/project_hold.rs` — operator-only project holds.

## MCP, storage and delivery

- `cas-cli/src/mcp/daemon.rs`, `cas-cli/src/mcp/server/` — daemon and stable front/backend transport.
- `cas-cli/src/mcp/server/front/tests.rs` — framing, replacement and in-flight idle regressions.
- `cas-cli/src/mcp/tools/core/`, `cas-cli/src/mcp/tools/service/` — operations, authenticated dispatch and bounded futures.
- `cas-cli/src/mcp/tools/service/agent_search_system/message.rs` — host-local clock and message provenance.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs` — close gates, batch-squash precedence and delivery attribution.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/task_attribution.rs` — authenticated import grants and source ownership.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/verified_delivery.rs` — immutable candidate approval and landed-patch proof.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/merge_queue_proof.rs` — queue receipts and delivery containment.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/rule_references.rs` — stakeholder-rule coverage at the delivered tip.
- `cas-cli/src/mcp/tools/core/task/lifecycle/supervisor_push.rs`, `cas-cli/src/mcp/tools/core/task/block_hold.rs` — dependency wakes, verification expiry and close resumption.
- `cas-cli/src/mcp/tools/core/task/lifecycle/merge_close.rs`, `cas-cli/src/mcp/tools/core/workflow/worktree_ops.rs` — merge reconciliation and publication guards.
- `cas-cli/src/mcp/tools/core/workflow/train_merge.rs`, `cas-cli/src/mcp/tools/core/task/release_train.rs` — ordered train admission and default task targeting.
- `cas-cli/src/mcp/tools/core/workflow/premerge_verification.rs`, `cas-cli/src/mcp/tools/core/task/lifecycle/qa_dispatch.rs` — candidate review and shared-lane ownership.
- `cas-cli/src/mcp/tools/core/task/source_claim.rs`, `cas-cli/src/cloud/peer_claims.rs` — source-claim admission, renewal and release.
- `cas-cli/src/store/`, `cas-cli/src/migration/migrations/` — decorated stores and numbered schema migrations.
- `cas-cli/src/supervisor_continuity.rs`, `crates/cas-store/src/agent_store/continuity.rs` — succession handoffs and delivery recovery.
- `crates/cas-store/src/prompt_queue_store/`, `crates/cas-store/src/worker_task_queue_store.rs` — durable prompts and ordered worker backlogs.
- `crates/cas-core/src/hooks/`, `crates/cas-core/src/memory/`, `crates/cas-core/src/search/temporal/` — shared hooks and memory/search primitives.

## QA and Commander

- `cas-cli/src/qa_registry.rs`, `cas-cli/src/qa_impact.rs`, `docs/qa/journeys.yaml` — journey validation and diff selection.
- `cas-cli/src/qa_ledger.rs`, `cas-cli/src/qa_evidence.rs`, `cas-cli/src/cli/qa_ledger.rs` — versioned evidence grammar, task-tip scaffolds and observable outcomes.
- `cas-cli/src/builtins/skills/cas-qa-craft/scripts/walk-kit.mjs`, `scripts/test-walk-kit.mjs` — isolated surface walks, measured replies, captures and incremental ledgers.
- `cas-cli/src/qa_walk_progress.rs` — ledger timestamps and focused-epic QA split advice.
- `cas-cli/src/qa_pass/`, `cas-cli/src/qa_batch.rs`, `cas-cli/src/qa_post_release.rs` — independent QA, batch verdicts and once-per-task-set receipt offers.
- `cas-cli/src/fast_lane.rs`, `cas-cli/src/fast_lane_walk.rs` — hotfix admission, failed-walk choices and revert defaults.
- `cas-cli/src/quality_ledger.rs`, `crates/cas-store/src/quality_event_store.rs` — append-only turns, friction and delivery evidence.
- `cas-cli/src/hub/`, `cas-cli/src/bridge/server/` — pairing, operator inbox, account controls and HTTP/SSE routes.
- `hub-web/src/main.ts`, `hub-web/src/conversation-store.ts`, `hub-web/src/inbox/` — browser bootstrap, selection and reply grants.
- `hub-web/src/connection.ts`, `hub-web/src/connection-lifecycle.ts`, `hub-web/src/status-refresh.ts` — streams and ordered status publication.
- `hub-web/src/conversation-history.ts`, `hub-web/src/thread-window.ts`, `hub-web/src/early-thread.ts` — history paging and bounded windows.
- `hub-web/src/project-navigation.ts`, `hub-web/src/epic-tabs.ts`, `hub-web/src/team-view.ts` — project cards, task ledger and team attention.
- `hub-web/src/markdown-renderer.ts`, `hub-web/src/rich-attachment.ts`, `hub-web/src/rich-diagram.ts` — safe Markdown and lazy evidence rendering.
- `hub-web/src/composer-uploads.ts`, `hub-web/src/sent-attachments.ts`, `cas-cli/src/hub/artifact_proxy.rs` — uploads and authorized media.
- `hub-web/src/pairing-dialog.ts`, `hub-web/src/write-grant.ts`, `hub-web/src/accounts-panel.ts` — pairing and scoped controls.
- `hub-web/e2e/journeys/`, `hub-web/preview/` — catalog journeys, protocol doubles and real-hub fixtures.
- `slack-bridge/src/router-main.ts`, `slack-bridge/src/daemon-main.ts` — Slack router and per-user daemon.

## Cross-cutting and entrypoints

- **Tests:** inline Rust modules and ten explicit `cas-cli/tests/integration/` harnesses; isolation in `cas-cli/src/test_env_guard.rs`.
- **Browser checks:** Vitest in `hub-web/src/`, Playwright in `hub-web/e2e/`; affected selection in `scripts/journeys-for-diff.py`.
- **Docs:** `cas-cli/docs/ARCHITECTURE.md`, `cas-cli/docs/CONTRIBUTING.md`, `docs/design/`, `docs/qa/`.
- **Release:** `scripts/release.sh`, `scripts/release_changelog.py`, `scripts/fast-rows-pre-push.sh` and `.github/workflows/ci.yml`.
- **Generated output:** `scripts/regenerate-generated-artifacts.sh`, `scripts/gen-builtin-reference-history.sh`, `scripts/cas-merge-drivers.py`.
- **Test policy:** `scripts/check-test-shape.py`, `scripts/test-check-test-shape.py`, `scripts/ci-script-tests-for-diff.py`.
- **CLI:** `cas-cli/src/main.rs`; **library:** `cas-cli/src/lib.rs`; **service:** `cas serve`.
