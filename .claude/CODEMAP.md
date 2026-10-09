# cas — Codemap

> Auto-generated structural map. Regenerate with `/codemap` when the layout drifts (modules added, removed, or renamed).

Rust workspace for the CAS coding-agent system. Product/domain material belongs in `docs/PRODUCT_OVERVIEW.md`; this file is a navigational index.

## Top-level layout

- `.cargo/` — workspace Cargo configuration and platform build settings.
- `.claude/` — rendered Claude agents, settings, workflows, and this codemap.
- `.codex/` — rendered Codex agents, hooks, and local configuration.
- `.config/` — checked-in test-runner configuration (`nextest.toml`).
- `.github/` — CI/release workflows, reusable actions, and public issue templates.
- `cas-cli/` — binary crate composing the CLI, MCP server, hooks, cloud sync, and factory.
- `contrib/` — installable shell helpers and their test harness.
- `crates/` — shared Rust libraries for storage, search, factory, terminal, MCP, and types.
- `docs/` — architecture, design, operations, research, release, and project records.
- `fixtures/` — checked-in retrieval-parity baselines and query sets.
- `homebrew/` — Homebrew formula and formula update helper.
- `hub-web/` — TypeScript/Vite Commander web client, Vitest units, and Playwright journeys.
- `migration/` — historical cloud-move logs, reports, and systemd material.
- `ops/` — deployable systemd units and launch wrappers.
- `scripts/` — install, release, CI policy, scoped-test, worktree, and build-cache helpers.
- `site/` — static project landing page and system PDF.
- `slack-bridge/` — standalone Node/TypeScript Slack router and per-user daemon.
- Root config/docs — `Cargo.toml`, `README.md`, `CLAUDE.md`, `AGENTS.md`, `CONTRIBUTING.md`, and `.mcp.json`.

## Workspace / packages

- `cas-cli` — binary `cas`; composes all service crates and owns user-facing commands.
- `crates/cas-types` — shared domain, wire, provenance, task, agent, memory, hook, and verification types.
- `crates/cas-store` — SQLite stores, queues, history, knowledge, archives, verification, and vector persistence.
- `crates/cas-search` — hybrid BM25/semantic retrieval, code search, LMDB persistence, grep, and scoring.
- `crates/cas-core` — hook contexts/transcripts, memory hygiene, temporal search, extraction, dedup, and sync.
- `crates/cas-code` — multi-language code analysis, parsing, chunking, and indexing support.
- `crates/cas-mcp` — MCP daemon configuration, protocol types, and server support.
- `crates/cas-mcp-proxy` — policy-aware upstream MCP proxy engine and health tracking (`code-mode-mcp`).
- `crates/cas-factory` — worker spawning, provider/lane routing, spec resolution, directors, probes, and sessions.
- `crates/cas-factory-protocol` — factory client/server wire protocol, codecs, compression, and transport.
- `crates/cas-mux` — in-process terminal multiplexer, pane routing, harness backends, and injection.
- `crates/cas-pty` — PTY creation/configuration and Claude, Codex, Grok, and OpenCode conformance.
- `crates/cas-recording` — asciinema-style terminal recording format, readers, writers, and export.
- `crates/cas-diffs` — diff parsing, inline rendering, widgets, and syntax highlighting.
- `crates/cas-operator-crypto` — HPKE/JWS operator-envelope crypto shared with Cloud; `tests/interop.rs` pins cross-implementation fixtures.
- `crates/cas-tui-test` — PTY-backed TUI runner, screen assertions, input sequences, and artifacts.
- `crates/ghostty_vt` and `crates/ghostty_vt_sys` — safe Rust terminal wrapper and low-level Ghostty FFI.

## cas-cli/src — application hub

`cas-cli/src/{main.rs,lib.rs}` start and export the CLI; `cas-cli/src/lib.rs` owns the `panic = "unwind"` guard and test-environment boundaries.

