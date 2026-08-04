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
[![CI](https://github.com/codingagentsystem/cas/actions/workflows/ci.yml/badge.svg)](https://github.com/codingagentsystem/cas/actions)
[![Latest Release](https://img.shields.io/github/v/release/codingagentsystem/cas)](https://github.com/codingagentsystem/cas/releases)

[Factory](#factory) · [Context System](#context-system) · [Quick Start](#quick-start) · [Installation](#installation) · [Architecture](#architecture) · [Contributing](CONTRIBUTING.md)

<img src="casdemo.png" alt="CAS Factory TUI" width="800" />

</div>

---

## What is CAS?

CAS is a multi-agent coding factory and persistent context system for AI agents. It has two core capabilities:

1. **Factory** — A terminal UI that orchestrates multiple Claude Code instances working in parallel on the same codebase, with a supervisor agent coordinating worker agents across isolated git worktrees.

2. **Context System** — An MCP server that gives agents persistent memory, task tracking, rules, and skills across sessions, backed by SQLite and full-text search.

## Factory

Factory mode turns your terminal into a multi-agent coding operation. A supervisor agent breaks work into tasks while worker agents execute them in parallel — each in its own git worktree to avoid conflicts.

```bash
# Launch the factory TUI
cas

# Launch with 3 workers in isolated worktrees
cas -w 3
```

### How it works

```
┌─────────────────────────────────────────────────────────┐
│  CAS Factory                                            │
├──────────────────────┬──────────────────────────────────┤
│                      │                                  │
│  Supervisor          │  Worker 1        Worker 2        │
│                      │                                  │
│  Plans EPICs,        │  Executes tasks  Executes tasks  │
│  breaks down work,   │  in isolated     in isolated     │
│  assigns tasks,      │  git worktree    git worktree    │
│  reviews & merges    │                                  │
│                      │                                  │
├──────────────────────┴──────────────────────────────────┤
│  Shared: CAS database (memories, tasks, rules, skills)  │
└─────────────────────────────────────────────────────────┘
```

- **Supervisor** plans work, creates tasks, assigns them to workers, reviews completed work, and merges branches
- **Workers** each get their own git worktree and branch — no merge conflicts during parallel execution
- **Shared context** — all agents read/write the same CAS database for memories, tasks, rules, and coordination messages
- **Built-in terminal multiplexer** — side-by-side or tabbed views of all agent sessions, with a custom VT parser (based on Ghostty)

### Factory features

| Feature | Description |
|---------|-------------|
| **Worktree isolation** | Each worker gets its own git worktree and branch — parallel edits without conflicts |
| **Task coordination** | Supervisor assigns tasks with dependencies; workers claim, execute, and report back |
| **Live TUI** | Side-by-side or tabbed terminal views of all agents, with real-time status bar |
| **Message passing** | Push-based communication between supervisor and workers via prompt queue |
| **Session management** | Attach, detach, list, and kill factory sessions (`cas attach`, `cas list`, `cas kill`) |
| **Desktop notifications** | Optional alerts when tasks complete or workers hit blockers (`--notify`) |
| **Session recording** | Record terminal sessions for playback (`--record`) |

### When to use Factory

- **Large features** — break an epic into subtasks and parallelize across workers
- **Codebase-wide refactors** — workers modify different files simultaneously without conflicts
- **Multi-step workflows** — tasks with dependencies execute in the right order
- **Code review** — supervisor reviews worker output before merging to the main branch

## Context System

CAS runs as an [MCP server](https://modelcontextprotocol.io/) that gives your agent persistent context across sessions — 50+ tools for memory, tasks, rules, skills, and search.

### MCP Tools

When your agent has CAS configured, it can:

```
# Remember something across sessions
mcp__cas__memory action=remember content="This project uses Zod for validation"

# Create and track tasks
mcp__cas__task action=create title="Implement auth" priority=1

# Search past context
mcp__cas__search action=search query="error handling patterns"

# Create a rule that auto-syncs to .claude/rules/
mcp__cas__rule action=create content="Always validate input at API boundaries"
```

### What persists

| Feature | Description |
|---------|-------------|
| **Memory** | Learnings, preferences, and observations that survive across sessions |
| **Tasks** | Work items with dependencies, priorities, and structured progress notes |
| **Rules** | Coding conventions that earn trust through use and auto-sync to your editor |
| **Skills** | Reusable agent capabilities with templates and usage tracking |
| **Search** | Fast full-text search (BM25) across all stored context |

## Quick Start

```bash
# Install
curl -fsSL https://cas.dev/install.sh | sh

# Initialize in your project
cas init

# Launch the factory TUI
cas
```

## Installation

### curl (recommended)

```bash
curl -fsSL https://cas.dev/install.sh | sh
```

### Homebrew

```bash
brew tap codingagentsystem/cas
brew install cas
```

> **Homebrew users — auto-upgrade Claude Code in the background:**
> If you installed Claude Code via Homebrew, set this env var to let it
> self-upgrade automatically (added in Claude Code 2.1.129):
>
> ```bash
> export CLAUDE_CODE_PACKAGE_MANAGER_AUTO_UPDATE=1
> ```
>
> Add it to your shell profile (`~/.zprofile`, `~/.bashrc`, etc.) to make it
> permanent. Claude Code will run `brew upgrade claude` in the background and
> prompt you to restart.
>
> **This is for Claude Code only — not CAS.** CAS updates via `cas update`.
>
> Reference: [Claude Code 2.1.129 changelog](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md)

### Build from source

```bash
git clone https://github.com/codingagentsystem/cas.git
cd cas
cargo build --release
# Binary at target/release/cas
```

## CLI

```bash
cas                   # Launch the factory TUI
cas -w 3              # Launch with 3 workers
cas claude alt --continue  # Launch Claude Code using ~/.claude-alt
cas serve             # Start MCP server for Claude Code
cas init              # Initialize CAS in your project
cas attach            # Attach to a running factory session
cas list              # List running factory sessions
cas kill              # Kill a factory session
cas config list       # View all configuration options
cas doctor            # Run diagnostics
cas update            # Self-update to latest version
cas login             # Log in to CAS Cloud (optional)
cas cloud sync        # Sync data to/from cloud (optional)
```

### Claude Code Integration

Add to your Claude Code MCP config (`.claude/settings.json` or project `.mcp.json`):

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

#### Managing CAS skill exposure with `skillOverrides`

CAS ships several builtin skills (e.g. `cas-code-review`, `cas-worker`, `fallow`, `codemap`).
If you are running CAS alongside other skill packs, or simply want to reduce context noise,
Claude Code 2.1.129 added `skillOverrides` in `settings.json` that lets you control per-skill
visibility without disabling CAS entirely.

Three modes are available:

| Mode | Effect |
|------|--------|
| `"off"` | Skill is completely hidden — invisible to the model and absent from the `/` command list |
| `"user-invocable-only"` | Skill is hidden from the model's context but the user can still invoke it via `/skill-name` |
| `"name-only"` | Skill name appears in suggestions but the full description is collapsed (saves context tokens) |

**Example** — add to your global `~/.claude/settings.json` or project `.claude/settings.json`:

```json
{
  "skillOverrides": {
    "cas-brainstorm": "off",
    "cas-ideate": "user-invocable-only",
    "cas-memory-management": "name-only"
  }
}
```

> **Tip:** To suppress all CAS skills temporarily, set each to `"off"`. To keep them available
> for manual use while decluttering the model context, prefer `"user-invocable-only"`.
>
> Reference: [Claude Code 2.1.129 changelog](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md)

## Architecture

### Data storage

CAS stores all data locally in your project:

```
.cas/
├── cas.db          # SQLite — memories, tasks, rules, skills
├── config.yaml     # Project configuration
└── indexes/
    └── tantivy/    # Full-text search index
```

**Storage tiers:**
- **Project** (`.cas/`) — project-specific context
- **Global** (`~/.config/cas/`) — cross-project preferences and learnings

### Workspace Crates

| Crate | Purpose |
|-------|---------|
| `cas-cli` | CLI binary, MCP server, and factory TUI |
| `cas-factory` | Multi-agent session lifecycle and coordination |
| `cas-factory-protocol` | Message protocol between supervisor and workers |
| `cas-pty` | PTY management for agent terminal sessions |
| `cas-mux` | Terminal multiplexer layout and rendering |
| `cas-core` | Core logic, hooks, and integrations |
| `cas-store` | SQLite storage layer |
| `cas-search` | Full-text search (BM25 via Tantivy) |
| `cas-mcp` | MCP protocol handlers |
| `cas-types` | Shared data types |
| `cas-code` | Code analysis (tree-sitter) |
| `cas-diffs` | Diff tracking and formatting |
| `cas-recording` | Terminal session recording and playback |
| `ghostty_vt` | Virtual terminal parser (based on Ghostty) |

### Built With

- **Rust** for performance and reliability
- **SQLite** for local-first storage
- **Tantivy** for full-text search (BM25)
- **Ratatui** for the factory TUI
- **Ghostty VT** for terminal emulation
- **rmcp** for MCP protocol support

## Configuration

CAS is configured via `.cas/config.yaml` in your project root. Run `cas config list` to see all options or `cas config describe <key>` for details on any setting.

### Cloud Sync (optional)

CAS works fully offline. Optionally sync your context across devices:

```bash
cas login
cas cloud sync
```

Cloud sync is not required — all core features work locally with SQLite.

### Team Memories (optional)

Share learnings across a team without manual flags. After an admin has
created a team in the CAS Cloud dashboard:

```bash
# Standard setup — log in. CAS fetches your team membership automatically.
cas login

# If you belong to exactly one team, auto-scope kicks in immediately.
# If you belong to multiple teams, pick a user-wide default:
cas cloud team default petra-stella   # slug from your team dashboard

# From now on, every memory captured via mcp__cas__memory
# action=remember (Claude Code) in a Project-scoped, non-Preference
# context automatically dual-enqueues into the team push queue
# across all your CAS-initialized projects.

# Next sync drains both personal and team queues
cas cloud sync
```

Teammates on a fresh machine get the same setup:

```bash
cas login
# single-team users: done — sync will auto-scope to your team
# multi-team users: pick a default:
cas cloud team default petra-stella
cas cloud team-memories
```

**Multiple teams.** If you belong to more than one team, set the
user-wide default once, then override per-project as needed:

```bash
cas cloud team default petra-stella          # user-wide default (all projects)
cas cloud team set <uuid>                    # per-project override (this dir only)
cas cloud team default --personal            # revert to personal scope (no team)
```

**Backfilling pre-existing memories.** If you had personal entries
before the team was configured, promote them retroactively:

```bash
cas memory share --dry-run --all             # preview
cas memory share --all                       # promote everything eligible
cas memory share --since 7d                  # or just the last week
cas memory share 2026-03-01-1                # or one at a time, by id
cas memory unshare 2026-03-01-1              # reverse — mark as Private
```

Preference-typed and Global-scoped entries always stay personal. To
pause automatic promotion without clearing the team, set
`team_auto_promote: false` in `~/.cas/cloud.json`.

## Contributing

CAS is source-available under the MIT license. We welcome bug reports and feature suggestions through [Issues](https://github.com/codingagentsystem/cas/issues) and [Discussions](https://github.com/codingagentsystem/cas/discussions).

See [CONTRIBUTING.md](CONTRIBUTING.md) for details.

## License

[MIT](LICENSE)
