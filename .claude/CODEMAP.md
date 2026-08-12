# cas — Codemap
> Auto-generated structural map. Regenerate with `/codemap` when the layout drifts (modules added, removed, or renamed).

Rust workspace for the CAS coding-agent system. Product/domain material belongs in `docs/PRODUCT_OVERVIEW.md`; this file is a navigational index.

## Top-level layout
- `cas-cli/` — binary crate `cas`: CLI, MCP hub/server, hooks, factory TUI/daemon, bridge, and application services.
- `crates/` — 16 workspace libraries: shared types, storage, search, factory, terminal, MCP, and Ghostty bindings.
- `contrib/shell-helpers/` — installable `cas-update` wrapper plus its shell-test harness.
- `docs/` — plans, specs, reports, release notes, research, guides, and operational records.
- `fixtures/retrieval-parity/` — checked-in retrieval benchmark baseline and query set.
- `scripts/` — install, release, migration, scoped-test, build-regression, worktree, portable-ISA, release-preflight, and worker build-cache (`refresh-worker-build-cache.sh`) helpers.
- `migration/` — historical cloud-move logs and systemd material; not the active database migration tree.
- `homebrew/` — Homebrew formula and formula update helper.
- `slack-bridge/` — standalone Node/TypeScript service routing Slack traffic to CAS daemons.
- `site/` — static project landing page and system PDF.
- `vendor/ghostty/` — vendored libghostty-vt source; follow its local `AGENTS.md` before editing.
- `.claude/` — tracked Codemap plus CAS-managed Claude agents, skills, workflows, and settings.
- `.codex/` — Codex mirror of managed agent/skill configuration.
- `.cargo/` — workspace Cargo configuration.
- `.github/` — CI/release workflows, including portable ISA validation, and public issue templates.
- `.config/` — local development/editor configuration.
- Root docs — `README.md`, `CONTRIBUTING.md`, `CLAUDE.md`, `AGENTS.md`, `CAS-DEEP-DIVE.md`, `CHANGELOG.md`, and `investigation-mcp-worktree.md`.
- Root config/assets — `Cargo.toml`, `.mcp.json`, `.env.worktree.template`, `casdemo.png`, and licensing files.

## Workspace / packages
- `cas-cli` — binary `cas`; composes all service crates and owns user-facing commands.
- `crates/cas-types` — shared domain/wire types for tasks, agents, memories, hooks, search, and verification.
- `crates/cas-store` — SQLite schemas, stores, migrations support, queues, history, knowledge, and vector persistence.
- `crates/cas-search` — hybrid BM25/semantic retrieval and index abstractions.
- `crates/cas-core` — business logic, hook context construction, and shared orchestration.
- `crates/cas-code` — code indexing and symbol-search support.
- `crates/cas-mcp` — MCP protocol types and handlers.
- `crates/cas-mcp-proxy` — upstream MCP proxy engine.
- `crates/cas-factory` — worker spawning, configuration, director detection, and availability probes.
- `crates/cas-factory-protocol` — factory client/server wire types.
- `crates/cas-mux` — in-process terminal multiplexer and pane injection.
- `crates/cas-pty` — PTY creation/configuration for Claude, Codex, and Grok workers.
- `crates/cas-recording` — asciinema-style terminal recording support.
- `crates/cas-diffs` — diff parsing, rendering, and syntax highlighting.
- `crates/cas-tui-test` — PTY-backed TUI test helpers.
- `crates/ghostty_vt` and `crates/ghostty_vt_sys` — safe/FFI libghostty-vt layers; `build_support.rs` and `tests/portable_target.rs` enforce portable target selection.

## cas-cli/src — application hub
`main.rs` starts the CLI; `lib.rs` exports application modules and the canonical `TestEnvGuard`.
- `cli/mod.rs` — clap command dispatch; use this first to locate a CLI subcommand.
- `cli/factory/` — `cas factory` launch/configuration, lifecycle, worktree, liveness, parity, communication probes, and `doctor.rs` diagnostics.
- `cli/{hub,hub_service,hub_reverse_pairing}.rs` — `cas hub` fleet-control CLI: launch, launchd/systemd service install, reverse pairing.
- `cli/limits.rs` — `cas limits` usage/limit reporting.
- `cli/sync/agents_md.rs` — managed `AGENTS.md` sync command (core logic in `crates/cas-core/src/sync/agents_md.rs`).
- `cli/{codemap_cmd,project_overview_cmd,knowledge_cmd}.rs` — documentation freshness gates and knowledge commands.
- `cli/history_cmd.rs` — history index/search/status/repair command surface.
- `cli/{hook,update,integrate,config,config_tui,init}.rs` — hook dispatch, managed-file sync, integrations, configuration, and setup.
- `cli/{claude,claude_md}.rs` — Claude profile launch/login commands and managed `CLAUDE.md` material.
- `cli/{doctor,status,index_cmd}.rs` — diagnostics, status, and index operations.
- `hub/` — machine-local Commander hub (EPIC cas-bec9): `server.rs`/`runtime.rs`, `discovery.rs`, `tailscale.rs`, `auth.rs`/`identity.rs`, event stream, death detection.
- `mcp/{daemon,socket,server}/` — always-available MCP daemon, Unix socket, and request routing.
- `mcp/tools/core/` — task, memory, knowledge, search, rules, skills, and coordination MCP handlers.
- `mcp/tools/service/` — factory operations, supervisor queue, messaging, history/code search, liveness, and recovery handlers.
- `ui/factory/` — bare-`cas` TUI, factory daemon runtime (incl. `runtime/ci_watch.rs` CI red-run relays), director events/prompts, app state, and rendering.
- `bridge/server/` — HTTP bridge server used by `cas bridge serve`.
- `hooks/handlers/` — SessionStart, PreToolUse, PostToolUse, and session-stop hook behavior.
- `builtins/` — embedded Claude/Codex/Grok skills, agents, and workflows synced into harness directories; `skills/cli-routing/` routes CLI work and `skills/cas-code-review/` carries review personas/workflows.
- `store/` — layered project/global stores, syncing/notifying wrappers, sharing policy, and test mocks.
- `cloud/` — cloud sync, embeddings, code-vector support, embedding drain, and sync queue.
- `knowledge/` — source selection, chunking, prompt construction, merge policy, and distillation pipeline.
- `history/` — incremental Git history index, FTS search, provenance, symbol links, and epoch tracking.
- `ambient_recall.rs` — bounded, scope-gated hook recall contracts and ranking.
- `hybrid_search/` — lexical/semantic/code/knowledge search composition and capability-aware weighting.
- `migration/migrations/` — numbered SQLite migrations; current tip `m232_worker_completion_receipts_add_artifact_path.rs`.
- `daemon/` — background maintenance, filesystem watching, and indexing scheduling.
- `sync/` — managed artifact rendering from builtins into `.claude/` and harness mirrors.
- `worktree/` — create, manage, salvage, sweep, and clean worktrees.
- `orchestration/`, `notifications/`, `telemetry/`, `tracing/`, `logging.rs`, `sentry.rs` — runtime coordination and observability.
- `internal_llm.rs` — marks internal model calls so ambient recall cannot recurse into them.