- `cas-cli/src/cli/` — clap dispatch for artifact, auth, cloud, config, factory, hub, knowledge, memory, provider, status, update, and worktree flows.
- `cas-cli/src/cli/factory/` — factory lifecycle, daemon attach, probes, parity checks, queries, worktrees, and wedged-worker recovery.
- `cas-cli/src/cli/{hook,sync}/` — hook event dispatch, generated hook configuration, and managed agent-file rendering.
- `cas-cli/src/cli/{codemap_cmd,project_overview_cmd,knowledge_cmd}.rs` — documentation freshness gates and knowledge operations.
- `cas-cli/src/cli/{history_cmd,index_cmd,retrieval_parity}.rs` — Git history search, code indexes, and retrieval parity commands.
- `cas-cli/src/cli/integrate/{violet,violet_retirement}.rs` — `cas integrate violet` (renamed from `mecha_cassy.rs`); `cas-cli/src/cli/doctor/slack_transport.rs` checks the Slack route.
- `cas-cli/src/cli/{jev,hub_operator,store_choice}.rs` — `cas jev`, operator-inbox commands, and store selection.
- `cas-cli/src/artifacts/` — publishable-path guard, streaming digest, and the Cloud begin/PUT/complete upload client for published artifacts.
- `cas-cli/src/cloud/` — cloud sync, devices, teams, proposals, embeddings, aliases, and queued push/pull; `cas-cli/src/cloud/sync_queue/unauthored.rs` ledgers pulled rows this project did not author.
- `cas-cli/src/config/` — settings, runtime hooks, access policy, metadata registry, and seeded coordination/daemon/history/QA sections.
- `cas-cli/src/daemon/` — background maintenance, decay, observation, source watching, indexing, and bounded relevance evaluation.
- `cas-cli/src/history/` — incremental Git history index, changelog/refs, FTS search, provenance, symbols, and epochs.
- `cas-cli/src/hooks/handlers/` — session-start context/budget/hygiene, PreToolUse/PostToolUse, stop handling, and issue triage.
- `cas-cli/src/hooks/handlers/handlers_events/{neon_sql_guard,publication_gate,slack_transport,browser_tier_guard}.rs` — Neon write block, publish-before-verify gate, Violet-only Slack writes, browser-test tiering.
- `cas-cli/src/{hybrid_search,knowledge}/` — lexical/semantic/code composition, caches, knowledge-source selection, and distillation.
- `cas-cli/src/{store,migration}/` — layered stores/sync wrappers plus numbered SQLite migrations and migration orchestration.
- `cas-cli/src/worktree/` — discovery, Git operations, target locking, external links, salvage, sweep, and cleanup.
- `cas-cli/src/bridge/server/` — HTTP/SSE bridge handlers used by `cas bridge serve`; `cas-cli/src/hub/` owns Commander pairing and fleet runtime.
- `cas-cli/src/hub/operator_inbox/` — signed operator-message inbox (`jws.rs`, `wire.rs`, `drain.rs`, `machine.rs`); `cas-cli/src/hub/{connection_recovery,launch_env,projects,observation}.rs` and `hub/auth/` cover reconnects, session launch, and installation accounts.
- `cas-cli/src/mcp/` — always-available daemon, Unix socket, MCP request routing, prompts, resources, and tool handlers.
- `cas-cli/src/{orchestration,notifications,telemetry,tracing}/`, `cas-cli/src/{otel,sentry}.rs` — coordination and observability plumbing.
- `cas-cli/src/{agent_id,capability,harness_policy}.rs` — identity, capability, and provider-harness policy boundaries.
- `cas-cli/src/factory_{context_reset,isolation,preflight,session_scope,worker_check,hook_canary}.rs` — factory safety, session scoping, and worker/hook health checks.
- `cas-cli/src/factory_target_cache/` — per-worker build-cache seeding (`lane.rs`, `owner.rs`, `parked.rs`, `retirement.rs`, `scratch.rs`).
- `cas-cli/src/factory_daemon_health.rs` — daemon loop-health file readable outside the daemon; `cas-cli/src/factory_permission_relay.rs` relays worker permission prompts parked for a team lead.
- `cas-cli/src/{qa_pass,qa_evidence,qa_journeys}.rs` + `cas-cli/src/qa_pass/{github_gate,preflight}.rs` — independent QA-pass eligibility, journey selection, and close-time evidence checks.
- `cas-cli/src/jev/` — Jev calibrated decisions (`gate.rs`, `files.rs`); `cas-cli/src/git_evidence/` — shared git measurement for close gates.
- `cas-cli/src/{github_issue_attach,github_repo,task_assignment,light_lane}.rs` — cited-issue attach, repo resolution, assignment wake, and one-shot light-lane work.
- `cas-cli/src/{db_branch,ops/fleet,maintenance_jobs,server_signals,review_body}.rs` — per-task Neon branches, fleet ops, scheduled jobs, server signals, PR review bodies; `store/foreign_project_guard.rs` blocks cross-project writes.
- `cas-cli/src/retrieval_eval.rs` and `cas-cli/src/retrieval_parity/` — labeled scoring against committed retrieval fixtures and diffs.

