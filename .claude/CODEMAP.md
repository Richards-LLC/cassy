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
- `cas-cli/src/cli/{doctor/communication.rs,violet_cmd/credentials.rs}` - communication SLO doctor row and Violet credential commands.
- `cas-cli/src/cli/{factory,integrate,config}/` - factory operations, integrations, and operator/runtime configuration.
- `cas-cli/src/config/` - access controls, metadata registry, operator policy, and seeded settings.
- `cas-cli/src/cloud/{peers_detail_work,rate_limit,shutdown}.rs` - peer detail workflows, rate limiting, and shutdown; cloud APIs and proposals live alongside them.
- `cas-cli/src/cloud/{sync_lease.rs,sync_queue/,syncer/}` - store sync exclusion, queued push/pull, and team/project sync.
- `cas-cli/src/hub/` and `cas-cli/src/hub/idle_sync.rs` - Commander pairing, operator inbox, idle-store sync, and fleet runtime.
- `cas-cli/src/daemon/` and `history/` - background maintenance and Git history search.
- `cas-cli/src/worktree/` and `cas-cli/src/worktree/git/epic_branch.rs` - worktree lifecycle and safe epic-branch setup.
- `cas-cli/src/factory_build_memory.rs` and `cas-cli/src/factory_target_cache/legacy.rs` - build isolation and per-worker cache lifecycle.
- `cas-cli/src/hooks/handlers/` - session context, tool policy, event handling, and hook lifecycle.
- `cas-cli/src/hooks/handlers/handlers_events/worker_denied_tools.rs` - worker command denial policy.
- `cas-cli/src/issue_reconcile.rs` - task and shipped-delivery references to GitHub issues.
- `cas-cli/src/qa_batch.rs`, `qa_pass.rs`, and `qa_evidence.rs` - batch receipts, independent QA, and close-time evidence.
- `cas-cli/src/supervisor_notices.rs` - durable owner-addressed factory notices.
- `cas-cli/src/migration/migrations/` - numbered SQLite migrations; `m271`-`m277` add worker, QA, and communication-SLO records.
- `cas-cli/src/store/` - store traits and notification, sync, and layered wrappers.
- `cas-cli/src/bridge/server/` - HTTP/SSE handlers for `cas bridge serve`.
- `cas-cli/src/orchestration/`, `notifications/`, and `telemetry/` - coordination and observability plumbing.

## Factory coordination

- `cas-cli/src/ui/factory/app/` - TUI state, panels, selection, rendering, worker/epic views, and worktree actions.
- `cas-cli/src/ui/factory/app/delivery_evidence.rs` - task delivery evidence and close-time presentation.
- `cas-cli/src/ui/factory/director/` - task and worker coordination, reminders, events, and supervisor-stall handling.
- `cas-cli/src/ui/factory/daemon/` - PTY-owning daemon and worker lifecycle.
- `cas-cli/src/ui/factory/daemon/runtime/` - daemon runtime modules and background job dispatch.
- `cas-cli/src/ui/factory/daemon/runtime/{delivery_probe_jobs,external_wake,factory_jobs,inject_commands,message_attention,poison_sweep_jobs,prompt_queue_jobs,queue_control_jobs,relay_jobs,render_scheduler}.rs` - probes, wakes, queue work, notices, and rendering.
- `cas-cli/src/ui/factory/daemon/runtime/fixtures/` - recorded CI and merge-queue inputs for runtime tests.
- `cas-cli/src/ui/factory/server_registry/` and `{boot,client,protocol,session}.rs` - registered servers, startup, transport, and session state.
- `crates/cas-factory/src/` - provider/lane routing, task specs, probes, directors, configuration, and sessions.

## MCP service and tools

- `cas-cli/src/mcp/{daemon.rs,socket.rs,server/}` - daemon lifecycle, Unix transport, runtime, watchdog, prompts, and resources.
- `cas-cli/src/mcp/tools/core/` - task, memory, knowledge, artifacts, search, rules, skills, workflow, and coordination handlers.
- `cas-cli/src/mcp/tools/core/task/` - task queries, notes, proposals, dependencies, lifecycle, and close gates.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/` - evidence-only, loaded-proof (`loaded_proof.rs`), delivery, snapshot, and epic close checks.
- `cas-cli/src/mcp/tools/core/task/lifecycle/epic_lane.rs` - epic target resolution and lane evidence.
- `cas-cli/src/mcp/tools/service/` - factory/reminder ops, liveness, external verification, servers, and tool schemas.
- `cas-cli/src/mcp/tools/service/agent_search_system/` - context/code/history search, messaging, and supervisor queue operations.
- `cas-cli/src/mcp/tools/types/` - shared request and response schemas for MCP actions.

## Builtins, tests, and clients

- `cas-cli/src/builtins/` - embedded agents, skills, harness mirrors, jobs, and sync manifest.
- `cas-cli/src/builtins/skills/` - canonical skill sources rendered into harness mirrors.
- `cas-cli/tests/` - CLI, hooks, cloud, factory/MCP, hub, search, verification, e2e, and multi-agent targets.
- `cas-cli/tests/mcp_tools_test/` and `e2e/` - action coverage and end-to-end factory, hook, task, team, and worktree journeys.
- `hub-web/src/` - Commander SPA state, pairing, sessions, panes, attention, messaging, and Vitest tests; `fonts.css` and `fonts/` hold bundled fonts and licenses.
- `hub-web/e2e/journeys/` - Playwright journeys and fake-hub support; `preview/` and `scripts/` hold isolated tooling.
- `slack-bridge/src/` - Slack router and daemon entrypoints, commands, sessions, filtering, and tests.

## Crates - key module roots

- `crates/cas-store/src/` - task/agent/knowledge/history/verification stores; `communication_slo/`, `qa_batch_store.rs`, and `lane_guard.rs` cover SLOs, batch receipts, and daemon-lane checks.
- `crates/cas-core/src/{hooks,memory,search/temporal,sync,extraction}/` - hook input, memory hygiene, temporal search, sync, and extraction.
- `crates/cas-search/src/{bm25,code_search,lmdb_store,grep,parallel,scorer}.rs` - text/code indexes, persistence, grep, parallel retrieval, and scoring.
- `crates/cas-types/src/{task,agent,entry,rule,skill,verification,delivery,lifecycle}.rs` - shared domain, coordination, and typed relay records.
- `crates/cas-mux/src/{backend,pane}/`, `mux.rs`, and `submit_gate.rs` - terminal backends, panes, rendering, and input gating.
- `crates/cas-pty/{src,conformance}/` - harness adapters and pinned conformance receipts; `src/{input_queue.rs,violet_env.rs}` hold queued input and Violet environment handling.
- `crates/cas-mcp/src/{daemon.rs,types/}` - embedded MCP daemon lifecycle and protocol definitions.
- `crates/{cas-recording,cas-diffs,cas-tui-test}/src/` - terminal recordings, diff widgets, and TUI support.
- `crates/ghostty_vt_sys/{build_support.rs,tests/portable_target.rs}` - FFI build and portable-target enforcement.

## Cross-cutting

- **Managed files:** builtin sources in `cas-cli/src/builtins/` render harness files under `.claude/` and `.codex/`.
- **Docs:** `cas-cli/docs/` has architecture/contributing/migration material; `docs/analysis/`, `docs/specs/`, and `docs/design/` hold research and design records.
- **Release:** `docs/release-notes/` contains Slack drafts; `docs/release-reports/` holds versioned Markdown, HTML, and PDF reports.
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