## cas-cli/tests and benchmarks
- `cas-cli/tests/` — integration coverage for CLI output, hooks, factory/MCP operations, search, cloud sync, and verification.
- `cas-cli/tests/common/` and `cas-cli/tests/e2e/` — shared fixtures and end-to-end helpers.
- `cas-cli/benches/code_indexing.rs` — Criterion benchmark for code indexing.
- Inline `#[cfg(test)]` modules — unit tests colocated with Rust implementation modules.

## crates — key module roots
- `cas-store/src/agent_store/` — worker identities, task leases, and lease-history operations.
- `cas-store/src/{knowledge_store,history_store,code_vector_store}.rs` — durable knowledge pages, Git history, and code-vector state.
- `cas-store/src/{prompt_queue_store,supervisor_queue_store}.rs` — durable agent prompt delivery and supervisor review queues.
- `cas-search/src/lmdb_store.rs` — LMDB-backed vector/cache support for retrieval.
- `cas-core/src/hooks/` — hook input schemas and context builders.
- `cas-mcp/src/types/ops_secondary.rs` — secondary MCP operation request types, including coordination.
- `cas-factory/src/{spec_resolver,probe}.rs` — harness/model spec resolution and worker availability checks.
- `cas-mux/src/{mux,pane,spec}.rs` — pane routing, injection boundary, and worker process specs.

## Supporting services and assets
- `slack-bridge/src/router-main.ts` — Slack event/router process entrypoint.
- `slack-bridge/src/daemon-main.ts` — per-user bridge daemon entrypoint.
- `slack-bridge/src/` — Bolt/Web API handling, routing, and daemon communication.
- `contrib/shell-helpers/cas-update` — user-facing shell update command.
- `contrib/shell-helpers/install.sh` — installs shell helpers; `tests/cas-update-test.sh` validates behavior.
- `fixtures/retrieval-parity/{baseline-soundwave.json,queryset.toml}` — retrieval parity fixture inputs.
- `homebrew/cas.rb` — package formula; `homebrew/update-formula.sh` updates it.

## Cross-cutting
- **Tests:** colocated Rust unit tests plus integration suites in `cas-cli/tests/` and crate-specific `crates/*/tests/`.
- **Test isolation:** use `TestEnvGuard` from `cas-cli/src/lib.rs`; do not introduce a second HOME/environment helper.
- **Release tooling:** `scripts/release.sh`, `scripts/check-release-preflight.sh`, migration-snapshot and portable-ISA check/test scripts, `CHANGELOG.md`, and dated `docs/release-notes/` announcements.
- **Research/reports:** `docs/analysis/`, `docs/spikes/`, and `docs/reports/`; ambient-recall benchmarks and the Codex factory startup SIGILL report live there.
- **Docs:** `docs/specs/`, `docs/requests/`, `docs/guides/`, `docs/onboarding/`, `docs/reviews/`, `docs/brainstorms/`, and `docs/ideation/` organize durable planning material.
- **Config:** `.claude/settings.json`, `.codex/config.toml`, `.mcp.json`, root `Cargo.toml`, and `.env.worktree.template`.
- **Generated/local state:** `target/`, `node_modules/`, and worktree-local CAS state are intentionally not source-map entries.

## Entrypoints
- CLI: `cas-cli/src/main.rs` → `cas`.
- Library: `cas-cli/src/lib.rs` → crate `cas`.
- Factory TUI: `cas-cli/src/ui/factory/app/mod.rs` → bare `cas`.
- Factory daemon: `cas-cli/src/ui/factory/daemon/` → `cas factory` runtime.
- MCP hub: `cas-cli/src/mcp/daemon.rs` → `cas serve`.
- HTTP bridge: `cas-cli/src/bridge/server/` → `cas bridge serve`.
- Slack bridge: `slack-bridge/src/{router-main,daemon-main}.ts` → npm `start:*` scripts.
- Hooks: `cas-cli/src/cli/hook.rs` → `cas hook <event>`.
- Tests: `cargo test -p cas --lib` (fast) or `cargo test --workspace --no-fail-fast` (workspace gate).
