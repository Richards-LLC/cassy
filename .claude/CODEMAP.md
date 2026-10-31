# Cassy - Codemap

> Structural map of Cassy's Rust workspace and TypeScript clients. Regenerate with `/codemap` when modules move.

## Top-level layout

- `cas-cli/`, `crates/` - executable composition and shared Rust packages.
- `hub-web/`, `slack-bridge/` - Commander browser client and Slack routing daemon.
- `contrib/`, `homebrew/` - installable shell helpers, tests and package formulas.
- `docs/`, `site/` - design, QA, operations and release records; static website and system PDF.
- `fixtures/`, `migration/` - retrieval parity baselines and historical Cloud-move scripts.
- `ops/`, `scripts/` - service units, launch wrappers, installation, proof and release tooling.
- `.cargo/`, `.config/` - Cargo settings and nextest profiles.
- `.claude/`, `.codex/` - instructions, harness projections and working context.
- `.githooks/`, `.github/` - Git policy hooks, CI workflows and issue templates.
- `.github/actions/run-script-suite-tests/` - path-scoped script and visual QA dependencies/execution shared by PR, slim-lane and assembly jobs.
- `Cargo.toml`, `package.json`, `rust-toolchain.toml` - workspace, tooling and toolchain roots.
- `README.md`, `AGENTS.md`, `CLAUDE.md`, `CONTRIBUTING.md` - onboarding and contributor policy.
- `DESIGN.md`, `CODING_STANDARDS.md`, `CHANGELOG.md`, `CAS-DEEP-DIVE.md` - design, standards and release/system references.

## Workspace / packages

- `cas-cli/` - Rust binary `cas` and composed application library.
- `crates/cas-types/` - task, agent, delivery, provenance, event and wire types.
- `crates/cas-store/` - SQLite stores, task/QA queues, verification, knowledge and vectors.
- `crates/cas-store/src/hash_id.rs` - shared counter-mixed ID generation and format policies for task, skill, spec and code stores.
- `crates/cas-search/` - Tantivy indexes, LMDB vectors, scoring and retrieval primitives.
- `crates/cas-core/` - hooks, extraction, memory hygiene, temporal search and sync.
- `crates/cas-code/` - tree-sitter parsing, symbol extraction and code chunks.
- `crates/cas-mcp/` - action registry, protocol types and daemon support.
- `crates/cas-mcp-proxy/` - upstream MCP connections, caller policy and health.
- `crates/cas-factory/` - lifecycle, sessions, probes, routing and spec resolution.
- `crates/cas-factory-protocol/` - factory transport, framing, compression and messages.
- `crates/cas-mux/` - panes, harness backends, input routing and terminal rendering.
- `crates/cas-pty/` - PTY setup, harness adapters, input queues and account isolation.
- `crates/cas-recording/` - recording readers, writers and exports.
- `crates/cas-diffs/` - diff parsing, highlighting and terminal widgets.
- `crates/cas-operator-crypto/` - HPKE/JWS envelopes shared with Cloud.
- `crates/cas-tui-test/` - PTY runners, screen assertions and test artifacts.
- `crates/ghostty_vt/`, `crates/ghostty_vt_sys/` - terminal parser and Ghostty FFI/build support.

## CLI, hooks and application services

- `cas-cli/src/agent_model.rs` - role-scoped pane model/effort observations shared by native hooks, Commander model selection, catalog and status.

