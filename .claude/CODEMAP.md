# cas — Codemap
> Auto-generated structural map. Regenerate with `/codemap` when the layout drifts (modules added, removed, or renamed).

Rust workspace (edition 2024, MSRV 1.85): one binary crate (`cas-cli`) + 16 library crates. Product/domain content belongs in `docs/PRODUCT_OVERVIEW.md` (`project-overview` skill) — this file is structure only.

## Top-level layout
- `cas-cli/` — binary crate `cas`; CLI subcommands, hooks, factory TUI, MCP server, bridge HTTP server, daemon
- `crates/` — 16 workspace library crates (see Workspace section)
- `docs/` — planning artifacts: `requests/` (BUG/FEATURE inbox + `completed/` archive; being retired in favour of GitHub Issues), `release-notes/`, `reports/` (durable incident/diagnosis writeups), `notes/`, `guides/`, `brainstorms/`, `ideation/`, `reviews/`, `spikes/`, `onboarding/`
- `scripts/` — `release.sh`, `bump-release-version.sh`, `cas-install.sh`, `bootstrap-zig.sh`, `check-build-regression.sh`, `benchmark-build.sh`, `install-git-hooks.sh`, `worktree-boot.sh`, `provision-hetzner.sh`
- `migration/` — one-shot cloud-move phase logs. Not active build infra (schema migrations live in `cas-cli/src/migration/`)
- `homebrew/` — `cas.rb` formula + `update-formula.sh`
- `slack-bridge/` — standalone Node/TS Slack relay service (own `package.json`)
- `site/` — static landing page (`index.html`, PDF)
- `vendor/ghostty` — vendored libghostty-vt C source
- `.context/zig` — gitignored pinned Zig toolchain, created by `scripts/bootstrap-zig.sh`; `export ZIG=$PWD/.context/zig/zig` before any cargo build pulling ghostty-vt
- `.claude/` — `CODEMAP.md` (tracked; commit after `/codemap` to reset the git-history freshness gate), `settings.json`, plus `agents/`/`skills/`/`workflows/` which are sync **output** of `cas integrate`
- `.codex/` — Codex CLI mirror (`agents/`, `skills/`, `config.toml`) built from `.claude/`
- `.cas/` — project SQLite DB (`cas.db`), logs, `worktrees/` (isolated factory worker checkouts). Gitignored; see the store caveat under Cross-cutting
- `.github/` — `workflows/{ci,release}.yml` and `ISSUE_TEMPLATE/{bug_report,feature_request}.md` + `config.yml` (public bug intake)
- `.cargo/` — cargo config
- Root: `README.md`, `CHANGELOG.md`, `CONTRIBUTING.md`, `CLAUDE.md`, `CAS-DEEP-DIVE.md`, `LICENSE`, `Cargo.toml`, `.mcp.json`, `.env.worktree.template`

## Workspace / packages
Binary lives in `cas-cli`; everything else is a library consumed by it. Release profiles enforce `panic = "unwind"` — a compile-time guard in `cas-cli/src/lib.rs` rejects `panic = "abort"` for non-test builds (the MCP panic catcher depends on unwinding).

- `cas-cli` — binary `cas`; glue between CLI, hooks, TUI, MCP server, daemon, bridge
- `crates/cas-types` — shared types (Task, Agent, Memory, HookInput, `search_manifest.rs`, `verification.rs`, `visibility.rs`)
- `crates/cas-store` — SQLite storage layer, schema, store traits
- `crates/cas-search` — hybrid BM25 + semantic search over memories/tasks/code
- `crates/cas-core` — business logic and hook context computation
- `crates/cas-code` — code indexing and symbol search
- `crates/cas-mcp` — MCP protocol types/handlers; `types/ops_secondary.rs` holds `CoordinationRequest`
- `crates/cas-mcp-proxy` — MCP proxy engine
- `crates/cas-factory` — spawn pipeline, spec resolution, availability probe, director detection
- `crates/cas-factory-protocol` — wire types for factory client↔server messaging
- `crates/cas-mux` — terminal multiplexer owning every factory PTY pane
- `crates/cas-pty` — PTY management; `PtyConfig::claude` / `PtyConfig::codex`
- `crates/cas-recording` — asciinema-style terminal recording
- `crates/cas-diffs` — diff parsing, rendering, syntax highlighting
- `crates/cas-tui-test` — PTY-based TUI test framework
- `crates/ghostty_vt` / `ghostty_vt_sys` — safe wrapper + FFI bindings to libghostty-vt

