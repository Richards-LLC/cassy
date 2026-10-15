# cas - Codemap

> Auto-generated structural map. Regenerate with /codemap when modules or directories move.

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
- `cas-cli/src/cli/factory/` - factory lifecycle, probes, parity checks, worktrees, and recovery commands.
- `cas-cli/src/cli/config/operator_ops.rs` - operator policy and project write-root configuration commands.
- `cas-cli/src/config/operator_policy.rs` - project write roots, legacy policy migration, and operator policy types.
- `cas-cli/src/hooks/handlers/handlers_events/write_roots.rs` - write-root policy evaluation for hook events.
- `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs` - PreToolUse guards, including operator-command detection and worker commit scope.
- `cas-cli/src/config/` - runtime settings, access controls, metadata registry, and seeded configuration sections.
- `cas-cli/src/daemon/` - background maintenance, source watching, indexing, and relevance evaluation.
- `cas-cli/src/history/` - Git history index, refs, changelog, FTS search, symbols, and provenance.
- `cas-cli/src/worktree/` - worktree discovery, Git operations, locking, salvage, sweep, and cleanup.
- `cas-cli/src/cloud/` - optional sync, teams, proposals, aliases, embeddings, and queued push/pull.
- `cas-cli/src/knowledge/` - project-source selection, chunking, distillation, merge, and LLM runner.
- `cas-cli/src/hybrid_search/` - lexical, semantic, and code-search composition.
- `cas-cli/src/store/` - store traits and notification, sync, and layered wrappers.
- `cas-cli/src/migration/migrations/` - numbered SQLite migrations; `m267_supervisor_queue_add_prompt_delivered_at.rs` adds a supervisor queue delivery timestamp.
- `cas-cli/src/hooks/handlers/` - session-start context, hygiene, PreToolUse/PostToolUse, stop handling, and triage.
- `cas-cli/src/hooks/operation_lessons.rs` - hook-observed operational lessons.
- `cas-cli/src/factory_target_cache/` - per-worker build-cache ownership, seeding, parking, and retirement.
- `cas-cli/src/qa_pass.rs` and `qa_evidence.rs` - independent QA eligibility, journey selection, and close-time proof.
- `cas-cli/src/git_evidence/` - shared Git measurements used by task close gates.
- `cas-cli/src/bridge/server/` - HTTP/SSE handlers for `cas bridge serve`.
- `cas-cli/src/hub/` - Commander pairing, operator inbox, and fleet runtime.
- `cas-cli/src/mcp/` - MCP daemon, Unix socket, prompt/resource serving, and tool dispatch.
- `cas-cli/src/orchestration/`, `notifications/`, and `telemetry/` - coordination and observability plumbing.

## Factory coordination

- `cas-cli/src/ui/factory/app/` - TUI state, panels, selection, rendering, worker/epic views, and worktree actions.
- `cas-cli/src/ui/factory/director/` - task and worker coordination, reminders, events, and supervisor-stall handling.
- `cas-cli/src/ui/factory/daemon/` - PTY-owning daemon and worker lifecycle.
- `cas-cli/src/ui/factory/daemon/runtime/` - delivery, CI watch, relay, team, queue/event, and client runtime.
- `cas-cli/src/ui/factory/daemon/runtime/merge_queue_ejection.rs` - merge-queue ejection behavior and its adjacent tests.
- `cas-cli/src/ui/factory/daemon/runtime/fixtures/` - recorded CI and merge-queue inputs for runtime tests.
- `cas-cli/src/ui/factory/daemon/runtime/violet_activity.rs` - Cloud-backed Slack activity claims and typed worker wake.
- `cas-cli/src/ui/factory/server_registry/docker.rs` - registered servers launched in Docker containers.
- `cas-cli/src/ui/factory/{boot,client,protocol,session}.rs` - startup, client transport, protocol, and session state.
- `crates/cas-factory/src/` - provider/lane routing, task specs, probes, directors, configuration, and sessions.

## MCP service and tools

