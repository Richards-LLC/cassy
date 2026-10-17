# cas - Codemap

> Auto-generated structural map. Regenerate with `/codemap` when modules or directories move.

## Top-level layout

- `.cargo/` - Cargo workspace build settings.
- `.claude/` - managed Claude instructions, skills, hooks, and this map.
- `.codex/` - rendered Codex instructions, hooks, and local configuration.
- `.config/` - checked-in test-runner configuration.
- `.github/` - CI workflows, reusable actions, and issue templates.
- `cas-cli/` - the `cas` binary: CLI, MCP server, hooks, cloud, and factory.
- `contrib/` - installable shell helpers and their test harness.
- `crates/` - shared Rust libraries for storage, search, factory, terminal, MCP, and types.
- `docs/` - architecture, operations, QA, release records, research, and project notes.
- `fixtures/` - checked-in retrieval-parity baselines and query sets.
- `homebrew/` - Homebrew formula and update helper.
- `hub-web/` - TypeScript/Vite Commander client, unit tests, and Playwright journeys.
- `migration/` - historical cloud-move records and deployment material.
- `ops/` - deployable systemd units and launch wrappers.
- `scripts/` - install, release, CI policy, scoped-test, worktree, and build-cache tools.
- `site/` - static project site and system PDF.
- `slack-bridge/` - standalone TypeScript Slack router and per-user daemon.
- Root files - `Cargo.toml` workspace, `README.md`, `AGENTS.md`, `CLAUDE.md`, and contributor guidance.

## Workspace / packages

- `cas-cli` - binary `cas`; user-facing commands and service composition.
- `crates/cas-types` - shared task, agent, memory, rule, skill, event, and wire types.
- `crates/cas-store` - SQLite stores, queues, knowledge, verification, and vector persistence.
- `crates/cas-search` - BM25, code search, grep, indexing, and score combination.
- `crates/cas-core` - hooks, memory hygiene, extraction, temporal search, and sync.
- `crates/cas-code` - tree-sitter code analysis and indexing support.
- `crates/cas-mcp` - MCP protocol types and daemon support.
- `crates/cas-mcp-proxy` - policy-aware upstream MCP proxy and health tracking.
- `crates/cas-factory` - worker lifecycle, provider routing, probes, directors, and sessions.
- `crates/cas-factory-protocol` - factory client/server wire protocol and transport.
- `crates/cas-mux` - terminal panes, harness backends, routing, and rendering.
- `crates/cas-pty` - PTY setup and Claude, Codex, Grok, and OpenCode adapters.
- `crates/cas-recording` - terminal recording format, readers, and export.
- `crates/cas-diffs` - diff parsing, inline rendering, and syntax highlighting.
- `crates/cas-operator-crypto` - HPKE/JWS envelopes shared with Cloud.
- `crates/cas-tui-test` - PTY-backed TUI runner and screen assertions.
- `crates/ghostty_vt` - safe Rust wrapper for the Ghostty terminal parser.
- `crates/ghostty_vt_sys` - low-level Ghostty FFI and build support.

## cas-cli/src - binary and service hub

The CLI crate wires command surfaces to shared stores and service crates.

