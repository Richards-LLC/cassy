<div align="center">

<pre>
  ██████╗ █████╗ ███████╗
 ██╔════╝██╔══██╗██╔════╝
 ██║     ███████║███████╗
 ██║     ██╔══██║╚════██║
 ╚██████╗██║  ██║███████║
  ╚═════╝╚═╝  ╚═╝╚══════╝
</pre>

**Multi-agent coding factory with persistent memory.**

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/pippenz/cas/actions/workflows/ci.yml/badge.svg)](https://github.com/pippenz/cas/actions)
[![Latest Release](https://img.shields.io/github/v/release/pippenz/cas)](https://github.com/pippenz/cas/releases)

[Factory](#factory) · [Context System](#context-system) · [Knowledge](#knowledge) · [Quick Start](#quick-start) · [Installation](#installation) · [Architecture](#architecture) · [Contributing](CONTRIBUTING.md)

<img src="casdemo.png" alt="CAS Factory TUI" width="800" />

</div>

---

## What is CAS?

CAS is a multi-agent coding factory and persistent context system for AI coding agents. Three things live in one binary:

1. **Factory** — a terminal UI that runs a supervisor agent and a fleet of worker agents in parallel on one repository, each worker in its own git worktree. Workers can be Claude Code, Codex, or Grok — mixed in the same session.
2. **Context System** — an MCP server giving every agent persistent memory, tasks, rules, skills, entities and search, backed by SQLite and a Tantivy BM25 index.
3. **Knowledge** — a self-distilling wiki of the repository, built from its own docs and code, injected as a cheap index at session start and pulled on demand.

Everything is local-first. Cloud sync exists and is optional; nothing phones home when you are logged out.

## Factory

```bash
cas                   # launch the factory TUI (supervisor only)
cas -w 3              # launch with 3 workers, each in its own worktree
cas codex             # Codex as the supervisor
cas grok              # Grok as the supervisor
cas claude alt        # Claude supervisor signed in as the ~/.claude-alt account
```

A supervisor plans epics, cuts tasks, assigns them, reviews the work and merges the branches. Workers claim one task at a time, work in an isolated checkout under `.cas/worktrees/`, and report back through the shared database. The TUI shows every agent side by side (or `--tabbed`), plus a sidecar with the active epic, the task list, the live diff and an activity feed.

| Capability | How it shows up |
|---|---|
| **Worktree isolation** | Each worker gets its own worktree + branch. `--no-worktrees` shares one directory, `--worktree-root` relocates them |
| **Mixed harnesses** | `--supervisor-cli` / `--worker-cli`, or per-slot `--worker-spec '{"name":"alice","cli":"codex","effort":"high"}'` |
| **Account selection** | `cas claude <profile>` picks the Claude account directory; spawned workers inherit it. `cas claude --list-profiles` shows what's detected |
| **No silent downgrades** | `--strict-cli` refuses to quietly reroute a Codex worker to Claude when Codex is unavailable |
| **Task coordination** | Tasks carry dependencies, priorities, leases and close gates — a close is refused when the branch is unmerged or the tree is unverifiable |
| **Session control** | `cas list`, `cas attach`, `cas kill`, `cas kill-all`; `--notify` for desktop alerts, `--record` for replayable sessions |
| **Liveness triage** | `cas factory is-wedged`, `debug`, `kill` classify and recover a stuck worker; `cas factory preflight` reports readiness before spawning |
| **Headless control** | `cas factory status|agents|activity|message` drive a running session without attaching a terminal |

## Context System

`cas serve` speaks [MCP](https://modelcontextprotocol.io/). It exposes twelve umbrella tools — each dispatching dozens of actions — plus two proxy tools when upstream MCP servers are configured:

```
coordination, knowledge, memory, pattern, rule, search,
skill, spec, system, task, team, verification
```

```
# Remember something across sessions
mcp__cas__memory action=remember content="This project uses Zod for validation"

# Create and track work
mcp__cas__task action=create title="Implement auth" priority=1

# Search everything the project knows
mcp__cas__search action=search query="error handling patterns"

# Create a rule that syncs to .claude/rules/cas/
mcp__cas__rule action=create content="Always validate input at API boundaries"
```

| Surface | What it is |
|---|---|
| **Memory** | Learnings, preferences, context and observations that survive sessions, with tiers and importance |
| **Tasks** | Work items with dependencies, leases, structured notes, verification and merge-state close gates |
| **Rules** | Normative constraints that earn trust through use; proven rules sync to `.claude/rules/cas/` |
| **Skills** | Procedural playbooks synced to `.claude/skills/`, shipped in Claude / Codex / Grok flavors |
| **Search** | BM25 full-text over entries, tasks, rules, skills, specs and indexed code symbols |
| **Coordination** | Agent registry, messaging, worker spawn/shutdown, worktree operations — the factory's control plane |

**Search, honestly.** Local search is BM25 only (Tantivy), and the knowledge surface uses SQLite FTS5. There is no local embedder: semantic ranking exists only when you are logged in to the cloud, and the ranker redistributes a dead channel's weight instead of silently scaling every result down. See [ARCHITECTURE.md](cas-cli/docs/ARCHITECTURE.md) for the full accounting.

## Knowledge

A project can explain itself instead of being re-read from scratch every session.

```bash
cas knowledge build --dry-run   # plan the pass, call no model, cost nothing
cas knowledge build             # distill changed sources into .cas/knowledge/
cas knowledge status            # ledger and page counts
cas knowledge list
cas knowledge search "hook scoring"
cas knowledge read cas-kn007    # by page id, or by path: subsystem/hooks.md
```

`build` reads the repo's own documentation, README, agent instructions, key configuration and a summary of every indexed code module, and writes prose pages as ordinary markdown under `.cas/knowledge/` — greppable, hand-editable, reviewable in a PR. Sources are fingerprinted, so a pass over an unchanged project distills nothing and costs nothing. Distillation calls a model, so it never runs on its own.

Two properties worth knowing:

- **Hand-written text is never overwritten.** A page can be locked; a locked page cannot be re-distilled, garbage-collected, or overwritten by a teammate's copy arriving over sync. Text above the first generated section is treated as yours even on an unlocked page.
- **Session start gets an index, not the bodies.** The startup briefing carries one pointer line per page — id, type, title, snippet — capped and byte-identical between runs so it does not defeat prompt caching. Bodies are pulled through the `knowledge` MCP tool only when a question needs them.

## Quick Start

```bash
# Install (Linux x86_64)
curl -fsSL https://raw.githubusercontent.com/pippenz/cas/main/scripts/cas-install.sh | bash

# Initialize in your project — writes .mcp.json, .claude/settings.json hooks,
# .codex config, and syncs the builtin skills/agents
cd your-project
cas init

# Check the install
cas doctor

# Launch the factory
cas
```

## Installation

### Linux (x86_64)

```bash
curl -fsSL https://raw.githubusercontent.com/pippenz/cas/main/scripts/cas-install.sh | bash
```

Installs the latest release binary to `~/.local/bin/cas`. Override with `CAS_INSTALL_DIR`, `CAS_VERSION`, or `CAS_REPO`.

### macOS (Apple Silicon)

The installer script is Linux-only today. Releases publish a macOS ARM64 tarball — grab `cas-aarch64-apple-darwin.tar.gz` from [Releases](https://github.com/pippenz/cas/releases) and drop `cas` on your `PATH`, or build from source. A full from-zero Mac walkthrough lives in [docs/onboarding/macbook-from-zero.md](docs/onboarding/macbook-from-zero.md).

### Build from source

Building links the vendored libghostty-vt terminal engine, which needs a Zig compiler. The build script looks for `$ZIG`, then `zig` on `PATH`, then `.context/zig/zig`.

```bash
git clone https://github.com/pippenz/cas.git
cd cas
./scripts/bootstrap-zig.sh          # or: brew install zig / your package manager
export ZIG="$PWD/.context/zig/zig"  # skip if zig is already on PATH
cargo build --profile release-fast  # binary at target/release-fast/cas
```

`cargo build --release` produces a smaller, slower-to-build binary. Rust 1.85+ (edition 2024) is required.

### Staying current

```bash
cas update              # self-update from this repo's GitHub releases
cas update --schema-only  # apply pending database migrations only
cas changelog           # release notes
```

## CLI

```bash
cas                   # launch the factory TUI
cas -w 3              # ...with 3 workers
cas claude|codex|grok # choose the supervisor harness (all factory flags pass through)
cas open              # interactive project picker
cas init              # initialize CAS in the current project
cas serve             # run the MCP server
cas doctor            # diagnostics (see below)
cas knowledge ...     # distilled project wiki
cas attach|list|kill  # factory session control
cas status            # session status snapshot
cas config list       # every setting, current vs default
cas config describe cloud.auto_sync
cas worktree sweep    # worktree diagnostics and cleanup (cas sweep-all for every repo)
cas bridge serve      # local HTTP control/status API for external orchestrators
cas login             # CAS Cloud (optional)
cas cloud sync        # push + pull (optional)
```

### Editor integration

`cas init` wires this up for you. To do it by hand, add CAS to `.mcp.json` (Claude Code and Grok both read it) or your Claude Code settings:

```json
{
  "mcpServers": {
    "cas": {
      "command": "cas",
      "args": ["serve"]
    }
  }
}
```

CAS ships builtin skills (`cas-code-review`, `cas-worker`, `cas-github-issues`, `codemap`, …) in Claude, Codex and Grok flavors, kept in lockstep by a parity test. If they crowd your context, Claude Code's `skillOverrides` in `settings.json` can set any skill to `"off"`, `"user-invocable-only"`, or `"name-only"` without disabling CAS.

## Diagnostics

```bash
cas doctor                # database, schema, index, config, sync target, MCP wiring
cas doctor --fix          # initialize and apply pending migrations
cas doctor --foreign-rows # full list of cross-project rows in this database
```

Doctor reports which cloud bucket the project resolves to and why, warns when two local projects claim the same one, and detects rows replicated from other projects. Foreign rows are matched on `(id, title)` together — never on id alone, because short task ids genuinely collide across projects, so an id-only sweep would delete live work. The report is read-only; nothing is deleted or modified by it.

## Architecture

### On disk

```
.cas/
├── cas.db          # SQLite — memories, tasks, rules, skills, entities, agents
├── config.toml     # project configuration
├── knowledge/      # distilled wiki pages (markdown)
├── index/          # tantivy/ (BM25), code/ (symbols), knowledge-vectors/
├── logs/           # daily rolling logs
└── worktrees/      # factory worker checkouts
```

Host-level state (session registry, known repos, recordings, `[factory.defaults]`) lives in `~/.cas/`; the cross-project "global" memory scope lives in `~/.config/cas/`.

### Workspace

One binary crate (`cas-cli`) plus 16 libraries under `crates/`:

| Crate | Purpose |
|-------|---------|
| `cas-cli` | CLI, MCP server, factory TUI, daemon, bridge |
| `cas-factory` / `cas-factory-protocol` | Spawn pipeline, spec resolution, supervisor↔worker wire types |
| `cas-mux` / `cas-pty` | Terminal multiplexer and PTY management for agent panes |
| `cas-core` | Business logic, hooks, session-start context assembly |
| `cas-store` | SQLite storage layer, schema, migrations |
| `cas-search` | BM25 index (Tantivy) + vector store and score combination |
| `cas-code` | Code indexing and symbol search (tree-sitter) |
| `cas-mcp` / `cas-mcp-proxy` | MCP protocol types/handlers and upstream proxy engine |
| `cas-types` | Shared data types |
| `cas-diffs` | Diff parsing, rendering, syntax highlighting |
| `cas-recording` | Terminal session recording and playback |
| `cas-tui-test` | PTY-based TUI test framework |
| `ghostty_vt` / `ghostty_vt_sys` | Virtual terminal parser (vendored libghostty-vt) |

Built with Rust, SQLite, Tantivy, Ratatui, Ghostty VT and rmcp.

Deeper material: [ARCHITECTURE.md](cas-cli/docs/ARCHITECTURE.md) (crates, stores, hook scoring, search reality check) · [CONTRIBUTING.md](cas-cli/docs/CONTRIBUTING.md) (adding commands, MCP tools, migrations, tests) · [CODEMAP.md](.claude/CODEMAP.md) (module-level navigation) · [CHANGELOG.md](CHANGELOG.md).

## Cloud (optional)

CAS is fully usable offline; the cloud adds sync across machines, semantic search and team sharing.

```bash
cas login
cas cloud status      # what is pending, what is embedded
cas cloud sync        # push then pull
```

Sync is project-scoped: a push or pull that cannot determine which project it is running in refuses to make the request rather than sending or asking for everything. Personal pushes are incremental: `cas cloud push` consumes the current project's pending `sync_queue` rows, removing successful rows while leaving failures retryable. `--dry-run` previews the next bounded queue batch; `--entries-only` and `--tasks-only` narrow both that plan and the real push. `cas doctor` tells you which bucket a project resolves to; `cas cloud project set` pins it explicitly.

### Teams

After an admin creates a team in the dashboard:

```bash
cas login                             # single-team users are auto-scoped
cas cloud team default <team-slug>    # multi-team: pick a user-wide default
cas cloud team set <uuid>             # per-project override
cas cloud team-memories               # pull the team's memories for this project
```

Project-scoped, non-preference memories then dual-enqueue to the team queue automatically. Preference-typed and global-scoped entries always stay personal. Backfill existing entries with `cas memory share --dry-run --all` (then without `--dry-run`), `--since 7d`, or one id at a time; `cas memory unshare <id>` reverses it. Set `team_auto_promote: false` in `~/.cas/cloud.json` to pause promotion without clearing the team.

### Remote sessions

With devices registered (`cas device`), `cas attach --remote <device>:<factory-id>` resolves the device's SSH host from the cloud API and hands you the remote session; `--worker <name>` focuses one pane. `cas bridge serve` exposes a token-authenticated local HTTP API for external orchestration tools.

## Contributing

This repository is where CAS development happens; it began as a fork of a source-available upstream project and has moved a long way since. Bug reports and feature suggestions are welcome through [Issues](https://github.com/pippenz/cas/issues) and [Discussions](https://github.com/pippenz/cas/discussions); PRs are considered case by case.

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)
