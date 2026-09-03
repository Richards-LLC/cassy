# cas — Codemap
> Auto-generated structural map. Regenerate with `/codemap` when the layout drifts (modules added, removed, or renamed).

Rust workspace for the CAS coding-agent system. Product/domain material belongs in `docs/PRODUCT_OVERVIEW.md`; this file is a navigational index.

## Top-level layout
- `cas-cli/` — binary crate `cas`: CLI, MCP hub/server, hooks, factory TUI/daemon, bridge, builtin skill sync, and application services.
- `crates/` — 16 workspace libraries for shared types, storage, search, factory, terminal, MCP, and Ghostty bindings.
- `hub-web/` — TypeScript/Vite Commander SPA; checked-in `dist/` is the offline Cargo web-asset input.
- `slack-bridge/` — standalone Node/TypeScript service routing Slack traffic to CAS daemons.
- `contrib/shell-helpers/` — installable `cas-update` wrapper and shell-test harness.
- `docs/` — specs, guides, analysis, reports, reviews, release notes, research, and operational runbooks.
- `fixtures/retrieval-parity/` — checked-in retrieval benchmark baseline and query set.
- `migration/` — historical cloud-move logs, reports, and systemd material; not the active database migration tree.
- `ops/systemd/` — deployable Cassy Actions Runner service units and launch wrappers.
- `scripts/` — install, release train/gate, CI policy, scoped-test, portable-ISA, worktree, watchdog, and worker build-cache helpers.
- `homebrew/` — Homebrew formula, documentation, and formula update helper.
- `site/` — static project landing page and system PDF.
- `vendor/` — vendored Ghostty/libghostty-vt and pinned blake3 source.
- `.claude/` — tracked `CODEMAP.md`, `agents/`, `workflows/`, `settings.json`; `.claude/skills/` is an untracked sync output of `cas-cli/src/builtins/`.
- `.codex/` — Codex mirror of managed agent, skill, and hook configuration.
- `.cargo/` — workspace Cargo configuration.
- `.github/` — CI/release workflows, reusable actions, and public issue templates.
- `.config/` — local development and test-runner configuration.
- Root docs — `README.md`, `CONTRIBUTING.md`, `CLAUDE.md`, `AGENTS.md`, `CAS-DEEP-DIVE.md`, `CHANGELOG.md`, and `investigation-mcp-worktree.md`.
- Root config/assets — `Cargo.toml`, `.mcp.json`, `.env.worktree.template`, `casdemo.png`, and licensing files.

## Workspace / packages
- `cas-cli` — binary `cas`; composes all service crates and owns user-facing commands.
- `crates/cas-types` — shared domain, wire, provenance, task, agent, memory, hook, search, and verification types.
- `crates/cas-store` — SQLite stores, queues, history, knowledge, archives, verification gates, and vector persistence.
- `crates/cas-search` — hybrid BM25/semantic retrieval, code search, LMDB store, grep, and scoring.
- `crates/cas-core` — hook contexts/transcripts, memory hygiene, temporal search, extraction, dedup, and managed sync.
- `crates/cas-code` — multi-language code analysis, parsing, chunking, and indexing support.
- `crates/cas-mcp` — MCP daemon configuration, protocol types, and server support.
- `crates/cas-mcp-proxy` — policy-aware upstream MCP proxy engine and health tracking.
- `crates/cas-factory` — worker spawning, provider/lane registry routing, spec resolution, director detection, probes, and sessions.
- `crates/cas-factory-protocol` — factory client/server wire protocol, codecs, compression, and transport.
- `crates/cas-mux` — in-process terminal multiplexer, pane routing, harness backends, and injection.
- `crates/cas-pty` — PTY creation/configuration plus Claude, Codex, Grok, and OpenCode conformance.
- `crates/cas-recording` — asciinema-style terminal recording format, readers, writers, and export.
- `crates/cas-diffs` — diff parsing, inline rendering, widgets, and syntax highlighting.
- `crates/cas-tui-test` — PTY-backed TUI runner, screen assertions, input sequences, and artifacts.
- `crates/ghostty_vt` — safe Rust wrapper for libghostty-vt terminal emulation.
- `crates/ghostty_vt_sys` — low-level Ghostty FFI bindings, build support, and portable-target tests.