## cas-cli/src — CLI surface (`cli/`)
`cli/mod.rs` is the clap dispatch root.
- `cli/factory/` — `cas factory` subtree: `mod.rs` (builds `FactoryConfig`, launches daemon), `wedged.rs` (`is-wedged`/`debug`/`kill` liveness triage + `transcript_mtime_age`), `parity.rs` (skill/instruction parity gate), `probe_comm/` (end-to-end comms probe), `lifecycle.rs`, `queries.rs`, `worktree_ops.rs`, `cloud_attach.rs`, `remote_attach.rs`
- `cli/cloud.rs` — `cas cloud` push/pull/sync. Historical pull here caused cross-project store contamination; see `docs/reports/2026-08-03-task-store-contamination-cas-de89.md`
- `cli/integrate/` — `cas integrate <platform>` (Vercel/Neon/GitHub); `lock.rs` holds `IntegrateLock`
- `cli/init/`, `cli/config/`, `cli/config_tui/` — project init, config read/write, config TUI. `config/settings.rs` holds `STOCK_WORKER_{HARNESS,MODEL,REASONING_EFFORT}` and `FactoryConfig.strict_cli`
- `cli/update/`, `cli/hook/`, `cli/hook_tests/` — `cas update` atomic rewrite of `managed_by:cas` files; hook install/inspect + golden-JSON tests
- `cli/codemap_cmd.rs`, `cli/project_overview_cmd.rs` — freshness-gate subcommands
- `cli/{doctor,status,list,queue,open,known_repos,memory,mcp_cmd,bridge,changelog,claude_md,auth,device}.rs`

## cas-cli/src/mcp — MCP server
Tool dispatch for `mcp__cas__*`; each call is panic-isolated via `tokio::spawn` + `JoinError::is_panic`.
- `mcp/{daemon,socket,mod}.rs`, `mcp/server/` — lifecycle, unix socket, request routing. `daemon.rs` also drives scheduled cloud sync
- `tools/core/task/lifecycle/close_ops.rs` — the close gates: commit-claim, zero-commit routing, zero-diff guard, per-task and per-epic merge-state gates, `park_task_awaiting_merge` (writes `factory_branch_anchor`)
- `tools/core/task/lifecycle/{stale_close_guard,supervisor_push}.rs` — post-close guards, transition→supervisor push seam
- `tools/core/agent_coordination/` — register/whoami/messaging, `agent_management.rs` (lease-history renderer), task claiming + supervisor force-transfer
- `tools/core/workflow/worktree_ops.rs` — `worktree_merge` target resolution: explicit task → assignee tasks → `allow_trunk` → refuse. Session `focus_epic` is deliberately **not** merge authority (cas-b86e)
- `tools/core/{memory,search,rules,skills,system,knowledge,maintenance}.rs`
- `tools/service/factory_ops.rs` — `worker_status`, `worker_activity`, `spawn_workers`, `epic_status`, `focus_epic`; Codex rollout resolution + activity/in-flight/context signals
- `tools/service/harness_observation.rs` — artifact-backed turn observations; asymmetric evidence model (Codex turn_id correlation vs Claude inbox-only)
- `tools/service/agent_search_system/message.rs` — `message` / `message_status` and delivery-report formatting
- `tools/service/{agent_liveness,orphan_recovery,factory_remind,pattern_ops,spec_ops,worktree_verification_team_ops}.rs`, `panic_catch.rs`

