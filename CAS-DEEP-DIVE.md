# CAS Deep Dive — Architecture, Code, and Post-1.0 Progress

This document provides the raw technical details of CAS in response to a review request. Everything below is sourced directly from the codebase.

---

## Table of Contents

1. [Repo Tree](#1-repo-tree)
2. [Supervisor / Worker Code](#2-supervisor--worker-code)
3. [Task Format and Protocol](#3-task-format-and-protocol)
4. [Memory / Context Schema](#4-memory--context-schema)
5. [Review / Merge / Verification Logic](#5-review--merge--verification-logic)
6. [Supervisor Prompt / Agent Config](#6-supervisor-prompt--agent-config)
7. [Worker Prompt / Config](#7-worker-prompt--config)
8. [Spawning Workers and Task Assignment](#8-spawning-workers-and-task-assignment)
9. [Persistence Layer](#9-persistence-layer)
10. [Work Since 1.0 and Forking](#10-work-since-10-and-forking)

---

## 1. Repo Tree

**Scale**: ~14,300 Rust source files, ~3.8M lines of Rust (including vendor/generated), 16 workspace crates, 150 database migrations.

```
cas-src/
├── cas-cli/                    # Main binary crate
│   ├── src/
│   │   ├── main.rs             # Entry point
│   │   ├── cli/                # Clap command definitions + handlers
│   │   │   ├── mod.rs          # Commands enum (add new subcommands here)
│   │   │   ├── factory/        # `cas` / `cas -w N` factory launcher
│   │   │   ├── doctor.rs       # `cas doctor` health check
│   │   │   ├── hook.rs         # `cas hook` for Claude Code hooks
│   │   │   └── ...
│   │   ├── mcp/                # MCP server (55+ tool handlers)
│   │   │   ├── server/         # CasCore with OnceLock store caches
│   │   │   └── tools/          # core/ (data) + service/ (orchestration)
│   │   ├── hooks/              # Claude Code hook handlers
│   │   │   ├── handlers/       # SessionStart, PostToolUse, etc.
│   │   │   └── scorer.rs       # BM25+temporal context scoring
│   │   ├── ui/                 # Ratatui TUI
│   │   │   ├── factory/        # Factory view (app, director, session, etc.)
│   │   │   │   ├── director/   # Sidecar panels (tasks, agents, changes, activity)
│   │   │   │   ├── daemon/     # Fork-first daemon, WebSocket server
│   │   │   │   └── ...
│   │   │   ├── components/     # Reusable TUI widgets
│   │   │   ├── theme/          # Palette, agent colors, icons
│   │   │   └── markdown/       # Markdown renderer for TUI
│   │   ├── store/              # Notifying + syncing store wrappers
│   │   ├── worktree/           # Git worktree management
│   │   ├── migration/          # 150 forward-only schema migrations
│   │   ├── daemon/             # Background maintenance (indexing, decay)
│   │   ├── consolidation/      # Memory consolidation
│   │   ├── extraction/         # AI-powered observation extraction
│   │   └── sync/               # Rule/skill sync to .claude/
│   ├── tests/                  # Integration + E2E + property-based tests
│   └── benches/                # Criterion benchmarks
│
├── crates/
│   ├── cas-types/              # Entry, Task, Rule, Skill, Agent, Verification, etc.
│   ├── cas-store/              # SQLite storage (trait defs + implementations)
│   ├── cas-search/             # Tantivy BM25 full-text search
│   ├── cas-core/               # Business logic, hooks framework, shared search types
│   ├── cas-mcp/                # MCP protocol types
│   ├── cas-factory/            # FactoryCore: PTY lifecycle, config, recording
│   ├── cas-factory-protocol/   # WebSocket protocol (MessagePack) for TUI/Desktop/Web
│   ├── cas-mux/                # Terminal multiplexer (side-by-side / tabbed)
│   ├── cas-pty/                # PTY management
│   ├── cas-recording/          # Session recording + playback
│   ├── cas-code/               # Tree-sitter code analysis
│   ├── cas-diffs/              # Diff parsing + syntax highlighting
│   ├── cas-mcp-proxy/          # MCP proxy for multi-server aggregation
│   ├── cas-tui-test/           # TUI testing framework
│   ├── ghostty_vt/             # Virtual terminal parser (Ghostty-based)
│   └── ghostty_vt_sys/         # Zig FFI for Ghostty VT
│
├── scripts/                    # Build, release, bootstrap scripts
├── homebrew/                   # Homebrew formula
└── site/                       # Landing page
```

---

## 2. Supervisor / Worker Code

### FactoryCore (`crates/cas-factory/src/core.rs`)

This is the orchestration kernel. It wraps `cas-mux` (terminal multiplexer) and manages the lifecycle of all agent PTY sessions.

```rust
pub struct FactoryCore {
    mux: Mux,                          // Terminal multiplexer
    config: FactoryConfig,
    supervisor_name: Option<String>,
    worker_names: Vec<String>,
    worker_cwds: HashMap<String, PathBuf>,
    cas_root: Option<PathBuf>,         // Shared CAS DB path
    rows: u16,
    cols: u16,
    recording: Option<RecordingManager>,
}
```

**Key operations:**

```rust
// Spawn supervisor (must be first)
factory.spawn_supervisor(Some("my-supervisor"))?;

// Spawn workers (each gets its own PTY)
factory.spawn_worker("worker-1", Some(worktree_path))?;

// Poll events (output, exits, focus changes)
let events = factory.poll_events();

// Shutdown
factory.shutdown_worker("worker-1")?;
```

Each worker is a full Claude Code CLI process running in its own PTY. The supervisor is also a Claude Code process. They communicate through a shared SQLite database, not through direct IPC.

### Worktree Management (`cas-cli/src/worktree/manager/worker_ops.rs`)

Workers get isolated git worktrees:

```rust
// Each worker gets: .cas-worktrees/<worker-name>/
// On branch: factory/<worker-name>
// Forked from: current branch (or specified parent)

pub fn create_for_worker(&mut self, worker_name: &str) -> WorktreeResult<Worktree> {
    let worktree_path = self.worktree_path_for_worker(worker_name);   // .cas-worktrees/<name>
    let branch_name = self.branch_name_for_worker(worker_name);       // factory/<name>
    let parent_branch = self.git.current_branch()?;

    self.git.create_worktree(&worktree_path, &branch_name, Some(&parent_branch))?;
    // ... register in tracking map
}
```

Cleanup is conflict-aware — refuses to delete worktrees with uncommitted changes unless forced:

```rust
pub fn cleanup_workers(&mut self, force: bool) -> WorktreeResult<Vec<String>> {
    for name in worker_names {
        if !force && self.git.has_uncommitted_changes(&worktree.path)? {
            continue;  // Skip dirty worktrees
        }
        self.git.remove_worktree(&worktree.path, force)?;
        self.git.delete_branch(&worktree.branch, true)?;
    }
}
```

---

## 3. Task Format and Protocol

### Task Schema (`crates/cas-types/src/task.rs`)

Tasks are the central coordination unit:

```rust
pub struct Task {
    pub id: String,                     // e.g., "cas-a1b2"
    pub title: String,
    pub description: String,            // Problem statement (immutable after creation)
    pub design: String,                 // Technical approach
    pub acceptance_criteria: String,    // Concrete deliverables
    pub notes: String,                  // Session handoff notes
    pub status: TaskStatus,             // Open | InProgress | Blocked | Closed
    pub priority: Priority,             // P0 (critical) through P4 (backlog)
    pub task_type: TaskType,            // Task | Bug | Feature | Epic | Chore | Spike
    pub assignee: Option<String>,       // Worker name
    pub demo_statement: String,         // What can be demonstrated when complete
    pub pending_verification: bool,     // "Jailed" — only verifier can run
    pub pending_worktree_merge: bool,   // Awaiting branch merge
    pub epic_verification_owner: Option<String>,
    pub deliverables: TaskDeliverables, // Files changed, commit hash, merge commit
    pub branch: Option<String>,         // Git branch scope
    pub worktree_id: Option<String>,    // For auto-cleanup
    pub team_id: Option<String>,        // Team sync
    // ... timestamps, labels, external_ref, etc.
}
```

**Dependencies** are a separate table — blocks, parent-child (epic→subtask):

```rust
pub struct Dependency {
    pub from_id: String,
    pub to_id: String,
    pub dep_type: DependencyType,   // Blocks | ParentChild
    pub created_at: DateTime<Utc>,
    pub created_by: Option<String>,
}
```

Cycle detection uses a recursive CTE in SQLite — `would_create_cycle()` checks before adding any dependency.

### Factory WebSocket Protocol (`crates/cas-factory-protocol/`)

Binary MessagePack over WebSocket. Supports TUI, Desktop (Tauri), and Web clients.

**Client → Server:**

```rust
pub enum ClientMessage {
    // Connection
    Connect { client_type, protocol_version, auth_token, session_id, capabilities },
    Reconnect { session_id, client_id, last_seq, ... },
    Ping { id },

    // Terminal I/O
    SendInput { pane_id, data: Vec<u8> },
    Resize { cols, rows },
    Focus { pane_id },
    Scroll { pane_id, delta, cache_window, target_offset },
    RequestSnapshot { pane_id },

    // Factory operations
    SpawnWorkers { count, names },
    ShutdownWorkers { count, names, force },
    InjectPrompt { pane_id, prompt },
    Interrupt { pane_id },
    RefreshDirector,

    // Playback
    PlaybackLoad { recording_path },
    PlaybackSeek { timestamp_ms },
    PlaybackSetSpeed { speed },
}
```

**Server → Client:**

```rust
pub enum ServerMessage {
    Connected { session_id, client_id, mode },
    FullState { state: SessionState },

    // Incremental terminal updates (dirty rows only)
    PaneRowsUpdate { pane_id, rows: Vec<RowData>, cursor, seq },
    // Or raw PTY output for clients with native terminal emulation
    PaneOutput { pane_id, data: Vec<u8> },

    PaneSnapshot { pane_id, scroll_offset, scrollback_lines, snapshot, cache_rows },
    PaneExited { pane_id, exit_code },
    PaneAdded { pane: PaneInfo },
    PaneRemoved { pane_id },

    DirectorUpdate { data: DirectorData },
    Batch { messages: Vec<ServerMessage> },  // Grouped updates

    // Boot sequence
    BootProgress { step, step_num, total_steps, completed },
    BootAgentProgress { name, is_supervisor, progress, ready },
    BootComplete,

    // Reconnect
    ReconnectAccepted { new_client_id, resync_needed },
    ConnectionHealth { rtt_ms, quality },
}
```

**Key design choices:**
- Row-level dirty tracking (not cell-level) — server renders via `ghostty_vt`, sends only changed rows as `StyleRun` spans
- Supports scrollback caching — client requests `cache_window` extra rows
- Sequence numbers for detecting missed updates on reconnect
- Feature negotiation via `ClientCapabilities` — clients that have native VT emulation get raw PTY bytes instead

### Supervisor ↔ Worker Communication

Not through the WebSocket protocol. Agents communicate through the **shared SQLite database** via MCP tools:

1. **Prompt Queue** (`PromptQueueStore`) — supervisor sends messages to workers
2. **Supervisor Notification Queue** (`SupervisorQueueStore`) — workers notify supervisor
3. **Task assignment** — supervisor sets `task.assignee = "worker-name"`
4. **Coordination tool** — `mcp__cas__coordination action=message target=<name> message="..."`

The **Director** (TUI sidecar) detects state changes by polling the database and auto-injects prompts into agent PTYs when events occur.

---

## 4. Memory / Context Schema

### Entry (Memory) (`crates/cas-types/src/entry.rs`)

MemGPT-inspired tiered memory system:

```rust
pub struct Entry {
    pub id: String,                     // "2025-01-15-001" format
    pub entry_type: EntryType,          // Learning | Preference | Context | Observation
    pub observation_type: Option<ObservationType>,  // Decision | Bugfix | Feature | Refactor | ...
    pub memory_tier: MemoryTier,        // InContext | Working | Cold | Archive
    pub content: String,
    pub raw_content: Option<String>,    // Stored when content is compressed
    pub compressed: bool,
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub domain: Option<String>,         // "payments", "auth", "api", etc.

    // Feedback signals
    pub helpful_count: i32,
    pub harmful_count: i32,
    pub access_count: i32,
    pub importance: f32,                // 0.0-1.0, user-settable

    // Forgetting curve (spaced repetition)
    pub stability: f32,                 // 0.0-1.0, higher = more resistant to decay
    pub review_after: Option<DateTime<Utc>>,
    pub last_reviewed: Option<DateTime<Utc>>,

    // Epistemic tracking (Hindsight-inspired)
    pub belief_type: BeliefType,        // Fact | Opinion | Hypothesis
    pub confidence: f32,                // 0.0-1.0

    // Temporal bounds
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,

    // Extraction pipeline
    pub pending_extraction: bool,
    pub pending_embedding: bool,

    // Scoping
    pub scope: Scope,                   // Global | Project
    pub branch: Option<String>,         // Worktree isolation
    pub session_id: Option<String>,
    pub team_id: Option<String>,
}
```

**Memory tiers:**

| Tier | Behavior |
|------|----------|
| `InContext` | Always injected into every session (pinned) |
| `Working` | Active, scored and ranked by BM25 + temporal signals |
| `Cold` | Less accessed, may be compressed, still searchable |
| `Archive` | Rarely accessed, compressed |

### Context Scoring (`cas-cli/src/hooks/scorer.rs`)

BM25 + temporal + graph scoring for session context injection:

```rust
pub struct HybridContextScorer {
    hybrid_search: HybridSearch,  // BM25 + temporal + optional graph retrieval
}

impl ContextScorer for HybridContextScorer {
    fn score_entries(&self, entries: &[Entry], context: &ContextQuery) -> Vec<(Entry, f32)> {
        // 1. Build query from task titles, cwd, recent files
        // 2. Run BM25 search with temporal boosting
        // 3. Blend: 70% hybrid score + 30% basic (feedback/importance signals)
        // 4. Entries not in search results get 30% basic score (penalty)
    }
}
```

The `ContextQuery` includes task titles, current working directory, user prompt, and recent files — used to find the most relevant memories for each session.

### Knowledge Graph (`EntityStore`)

Entities (people, projects, technologies) with relationships and mentions:

```rust
pub trait EntityStore {
    fn add_entity(&self, entity: &Entity) -> Result<()>;
    fn add_relationship(&self, relationship: &Relationship) -> Result<()>;
    fn add_mention(&self, mention: &EntityMention) -> Result<()>;  // Link entity → entry
    fn get_connected_entities(&self, entity_id: &str) -> Result<Vec<(Entity, Relationship)>>;
}
```

---

## 5. Review / Merge / Verification Logic

### Verification System (`crates/cas-types/src/verification.rs`)

Quality gate that fires when closing a task:

```rust
pub struct Verification {
    pub id: String,
    pub task_id: String,
    pub verification_type: VerificationType,  // Task | Epic
    pub status: VerificationStatus,           // Approved | Rejected | Error | Skipped
    pub confidence: Option<f32>,
    pub summary: String,
    pub issues: Vec<VerificationIssue>,
    pub files_reviewed: Vec<String>,
    pub duration_ms: Option<u64>,
}

pub struct VerificationIssue {
    pub file: String,
    pub line: Option<u32>,
    pub severity: IssueSeverity,    // Blocking | Warning
    pub category: String,           // "todo_comment", "temporal_shortcut", etc.
    pub code: String,               // Snippet
    pub problem: String,
    pub suggestion: Option<String>,
}
```

**Flow:**

1. Worker calls `mcp__cas__task action=close id=<task-id>`
2. Task gets `pending_verification = true` — agent is **"jailed"** (PreToolUse hook blocks all non-verification tools)
3. A `task-verifier` subagent spawns, reviews the diff, checks files
4. If approved → task closes, jail lifts
5. If rejected → issues are returned, worker must fix and re-close
6. Epic verification is similar but owned by the supervisor

### Merge Flow

Branch strategy in factory mode:

```
main ──────────────────────────► (stays clean)
       \
        └── factory/worker-1 ──► (worker's isolated branch)
        └── factory/worker-2 ──► (worker's isolated branch)
```

After a worker completes a task:
1. Worker closes task (triggers verification)
2. Supervisor reviews changes in the worker worktree
3. Supervisor merges worker branch: `git merge factory/<worker-name>`
4. Other workers rebase onto updated main
5. Worker's context is cleared, next task assigned

The `pending_worktree_merge` flag prevents task closure until the branch merge is complete.

---

## 6. Supervisor Prompt / Agent Config

The supervisor is a Claude Code instance launched with specific environment and system prompt. The Director (TUI sidecar) auto-injects prompts based on events.

### Auto-Prompt System (`cas-cli/src/ui/factory/director/prompts.rs`)

The Director watches database state changes and generates prompts:

```rust
pub fn generate_prompt(event: &DirectorEvent, ...) -> Option<Prompt> {
    match event {
        DirectorEvent::TaskAssigned { task_id, task_title, worker } => {
            // → Injects into WORKER terminal:
            // "You have been assigned task {task_id}..."
            // "View details: mcp__cas__task action=show id=..."
            // "Start: mcp__cas__task action=start id=..."
            // "Post progress: mcp__cas__task action=notes"
        }
        DirectorEvent::TaskCompleted { task_id, worker } => {
            // → Injects into SUPERVISOR terminal:
            // "Worker {worker} completed {task_id}..."
            // "Tell worker to close their own task"
            // "Workers close tasks, supervisors close epics"
        }
        DirectorEvent::TaskBlocked { task_id, worker } => {
            // → Injects into SUPERVISOR terminal:
            // "Worker {worker} is blocked on {task_id}..."
        }
        DirectorEvent::WorkerIdle { worker } => {
            // → Injects into SUPERVISOR terminal:
            // "Worker {worker} idle. {N} ready tasks available."
            // "Assign: mcp__cas__task action=update id=<id> assignee={worker}"
        }
        DirectorEvent::AgentRegistered { agent_name } => {
            // → Notifies supervisor that a worker is ready
        }
    }
}
```

Every injected prompt includes response instructions:

```
---
To respond to this message, use: `mcp__cas__coordination action=message target=<name> message="..."`
```

The tool prefix adapts to the agent's harness — `mcp__cas__` for Claude, `mcp__cs__` for Codex.

### Supervisor System Prompt

The supervisor gets its identity and workflow injected via the CAS MCP `SessionStart` hook. Key elements:

- **Role**: Planner and coordinator, never implements code directly
- **Hard rules**: Never close tasks for workers. Never monitor/poll. Never implement.
- **Workflow**: Plan → Create EPIC → Break down tasks → Spawn workers → Assign tasks → Wait for messages → Merge → Complete
- **Worker spawn strategy**: Based on file-overlap analysis, not task count (prevents conflicts)

---

## 7. Worker Prompt / Config

Workers are Claude Code instances that receive:

1. **Worktree context**: They run in `.cas-worktrees/<worker-name>/` with `CAS_ROOT` pointing to the shared database
2. **CAS MCP tools**: `mcp__cas__task`, `mcp__cas__memory`, `mcp__cas__coordination`, etc.
3. **Auto-injected task assignment** from the Director when the supervisor assigns them work

Worker workflow (from the cas-worker skill):
1. Receive task assignment notification
2. `mcp__cas__task action=show id=<task-id>` — read full details
3. `mcp__cas__task action=start id=<task-id>` — mark in-progress
4. ACK to supervisor with execution plan
5. Implement, posting progress notes
6. If blocked → `status=blocked` with blocker note
7. `mcp__cas__task action=close id=<task-id> reason="..."` — triggers verification
8. Handle verification feedback if rejected

---

## 8. Spawning Workers and Task Assignment

### Spawn Path

```
User: `cas -w 3`
  → cli/factory/lifecycle.rs
    → FactoryConfig { workers: 3, enable_worktrees: true, ... }
      → FactoryCore::new(config)
        → Mux::new(rows, cols)          // Terminal multiplexer
      → factory.spawn_supervisor()
        → Pane::supervisor(...)          // Claude Code PTY with supervisor env
        → mux.add_pane(pane)
      → for each worker:
        → WorktreeManager::ensure_worker_worktree(name)
          → git worktree add .cas-worktrees/<name> -b factory/<name>
        → factory.spawn_worker(name, worktree_path)
          → mux.add_worker(name, cwd, cas_root, supervisor_name)
            → Claude Code PTY in worktree dir
```

### Task Assignment Path

```
Supervisor calls: mcp__cas__task action=update id=cas-1234 assignee=swift-fox
  → MCP tool handler: task_update()
    → task_store.update(task)           // SQLite write
    → notification: TaskAssigned event
      → Director detects assignee change
        → generate_prompt(TaskAssigned { task_id, worker: "swift-fox" })
          → Prompt { target: "swift-fox", text: "You have been assigned..." }
            → mux.inject_prompt("swift-fox", prompt_text)
              → Written to worker's PTY stdin
```

### Coordination MCP Tools

The `mcp__cas__coordination` tool provides:

| Action | Purpose |
|--------|---------|
| `whoami` | Agent identifies itself |
| `message` | Send message to another agent (via prompt queue) |
| `spawn_workers` | Request N new workers |
| `shutdown_workers` | Shut down workers |
| `worker_status` | Check which workers are alive |
| `clear_context` | Reset a worker's context after task completion |

---

## 9. Persistence Layer

### SQLite + WAL Mode (`crates/cas-store/`)

All data lives in a single `.cas/cas.db` SQLite database shared by all agents:

```rust
// Connection pool with busy timeout for concurrent access
pub const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

// Application-level retry with exponential backoff + jitter
// (breaks convoy patterns when multiple workers write simultaneously)
```

**Store traits** — each data type has its own trait:

| Trait | Purpose | Key Methods |
|-------|---------|-------------|
| `Store` | Memory entries | `add`, `get`, `list`, `list_pinned`, `list_helpful`, `list_pending_index` |
| `TaskStore` | Tasks + dependencies | `add`, `list_ready`, `list_blocked`, `create_atomic`, `would_create_cycle` |
| `RuleStore` | Coding rules | `list_proven`, `list_critical` |
| `SkillStore` | Agent skills | `list_enabled`, `search` |
| `EntityStore` | Knowledge graph | `add_entity`, `add_relationship`, `get_connected_entities` |
| `VerificationStore` | Quality gates | `add_verification`, `save_verification_issues` |
| `AgentStore` | Agent registry | Heartbeats, leases, status |
| `WorktreeStore` | Worktree tracking | `claim`, `release`, `list_by_branch` |
| `PromptQueueStore` | Message passing | Supervisor → worker messages |
| `SupervisorQueueStore` | Notifications | Worker → supervisor events |
| `RecordingStore` | Session recordings | Record agent events, messages, tasks |
| `CodeStore` | Code symbols | Tree-sitter indexed symbols |
| `EventStore` | Activity tracking | Session events for sidecar |

### Full-Text Search (`crates/cas-search/`)

Tantivy-based BM25 search index alongside SQLite:

- Indexes entries, tasks, rules, skills
- Cached `QueryParser` and `IndexReader` (not rebuilt per search)
- Atomic index rebuild via directory swap
- Background daemon re-indexes every 2 minutes

### Schema Migrations (`cas-cli/src/migration/`)

150 forward-only migrations, auto-detected and applied on startup:

```
m001_entries_base.rs → m150_*.rs
```

Each migration has: unique sequential ID, `up` SQL, and a `detect` query (for introspecting whether it's already applied). ID ranges are partitioned by domain (entries 1-50, rules 51-70, skills 71-90, etc.).

### Store Wrappers (`cas-cli/src/store/`)

Stores are composed with decorator layers:

```
SqliteStore (raw DB operations)
  → NotifyingStore (emits change events to socket)
    → SyncingStore (syncs rules/skills to .claude/ filesystem)
      → LayeredStore (merges project + global scope)
```

---

## 10. Work Since 1.0 and Forking

**v1.0.0** was tagged at commit `2ad9f7c` as the initial open-source release. Since then: **146 commits** across 60+ days.

### Commit Breakdown

| Category | Count | Notable |
|----------|-------|---------|
| **fix** | 52 | TUI rendering, cloud sync, verification deadlocks, theme accessibility |
| **perf** | 29 | SQLite write optimization, Tantivy caching, batch indexing, alloc reduction |
| **Merge** | 34 | Factory worker branch merges (the system building itself) |
| **feat** | 18 | MCP proxy, Tokyo Night theme, prompt queue priorities, delivery confirmation |
| **test** | 2 | Doctor snapshot, factory tests |
| **docs** | 2 | Worker/spike documentation |
| **refactor** | 1 | Search lock hierarchy simplification |

### Major Work Streams

#### Performance Optimization (29 commits)

Multiple factory-coordinated optimization passes:

- **SQLite**: `prepare_cached()` for all statements, recursive CTE cycle-check (replacing iterative BFS), sequence table for ID generation (replacing `MAX(LIKE)` scan), jittered write-retry backoff
- **Tantivy search**: Cached `QueryParser` + `IndexReader` (was rebuilding per search), cached 50MB `IndexWriter` (was allocating per write), atomic index rebuild with directory swap + reader reopen
- **MCP server**: Cached search index + config, eager store init to reduce timeouts, deduplicated helper functions
- **Factory runtime**: Cached `supervisor_owned_workers()` with `thread_local!`, deduplicated idle notifications, suppressed dead worker spam
- **Hooks**: Consolidated `SessionStart`/`SessionStop` DB connections, removed duplicate `score_entries` call, filtered task queries instead of `list(None)`
- **General**: Single-pass loading, compression threshold tuning, batch code symbol DB inserts, pre-computed word sets in deduplication

#### Factory Reliability (15+ commits)

- **Verification deadlock resolution**: Fixed recursive jail in subagents, fixed deadlock when supervisor owns verification for orphaned tasks
- **Worker lifecycle**: Fallback workflow when MCP tools unavailable, worktree mode detection, correct project path resolution in worktree context
- **Director**: Fixed epic selection + dedup, task panel filtered to active epic, epic event ordering fix (detect before filter), task assignee name-vs-ID mismatch fix
- **Communication**: Delivery confirmation for prompt queue, priority levels, full message preservation on undeliverable messages
- **TUI stability**: Preserved scroll position on new output, shutdown TUI when supervisor exits, debounced Ctrl+C

#### TUI / Accessibility (10+ commits)

- **Off-by-one fix in Ghostty VT style runs**: First character of every line was being clipped — root cause was 1-indexed column starts in the Zig VT parser
- **Theme accessibility**: 6+ rounds of contrast improvements for colorblind safety
- **Tokyo Night theme variant**
- **Minions theme** (opt-in fun mode with ASCII art and agent personalities)
- **Clipboard**: OSC 52 clipboard copy, auto-inject on image paste, fallback for systems without clipboard access

#### MCP Proxy (7 commits)

New `cas-mcp-proxy` crate — aggregates multiple MCP servers behind a single connection:
- Config-aware hot-reload
- Keyword-based search across servers
- JSON dispatch with dot-call syntax (`server.tool` addressing)

#### Cloud Sync Hardening (5+ commits)

- Project-scoped pull requests (prevent cross-project data leaks)
- Circuit breaker for TLS retry spam
- Capped event buffer
- Cached project canonical ID
- Background sync with 5s timeout (non-blocking startup)

### Known Open Issues (from `issues/` directory)

These are documented bugs/limitations that emerged from real factory sessions:

1. **Director relays stale worker messages** — messages from previous workers shown to new ones
2. **Idle notification spam** — workers generate excessive idle notifications
3. **Supervisor can't close orphaned worker tasks** — when worker dies mid-task
4. **Supervisor queries wrong database** — in certain worktree configurations
5. **Task verifier recursive jail** — verification spawns subagent that also gets jailed
6. **Verification deadlock for supervisor-owned tasks** — supervisor verifying its own epics
7. **Worker prompt missing worktree awareness** — worker doesn't know it's in a worktree
8. **Workers can't use CAS MCP in worktrees** — MCP server resolves wrong `.cas/` path
9. **Workers waste turns on MCP bootstrap** — first MCP call in a session is slow

Most of these have been partially or fully addressed in the post-1.0 commits. The issues directory serves as a backlog.

### Meta: CAS Building CAS

A significant portion of the post-1.0 work was done **by CAS factory sessions** — visible in the `Merge branch 'factory/<worker-name>'` commits. The system has been its own primary test case, which surfaced many of the reliability fixes listed above.