## cas-cli/src — application hub
`main.rs` starts the CLI; `lib.rs` exports application modules, the `panic = "unwind"` guard, and the canonical `TestEnvGuard`.
- `cli/mod.rs` — clap command dispatch; command modules cover auth, cloud, config, factory, hub, knowledge, memory, provider, status, update, and worktree flows.
- `cli/{setup,first_run,init,integrate,sync,update}.rs` — `cas setup` guided machine setup, first-run, integration, managed-file sync, and update transactions.
- `cli/factory/` — `cas factory` launch/configuration, lifecycle, worktree, liveness, parity, communication probes, and diagnostics.
- `cli/{hub,hub_service,hub_reverse_pairing}.rs` — `cas hub` fleet-control CLI, managed service installation, and reverse pairing.
- `cli/{codemap_cmd,project_overview_cmd,knowledge_cmd}.rs` — documentation freshness gates and `cas knowledge build|search|read`.
- `cli/{history_cmd,index_cmd,retrieval_parity}.rs` — history indexing/search, code index operations, and retrieval parity commands.
- `cli/{hook,mcp_cmd,changelog,known_repos}.rs` — hook dispatch, MCP registration, changelog, and known-repo commands.
- `cli/{claude,codex,claude_md,account_picker,provider_default,viktor}.rs` — harness profiles, login material, provider defaults, and Viktor access.
- `cli/{doctor,status,statusline,limits,queue,sweep,worktree}.rs` — diagnostics, usage, queues, cleanup, and worktree operations.
- `builtins.rs` + `builtins/{skills,agents,codex,grok}/` — embedded managed skills/agents/workflows and the `managed_by: cas` sync manifest; `builtins/reference-history.json` tracks reference hashes.
- `builtins/skills/cas-cut-release/` — fail-closed release-train skill (`SKILL.md`, `references/failure-log.md`) paired with `scripts/release-gate.sh`.
- `bridge/server/` — HTTP bridge used by `cas bridge serve`, including factory and MCP-facing handlers.
- `cloud/` — cloud sync, devices, teams, task proposals, embeddings, code vectors, and sync queue coordination.
- `config/{settings,runtime,hooks}.rs`, `config/access/` — settings, runtime hooks, and access policy.
- `config/meta/seed/` — generated config seed sections: coordination, daemon, llm, memory, release, skills, skill_validation, notifications, issues.
- `daemon/` — background maintenance, decay, observation, queue processing, filesystem watching, indexing, and bounded relevance evaluation (`relevance.rs`).
- `mcp/{daemon,socket,server}/` — always-available MCP daemon, Unix socket, and request routing.
- `mcp/tools/core/` — task, memory, knowledge, search, rules, skills, workflow, system, opinion, maintenance, and agent-coordination handlers.
- `mcp/tools/service/` — factory ops/remind, liveness, orphan recovery, external verification, pattern/spec ops, panic catch, and worktree/verification team ops.
- `hooks/handlers/` — SessionStart context (`session_query.rs`, `session_budget.rs`, `session_hygiene.rs`), PreToolUse/PostToolUse, session-stop, and issue triage.
- `hub/` — machine-local Commander hub: server/runtime, discovery, pairing auth, identity, attention, events, and death detection.
- `history/` — incremental Git history index, changelog/refs, FTS search, provenance, symbols, and epochs.
- `hybrid_search/` — lexical/semantic/code/knowledge search composition, caches, filters, graph, scoring, and legacy Tantivy-root repair (`legacy_index.rs`).
- `knowledge/` — `sources.rs` selection, `chunk.rs`, `prompt.rs`, `llm.rs`, `merge.rs` policy, and the `pipeline.rs` distillation build.
- `memory_migration/` — legacy memory import, audit, preconditions, routing, rollback, and reindex operations.
- `migration/migrations/` — numbered SQLite migrations; current tip `m248_tasks_retire_pending_supervisor_review.rs`.
- `store/` — layered project/global stores, syncing/notifying wrappers, sharing policy, detection, and test mocks.
- `sync/` — managed artifact rendering from builtins into `.claude/`, `.codex/`, and harness mirrors.
- `ui/factory/` — bare-`cas` TUI: `app/`, `boot/`, `director/`, `renderer/`, `client.rs`, `server_registry.rs`, `phoenix.rs`, cgroups/process groups.
- `ui/factory/daemon/` — factory daemon owning PTYs: `process.rs`, `fork_first.rs`, `cloud_client.rs`, and `runtime/` (lifecycle, delivery, relay, queue/events, ci_watch, teams, ws_client, session_summarizer).
- `worktree/` — discovery, Git operations, external-link handling, salvage, sweep, target locking, and cleanup.
- `ai_enrichment.rs`, `prompt_revalidation.rs`, `retrieval_parity/` — bounded model enrichment, prompt checks, and retrieval fixtures/diffs.
- `retrieval_eval.rs` — labeled retrieval evaluation scoring precision@5 / recall@5 against a committed baseline.
- `agent_id.rs`, `capability.rs`, `harness_policy.rs`, `factory_{context_reset,isolation,preflight,target_cache}.rs` — identity, capability, harness policy, factory safety, preflight, and cache boundaries.
- `ambient_recall.rs`, `consolidation/`, `extraction/`, `notifications/`, `orchestration/`, `telemetry/`, `tracing/`, `otel.rs`, `sentry.rs` — recall, extraction, coordination, and observability plumbing.

