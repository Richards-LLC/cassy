<!-- CAS:BEGIN - This section is managed by CAS. Do not edit manually. -->
# IMPORTANT: USE CAS FOR TASK AND MEMORY MANAGEMENT

**DO NOT USE BUILT-IN TOOLS (TodoWrite, EnterPlanMode) FOR TASK TRACKING.**

Use CAS MCP tools instead:
First use each session — load MCP schemas: ToolSearch(query="select:mcp__cas__task,mcp__cas__memory,mcp__cas__search"). ToolSearch only loads the schema — it does not call the tool. Once it succeeds, call `mcp__cas__task` etc. directly; never re-run ToolSearch for a tool already resolved.
- `mcp__cas__task` with action: create - Create tasks (NOT TodoWrite)
- `mcp__cas__task` with action: start/close - Manage task status
- `mcp__cas__task` with action: ready - See ready tasks
- `mcp__cas__memory` with action: remember - Store memories and learnings
- `mcp__cas__search` with action: search - Search all context

CAS provides persistent context across sessions. Built-in tools are ephemeral.

Release notes: every PR merged to `staging` or `main` must be announced in Slack per `docs/release-notes/RUBRIC.md` (run the `release-notes` skill; it installs the rubric if missing).
<!-- CAS:END -->

# CLAUDE.md

## Build & Test

```bash
cargo build                          # Dev build
cargo build --release                # Release build (LTO, strip)
cargo build --profile release-fast   # Fast release (thin LTO, 16 codegen units)
cargo test                           # All tests
cargo test test_name                 # Single test by name
cargo test --test cli_test           # Tests in a specific file
cargo bench --bench code_indexing    # Benchmarks
make test-release-panic              # Verify A2/A3/B3 panic isolation under release profiles
```

The MCP server is always included because factory agents depend on `cas serve`; the optional `mcp-proxy` feature is enabled by default. Binary is `cas` (lib + bin in `cas-cli/`). Build script embeds git hash and build date.

**Build profiles must use `panic = "unwind"`.** The MCP tool-dispatch panic catcher (EPIC cas-c351) relies on `tokio::spawn` + `JoinError::is_panic`, which only observes a panic if the worker thread unwinds. A compile-time guard in `cas-cli/src/lib.rs` refuses non-test builds with `panic = "abort"` — do not work around it; the entire point of that catcher is to keep `cas serve` alive across handler bugs.

## Rust Version

Minimum supported Rust version: **1.85** (edition 2024).

## Architecture & Contributing

Module layout, crate purposes, store traits, CasCore, hook scoring:
-> See [cas-cli/docs/ARCHITECTURE.md](cas-cli/docs/ARCHITECTURE.md)

Adding CLI commands, MCP tools, migrations, testing setup, skill/rule sync:
-> See [cas-cli/docs/CONTRIBUTING.md](cas-cli/docs/CONTRIBUTING.md)

Codebase navigation map (breadcrumb index of all modules):
-> See [.claude/CODEMAP.md](.claude/CODEMAP.md)

## Output hygiene — avoid Claude Code Ink crash

Claude Code's React-Ink UI throws `<Box> can't be nested inside <Text>` when streamed markdown produces a Box-in-Text layout. The process stays alive (Bun keeps the event loop) but the pane is dead — tool calls after that point never complete. Until Claude Code ships a fix (cas-97ba tracks), avoid these output shapes when responding in chat:

- **Do not echo the contents of a markdown file back in your response** after writing it with `Write` — confirm with a short prose summary instead. Streaming long generated markdown (CODEMAP.md, PRODUCT_OVERVIEW.md, skill bodies, etc.) is a common trigger.
- **Avoid nested fenced code blocks** (a ` ```markdown ` block whose contents include headings, blockquotes, bullets, or a second fence). This is the most reproducible tripwire today. Describe the inner shape in prose or use backticks for inline samples.
- **Keep fenced blocks minimal in chat output** — use them for plain shell commands or short snippets, not for richly-structured markdown previews.

Writing to disk is always safe; the risk is only when the content streams back through the Ink renderer.

## Don't assume — always verify

When diagnosing a bug or reasoning about behavior, **verify the claim against the actual code/data before acting on it.** Trace the real path, read the real handler, confirm the symptom maps to the line you think it does. Do not propose, implement, or ship a fix on a plausible-but-unconfirmed theory. A diagnosis is only "done" when you can point at the concrete evidence (the file:line, the test output, the reproduced behavior). Environment details the user gives (OS, terminal, hardware) are clues to verify against, not facts to wave away. This applies to root-cause analysis, "this already works", "that's the harness not us", and every other confident assertion.

## CAS system bugs are in-repo fixes

This repo **is** the CAS source. When a bug is reported in the verifier, hooks, factory orchestration, MCP dispatch, the task-verifier agent, worker prompts, or built-in skills — regardless of which downstream project (gabber-studio, OpenClaw, etc.) surfaced it — the fix lands here as a Rust or markdown change via a task assigned to a worker. Do not file the bug with team-lead, do not "report upstream", do not treat cas-src IS CAS as an external dependency. Other projects consume CAS; they do not modify it. If you catch yourself wanting to escalate a CAS bug, stop and create the fix task in this repo instead.

## Releases and harness diaries → Slack (mandatory)

Runtime releases and harness-diary updates have **separate** #cas-internal publication duties. A runtime release requires two distinct top-level posts (user and dev). A diary update requires one top-level cross-harness summary with exactly three replies ordered **Grok, Claude, Codex**. If one merge contains both, publish both workflows; a diary-only merge must not pose as a runtime release. All messages use impact-first prose with no ticket IDs or internal agent/factory narration.
-> See [docs/RELEASE_SLACK_RUBRIC.md](docs/RELEASE_SLACK_RUBRIC.md)
