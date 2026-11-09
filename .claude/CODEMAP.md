# Cassy - Codemap

> Regenerated structural map of the integrated release train (18 Rust workspace members plus TypeScript clients). Regenerate with `/codemap` when modules move.

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
- `changelog.d/` - per-delivery unreleased changelog fragments; `scripts/release_changelog.py promote` folds them into `CHANGELOG.md`; `verify-deliveries` checks explicit task claims against Git commits and patch equivalence once during release preparation.

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

- `cas-cli/src/model_eval.rs` - dataset-bound model evaluation targets, task-scoped proof validation and matching machine-readable run-log measurements; shares numeric bars and dependency holds with phase outcomes.
- `cas-cli/src/phase_outcome.rs` - measured Epic/Gate outcomes, inherited dependency holds and operator exceptions.
- `cas-cli/src/main.rs`, `cas-cli/src/lib.rs`, `cas-cli/src/cli/mod.rs` - executable, application modules and command dispatch.
- `cas-cli/src/cli/release_precheck.rs`, `scripts/release-precheck.py` - hosted exact-SHA prechecks, dispatch recovery, receipts and ref cleanup.
- `cas-cli/src/cli/init.rs`, `cas-cli/src/cli/setup.rs`, `cas-cli/src/cli/project_files.rs`, `cas-cli/src/cli/project_files/` - initialization, provisioning, content-based managed sync commits and merge-residue recovery.
- `cas-cli/src/accounts.rs`, `cas-cli/src/cli/accounts.rs`, `cas-cli/src/account_registry/` - account inventory, authentication-scope-safe link planning, identity discovery and native usage snapshots.
- `cas-cli/src/account_selection.rs`, `cas-cli/src/account_lifecycle.rs`, `cas-cli/src/account_links.rs` - recommendations, guarded mutations and shared-link repair.
- `cas-cli/src/cli/claude.rs`, `cas-cli/src/cli/codex.rs`, `cas-cli/src/cli/account_picker.rs`, `cas-cli/src/cli/limits.rs` - harness launch, login, account choice and capacity reports.
- `cas-cli/src/agent_model.rs` - role-scoped model/effort observations used by hooks, Commander and status.
- `cas-cli/src/cli/doctor/`, `cas-cli/src/cli/update/`, `cas-cli/src/factory_preflight.rs`, `cas-cli/src/gh_budget.rs`, `cas-cli/src/gh_budget_tests.rs` - GitHub request budgets and health (including retired MCP guidance), update receipts and account/root readiness.
- `cas-cli/src/cli/config/`, `cas-cli/src/config/` - command/config registry and operator policy.
- `cas-cli/src/cli/hook/`, `cas-cli/src/hooks/handlers/` - hook dispatch, native lifecycle handlers and policy, including pattern-signal refusal for every factory role and inherited native subagents.
- `cas-cli/src/hooks/handoff_updates.rs` - bounded Violet reads of handoff-named channels and threads; newer human posts precede the saved handoff.
- `cas-cli/src/hooks/handlers/handlers_events/docs_freshness.rs`, `cas-cli/src/cli/codemap_cmd.rs` - checkout-bound documentation freshness and structural-change reports.
- `cas-cli/src/cloud/`, `cas-cli/src/coordination/`, `cas-cli/src/cloud/peer_mailbox/workers.rs` - sync/presence, scoped peer artifact pointers, active same-epic worker authority and quiet supervisor copies.
- `cas-cli/src/cloud/peer_mailbox/transcript.rs` - own-account supervisor transcript requests and bounded, redacted daemon replies before prompt admission; `cas peers --transcript` and coordination share the same request path.
- `cas-cli/src/knowledge/`, `cas-cli/src/daemon/`, `cas-cli/src/hybrid_search/` - knowledge distillation, indexing and composed retrieval.
- `cas-cli/src/extraction/`, `cas-cli/src/consolidation/`, `cas-cli/src/sync/` - extraction, memory decay and harness synchronization.
- `cas-cli/src/artifacts/`, `cas-cli/src/deliverable/` - task artifacts, brand resolution, HTML/PDF finalization and immutable captures.
- `cas-cli/src/builtins/` - canonical skills, agents, job definitions, harness catalogs and reference-history manifest.
- `cas-cli/src/rule_refs.rs`, `cas-cli/src/cli/rules.rs` - tracked rule-marker navigation, shared scope parser and rule-change cleanup chores; `cas-cli/src/cli/rules.brief.md` describes terminal output.
- `fixtures/verification-plan-fit/` - verifier prompt replays for narrowed reader scope and missed plan targets.
- `cas-cli/src/builtins/skills/cas-test-audit/` - read-only test-value auditing, detector recipes and CI-timing evidence.