- `cas-cli/src/mcp/{daemon.rs,socket.rs,server/}` - daemon lifecycle, Unix transport, runtime, watchdog, prompts, and resources.
- `cas-cli/src/mcp/tools/core/` - task, memory, knowledge, artifacts, search, rules, skills, workflow, system, and coordination handlers.
- `cas-cli/src/mcp/tools/core/task/` - task queries, notes, proposals, dependencies, lifecycle, and close gates.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/` - evidence-only, receipt scope, snapshot, delivery, and epic close checks.
- `cas-cli/src/mcp/tools/service/` - factory/reminder ops, liveness, external verification, servers, worktrees, and tool schemas.
- `cas-cli/src/mcp/tools/service/qa_pass_ops.rs` - `verification action=qa_record|qa_waive|qa_status`.
- `cas-cli/src/mcp/tools/service/agent_search_system/` - context/code/history search, messaging, and supervisor queue operations.
- `cas-cli/src/mcp/tools/types/` - shared request and response schemas for MCP actions.

## Builtins, tests, and clients

- `cas-cli/src/builtins/` - embedded agents, skills, harness mirrors, jobs, and sync manifest.
- `cas-cli/src/builtins/skills/` - canonical skill sources rendered into harness mirrors.
- `cas-cli/src/builtins/skills/violet/references/push-wake.md` - Violet push activity and worker-wake reference.
- `cas-cli/tests/` - CLI, hooks, cloud, factory/MCP, hub, search, verification, e2e, and multi-agent targets.
- `cas-cli/tests/mcp_tools_test/` - action coverage for task lifecycle, close gates, QA, and verification.
- `cas-cli/tests/e2e/` - factory, hooks, memory/rules, tasks, teams, verification, and worktree journeys.
- `hub-web/src/` - Commander SPA state, pairing, sessions, panes, attention, messaging, and Vitest tests.
- `hub-web/e2e/journeys/` - Playwright journeys and fake-hub support.
- `hub-web/{playwright.config.ts,playwright.real-hub.config.ts,specs/}` - browser configurations and test plans.
- `hub-web/preview/` and `hub-web/scripts/` - isolated preview build and journey tooling.
- `slack-bridge/src/` - Slack router and daemon entrypoints, commands, sessions, filtering, and tests.

## Crates - key module roots

- `crates/cas-store/src/` - task/agent/knowledge/history/verification stores and SQLite migrations.
- `crates/cas-core/src/{hooks,memory,search/temporal,sync,extraction}/` - hook input, memory hygiene, temporal search, sync, and extraction.
- `crates/cas-search/src/{bm25,code_search,lmdb_store,grep,parallel,scorer}.rs` - text/code indexes, persistence, grep, parallel retrieval, and scoring.
- `crates/cas-types/src/{task,agent,entry,rule,skill,verification,delivery}.rs` - shared domain and coordination records.
- `crates/cas-mcp/src/{daemon.rs,types/}` - embedded MCP daemon lifecycle and protocol definitions.
- `crates/cas-mux/src/{backend,pane}/` and `mux.rs` - terminal backends, panes, and rendering.
- `crates/cas-pty/{src,conformance}/` - harness adapters and pinned conformance receipts.
- `crates/{cas-recording,cas-diffs,cas-tui-test}/src/` - terminal recordings, diff widgets, and TUI support.
- `crates/ghostty_vt_sys/{build_support.rs,tests/portable_target.rs}` - FFI build and portable-target enforcement.

## Cross-cutting

- **Managed files:** builtin sources in `cas-cli/src/builtins/` render harness files under `.claude/` and `.codex/`.
- **Docs:** `cas-cli/docs/` has architecture/contributing/migration material; `docs/` holds durable project records.
- **Release:** `docs/release-notes/` contains Slack drafts; `docs/release-reports/` holds versioned Markdown, HTML, and PDF reports.
- **Journey QA:** `docs/qa/` defines journeys and evidence gates; `docs/qa/journey-evaluations/` stores receipts; `scripts/journey-eval.sh` selects affected journeys.
- **CI/release tooling:** `.github/` and `scripts/` hold CI policy; `scripts/release-budget-breach.py` checks release-stage budgets.
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
