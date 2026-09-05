<!-- CAS:BEGIN - This section is managed by CAS. Do not edit manually. -->
# IMPORTANT: USE Cassy FOR TASK AND MEMORY MANAGEMENT

**DO NOT USE BUILT-IN TOOLS (TodoWrite, EnterPlanMode) FOR TASK TRACKING.**

Use CAS MCP tools instead:
First use each session — load MCP schemas: ToolSearch(query="select:mcp__cas__task,mcp__cas__memory,mcp__cas__search"). ToolSearch only loads the schema — it does not call the tool. Once it succeeds, call `mcp__cas__task` etc. directly; never re-run ToolSearch for a tool already resolved.
- `mcp__cas__task` with action: create - Create tasks (NOT TodoWrite)
- `mcp__cas__task` with action: start/close - Manage task status
- `mcp__cas__task` with action: ready - See ready tasks
- `mcp__cas__memory` with action: remember - Store memories and learnings
- `mcp__cas__search` with action: search - Search all context

Cassy provides persistent context across sessions. Built-in tools are ephemeral.

Bug routing: `cas config get issues.repo` / `issues.components.{cassy,mecha_cassy,cloud}` name the project, Cassy, MechaCassy and Cloud trackers; file operational bugs in the matching repo before moving on.
Release notes: when a merge reaches `staging` or `main`, use the `release-notes` skill and follow docs/release-notes/RUBRIC.md.
<!-- CAS:END -->

<!-- codex-only:start -->
<!--
## Codex-specific notes

- Use the `mcp__cs__` CAS MCP tools. Codex sessions do not run the Claude hook
  prefix translator, so this generated AGENTS.md carries the Codex tool prefix
  directly.
- Codex does not support Claude hooks. Follow the factory worker lifecycle and
  let the supervisor own verification and review flow.
-->
<!-- codex-only:end -->

# CLAUDE.md

## Build & Test

```bash
cargo build                          # Dev build
cargo build --release                # Release build (LTO, strip)
cargo build --profile release-fast   # Fast release (thin LTO, 16 codegen units)
cargo check -p cas --lib --tests     # Worker iteration: compile feedback, no test linking/runs
scripts/run-scoped-tests.sh -p cas --lib module_name
scripts/run-scoped-tests.sh -p cas --test cli_test
cargo nextest run -p cas             # Full suite: supervisor integration/release gates only
cargo test -p cas --doc              # Doctests (nextest does not support them)
cargo bench --bench code_indexing    # Benchmarks
make test-release-panic              # Verify A2/A3/B3 panic isolation under release profiles
```

Install the standard local runner once with `cargo install cargo-nextest` (or
`make -C cas-cli install-tools`). `scripts/run-scoped-tests.sh` defaults to
nextest and rejects a silent zero-test success. Factory workers should iterate
with `cargo check`, then run only the affected `--lib` or `--test` target; the
PreToolUse guard rejects an unscoped worker test run. Full suites are owned by
the supervisor integration merge and release gate.

Gate evidence: PR #655/run 33430464567; PR #657/run 33435093275.

Factory worker spawns use `sccache` automatically when it is installed, while
keeping a separate target directory per worktree so concurrent Cargo builds do
not serialize. An existing `RUSTC_WRAPPER` wins; set
`CAS_FACTORY_DISABLE_SCCACHE=1` for the emergency opt-out. CI uses the GitHub
cache-v2 backend and keeps the cold Build Benchmark explicitly uncached.

New isolated workers also seed their private `target/` from compiled artifacts
hardlinked out of the quiescent snapshot named by `.cas/build-cache/current`;
small Cargo dep-info files are copied with their target root rebased. Refresh that
baseline after an epic/main integration merge with
`scripts/refresh-worker-build-cache.sh`; the script builds a new snapshot to
completion and only then publishes its pointer, so no worker ever seeds from a
live Cargo writer. Old snapshots remain valid for in-flight seeders and should
only be removed during a maintenance window. Set
`CAS_FACTORY_DISABLE_TARGET_SEED=1` to skip seeding. Do not replace this with a
shared live `CARGO_TARGET_DIR`: its Cargo lock serializes the worker fleet.