## Factory coordination surfaces

- `cas-cli/src/ui/factory/app/` — bare-`cas` TUI state, panels, selection, rendering, worker/epic views, and worktree actions.
- `cas-cli/src/ui/factory/director/` — mission/task/worker coordination, prompts, reminders, events, radar, and supervisor-stall tests.
- `cas-cli/src/ui/factory/daemon/` — PTY-owning daemon; `cas-cli/src/ui/factory/daemon/runtime/` covers lifecycle, delivery, CI watch, relay, teams, queue/events, and merge sweep.
- `cas-cli/src/ui/factory/daemon/runtime/{loop_watchdog,commander_mirror,send_dedupe,terminal_exchange}.rs` — loop watchdog, supervisor-answer mirror into Commander, send dedupe, and PTY exchange.
- `cas-cli/src/ui/factory/server_registry/docker.rs` — registered servers backed by `docker run` containers.
- `cas-cli/src/ui/factory/{boot,client,protocol,server_registry,session}.rs` — startup, client transport, protocol, server lifecycle, and session state.
- `crates/cas-factory/src/{routing,spec_resolver,probe,director,config}.rs` — provider/lane registry, worker specs, probes, directors, and configuration.
- `crates/cas-factory/src/session/` — worker session lifecycle, resume state, and focused session tests.
- `crates/cas-factory/policy/lane-registry.toml` — checked-in provider lanes and capability registry consumed by routing.

## MCP service and tool tree