## cas-cli/src/ui/factory — factory TUI
Rust TUI over an in-process PTY mux (not tmux); `cas` with no subcommand launches it.
- `factory/daemon/runtime/` — `queue_and_events.rs` (prompt-queue drain, generation-scoped spawn cancellation), `delivery.rs` (`deliver_to_worker` → `Mux::inject`; shared by normal and urgent paths), `teams.rs`, `ws_client.rs`, `gui_client.rs`, `fork_first.rs`
- `factory/daemon/process.rs` — daemon entry paths; `open_log_file_truncate` does a truncate open then a separate O_APPEND reopen (append+truncate is EINVAL on every platform)
- `factory/app/` — `FactoryApp` state; `render_and_ops/epic_workers.rs` (spawn/shutdown, task pre-assign/release), `sidecar_and_selection.rs` (alt-screen scroll forwarding), `branch_visibility.rs`
- `factory/director/` — `events.rs` (`DirectorEvent`, `WorkerStalled` detection, `held_workers` idle/stall suppression gate read at ~:1001), `prompts.rs` (assignment/idle/stall prompt text, delivery-time revalidation)
- `factory/{boot,renderer,protocol,layout,input,client,client_input,status_bar,notification,session,phoenix}.rs`
- `ui/{components,widgets,markdown,theme}/` — shared TUI primitives

## cas-cli/src — other subsystems
- `hooks/handlers/handlers_events/` — SessionStart/PreToolUse gates, codemap + project-overview freshness, `attribution.rs` (PostToolUse commit attribution, writes task anchors)
- `hooks/handlers/handlers_middle/` — post-tool, session-stop, session hygiene/WIP banner
- `builtins/` + `builtins.rs` — embedded skills/agents/workflows and the sync gate. **Three harness trees ship**: `builtins/skills/` (Claude), `builtins/codex/`, `builtins/grok/` — each with its own `cas-supervisor`/`cas-worker` copies, registered as `{CLAUDE,CODEX,GROK}_BUILTIN_SKILLS`. Also `builtins/agents/`, `builtins/workflows/` (`cas-code-review.js`). A managed skill body owns its `references/` directory for sync purposes
- `migration/migrations/` — numbered schema migrations `m1xx`–`m215`; recent: `m212_worker_delivery_transactions`, `m213_verification_proof_boundaries`, `m214_known_repo_bindings`, `m215_verifier_server_handoffs`. Pattern is nullable column + idempotent `detect`
- `store/` — layered project+global store, `notifying_*`/`syncing_*` wrappers, `share_policy.rs`, `mock/`
- `cloud/syncer/` — `pull.rs` (`sync_with_sessions` drains personal push, team queue, then pull), project-scoping guards, push envelope carrying `project_canonical_id`
- `sync/`, `bridge/`, `daemon/` — builtin→`.claude/` sync, HTTP bridge, background maintenance
- `extraction/`, `consolidation/`, `hybrid_search/`, `rules/` — memory pipeline
- `telemetry/`, `tracing/`, `otel.rs`, `sentry.rs`, `logging.rs` — observability
- `worktree/` — worktree creation, salvage, sweep, cleanup
- `harness_policy.rs`, `agent_id.rs`, `duplicate_check.rs`, `error.rs`, `async_runtime.rs`

## crates — notable internals
- `cas-store/src/agent_store/` — agents + task leases; `ops_task_leases.rs`, lease history (`reason` column since m207)
- `cas-store/src/prompt_queue_store.rs` — prompt queue: enqueue, delivery stages, bounded retry/abandon, per-target progress
- `cas-store/src/sync_queue/queue_ops.rs` — sync queue upsert; `UNIQUE(entity_type, entity_id, team_id)` with ON CONFLICT resetting payload/created_at/retry_count/last_error (so `created_at` is last-touch, not first-enqueue)
- `cas-store/src/task_store.rs` — tasks are scoped by **database location**, not a column; `Scope::Project` is hardcoded at read time (~:224) and carries no provenance
- `cas-store/src/knowledge_store.rs` — distilled repo knowledge (m218): markdown bodies on disk under `.cas/knowledge/`, index + blake3 source ledger in cas.db, contentless FTS5 (`content=''`) so no column holds body prose. `commit_ingest` applies rows + index + ledger + tombstone cascade in ONE SQLite tx, with bodies staged/published via `BodyTransaction` so the filesystem rolls back with the DB. `locked=1` is user-sovereign (distillation can neither overwrite nor set it); `classify_sources` is pure
- `cas-store/src/{event_store,code_store,entity_store,file_change_store,layered,mock}.rs`
- `cas-factory/src/spec_resolver.rs` — multi-layer worker/supervisor spec cascade + `apply_codex_fallback` (Codex→Claude, no reverse)
- `cas-factory/src/probe.rs` — Codex availability probe (`codex --version` + `~/.codex/auth.json`)
- `cas-factory/src/{core,config,director,changes,notify,recording}.rs`, `session/`
- `cas-mux/src/mux.rs` — `Mux::inject` pane routing; `pane/mod.rs` — `Pane::inject_prompt` (the PTY write boundary), alt-screen tracking
- `cas-mux/src/spec.rs` — `WorkerSpec { name, cli, model, effort }`, `Effort::as_claude_arg()` / `as_codex_config()`