- `cas-cli/src/main.rs` and `lib.rs` - binary entrypoint, exports, panic policy, and test environment boundary.
- `cas-cli/src/cli/` - clap commands for config, factory, knowledge, memory, provider, status, and worktrees.
- `cas-cli/src/cli/{doctor/communication.rs,doctor/backlog_hygiene.rs,violet_cmd/credentials.rs}` - communication SLO and backlog-hygiene doctor rows, Violet credential commands.
- `cas-cli/src/cli/project_files.rs` - managed project-file inventory and Git reconciliation used by `cas update`, init and doctor.
- `cas-cli/src/{accounts.rs,cli/accounts.rs}` - Claude account folder linking and drift (`cas accounts link|drift`).
- `cas-cli/src/{coordination.rs,coordination/external_work.rs,cli/coordination.rs}` - coordination action registry and the external-work claim journal (inactive until Cloud/Violet ship).
- `cas-cli/src/{backlog_hygiene.rs,factory_scratch.rs,factory_build_guard.rs}` - stale-backlog detectors, `factory.scratch_root` resolution, and pressure-aware build admission.
- `cas-cli/src/cli/{factory,integrate,config}/` - factory operations, integrations, and operator/runtime configuration.
- `cas-cli/src/config/` - access controls, metadata registry, operator policy, and seeded settings.
- `cas-cli/src/cloud/{peers_detail_work,peers_session_start,peer_merge,rate_limit,shutdown}.rs` - peer details, SessionStart peer block, about-to-merge notes and overlap warnings, rate limiting, shutdown.
- `cas-cli/src/cloud/{sync_lease.rs,sync_queue/,syncer/}` - store sync exclusion, queued push/pull, team/project sync; `syncer/entry_identity.rs` holds memory ids and Cloud rekey rule v1.
- `cas-cli/src/hub/` and `cas-cli/src/hub/idle_sync.rs` - Commander pairing, operator inbox (`operator_inbox/presence.rs` machine presence), idle-store sync, and fleet runtime.
- `cas-cli/src/daemon/` and `history/` - background maintenance and Git history search.
- `cas-cli/src/worktree/` and `cas-cli/src/worktree/git/epic_branch.rs` - worktree lifecycle and safe epic-branch setup.
- `cas-cli/src/hybrid_search/` - lexical, semantic, and code-search composition.
- `cas-cli/src/factory_build_memory.rs` and `cas-cli/src/factory_target_cache/legacy.rs` - build isolation and per-worker cache lifecycle.
- `cas-cli/src/hooks/handlers/` - session context, tool policy, event handling, and hook lifecycle.
- `cas-cli/src/hooks/handlers/handlers_events/{worker_denied_tools.rs,pre_tool/shell_effects.rs}` - worker command denial and shell write-path tracking (cwd, `sh -c`, links); `cli/hook/native_pre_tool.rs` is the native hook entry.
- `cas-cli/src/hooks/operation_lessons.rs` - hook-observed operational lessons.
- `cas-cli/src/git_evidence/` - shared Git measurements used by task close gates.
- `cas-cli/src/issue_reconcile.rs` - task and shipped-delivery references to GitHub issues.
- `cas-cli/src/qa_batch.rs`, `qa_pass.rs`, and `qa_evidence.rs` - batch receipts, independent QA, and close-time evidence.
- `cas-cli/src/supervisor_notices.rs` - durable owner-addressed factory notices.
- `cas-cli/src/migration/migrations/` - numbered SQLite migrations; `m271`-`m278` add worker, QA, communication-SLO and external-work journal records.
- `cas-cli/src/store/` - store traits and notification, sync, and layered wrappers.
- `cas-cli/src/bridge/server/` - HTTP/SSE handlers for `cas bridge serve`.
- `cas-cli/src/mcp/` - MCP daemon, Unix socket, prompt/resource serving, and tool dispatch; `mcp/server/memory.rs` bounds serve allocator arenas.
- `cas-cli/src/orchestration/`, `notifications/`, and `telemetry/` - coordination and observability plumbing.

## Factory coordination

- `cas-cli/src/ui/factory/app/` - TUI state, panels, selection, rendering, worker/epic views, and worktree actions.
- `cas-cli/src/ui/factory/app/{delivery_evidence,merge_alert}.rs` - task delivery evidence and merge alerts.
- `cas-cli/src/ui/factory/director/` - task and worker coordination, reminders, events, and supervisor-stall handling.
- `cas-cli/src/ui/factory/daemon/` - PTY-owning daemon and worker lifecycle.
- `cas-cli/src/ui/factory/daemon/runtime/` - daemon runtime modules and background job dispatch.
- `cas-cli/src/ui/factory/daemon/runtime/{delivery_probe_jobs,external_wake,factory_jobs,inject_commands,message_attention,poison_sweep_jobs,prompt_queue_jobs,queue_control_jobs,relay_jobs,render_scheduler}.rs` - probes, wakes, queue work, notices, and rendering.
- `cas-cli/src/ui/factory/daemon/runtime/merge_queue_ejection.rs` - merge-queue ejection behavior and adjacent tests.
- `cas-cli/src/ui/factory/daemon/runtime/{violet_activity,external_work_reconcile_jobs}.rs` - Slack activity claims, typed worker wake, and claim-journal reconciliation.
- `cas-cli/src/ui/factory/daemon/runtime/fixtures/` - recorded CI and merge-queue inputs for runtime tests.
- `cas-cli/src/ui/factory/server_registry/` and `{boot,client,protocol,session}.rs` - registered servers, startup, transport, and session state.
- `crates/cas-factory/src/` - provider/lane routing, task specs, probes, directors, configuration, and sessions.

## MCP service and tools