- `cas-cli/src/mcp/{daemon.rs,socket.rs,server/}` — daemon lifecycle, Unix transport, runtime, parent watchdog, prompts, and resources.
- `cas-cli/src/mcp/tools/core/` — task, memory, knowledge, artifact, search, rules, skills, workflow, system, opinion, maintenance, and coordination handlers.
- `cas-cli/src/mcp/tools/core/guidance.rs` — request-local caller and supervisor prefixes for executable recovery hints.
- `cas-cli/src/mcp/tools/core/task/` — task queries, notes, proposals, dependencies, updates, and lifecycle proof/close gates.
- `cas-cli/src/mcp/tools/core/task/lifecycle/{qa_dispatch,qa_evidence_gate}.rs` — independent-QA dispatch at merge park and the QA evidence close gate.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/` — close gates incl. `evidence_only.rs`, `snapshot_approval.rs`, `delivery_evolution.rs`, `epic_verdict_cache.rs`; `task/integration_batch.rs` pins batch-squash deliveries.
- `cas-cli/src/mcp/tools/core/{jev.rs,workflow/shadow_review/,agent_coordination/branch_adoption.rs}` — Jev tool, shadow Spec/Standards review, and task branch adoption.
- `cas-cli/src/mcp/tools/service/` — factory/reminder ops, liveness, orphan recovery, external verification, server/worktree ops, patterns/specs, and panic containment.
- `cas-cli/src/mcp/tools/service/qa_pass_ops.rs` — `verification action=qa_record|qa_waive|qa_status`.
- `cas-cli/src/mcp/tools/service/{db_branch_ops,mutation_receipt,tool_schema}.rs` — Neon branch actions, idempotent mutation receipts, and served tool schemas.
- `cas-cli/src/mcp/tools/service/recovery_guidance_tests.rs` — recovery-hint contracts across Claude, Codex, Grok, and OpenCode callers.
- `cas-cli/src/mcp/tools/service/agent_search_system/` — agent search, code/history/context retrieval, messaging, and supervisor queue operations.
- `cas-cli/src/mcp/tools/service/worker_liveness/` — worker receipt parsing and liveness tests; `cas-cli/src/mcp/tools/service/opencode_liveness.rs` covers OpenCode probes.
- `cas-cli/src/mcp/tools/types/` — shared request/response schemas for task, search, system, looping, rules/skills, verification, and worktrees.
- `cas-cli/src/mcp/tools/{mod.rs,mod_tests.rs,traffic_limits.rs}` — tool registration, action-surface tests, and dispatch limits.

## Builtins, tests, and supporting clients

- `cas-cli/src/builtins/` — embedded managed prompts, agents, skills, harness mirrors, and `cas-cli/src/builtins/reference-history.json` sync manifest.
- `cas-cli/src/builtins/agents/task-verifier.body.md` — the only managed agent; `cas-cli/src/builtins/jobs/` — duplicate, learning, rule, and session-summary job prompts (formerly agents).
- `cas-cli/src/builtins/codex/skills/` — Codex-only skills (e.g. `cas-codex-supervisor-checklist.md`, `cas-retro/`); the Grok tree was removed.
- `cas-cli/src/builtins/skills/` — canonical shared skills (e.g. `cas-qa-craft/`, `cas-supervisor/`, `violet/`, `cas-release-notes/`, `cas-jev/`, `cas-shadow-review/`), references, scripts, and assets rendered to harness mirrors.
- `cas-cli/tests/` — integration targets for CLI, hooks, cloud, factory/MCP, hub, search, verification, e2e, and multi-agent behavior.
- `cas-cli/tests/hooks_test/main.rs` — integration-test entrypoint that includes the hooks test module tree.
- `cas-cli/tests/mcp_tools_test/` — MCP action coverage; `cas-cli/tests/mcp_tools_test/task_tools/` holds lifecycle, dependency, close-gate, QA (`independent_qa.rs`, `qa_evidence_gate.rs`), cited-issue, and verification suites.
- `cas-cli/tests/{pull_authorship,credential_debug_guard}_test.rs` — unauthored-pull ledger and credential-leak debug-output guards.
- `cas-cli/tests/e2e/` — factory, hooks, memory/rules, multi-agent, tasks, teams, verification, and worktree flows.
- `cas-cli/tests/{factory_parity,factory_codex_skill_guardrails,factory_mcp_ops}_test.rs` — factory parity and worker-facing contract gates.
- `cas-cli/tests/{builtin_archive_portability,builtin_doc_hygiene,agent_definition_contract,skill_hygiene}_test.rs` — managed prompt/skill hygiene gates.
- `cas-cli/tests/{retrieval_eval,retrieval_parity,project_identity_parity}_test.rs` — retrieval and cross-surface parity checks.
- `cas-cli/tests/{common,e2e,fixtures,support,snapshots,proptest}/` — shared fixtures (mock Neon MCP, mock Tailscale), `support/hub_fixture.rs`, snapshots, and property tests.
- `hub-web/src/` — Commander SPA state, pairing, sessions, panes, attention, messaging, terminal adapters, and colocated `*.test.ts` Vitest tests.
- `hub-web/src/{thread-model,conversation-*,markdown-renderer,composer-markup,attachment-sheet}.ts` — conversation thread model, rendering, composer, and attachments.
- `hub-web/src/{attention-objects,context-rail,palette-commands,session-connection,refusal,toast-placement,machine-accent}.ts` — attention, rail, command palette, connection state, and chrome.
- `hub-web/src/{build.d.ts,paired-machines.ts,pair-dialog-markup.ts}` — injected Hub build identity, paired-machine footer/register, and pair dialog.
- `hub-web/src/inbox/` — operator inbox client: HPKE envelopes (`hpke.ts`), hub enrollment, replay, projection, and `inbox-view.ts`.
- `hub-web/src/{fleet-ops*,installation-*,launch-session,connection-diagnostics,event-recovery,conversation-store}.ts` — fleet operations, installation inventory, session launch, and connection recovery.
- `hub-web/e2e/journeys/` — Playwright user journeys (`*.journey.ts`, `hub-double.ts` fake hub, `serve-dist.mjs`); `hub-web/e2e/generated/` — agent-generated specs.
- `hub-web/{playwright.config.ts,playwright.real-hub.config.ts,playwright.responsive.config.ts,specs/}` — Playwright configs and test plans; `hub-web/{.claude,.codex}/agents/` hold planner/generator/healer agents.
- `hub-web/*.brief.md` (polish, composer-voice, conversation-layout, pane-activity, …) — concept briefs with acceptance budgets and visual-QA handoff.
- `hub-web/preview/` + `vite.preview.config.ts` — standalone preview build; `hub-web/scripts/{run-verified-tests,check-journey-clock}.mjs` gate test runs.
- `slack-bridge/src/` — Slack router/daemon entrypoints, commands, sessions, filtering, formatting, and tests.

## Crates — key module roots

- `crates/cas-store/src/{task_store,prompt_queue_store,supervisor_queue_store,spawn_queue_store}.rs` — durable task and coordination queues; `prompt_queue_store/{operator_delivery,operator_cloud,device_receipts}.rs` back the operator outbox (migrations m263–m265).
- `crates/cas-store/src/agent_store/` plus `crates/cas-store/src/{knowledge_store,history_store,code_vector_store,retrieval_store}.rs` — agents, knowledge, history, vectors, and retrieval outcomes.
- `crates/cas-store/src/{verification_store,external_verification_gate,surfaced_artifact_store,version_store}.rs` — verification, injected-context artifacts, and rule/skill versions.
- `crates/cas-store/src/qa_pass_store.rs` + `crates/cas-types/src/qa_pass.rs` — QA-pass rounds, no-self-review rule, and record types.
- `crates/cas-store/src/artifact_store.rs` — published-artifact ledger and its signed-upload-URL guard.
- `crates/cas-core/src/{hooks,memory,search/temporal,sync,extraction}/` — hook input, memory hygiene, temporal search, managed sync, and extraction; `hooks/wire_contract.rs` pins harness hook payloads, `memory/handoff.rs` session handoffs, `env_overlay.rs` env layering.
- `crates/cas-search/src/{bm25,code_search,lmdb_store,grep,parallel,scorer,traits}.rs` — text/code indexes, persistence, grep, retrieval parallelism, and scoring.
- `crates/cas-types/src/{provenance,task,agent,delivery,verification,spec}.rs` — lineage and core coordination records; `violet_compatibility.rs` (+ `violet-compatibility.json`) pins the Violet tool contract, `factory_worker_policy.rs` worker policy.
- `crates/cas-mcp/src/{daemon.rs,types/}` — embedded MCP daemon lifecycle and protocol/type definitions.
- `crates/cas-mux/src/{backend,pane}/` plus `crates/cas-mux/src/{mux,pty,render,harness}.rs` — terminal backends and pane operations; `crates/cas-mux/tests/claude_factory_contract_runtime.rs` pins the Claude worker contract.
- `crates/cas-pty/{src,conformance}/` — runtime adapters and pinned harness contract receipts; `src/claude_trust.rs` pre-accepts Claude workspace trust. `crates/cas-mux/src/worker_resources.rs` — per-worker resource telemetry.
- `crates/{cas-recording,cas-diffs,cas-tui-test}/src/` — terminal recordings, diff widgets, and TUI test support.
- `crates/ghostty_vt_sys/{build_support.rs,tests/portable_target.rs}` — FFI build support and portable-target enforcement.
- `crates/*/{tests,benches}/` plus inline `#[cfg(test)]` modules provide lower-level integration and unit coverage.