- `cas-cli/src/main.rs`, `cas-cli/src/lib.rs`, `cas-cli/src/cli/mod.rs` - executable, module composition and command dispatch.
- `cas-cli/src/cli/init.rs`, `cas-cli/src/cli/setup.rs` - initialization, recovery and provisioning.
- `cas-cli/src/account_selection.rs`, `cas-cli/src/config/accounts.rs` - recommendation policy, exclusions and thresholds.
- `cas-cli/src/account_registry.rs`, `cas-cli/src/account_registry/resolve.rs` - non-secret identity/alias discovery and native usage snapshots.
- `cas-cli/src/account_lifecycle.rs`, `cas-cli/src/account_links.rs` - guarded profile mutations and shared-link repair.
- `cas-cli/src/accounts.rs`, `cas-cli/src/cli/accounts.rs` - subscription inventory, account configuration and drift operations.
- `cas-cli/src/cli/limits.rs` - observation/reset dates, native Claude/Codex capacity and bounded reports.
- `cas-cli/src/cli/claude.rs`, `cas-cli/src/cli/codex.rs`, `cas-cli/src/cli/account_picker.rs` - provider launch, login and account choice.
- `cas-cli/src/cli/doctor/`, `cas-cli/src/cli/update/` - health producers/rendering and update phases/receipts.
- `cas-cli/src/cli/project_files/` - managed-file reconciliation and locking.
- `cas-cli/src/cli/config/`, `cas-cli/src/config/` - command/config registry and operator policy, including QA/capture/upkeep settings.
- `cas-cli/src/factory_preflight.rs`, `cas-cli/src/capability.rs` - account/root validation, cached probes and readiness.
- `cas-cli/src/claude_credentials.rs` - shared Keychain service identity; native capacity does not read its secrets.
- `cas-cli/src/cli/hook.rs`, `cas-cli/src/cli/hook/`, `cas-cli/src/hooks/handlers/` - hooks, native dispatch, generation and policy.
- `cas-cli/src/hooks/handlers/handlers_events/docs_freshness.rs`, `cas-cli/src/hooks/handlers/handlers_events/codemap.rs`, `cas-cli/src/cli/codemap_cmd.rs` - checkout-bound documentation freshness and structural-change reports.
- `cas-cli/src/cloud/`, `cas-cli/src/coordination/` - sync/presence, binding intake and ownership.
- `cas-cli/src/worktree/`, `cas-cli/src/store/`, `cas-cli/src/migration/migrations/` - Git lifecycle, store decorators and numbered migrations.
- `cas-cli/src/artifacts/`, `cas-cli/src/artifacts/qa.rs` - portable proofs and task-bound QA file export/recovery.
- `cas-cli/src/knowledge/`, `cas-cli/src/daemon/`, `cas-cli/src/hybrid_search/` - distillation, background indexing and composed retrieval.
- `cas-cli/src/extraction/`, `cas-cli/src/consolidation/`, `cas-cli/src/sync/` - extraction, memory decay and harness sync.
- `cas-cli/src/deliverable/`, `cas-cli/src/deliverable/assets/` - brand resolution, HTML/PDF finalization and immutable captures/pairs.
- `cas-cli/src/builtins/` - canonical skills/agents/jobs, mirrors and reference-history manifest.

## Factory runtime and terminals

- `cas-cli/src/ui/factory/app/` - TUI state, provisioning, rendering and worker/worktree actions.
- `cas-cli/src/ui/factory/client.rs`, `cas-cli/src/ui/factory/session/` - attach signal guards and fingerprinted socket cleanup.
- `cas-cli/src/ui/factory/daemon/runtime/` - lifecycle, startup, events, probes, relays and rendering schedule; `fixtures/claude-2.1.312-usage.txt` records the native Claude Usage screen.
- `cas-cli/src/ui/factory/daemon/runtime/loop_watchdog.rs`, `cas-cli/src/ui/factory/daemon/runtime/client_input.rs` - wedged-loop exit and input ownership.
- `cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs`, `cas-cli/src/mcp/tools/service/factory_remind.rs` - durable reminders and task-wait defaults.
- `cas-cli/src/ui/factory/daemon/runtime/violet_intake.rs`, `cas-cli/src/ui/factory/daemon/runtime/violet_reader.rs` - Slack intake and ordered wake recovery.
- `cas-cli/src/ui/factory/daemon/runtime/violet_consumer.rs` - per-session consumer receipts and communication health.
- `cas-cli/src/ui/factory/director/`, `cas-cli/src/ui/factory/server_registry.rs` - dispatch/readiness attribution and owned-server process checks.
- `cas-cli/src/ui/factory/supervisor_commands.rs`, `cas-cli/src/ui/factory/daemon/runtime/supervisor_commands.rs` - semantic allowlist, native mappings and outcome evidence.
- `cas-cli/src/ui/factory/daemon/runtime/work_events.rs` - paged work evidence, transcript indexing and scoped native phase lifecycle.
- `cas-cli/src/task_assignment.rs`, `cas-cli/src/task_ownership.rs` - supervisor/task ownership and delivery binding.
- `cas-cli/src/worker_task_queue.rs` - ordered worker backlog policy, start authorization, delivery handoff and durable daemon reconciliation.