## cas-cli/tests and benchmarks
- `cas-cli/tests/` — ~100 integration targets for CLI, hooks, factory/MCP, hub, cloud, search, verification, e2e, snapshots, and multi-agent behavior.
- `cas-cli/tests/{builtin_archive_portability,builtin_doc_hygiene,builtin_skill_description,skill_hygiene,agent_definition_contract}_test.rs` — builtin skill/agent hygiene gates.
- `cas-cli/tests/mcp_tools_test/` and `mcp_action_surface_test.rs` — MCP handler suites, task lifecycle gates, and action-surface coverage.
- `cas-cli/tests/{setup,hub_detached_lifecycle,hub_launcher_path,team_pull_lww,project_pull_archived}_test.rs` — setup, hub lifecycle, and cloud pull coverage.
- `cas-cli/tests/common/`, `e2e/`, `fixtures/`, `support/` (`builtin_catalog.rs`), `snapshots/`, `proptest/` — shared fixtures, helpers, and snapshot data.
- `cas-cli/benches/code_indexing.rs` — Criterion benchmark for code indexing.
- `hub-web/src/*.test.ts` and `hub-web/e2e/` — Commander unit tests and browser-facing test harness.
- Inline `#[cfg(test)]` modules plus crate-specific `crates/*/tests/` — lower-level unit and integration coverage.

## crates — key module roots
- `cas-store/src/{agent_store,delegation_receipt_store,external_task_dependency_store}.rs` — leases, delegation receipts, and external task dependencies.
- `cas-store/src/{knowledge_store,history_store,code_vector_store,trace_archive,retrieval_store}.rs` — durable knowledge pages/ledger, history, vectors, trace archives, and retrieval outcomes.
- `cas-store/src/{surfaced_artifact_store,version_store,viktor_*_store,external_verification_gate}.rs` — surfaced artifacts, rule/skill versions, Viktor gateway state, and verification gates.
- `cas-store/src/{prompt_queue_store,supervisor_queue_store,spawn_queue_store,task_store}.rs` — durable prompts, supervisor/spawn queues, and task lifecycle.
- `cas-core/src/{hooks,memory,search/temporal,sync,extraction}/`, `dedup.rs` — hook config/context/transcript, memory hygiene/overlap, temporal search, managed sync, extraction.
- `cas-search/src/{bm25,code_search,lmdb_store,grep,parallel,scorer,traits}.rs` — text/code indexes, LMDB persistence, grep, parallel retrieval, scoring, and traits.
- `cas-mcp/src/{daemon.rs,types/}` — embedded daemon lifecycle and MCP operation/type definitions.
- `cas-factory/src/{routing,spec_resolver,probe,director,config}.rs` — static provider/lane routing, worker specs, probes, director detection, and config.
- `cas-factory/policy/lane-registry.toml` — checked-in provider lane and capability registry read by `routing.rs`.
- `cas-factory/src/session/{lifecycle,resume,state}.rs` — worker session lifecycle, resume, and state.
- `cas-mux/src/{backend,pane}/`, `{mux,pty,render,spec,harness,opencode}.rs` — provider backends, terminal panes, injection, rendering, and worker specs.
- `cas-pty/{src,conformance}/` — PTY runtime adapters and pinned harness contract receipts.
- `cas-types/src/{provenance,task,agent,delivery,verification,spec}.rs` — source lineage and core domain record types.
- `ghostty_vt_sys/{build_support.rs,tests/portable_target.rs}` — FFI build and portable target enforcement.