## Cross-cutting

- **Managed files:** sources live in `cas-cli/src/builtins/`; sync renders the `.claude/` and `.codex/` mirrors (`.grok/` and `.opencode/` are rendered locally, not checked in).
- **Docs:** `cas-cli/docs/` holds architecture/contributing/migration/proxy/TUI/worktree material; `docs/` holds durable project records.
- **CI/release:** `.github/`, `scripts/` (incl. `release-train.d/`, `release-{completion,interventions,learning,integration-gates}.py`, `assembly-proof.py`, `check-changelog-lint.sh`), `CHANGELOG.md`, `docs/release-notes/`, and `docs/release-reports/` hold gates and receipts.
- **CI tiers and guards:** `scripts/ci_tiers/` + `scripts/ci-test-impact.py` route test lanes; `scripts/check-{test-shape,test-env,builtin-contract-phrases,violet-references}.py` are repo-policy checks with `scripts/test-*.py` self-tests.
- **Visual QA:** `scripts/visual-qa-fixtures/` and `scripts/visual-qa-*.test.mjs` cover the `cas-ui-craft` scanner; `scripts/violet-credentials.sh` loads Violet tokens.
- **Journey QA:** `docs/qa/{journeys,journey-evaluation,evidence-close-gate,independent-qa-pass}.md` define journeys and QA gates; `scripts/{journeys-for-diff.py,journey-eval.sh,check-journey-evaluation.sh,test-journeys.sh}` select and evaluate them; receipts in `docs/qa/journey-evaluations/`.
- **Design/reports:** `docs/design/` (incl. `hub-messaging/round-*` concept rounds, `cli/`), `docs/factory/` model-lane rubrics + `data/`, `docs/reports/` harness-diary reports.
- **Operations:** `migration/`, `ops/systemd/`, `docs/branch-protection/`, `docs/ci/`, and `docs/factory/` hold host/runbook material.
- **Tests:** colocated Rust tests, crate `tests/`, `cas-cli/tests/`, Vitest suites under `hub-web/src/`, and Playwright under `hub-web/e2e/`.
- **Test isolation:** use `TestEnvGuard` from `cas-cli/src/lib.rs`; do not add a second HOME/environment helper.
- **Generated/local state:** `target/`, `dist/`, `node_modules/`, `hub-web/dist/`, `.cas/`, and `vendor/` are not source-map entries.

## Entrypoints

- CLI: `cas-cli/src/main.rs` → `cas`.
- Library: `cas-cli/src/lib.rs` → crate `cas`.
- Factory TUI: `cas-cli/src/ui/factory/app/mod.rs` → bare `cas`.
- Factory daemon: `cas-cli/src/ui/factory/daemon/mod.rs` → `cas factory` runtime.
- MCP hub: `cas-cli/src/mcp/daemon.rs` → `cas serve`.
- HTTP bridge: `cas-cli/src/bridge/server/` → `cas bridge serve`.
- Commander web: `hub-web/src/main.ts` → Vite bundle served by `cas hub`.
- Slack bridge: `slack-bridge/src/{router-main,daemon-main}.ts` → npm `start:*` scripts.
- Hooks: `cas-cli/src/cli/hook.rs` → `cas hook <event>`; setup: `cas-cli/src/cli/setup.rs` → `cas setup`.
- Worker tests: `scripts/run-scoped-tests.sh -p cas --lib <module>`; supervisor gate: `cargo nextest run -p cas`.
- Hub tests: `hub-web` `npm test` (Vitest) and `npm run journeys` (Playwright); `scripts/test-journeys.sh` checks journey tooling.