## Factory runtime and terminals

- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs` - fresh park binds a pushed source tip to the task's recorded start identity; parked deliveries retain their attribution/anchor fences.

- `cas-cli/src/cli/factory/queries.rs` - factory queries and explicit message destination/store validation; `cas-cli/tests/cli_test.rs` covers inherited-context routing and conflict refusal.
- `cas-cli/src/epic_design_review.rs` - per-epic stakeholder defect trigger, durable staffing pause, supervisor prompt and operator-only reasoned lift; wired through task mutations, starts, spawn admission and off-loop provisioning.
- `cas-cli/src/train_guardrails.rs`, `cas-cli/src/train_guardrails_tests.rs` - release-train guardrails: `cas factory train drop` (revert one child's merge, reopen it for its worker, re-enqueue the train; one repair cycle) and the daemon's once-per-state "train ready to cut" and max-age notices, which first offer each green tip to auto-cut.
- `docs/design/cli/design-review-lift.brief.md` - operator lift receipt and JSON output contract.
- `cas-cli/src/ui/factory/app/` - TUI state, provisioning, worker actions and delivery/assignment evidence.
- `cas-cli/src/ui/factory/app/render_and_ops/epic_workers.rs` - checkout-grouped cleanup intents, persisted readiness and immutable retirement snapshots.
- `cas-cli/src/ui/factory/daemon/runtime/gui_client.rs` - attach socket input drains before disconnect cleanup; PaneInfo snapshots include accepted/written PTY input byte counts from `crates/cas-pty/src/input_queue.rs` via `crates/cas-mux/src/pane/mod.rs`.
- `crates/cas-pty/src/pty.rs` - private startup-brief files, verified Claude launch paths, harness argv construction and owned PTY lifetimes.
- `crates/cas-pty/src/claude_binary.rs` - native-first Claude selection, typed missing/version/probe failures and bounded minimum-version checks shared by the hub and PTY.
- `cas-cli/src/ui/factory/client.rs`, `cas-cli/src/ui/factory/session/` - attach signal guards and fingerprinted socket cleanup.
- `cas-cli/src/ui/factory/daemon/runtime/` - startup, lifecycle, events, probes, queued work and render scheduling.
- `cas-cli/src/ui/factory/daemon/runtime/lifecycle.rs` - attention relays, merged-delivery approval recovery, merge-queue ejection return/page, verification expiry sweeps (with progress-based deadline extension up to an hour) and shutdown cleanup.
- `cas-cli/src/ui/factory/daemon/runtime/progress_watchdog.rs` - local no-progress watchdog: supervisor wake at 45 min, operator page at 90; excludes operator-owned blocked checks, contained parked deliveries and binding peer-owned work/target epics.
- `cas-cli/src/ui/factory/daemon/runtime/commander_mirror.rs` - transcript ownership, Claude/Codex/Grok answer and API-error mirrors, and bounded historical watches. Shared key-shape redaction lives in `cas-cli/src/hooks/handlers/handlers_events/message_display.rs`.
- `cas-cli/src/ui/factory/daemon/runtime/delivery.rs`, `cas-cli/src/ui/factory/daemon/runtime/prompt_queue_jobs.rs` - prompt readiness, clock-stamped pointer wakes, attributed director queue envelopes with immutable delivery-tip context, queued input and recipient receipts.
- `cas-cli/src/ui/factory/daemon/runtime/loop_watchdog.rs`, `cas-cli/src/ui/factory/daemon/runtime/client_input.rs` - wedged-loop exit and input ownership.
- `cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs`, `cas-cli/src/mcp/tools/service/factory_remind.rs` - durable reminder delivery and deadline/admission timer validation.
- `cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs`, `cas-cli/src/ui/factory/daemon/runtime/supervisor_single_turn_tests.rs` - supervisor PTY wake gates, paired operator input and native tool/dialog boundary regressions, including first-boot Claude native-turn confirmation and one owned-composer submit retry.
- `cas-cli/src/ui/factory/daemon/runtime/violet_intake.rs`, `cas-cli/src/ui/factory/daemon/runtime/violet_reader.rs`, `cas-cli/src/ui/factory/daemon/runtime/violet_consumer.rs` - Slack intake, ordered wakes routed to the reader's owner session and per-session receipts.
- `cas-cli/src/ui/factory/director/`, `cas-cli/src/ui/factory/server_registry.rs` - dispatch/readiness attribution and owned-server process checks.
- `cas-cli/src/ui/factory/supervisor_commands.rs`, `cas-cli/src/ui/factory/daemon/runtime/supervisor_commands.rs` - command allowlists, native mappings, caller-owned Finish queue and no-input dismissal cancellation, stale chooser recovery, native turn execution evidence and Claude Cassy reconnect receipts from live process versions.
- `cas-cli/src/task_assignment.rs`, `cas-cli/src/task_ownership.rs`, `cas-cli/src/worker_task_queue.rs` - task binding, ordered worker backlogs with pre-registration assignment admission, execution/held-task queue display and optional generation-fenced fresh-context continuation.
- `cas-cli/src/factory_auth_health.rs` - selected-account Codex catalogue evidence, shared MCP/CLI lane fallback admission, and terminal harness authentication/model failures; `cas-cli/src/ui/factory/app/assignment_evidence.rs` excludes those failures from idle staffing.
- `cas-cli/src/mcp/tools/service/factory_ops/spawn_names.rs` - off-loop reservation of isolated MCP worker names against the actual target repository's local/origin refs, registered identities and pending spawn names before queue/task binding.
- `cas-cli/src/factory_candidate_queue.rs`, `cas-cli/src/factory_candidate_queue_tests.rs`, `cas-cli/src/mcp/tools/service/candidate_ops.rs` - typed requests, runner ownership, bounded sandbox probes and daemon-managed candidate builds.
- `cas-cli/src/factory_ci_retry.rs` - exact-SHA merge retries, terminal CI notices, expiry and corrupt-intent quarantine.
- `cas-cli/src/mcp/tools/core/workflow/release_freeze.rs`, `cas-cli/src/mcp/tools/core/workflow/release_auto_cut.rs` - release freeze state, merge/staffing refusals and measured ETA; auto-cut of a green train with fragments under cadence guards, the Commander pause and the supervisor's notes draft.
- `cas-cli/src/mcp/tools/core/workflow/train_merge.rs` - durable ordered train merges, target reservation, exact parked anchors and daemon continuation through the existing merge path.
- `cas-cli/src/train_ci.rs`, `scripts/train-ci.py` - existing CI suite receipts, complete logs, red-tip supervisor wakes, queue holds and starting-tip release admission.
- `cas-cli/src/factory_worker_check.rs`, `cas-cli/src/factory_build_guard.rs`, `cas-cli/src/factory_capacity_wait.rs`, `cas-cli/src/factory_scratch.rs` - capped proof admission, builder guards, short owned temporary roots, durable capacity waits and private resolution of Cargo-home Rust tools.
- `cas-cli/src/factory_build_routing.rs` - passive capacity presence and spawn overflow routing to same-account peers, with expiring task-stable asks and peer cooldowns in the existing outbox.
- `cas-cli/src/factory_worker_runs.rs` - worker-bound detached command records, completion wakes, PID-fenced liveness and retry recovery.
- `cas-cli/src/cli/factory/hold.rs`, `crates/cas-store/src/project_hold.rs` - operator-only project hold commands, audited state, task presentation and action-relay admission; internal parked-task lists stay complete.
- `cas-cli/src/supervisor_message_wakes.rs` - typed wake limits and ask/answer/blocker exemptions; runtime queue selection rechecks declined wakes.
- `cas-cli/src/peer_ask_gate.rs`, `cas-cli/src/cloud/peer_mailbox/outbox.rs` - explicit-ask merge holds, linked/unambiguous typed-answer replay, ambiguous-answer evidence and explicit overrides; untyped mail never creates a hold.
- `cas-cli/src/worktree/git/rescue.rs` - rescue refs for unsaved work before shutdown and worktree removal.
- `cas-cli/src/worktree/retained.rs` - bounded read-only Git worktree inventory shared by supervisor startup, GC reports and epic-status recovery warnings; includes retired workers outside the active worktree store.

## GitHub health

- `cas-cli/src/github_repo.rs` - explicit origin binding and bounded, report-only default-branch queue health shared by setup/doctor; one query reads canonical identity and applicable active rules.
- `scripts/github-queue-health.graphql`, `scripts/github_queue_health.py` - shared embedded query and advisory release preflight; warns on drift/unavailable access without writing settings or failing release.

## MCP, storage and delivery gates

- `cas-cli/src/mcp/daemon.rs`, `cas-cli/src/mcp/server/`, `cas-cli/src/mcp/server/front.rs`, `cas-cli/src/mcp/server/backend.rs`, `cas-cli/src/mcp/server/front/tests.rs` - stable stdio framing, eager initial startup, in-flight tool accounting, standby activation and compatible backend replacement.
- `cas-cli/src/mcp/tools/core/`, `cas-cli/src/mcp/tools/service/` - core operations, service dispatch and bounded tool futures.
- `cas-cli/src/mcp/tools/service/factory_ops.rs`, `cas-cli/src/mcp/tools/service/server_handler.rs` - harness token-accounting readers, measured worker context notices and authenticated response context.
- `cas-cli/src/mcp/tools/core/task/lifecycle/`, `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs`, `cas-cli/src/mcp/tools/core/task/lifecycle/qa_dispatch.rs` - task closure, shared-lane ownership, batch-squash precedence and target-sync/generated-continuation attribution.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/task_attribution.rs` - task-scoped import exclusions from immutable supervisor grants. Only the authenticated `close_ops.rs` change-request handler parses import markers into explicit store grants; shared rejection storage never parses review prose.
- `cas-cli/src/mcp/tools/core/task/lifecycle/supervisor_push.rs`, `cas-cli/src/mcp/tools/core/task/block_hold.rs` - lifecycle outbox, recorded dependency completion wakes, dispatch expiry ownership and exact approved no-delivery close resumption and supervisor-owned remaining close recovery.
- `cas-cli/src/mcp/tools/service/worker_liveness.rs`, `cas-cli/src/cli/factory/wedged.rs` - bounded transcript/process evidence, background-job completion and CPU attribution across user-work descendants.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/` - delivery attribution, proof/upkeep gates and reviewed corrections; `gate_preflight.rs` snapshots park requirements. `verified_delivery.rs` binds synthetic premerge approvals to the task's landed source and accepts its exact reviewed merge tree or complete patch at the first authoritative landing, allowing later ordinary edits. A covering batch receipt preserves a landed committed target proof without waiving its task review. `merge_queue_proof.rs` discovers queue receipts from recorded default-branch landings and verifies workflow, tree and delivery containment for scoped-proof reuse.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/rule_references.rs` - immutable-tip rule coverage, declared causal anchors and stakeholder-scope checks; contract in `docs/RULE_REFERENCES.md`.
- `cas-cli/src/mcp/tools/core/task/lifecycle/merge_close.rs`, `cas-cli/src/mcp/tools/core/workflow/worktree_ops.rs` - batch QA and release receipts retry ordinary close for every tier without replacing dedicated review; merge/close reconciliation, dispatch-bound no-delivery resumption, audited target-only parked-anchor advances and guarded Git delivery operations; open release trains refuse direct main/trunk merges for stored and inferred worktrees, including `allow_trunk`.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/report_quality.rs`, `cas-cli/src/deliverable/report_quality.rs` - G7 report quality: a changed `docs/` report (`.html` beside its `.md`) parks only with its brief, critique floor, DESIGN.md tokens, no network, a `data-verdict` first, its height budget and, for business readers, no task ids or jargon.
- `cas-cli/src/mcp/tools/core/workflow/premerge_verification.rs` - immutable merge candidates, plan/evidence binding, managed review scratch, pre-publication dispatch, unchanged-patch-id verdict rebinding after target moves and candidate-to-landed promotion. Reviewed and no-dispatch refusals share guarded unpublished-merge restoration. Late bound-child verdicts revalidate the exact current source/tree and consume their original one-time authority atomically.
- `crates/cas-types/src/verification.rs`, `crates/cas-store/src/verification_store.rs` - portable proof boundaries and durable authenticated peer approvals.
- `cas-cli/src/mcp/tools/core/workflow/premerge_verification/automatic.rs` - bounded daemon-owned verifier transport, account-aware launch resolution, sealed child registration, exact-dispatch verdict submission, quiet approval continuation and bounded stderr failure notices.
- `cas-cli/src/mcp/tools/core/workflow/premerge_verification/peer.rs` - confirmed-publication receipts over authenticated peer mail, durable import into existing dispatch/verdict records, local Git rebinding and exact close validation; queue retention carries no proof authority.
- `cas-cli/src/mcp/tools/core/workflow/protected_pr.rs` - exact-source PR creation/reuse and expected-head auto-merge for confirmed default-branch queues, before local merge mutation.
- `cas-cli/src/mcp/tools/core/task/rebind.rs` - audited supervisor-approved branch replacement.
- `cas-cli/src/mcp/tools/core/task/recover_delivery.rs` - audited supervisor-only integrated identity recovery before independent batch attestation; retains historical anchors, content gates and review debt.
- `cas-cli/src/finding_triage.rs` - severity × reach on task labels and declared main journeys in `docs/qa/journeys.yaml` (Commander, factory and CLI); `mcp/tools/core/task/update.rs` owns label corrections. Shared axes live in `crates/cas-types/src/verification.rs`, persisted by `verification_store.rs` with additive migration m296.
- `cas-cli/src/mcp/tools/core/task/release_train.rs` - release train as the default work target: a task created with no epic or target joins the open `release-train` epic cut from the integration branch; a landed train closes and the next opens on the next create.
- `cas-cli/src/mcp/tools/core/task/source_claim.rs` - stakeholder-source preflight for creation, assignment, start/claim/transfer, ordered queues and spawning; existing Slack receipts and normalized GitHub claims, with recorded supervisor exceptions. `cas-cli/src/cloud/peer_claims.rs` renews sources for local nonterminal tasks and uses existing fenced release retries at close/cancel.
- `cas-cli/src/store/`, `cas-cli/src/migration/migrations/`, `cas-cli/src/worktree/` - store decorators (including durable rule-projection deletion reconciliation in `cas-cli/src/store/syncing.rs` and the active-rule cut in `cas-cli/src/store/detect.rs`), numbered migrations and Git/worktree lifecycle.
- `crates/cas-store/src/agent_store/continuity.rs`, `crates/cas-store/src/worker_task_queue_store.rs` - agent retirement snapshots and persistent worker queues.
- `cas-cli/src/supervisor_auto_clear.rs`, `cas-cli/src/ui/factory/daemon/runtime/supervisor_auto_clear_tests.rs` - supervisor latest-call context pressure (80% of reported window, configured cap fallback): idle-boundary check, bounded live-work handoff with pending actor/kind/task/age summaries, one judgement turn, then context-reset control and resume handoff.
- `cas-cli/src/supervisor_continuity.rs` - durable succession handoffs, unread Commander/Violet mail recovery and project-scoped integrated-delivery recovery actions at startup.
- `crates/cas-store/src/history_store.rs` - batched history provenance repair and indexed worker commit-prefix lookup.
- `crates/cas-store/src/prompt_queue_store.rs`, `crates/cas-store/src/qa_pass_store.rs` - durable prompts, guarded unread-mail succession, recipient surfacing and QA maintenance.
- `crates/cas-core/src/hooks/`, `crates/cas-core/src/memory/`, `crates/cas-core/src/search/temporal/` - hook schemas and memory/search primitives.
- `cas-cli/src/mcp/tools/service/core.rs`, `cas-cli/src/mcp/tools/core/memory.rs`, `cas-cli/src/mcp/tools/service/memory_lifecycle_tests.rs` - public memory dispatch, partial title/content updates, structured changed-field receipts and real-store lifecycle regressions.
- `cas-cli/src/mcp/tools/core/rule_drift.rs`, `cas-cli/src/mcp/tools/core/rules.rs` - rule comparison, stakeholder attribution, rule source/hold listing and encode chores. `cas-cli/src/mcp/tools/service/core.rs` routes authenticated personal-rule creation through `cas-cli/src/mcp/tools/service/pattern_ops.rs`. Candidate writes require two independent completed task/verdict citations before atomically recording a bundle.
- `crates/cas-types/src/entry/behavior.rs`, `cas-cli/src/migration/migrations/m294_rules_stakeholder.rs` - temporal validity and stakeholder provenance migration.

## QA and Commander

- `cas-cli/src/qa_registry.rs`, `cas-cli/src/qa_impact.rs`, `cas-cli/src/qa_impact_io.rs` - registry validation and diff-scoped journey selection.
- `cas-cli/src/builtins/skills/cas-qa-craft/scripts/model-boundary.py`, `scripts/test-model-boundary.py` - approved model-owned plan metadata and added-site signals for independent verifier judgment.
- `cas-cli/src/qa_ledger.rs`, `cas-cli/src/cli/qa_ledger.rs`, `cas-cli/src/qa_evidence.rs`, `cas-cli/src/qa_deployed_origins.rs` - versioned evidence grammar, cited visible/terminal/API outcomes, task-source-tip scaffolds and bundles; deployed-origin patterns.
- `cas-cli/src/builtins/skills/cas-qa-craft/scripts/walk-kit.mjs`, `scripts/test-walk-kit.mjs` - isolated parallel surface walks, measured replies, captures and incremental defect ledgers.
- `cas-cli/src/qa_walk_progress.rs` - bounded ledger-result timestamps and focused-epic split advice, independent of native liveness.
- `cas-cli/src/fast_lane.rs`, `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/fast_lane_gate.rs`, `docs/qa/fast-lane.md` - hotfix fast lane: eligibility, merge-check admission, post-merge receipt and revert PR.
- `cas-cli/src/fast_lane_walk.rs` - durable failed-walk Commander choice, authenticated reply binding, fifteen-minute revert default and project runner.
- `cas-cli/src/qa_pass/`, `cas-cli/src/qa_batch.rs`, `cas-cli/src/qa_author_override.rs`, `cas-cli/src/mcp/tools/service/qa_pass_ops.rs` - independent QA, release verdicts and audited author overrides. `qa_batch::release_author_waiver` permits only an exact-head supervisor waiver on a measured QA-record-only diff for release closeout; ordinary QA receipts and product implementation retain independent authors. Batch persistence marks its MCP response committed before continued delivery closes; `server_handler.rs` and `cas-cli/src/mcp/tools/core/task/lifecycle/merge_close.rs` retain bounded outcome summaries on each task, with complete long remedies in immutable receipt details. `qa_evidence.rs` excludes batch, post-commit and late-close system outcomes from producer citations, and formats restored/synced citations as separate timestamped notes. `cas-cli/src/qa_post_release.rs`'s runner also offers the supervisor, once per target branch and live parked task set, the exact `batch_receipt` call for parked batch-deferred deliveries already on origin/<target>, skipping valid QA receipts at their anchors (it never records one); `qa_batch::shipped_anchor` lets close ignore a parked blocker whose active delivery anchor is already on target.
- `cas-cli/src/ui/factory/daemon/runtime/quality_turns.rs` - incremental Claude/Codex/Grok turn accounting before optional Commander mirroring; typed actions and authenticated wake provenance; feeds newly recorded supervisor turns to the automatic clear.
- `cas-cli/src/migration/migrations/m292_quality_turns.rs`, `cas-cli/src/migration/migrations/m295_quality_friction.rs`, `cas-cli/src/migration/migrations/m297_quality_estimates.rs` - append-only quality ledger CHECK upgrades (Turn, Friction, estimates/outcomes), shared with store-open repair.
- `cas-cli/src/quality_ledger.rs`, `cas-cli/src/quality_ledger/estimates.rs`, `crates/cas-store/src/quality_event_store.rs`, `cas-cli/src/cli/ledger.rs`, `docs/ESTIMATES.md` - append-only quality events, measured reference-class p50/p90 estimates, linked outcomes, live/history queue identities and friction-ledger commands.
- `cas-cli/src/cli/hub.rs`, `cas-cli/src/cli/hub_service.rs`, `cas-cli/src/hub/tailscale.rs` - hub service lifecycle and Serve target ownership.
- `cas-cli/src/cli/hub_preview.rs` - `cas hub preview`: serves a candidate Commander dist on a read-only, time-boxed preview origin (`AuthStore::mint_preview_pairing`, `live_previews`).
- `cas-cli/src/bridge/server/`, `crates/cas-mcp/src/types/` - HTTP/SSE routes and MCP action schemas.
- `cas-cli/src/hub/auth/`, `cas-cli/src/hub/operator_inbox/`, `cas-cli/src/hub/accounts.rs` - pairing, durable operator input and account projections.
- `cas-cli/src/hub/dismissed_sessions.rs` - authenticated legacy session tombstones.
- `hub-web/src/machine-version.ts`, `hub-web/src/paired-machines.ts` - running hub, installed binary and available release labels.
- `hub-web/src/project-navigation.ts` - compact project cards (machine dots, train bar, status line), pinned Needs you questions, one-line sessions, quick filters and device-local collapse.
- `hub-web/src/main.ts`, `hub-web/src/conversation-store.ts`, `hub-web/src/inbox/` - browser bootstrap, selected conversation and reply grants.
- `hub-web/src/connection.ts`, `hub-web/src/connection-lifecycle.ts`, `hub-web/src/status-refresh.ts` - authenticated streams, subscriptions and ordered status publication.
- `hub-web/src/conversation-history.ts`, `hub-web/src/early-thread.ts`, `hub-web/src/thread-window.ts` - history paging and bounded transcript windows.
- `hub-web/src/epic-progress.ts` - Live epics in Tasks & progress: the selected project's epics across machines, open / in progress / merged split bars and in-place task disclosure.
- `hub-web/src/provider-mark.ts`, `hub-web/src/live-phase.ts`, `hub-web/src/team-view.ts`, `hub-web/src/progress-model.ts` - harness marks, live phase, attention-priority Team state and release/task projections.
- `hub-web/src/work-events.ts`, `hub-web/src/work-picture.ts`, `hub-web/src/work-rows-view.ts`, `hub-web/src/worker-timeline.ts` - operation disclosures and worker activity views.
- `hub-web/src/markdown-renderer.ts`, `hub-web/src/reply-images.ts`, `hub-web/src/code-frame.ts`, `hub-web/src/rich-diagram.ts`, `hub-web/src/rich-attachment.ts` - safe Markdown, inline reply images, source controls and lazy evidence renderers.
- `hub-web/src/composer-uploads.ts`, `hub-web/src/sent-attachments.ts`, `cas-cli/src/hub/composer_upload.rs` - upload identity, progress and stored attachment routes.
- `cas-cli/src/hub/artifact_proxy.rs`, `cas-cli/src/hub/local_image.rs`, `cas-cli/src/hub/chunk_assets.rs`, `cas-cli/src/hub/visual_pages.rs` - authorized media (including artifacts-directory reply images), content chunks and passive page previews.
- `hub-web/src/pairing-dialog.ts`, `hub-web/src/write-grant.ts`, `hub-web/src/accounts-panel.ts` - pairing, scoped input grants and account controls.
- `hub-web/src/account-usage-check.ts`, `cas-cli/src/hub/account_routes.rs` - per-account native usage rechecks shared by Accounts and New session, with distinct check/observation times.
- `hub-web/e2e/journeys/`, `hub-web/preview/` - catalog journeys, protocol doubles and real-hub fixtures.
- `slack-bridge/src/router-main.ts`, `slack-bridge/src/daemon-main.ts` - Slack router and per-user daemon entrypoints.

## Cross-cutting

- **Tests:** inline Rust modules, `crates/cas-store/tests/`, explicit `cas-cli/tests/` harnesses and `cas-cli/tests/integration/`; process environment isolation in `cas-cli/src/test_env_guard.rs`.
- **Browser proof:** Vitest in `hub-web/src/`, Playwright in `hub-web/e2e/`, affected selection via `scripts/journey-eval.sh` and `scripts/journeys-for-diff.py`.
- **Docs:** `cas-cli/docs/ARCHITECTURE.md`, `cas-cli/docs/CONTRIBUTING.md`, `docs/design/`, `docs/qa/`, `docs/analysis/` and `docs/qa/bug-families/`.
- `docs/status-words.md` - tool/CLI status definitions, observation boundaries, producer checks and named regressions.
- **Release:** `cas-cli/src/cli/release_automation.rs` (canonical train context, durable SHA/tag reminders, installed-release notice), `scripts/release.sh`, `scripts/release-gate.sh`, `scripts/release-integration-gates.py`, `scripts/deploy-hub-release.py`, `docs/release-notes/`, `docs/release-reports/`.
- **CI:** `.github/workflows/full-validation.yml` calls the shared `.github/workflows/ci.yml` graph; `.github/actions/full-gate-job/` and `scripts/train-ci.py` restore successful jobs from identical trees, bind full-set tree/main receipts, release admission and plumbing reuse; `cas-cli/src/train_ci.rs` records train/queue timings. `.github/actions/run-script-suite-tests/`, `scripts/resolve-ci-base.sh`, `scripts/ci-queue-flakes.py` own exhaustive suites, bases and shared recovery, including individual script and web test steps.
- **Release reuse:** `scripts/release-cache-key.py`, `scripts/find-release-prebuild.sh`, `.github/workflows/release-prebuild.yml`, `.github/workflows/release.yml` - dependency keys, queue archives, advisory main-cache priming and publication.
- **Host tooling:** `scripts/install-on-publish.sh`, `scripts/qa-rig/commander-two-machine/`, `ops/systemd/` and `docs/ci/`.
- `scripts/check-test-shape.py`, `scripts/test-check-test-shape.py` - structural and human-message assertion checks; `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/receipt_scope.rs` runs the message check at park.
- **Proof tooling:** `scripts/qa-candidate-build.py`, `scripts/run-verified-tests.sh`, `scripts/fast-rows-pre-push.sh`, `scripts/check-changed-markdown.py`.
- `scripts/check-scoped-test-surface.sh` - committed-path proof discovery with bounded Git pattern batches.
- **Config:** `.mcp.json`, `.claude/settings.json`, `.codex/hooks.json`, `cas-cli/src/config/` and `hub-web/DESIGN.md`.

## Entrypoints

- CLI: `cas-cli/src/main.rs` (`cas`); library composition: `cas-cli/src/lib.rs`.
- Factory TUI/daemon: `cas-cli/src/ui/factory/app/mod.rs`, `cas-cli/src/ui/factory/daemon/mod.rs`.
- MCP: `cas-cli/src/mcp/daemon.rs` (`cas serve`); bridge: `cas-cli/src/bridge/server/` (`cas bridge serve`).
- Commander: `hub-web/src/main.ts`; Slack services: `slack-bridge/src/router-main.ts`, `slack-bridge/src/daemon-main.ts`.
- Tests: capped named nextest filters for workers; Vitest/typecheck and affected journeys for web changes; merge queue owns the full Rust suite.

## In-place factory daemon updates

- `crates/cas-pty/src/adopt.rs` - owned inherited Unix master descriptors and direct-child waitpid handles.
- `crates/cas-mux/src/pane/handoff.rs`, `crates/cas-mux/src/mux/handoff.rs` - quiesced pane snapshots, drafts and launch policy restored around inherited PTYs.
- `cas-cli/src/ui/factory/app/handoff.rs`, `cas-cli/src/ui/factory/daemon/reexec.rs` - daemon schema preflight, sealed inherited manifest, same-PID exec and one-shot rollback.
- `cas-cli/src/cli/factory/reexec.rs` - local socket control and per-daemon post-update outcomes.
- `hub-web/e2e/journeys/real-hub.journey.ts` - HUB-J11 includes in-place daemon re-exec and same-PID attachment recovery.