## Supporting services and assets
- `hub-web/src/main.ts` — Commander SPA entrypoint; pairing, sessions, panes, attention, messaging, and terminal adapters sit beside it.
- `hub-web/{package.json,vite.config.ts,tsconfig.json}` — Vite build, TypeScript checks, and Vitest scripts for the web client.
- `hub-web/dist/` — checked-in browser bundle and Ghostty WASM consumed by the Rust hub; rebuild from `hub-web/`, never hand-edit.
- `slack-bridge/src/{router-main,daemon-main}.ts` — Slack event/router and per-user bridge daemon entrypoints.
- `contrib/shell-helpers/{cas-update,install.sh,tests/}` — user update command, installer, and shell tests.
- `fixtures/retrieval-parity/{baseline-soundwave.json,queryset.toml}` — retrieval parity fixture inputs.
- `homebrew/cas.rb` — package formula.
- `scripts/{release-gate,test-release-gate}.sh` — fail-closed release train for an assembled epic worktree and its self-test.
- `scripts/{release,bump-release-version,check-release-preflight,check-release-host,detect-pending-release,release-published-receipt}.sh` — release cut, preflight, host trust, and receipts.
- `scripts/{run-scoped-tests,check-scoped-test-surface,check-scoped-snapshot-tests,run-verified-tests}.sh` — worker scoped-test runners and guards.
- `scripts/{refresh-worker-build-cache,worktree-boot,watchdog-policy,test-ci-test-tiers,classify-ci-diff}.sh` — build-cache snapshots, worktree boot, watchdogs, and CI tier policy pins.

## Cross-cutting
- **Tests:** colocated Rust unit tests, crate `tests/`, `cas-cli/tests/`, and Vitest suites under `hub-web/src/`.
- **Test isolation:** use `TestEnvGuard` from `cas-cli/src/lib.rs`; do not introduce a second HOME/environment helper.
- **Docs:** `cas-cli/docs/` holds ARCHITECTURE, CONTRIBUTING, MIGRATIONS, PROXY_SETUP, CLIENT_CAPABILITIES, TUI and worktree design; `docs/` holds specs, guides, analysis, reports, reviews, and release records.
- **Release/CI:** `.github/workflows/{ci,release,release-prebuild,merge-queue-watchdog,self-hosted-fast-validation}.yml`, `scripts/`, `CHANGELOG.md`, `docs/release-notes/`, `docs/RELEASE_SLACK_RUBRIC.md`, and `docs/SLACK_POSTING_RUNBOOK.md`.
- **Research/operations:** `docs/{analysis,research,reports,reviews,spikes}/`, `docs/{branch-protection,ci,migration}/`, `migration/`, and `ops/systemd/` hold durable investigations and host operations.
- **Managed harness files:** source in `cas-cli/src/builtins/`; rendered by `cas-cli/src/sync/` into `.claude/`, `.codex/`, and Grok mirrors on `cas update`.
- **Config:** `.claude/settings.json`, `.codex/config.toml`, `.codex/hooks.json`, `.mcp.json`, root `Cargo.toml`, and `.env.worktree.template`.
- **Generated/local state:** `target/`, `node_modules/`, `hub-web/node_modules/`, `.cas/`, and worktree-local CAS state are intentionally not source-map entries.

## Entrypoints
- CLI: `cas-cli/src/main.rs` → `cas`.
- Library: `cas-cli/src/lib.rs` → crate `cas`.
- Factory TUI: `cas-cli/src/ui/factory/app/mod.rs` → bare `cas`.
- Factory daemon: `cas-cli/src/ui/factory/daemon/mod.rs` → `cas factory` runtime.
- MCP hub: `cas-cli/src/mcp/daemon.rs` → `cas serve`.
- HTTP bridge: `cas-cli/src/bridge/server/` → `cas bridge serve`.
- Commander web: `hub-web/src/main.ts` → Vite bundle served by `cas hub`.
- Slack bridge: `slack-bridge/src/{router-main,daemon-main}.ts` → npm `start:*` scripts.
- Hooks: `cas-cli/src/cli/hook.rs` → `cas hook <event>`.
- Setup: `cas-cli/src/cli/setup.rs` → `cas setup`.
- Tests: `scripts/run-scoped-tests.sh -p cas --lib <module>` for workers; `cargo nextest run -p cas` is the supervisor gate.