**Standing operator CI-load policy:** factory/* pushes run only Scoped
Validation; protected-default PRs run only the required Fast Validation and
macOS Check lanes. The merge queue validates its synthetic tree once; when its
successful tree is pushed unchanged to main, the main-push Fast Validation and
macOS lanes reuse that receipt and name the validating run. Direct pushes,
bypass merges, receipt lookup failures, and changed trees still run those
lanes. The non-required full/heavy tier (Clippy, Test Compile Guard, Build
Benchmark, and both Panic Isolation profiles) belongs only to
supervisor-controlled main pushes, schedules, or manual dispatches—never
factory/*, epic/*, tags, or pull requests. Keep this policy pinned by
`scripts/test-ci-test-tiers.sh`, rather than relying on convention.

Local sccache 0.10.0 does not produce cross-worktree Rust hits because absolute
checkout paths remain in its cache keys (measured 0/45 hits even with
`--remap-path-prefix`). Keep sccache enabled for same-path/CI reuse and for when
[upstream path normalization](https://github.com/mozilla/sccache/pull/2678)
lands; hardlink seeding is the current cross-worktree mechanism.

The MCP server is always included because factory agents depend on `cas serve`; the optional `mcp-proxy` feature is enabled by default. Binary is `cas` (lib + bin in `cas-cli/`). Build script embeds git hash and build date.

**Build profiles must use `panic = "unwind"`.** The MCP tool-dispatch panic catcher (EPIC cas-c351) relies on `tokio::spawn` + `JoinError::is_panic`, which only observes a panic if the worker thread unwinds. A compile-time guard in `cas-cli/src/lib.rs` refuses non-test builds with `panic = "abort"` — do not work around it; the entire point of that catcher is to keep `cas serve` alive across handler bugs.

## Rust Version

Minimum supported Rust version: **1.88** (edition 2024).

## Architecture & Contributing

Module layout, crate purposes, store traits, CasCore, hook scoring:
-> See [cas-cli/docs/ARCHITECTURE.md](cas-cli/docs/ARCHITECTURE.md)

Adding CLI commands, MCP tools, migrations, testing setup, skill/rule sync:
-> See [cas-cli/docs/CONTRIBUTING.md](cas-cli/docs/CONTRIBUTING.md)

Codebase navigation map (breadcrumb index of all modules):
-> See [.claude/CODEMAP.md](.claude/CODEMAP.md)

<!-- claude-only:start -->
## Output hygiene — avoid Claude Code Ink crash

Claude Code's React-Ink UI throws `<Box> can't be nested inside <Text>` when streamed markdown produces a Box-in-Text layout. The process stays alive (Bun keeps the event loop) but the pane is dead — tool calls after that point never complete. Until Claude Code ships a fix (cas-97ba tracks), avoid these output shapes when responding in chat:

- **Do not echo the contents of a markdown file back in your response** after writing it with `Write` — confirm with a short prose summary instead. Streaming long generated markdown (CODEMAP.md, PRODUCT_OVERVIEW.md, skill bodies, etc.) is a common trigger.
- **Avoid nested fenced code blocks** (a ` ```markdown ` block whose contents include headings, blockquotes, bullets, or a second fence). This is the most reproducible tripwire today. Describe the inner shape in prose or use backticks for inline samples.
- **Keep fenced blocks minimal in chat output** — use them for plain shell commands or short snippets, not for richly-structured markdown previews.

Writing to disk is always safe; the risk is only when the content streams back through the Ink renderer.
<!-- claude-only:end -->

## Don't assume — always verify

When diagnosing a bug or reasoning about behavior, **verify the claim against the actual code/data before acting on it.** Trace the real path, read the real handler, confirm the symptom maps to the line you think it does. Do not propose, implement, or ship a fix on a plausible-but-unconfirmed theory. A diagnosis is only "done" when you can point at the concrete evidence (the file:line, the test output, the reproduced behavior). Environment details the user gives (OS, terminal, hardware) are clues to verify against, not facts to wave away. This applies to root-cause analysis, "this already works", "that's the harness not us", and every other confident assertion.

## CAS system bugs are in-repo fixes

This repo **is** the CAS source. When a bug is reported in the verifier, hooks, factory orchestration, MCP dispatch, the task-verifier agent, worker prompts, or built-in skills — regardless of which downstream project (gabber-studio, OpenClaw, etc.) surfaced it — the fix lands here as a Rust or markdown change via a task assigned to a worker. Do not file the bug with team-lead, do not "report upstream", do not treat cas-src IS CAS as an external dependency. Other projects consume CAS; they do not modify it. If you catch yourself wanting to escalate a CAS bug, stop and create the fix task in this repo instead.

## Releases and harness diaries → Slack (mandatory)

Runtime releases and harness-diary updates have **separate** #cas-internal publication duties. A runtime release requires two distinct top-level posts (user and dev). A diary update requires one top-level cross-harness summary with exactly three replies ordered **Grok, Claude, Codex**. If one merge contains both, publish both workflows; a diary-only merge must not pose as a runtime release. All messages use impact-first prose with no ticket IDs or internal agent/factory narration.
-> See [docs/RELEASE_SLACK_RUBRIC.md](docs/RELEASE_SLACK_RUBRIC.md)