- `cas-cli/src/mcp/{daemon.rs,socket.rs,server/}` - daemon lifecycle, Unix transport, runtime, watchdog, prompts, and resources.
- `cas-cli/src/mcp/tools/core/` - task, memory, knowledge, artifacts, search, rules, skills, workflow, and coordination handlers.
- `cas-cli/src/mcp/tools/core/task/` - task queries, notes, proposals, dependencies, lifecycle, and close gates.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/` - evidence-only, loaded-proof, proof-claim citation (`proof_claims.rs`), delivery, snapshot, and epic close/autoclose checks; `task/block_hold.rs` holds structured task holds.
- `cas-cli/src/mcp/tools/core/task/lifecycle/epic_lane.rs` - epic target resolution and lane evidence.
- `cas-cli/src/mcp/tools/service/` - factory/reminder ops, liveness, external verification, external-work claims, servers, and tool schemas.
- `cas-cli/src/mcp/tools/service/agent_search_system/` - context/code/history search, messaging, and supervisor queue operations.
- `cas-cli/src/mcp/tools/types/` - shared request and response schemas for MCP actions.

## Builtins, tests, and clients

- `cas-cli/src/builtins/` - embedded agents, skills, harness mirrors, jobs, and sync manifest.
- `cas-cli/src/builtins/skills/` - canonical skill sources rendered into harness mirrors.
- `cas-cli/tests/` - CLI, hooks, cloud, factory/MCP, hub, search, verification, e2e, and multi-agent targets.
- `cas-cli/tests/mcp_tools_test/` and `e2e/` - action coverage and end-to-end factory, hook, task, team, and worktree journeys.
- `hub-web/src/` - Commander SPA state, pairing, conversations, raw output (`raw-output.ts`), attention, messaging, and Vitest tests; `inbox/{presence,reply-grants}.ts` hold machine presence and the gated reply-grant request; `fonts/` holds bundled fonts.
- `hub-web/e2e/journeys/` - Playwright journeys and fake-hub support; `preview/` and `scripts/` hold isolated tooling.
- `slack-bridge/src/` - Slack router and daemon entrypoints, commands, sessions, filtering, and tests.

## Crates - key module roots

- `crates/cas-store/src/` - task/agent/knowledge/history/verification stores; `communication_slo/`, `qa_batch_store.rs`, and `lane_guard.rs` cover SLOs, batch receipts, and daemon-lane checks.
- `crates/cas-core/src/{hooks,memory,search/temporal,sync,extraction}/` - hook input, memory hygiene, temporal search, sync, and extraction.
- `crates/cas-search/src/{bm25,code_search,lmdb_store,grep,parallel,scorer}.rs` - text/code indexes, persistence, grep, parallel retrieval, and scoring.
- `crates/cas-types/src/{task,agent,entry,rule,skill,verification,delivery,lifecycle}.rs` - shared domain, coordination, and typed relay records; `factory_claude.rs` and `factory_scratch.rs` hold Claude launch and scratch-root settings.
- `crates/cas-mux/src/{backend,pane}/`, `mux.rs`, and `submit_gate.rs` - terminal backends, panes, rendering, and input gating.
- `crates/cas-pty/{src,conformance}/` - harness adapters and pinned conformance receipts; `src/{input_queue,violet_env,codex_hook_warning}.rs` hold queued input, Violet environment and Codex hook warnings.
- `crates/cas-mcp/src/{daemon.rs,types/}` - embedded MCP daemon lifecycle and protocol definitions.
- `crates/{cas-recording,cas-diffs,cas-tui-test}/src/` - terminal recordings, diff widgets, and TUI support.
- `crates/ghostty_vt_sys/{build_support.rs,tests/portable_target.rs}` - FFI build and portable-target enforcement.

## Cross-cutting

- **Managed files:** builtin sources in `cas-cli/src/builtins/` render harness files under `.claude/` and `.codex/`.
- **Docs:** `cas-cli/docs/` has architecture/contributing/migration material; `docs/analysis/`, `docs/specs/`, and `docs/design/` hold research and design records.
- **Release:** `docs/release-notes/` contains Slack drafts; `docs/release-reports/` holds versioned Markdown, HTML, and PDF reports; both ride in the release PR with this map.
- **Journey QA:** `docs/qa/` defines journeys and evidence gates; `docs/qa/journey-evaluations/` stores receipts; `scripts/journey-eval.sh` selects affected journeys.
- **CI/release tooling:** `.github/` and `scripts/` hold CI policy; `scripts/resolve-ci-base.sh` resolves diff bases, `worker-proof-guards.py` checks worker evidence, and `check-communication-slos.py` runs the SLO fixture gate.
- **Visual QA:** `scripts/visual-qa-*` and `scripts/visual-qa-fixtures/` cover the UI craft scanner.
- **Operations:** `migration/`, `ops/systemd/`, `docs/branch-protection/`, and `docs/ci/` hold deployment and runbook material.
- **Tests:** colocated Rust unit tests, crate `tests/`, `cas-cli/tests/`, Vitest under `hub-web/src/`, and Playwright under `hub-web/e2e/`.
- **Local state:** `.cas/`, `.context/`, `target/`, `dist/`, `node_modules/`, and `vendor/` are generated or ignored, not source-map entries.

## Entrypoints

- CLI: `cas-cli/src/main.rs` -> `cas`.
- Library: `cas-cli/src/lib.rs` -> crate `cas`.
- Factory TUI: `cas-cli/src/ui/factory/app/mod.rs` -> bare `cas`.
- Factory daemon: `cas-cli/src/ui/factory/daemon/mod.rs` -> factory runtime.
- MCP server: `cas-cli/src/mcp/daemon.rs` -> `cas serve`.
- HTTP bridge: `cas-cli/src/bridge/server/` -> `cas bridge serve`.
- Commander web: `hub-web/src/main.ts` -> Vite client served by `cas hub`.
- Slack bridge: `slack-bridge/src/{router-main,daemon-main}.ts` -> npm start scripts.
- Hooks: `cas-cli/src/cli/hook.rs` -> hook event dispatch; setup: `cas-cli/src/cli/setup.rs`.
- Worker check: `scripts/run-scoped-tests.sh`; integration assembly proof belongs to the supervisor/merge queue.