## MCP and delivery gates

- `cas-cli/src/mcp/daemon.rs`, `cas-cli/src/mcp/server/` - MCP transport and server composition.
- `cas-cli/src/mcp/tools/core/` - task, memory, knowledge, artifact, search, rule and workflow handlers.
- `cas-cli/src/mcp/tools/service/`, `cas-cli/src/mcp/tools/types/` - service dispatch, bounded futures and request schemas.
- `cas-cli/src/mcp/tools/core/task/lifecycle/` - start/close, evidence gates and merged-delivery closure; `recall_terms.rs` filters content terms for task-create context and epic scope comparison.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs` - delivery anchors, close audit tracking and guarded note-preserving park.
- `cas-cli/src/mcp/tools/core/task/lifecycle/repository_proof.rs` - worktree-keyed receipts and introducing-merge boundaries.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/` - attribution, evidence-only delivery, queue proof, reviewed corrections and upkeep modules.
- `cas-cli/src/mcp/tools/core/task/lifecycle/merge_close.rs`, `cas-cli/src/mcp/tools/core/workflow/verification_tools.rs` - verdict-driven close/reopen.
- `cas-cli/src/mcp/tools/core/task/rebind.rs` - supervisor-approved replacement branch takeover, checked against the prior start and retained in the audit trail.
- `cas-cli/src/bridge/server/`, `crates/cas-mcp/src/types/` - HTTP/SSE handlers and protocol definitions.

## QA, storage and Commander