## Cross-cutting
- **Tests:** inline `#[cfg(test)] mod tests` per file, plus 59 integration files in `cas-cli/tests/` (factory, MCP tools, team cloud sync, bridge, search, code-review, hooks, proptest) with `common/` and `e2e/` helpers. Crate-level suites in `crates/*/tests/`. Recent additions: `pull_scoping_regression_test.rs`, `worktree_surface_test.rs`, `warning_hygiene_test.rs`.
- **Test env isolation:** ONE canonical guard — `TestEnvGuard` in `cas-cli/src/lib.rs` (`temp_home`, `with_vars`, `with_optional_vars`, `set`, `remove`, `set_current_dir`), serialized on a single lock and restoring via `Drop`. Do not add a second HOME/env helper.
- **Real-process tests:** `#[ignore]`d and run explicitly (`-- --ignored`), e.g. `crates/cas-mux/tests/{idle_pty_injection_runtime,urgent_interrupt_codex_runtime,nonurgent_idle_codex_runtime}.rs` plus `{codex,grok}_factory_contract_runtime.rs`. They serialize across binaries via the file lock in `crates/cas-mux/tests/support/real_pty_serial.rs`; live-harness helpers in `tests/support/codex_live.rs`.
- **Platform gating:** several `/proc`-based helpers are `#[cfg(target_os = "linux")]` and their callers with them — on macOS both compile out. Do not "clean up" a helper that looks dead on Darwin.
- **Docs:** `CLAUDE.md` cascades `~/CLAUDE.md` → `cas-src/CLAUDE.md`. Release notes are mandatory on every main merge — rubric at `docs/RELEASE_SLACK_RUBRIC.md`, drafts in `docs/release-notes/`.
- **Config:** `.claude/settings.json` (hooks + permissions, exec-form `args: [...]`), `.codex/config.toml`, `.mcp.json`, `.cas/config.toml` (factory knobs, `[code_review] owner = "supervisor"`; note its `[sync]` block is skill/rule rendering, not cloud sync), `~/.cas/config.toml` (user).
- **Public repo:** `pippenz/cas` is public with Issues enabled. Anything written to `docs/` or filed as an issue is world-readable — anonymize store/project inventories and never commit `.cas/cloud.json` contents.

## Entrypoints
- CLI: `cas-cli/src/main.rs` → binary `cas`
- Library: `cas-cli/src/lib.rs` (crate `cas`; also hosts `TestEnvGuard`)
- Factory TUI: `cas-cli/src/ui/factory/app/mod.rs` (bare `cas` launches this)
- MCP server: `cas-cli/src/mcp/daemon.rs` via the always-available `cas serve`
- HTTP bridge: `cas-cli/src/bridge/server/` via `cas bridge serve`
- Hook dispatch: `cas-cli/src/cli/hook.rs` via `cas hook <event>`
- Tests: `cargo test -p cas --lib` (fast), `cargo test --workspace --no-fail-fast` (gate), `cargo bench --bench code_indexing`
- Build: `cargo build --release`, then restart any running `cas serve` — factory behavior depends on the daemon matching HEAD