- `cas-cli/src/qa_registry.rs`, `cas-cli/src/qa_impact.rs`, `cas-cli/src/qa_impact_io.rs` - registry/sidecar validation and diff-scoped journey selection.
- `cas-cli/src/qa_ledger.rs`, `cas-cli/src/cli/qa_ledger.rs` - shared verifier grammar, scaffold and ledger checks.
- `crates/cas-store/src/quality_event_store.rs`, `cas-cli/src/quality_ledger.rs`, `cas-cli/src/cli/ledger.rs`, `cas-cli/src/cloud/syncer/quality_events.rs` - friction ledger: append-only events and scope-A rules, emitters, `cas ledger`, batched team sync.
- `cas-cli/src/qa_pass/`, `cas-cli/src/qa_pass/sync.rs`, `cas-cli/src/qa_batch.rs`, `cas-cli/src/qa_author_override.rs` - independent reviews, cross-host sync, batch receipts and the operator-only QA author override.
- `cas-cli/src/qa_evidence.rs`, `cas-cli/src/qa_post_release.rs` - bundle proof and post-release observations.
- `cas-cli/src/rapid_response.rs`, `cas-cli/src/cli/qa_rapid_response.rs` - operator-only, time-boxed per-epic rapid-response windows, scaled-down gates and the back-fill ledger.
- `cas-cli/src/qa_traffic.rs`, `cas-cli/src/hub/usage.rs` - local action counts and per-journey traffic shares.
- `crates/cas-store/src/agent_store/continuity.rs` - retirement snapshots, process evidence and successor claims.
- `crates/cas-store/src/worker_task_queue_store.rs` - persistent per-session worker queues and spawn payload binding.
- `crates/cas-store/src/prompt_queue_store/`, `crates/cas-store/src/qa_pass_store.rs` - durable prompts, recipient surfacing and indexed QA maintenance.
- `crates/cas-core/src/hooks/`, `crates/cas-core/src/memory/`, `crates/cas-core/src/search/temporal/` - hook schemas and temporal memory/search.
- `hub-web/src/main.ts`, `hub-web/src/conversation-store.ts`, `hub-web/src/inbox/` - startup, selected conversation state and reply grants.
- `hub-web/src/project-navigation.ts`, `hub-web/src/conversation-list.ts`, `hub-web/src/machine-version.ts` - grouped navigation, machine versions and release lookup.
- `hub-web/src/connection.ts`, `hub-web/src/connection-lifecycle.ts` - authenticated streams, recovery and session subscriptions.
- `hub-web/src/conversation-history.ts`, `hub-web/src/early-thread.ts`, `hub-web/src/thread-window.ts`, `hub-web/src/dismissed-asks.ts` - history paging, receipt progress, windowed turns and per-thread dismissal persistence.
- `hub-web/src/provider-mark.ts` - harness/provider avatar marks, state dots and model/effort header labels.
- `hub-web/src/supervisor-commands.ts` - slash palette, confirmations, native chooser controls and command receipts.
- `hub-web/src/live-phase.ts`, `hub-web/src/status-refresh.ts` - measured phase/elapsed view with explicit waiting, stale and unreported states; session-scoped latest status response ownership.
- `hub-web/src/progress-model.ts`, `hub-web/src/progress-view.ts`, `hub-web/src/team-view.ts` - shared release/task state, lazy Team and receipt trail.
- `hub-web/src/work-events.ts`, `hub-web/src/work-picture.ts`, `hub-web/src/work-rows-view.ts`, `hub-web/src/worker-timeline.ts` - typed operation disclosures, folded published-reasoning previews with safe Markdown/raw fallback, and live work pages.
- `hub-web/src/markdown-renderer.ts`, `hub-web/src/conversation-view.ts` - fenced source, native evidence/status tables and two-format copy.
- `hub-web/src/emoji-composer.ts`, `hub-web/src/emoji-picker.ts`, `hub-web/src/emoji-data.ts`, `hub-web/src/emoji-text.ts` - lazy local emoji search/recents, caret insertion and color glyph sequences.
- `hub-web/src/code-frame.ts`, `hub-web/src/rich-syntax.ts` - shared Copy/Wrap/Source/Expand frame for code and previews, and bounded lazy source color.
- `hub-web/src/rich-diagram.ts`, `hub-web/src/rich-diff.ts`, `hub-web/src/rich-attachment.ts`, `hub-web/src/rich-media.ts`, `hub-web/src/rich-pdf.ts` - lazy evidence renderers.
- `hub-web/src/image-viewer.ts`, `hub-web/src/image-viewer.css` - lazy image inspection dialog, turn gallery and pointer/keyboard controls; rationale in `hub-web/src/image-viewer.brief.md`.
- `hub-web/src/accounts-panel.ts`, `hub-web/src/accounts-model.ts` - per-machine account ledger, confirmations and launch-choice invalidation.
- `cas-cli/src/hub/accounts.rs`, `cas-cli/src/hub/account_routes.rs` - non-secret account projection and audited typed lifecycle routes.
- `cas-cli/src/hub/dismissed_sessions.rs` - authenticated ended-session dismissal route and atomic generation-scoped tombstones shared across paired devices.
- `cas-cli/src/hub/auth/`, `cas-cli/src/hub/operator_inbox/` - pairing/auth cache, durable phone input and recovery audits.
- `cas-cli/src/hub/composer_upload.rs`, `hub-web/src/composer-uploads.ts`, `hub-web/src/upload-transport.ts`, `hub-web/src/sent-attachments.ts` - byte progress, bounded upload identity and persistent sent-file presentation.
- `cas-cli/src/hub/secret_request.rs`, `cas-cli/src/config/meta/seed/secrets.rs` - Commander secret requests: the value is saved to a mode-600 file in `secrets.destination` and only its path is replied.
- `cas-cli/src/hub/artifact_proxy.rs`, `cas-cli/src/hub/chunk_assets.rs`, `cas-cli/src/hub/visual_pages.rs` - authorized media streams, hashed chunks and sandboxed pages.
- `hub-web/src/pairing-dialog.ts`, `hub-web/src/write-grant.ts`, `hub-web/src/context-resize.ts` - pairing, scoped folder grants and remembered layout width.
- `hub-web/e2e/journeys/`, `hub-web/preview/` - catalog journeys, isolated previews and real-hub recovery fixtures.
- `hub-web/e2e/journeys/hub-double-runtime.ts`, `hub-web/preview/hub-double-runtime.ts` - Node and browser adapters for the shared protocol double; the preview build selects browser crypto, time and raw CSP without a test runner.
- `slack-bridge/src/router-main.ts`, `slack-bridge/src/daemon-main.ts` - routing and daemon entrypoints.

## Cross-cutting and entrypoints

- **Rust proof:** inline modules, `crates/cas-store/tests/`, explicit `cas-cli/tests/` harnesses and `cas-cli/tests/integration/`; isolation in `cas-cli/src/test_env_guard.rs`.
- **Browser proof:** `hub-web/src/` Vitest, `hub-web/e2e/` Playwright, `scripts/journey-eval.sh` affected selection and `hub-web/scripts/` visual/performance probes.
- **Real-machine QA:** `scripts/qa-rig/commander-two-machine/` and `docs/qa/` journeys, sidecar cases and batch evaluations; `docs/qa/bug-families/supervisor-command-usage-screen.yaml` selects the native Claude Usage journey.
- **Contributor contracts:** `cas-cli/docs/CONTRIBUTING.md`, `cas-cli/docs/ARCHITECTURE.md`; design/terminal briefs and captures under `docs/design/`.
- **Release records:** `CHANGELOG.md`, `docs/release-notes/`, `docs/release-reports/` for titled entries, Slack drafts and Markdown/HTML/PDF proof.
- **Release tools:** `scripts/release.sh` prepares the queued release; `scripts/release-gate.sh` and `scripts/release-integration-gates.py` retain the fast-row gate/catalog after removal of the local release train. Supporting tools: `scripts/release-train-announce.py`, `scripts/release-report-post.py`, `cas-cli/src/cli/release_report.rs`.
- **Host release installation:** `scripts/install-on-publish.sh` installs a Linux systemd user timer or macOS LaunchAgent; `scripts/test-install-on-publish.sh` proves the scheduler lifecycle and release timer.
- **Hosted publication:** `.github/workflows/release.yml`, `scripts/deploy-hub-release.py`, `scripts/verify-hub-dist.sh`.
- **Queue flake recovery:** `scripts/ci-queue-flakes.py` owns queue-only native result classification, routed GitHub issue publication and open-issue quarantine; `scripts/test-ci-queue-flakes.py` covers the native runner and HTTP boundaries. `.github/workflows/ci.yml` wires read-only shards and the required publication fan-in.
- **Proof tooling:** `scripts/qa-candidate-build.py`, `scripts/run-verified-tests.sh`, `scripts/fast-rows-pre-push.sh`, `scripts/check-changed-markdown.py`.
- **Runtime/config:** `ops/systemd/`, `docs/ci/`, `.mcp.json`, `.claude/settings.json`, `.codex/hooks.json`.
- **CLI/library:** `cas-cli/src/main.rs`, `cas-cli/src/lib.rs`; **TUI/daemon:** `cas-cli/src/ui/factory/app/mod.rs`, `cas-cli/src/ui/factory/daemon/mod.rs`.
- **Services:** `cas-cli/src/mcp/daemon.rs` (`cas serve`), `cas-cli/src/bridge/server/` (`cas bridge serve`), `hub-web/src/main.ts` (`cas hub` client).
