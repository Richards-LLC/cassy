---
date: 2026-05-01
topic: codex-session-format-spec
focus: parser-ready spec for reading Codex CLI rollout JSONL files
scope: Codex + Claude Code as the only two supported harnesses (no OpenCode, no Cursor)
related: 2026-05-01-codex-cli-empirical-reference.md, 2026-05-01-codex-tui-session-loading-paths.md
---

# Codex Session Format — Parser-Ready Spec

The on-disk format for Codex CLI sessions ("rollouts"). Sufficient detail to write a `CodexSessionStore` adapter without re-reading Codex's source.

All `codex-rs/...` paths refer to the `openai/codex` repository. Verified against tag `rust-v0.128.0` (released 2026-04-30).

## 1. Storage location

**Default root:** `$CODEX_HOME` if set and existing, else `~/.codex` (`codex-rs/utils/home-dir/src/lib.rs:13-65`).

**Layout — date-nested, NOT per-project:**

```
$CODEX_HOME/sessions/YYYY/MM/DD/rollout-YYYY-MM-DDThh-mm-ss-<uuid>.jsonl
$CODEX_HOME/archived_sessions/<flat>/rollout-...jsonl   (archived: flat layout)
$CODEX_HOME/session_index.jsonl                         (sidecar: thread-name aliases)
$CODEX_HOME/state.db (or $CODEX_SQLITE_HOME)            (SQLite index, optional)
```

**Path construction** (`codex-rs/rollout/src/recorder.rs:1363-1393`):
```rust
let mut dir = config.codex_home().to_path_buf();
dir.push(SESSIONS_SUBDIR);
dir.push(timestamp.year().to_string());
dir.push(format!("{:02}", u8::from(timestamp.month())));
dir.push(format!("{:02}", timestamp.day()));
let filename = format!("rollout-{date_str}-{conversation_id}.jsonl");
```

The timestamp portion uses `-` separators (`hh-mm-ss`), not `:` — filename parser in `list.rs:925-940` scans right-to-left for the last `-` before a parseable UUID.

**Glob for enumeration:**
```
$CODEX_HOME/sessions/*/*/*/rollout-*-????????-????-????-????-????????????.jsonl
```
Or simpler: `**/rollout-*.jsonl` under `$CODEX_HOME/sessions`, then filter with `parse_timestamp_uuid_from_filename`.

## 2. File format

**JSONL, one record per line.** Each line deserializes to `RolloutLine` (`codex-rs/protocol/src/protocol.rs:2922-2927`):

```rust
#[derive(Serialize, Deserialize, Clone, JsonSchema)]
pub struct RolloutLine {
    pub timestamp: String,           // RFC3339 "YYYY-MM-DDThh:mm:ss.sssZ"
    #[serde(flatten)]
    pub item: RolloutItem,
}
```

`RolloutItem` adds `type` + `payload` (`protocol.rs:2772-2780`):

```rust
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, TS)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RolloutItem {
    SessionMeta(SessionMetaLine),
    ResponseItem(ResponseItem),
    Compacted(CompactedItem),
    TurnContext(TurnContextItem),
    EventMsg(EventMsg),
}
```

So every line on disk is:
```json
{ "timestamp": "...", "type": "session_meta" | "response_item" | "compacted" | "turn_context" | "event_msg", "payload": { ... } }
```

**Live append:** `recorder.rs:1800-1806` flushes after every line. Tail-watching (inotify / polling) works. The `SessionMeta` line is written on first item (deferred file creation — new sessions are invisible to a watcher until first item lands).

**Legacy quirk:** strip lines that look like `ghost_snapshot` rollout lines via `strip_legacy_ghost_snapshot_rollout_line` (`recorder.rs:968-989`).

## 3. Per-event schema

### 3.1 `type: "session_meta"` — first line

Wraps `SessionMetaLine` (`protocol.rs:2764-2770`):

```rust
pub struct SessionMetaLine {
    #[serde(flatten)]
    pub meta: SessionMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<GitInfo>,
}
```

`SessionMeta` (`protocol.rs:2713-2741`):

```rust
pub struct SessionMeta {
    pub id: ThreadId,                     // UUID; Codex calls it "thread_id"
    pub forked_from_id: Option<ThreadId>, // present iff session is a fork
    pub timestamp: String,                // "YYYY-MM-DDThh:mm:ss.sssZ"
    pub cwd: PathBuf,
    pub originator: String,               // "codex_cli", "codex_exec", "codex_vscode", ...
    pub cli_version: String,
    pub source: SessionSource,
    pub agent_nickname: Option<String>,   // sub-agents only
    pub agent_role: Option<String>,       // alias: "agent_type"
    pub agent_path: Option<String>,
    pub model_provider: Option<String>,
    pub base_instructions: Option<BaseInstructions>,  // { text: String }
    pub dynamic_tools: Option<Vec<DynamicToolSpec>>,
    pub memory_mode: Option<String>,      // "disabled" or absent
}
```

`GitInfo` (`protocol.rs:2929-2940`): `commit_hash: Option<GitSha>`, `branch: Option<String>`, `repository_url: Option<String>`.

`SessionSource` (`protocol.rs:2540-2551`):

```rust
pub enum SessionSource {
    Cli, VSCode, Exec, Mcp,
    Custom(String),
    Internal(InternalSessionSource),     // { memory_consolidation }
    SubAgent(SubAgentSource),            // see §6
    Unknown,
}
```

### 3.2 `type: "response_item"` — model I/O history

Payload is the `ResponseItem` enum (`codex-rs/protocol/src/models.rs:741-886`), internally tagged `{"type": "...", ...}`:

| Variant | `type` discriminant | Key fields |
|---|---|---|
| `Message` | `message` | `role: String`, `content: Vec<ContentItem>`, `phase: Option<MessagePhase>` |
| `Reasoning` | `reasoning` | `summary: Vec<ReasoningItemReasoningSummary>`, `content: Option<...>`, `encrypted_content: Option<String>` |
| `LocalShellCall` | `local_shell_call` | `call_id`, `status`, `action: LocalShellAction` |
| `FunctionCall` | `function_call` | `name`, `namespace`, `arguments: String` (raw JSON), `call_id` |
| `FunctionCallOutput` | `function_call_output` | `call_id`, `output: FunctionCallOutputPayload` |
| `CustomToolCall` | `custom_tool_call` | `call_id`, `name`, `input: String` |
| `CustomToolCallOutput` | `custom_tool_call_output` | `call_id`, `name`, `output: FunctionCallOutputPayload` |
| `ToolSearchCall` | `tool_search_call` | `call_id`, `execution`, `arguments: Value` |
| `ToolSearchOutput` | `tool_search_output` | `call_id`, `status`, `execution`, `tools: Vec<Value>` |
| `WebSearchCall` | `web_search_call` | `status`, `action: Option<WebSearchAction>` |
| `ImageGenerationCall` | `image_generation_call` | `id`, `status`, `revised_prompt`, `result` |
| `Compaction` | `compaction` (alias `compaction_summary`) | `encrypted_content: String` |
| `Other` | (catch-all `#[serde(other)]`) | none — forward-compat |

`ContentItem` (`models.rs:699-712`):

```rust
pub enum ContentItem {
    InputText { text: String },
    InputImage { image_url: String, detail: Option<ImageDetail> },
    OutputText { text: String },
}
```

**Tool args are JSON-encoded strings** (`FunctionCall.arguments: String`). Parse with `serde_json::from_str` after extraction.

**Tool call/result join key is `call_id`**, NOT `id` — analog to Claude's `tool_use_id`.

### 3.3 `type: "compacted"` — auto/manual compaction marker

`CompactedItem` (`protocol.rs:2782-2787`):

```rust
pub struct CompactedItem {
    pub message: String,
    pub replacement_history: Option<Vec<ResponseItem>>,
}
```

### 3.4 `type: "turn_context"` — durable per-turn baseline

`TurnContextItem` (`protocol.rs:2812-2849`) — written once per real user turn AND after compaction. Carries: `turn_id`, `trace_id`, `cwd`, `current_date`, `timezone`, `approval_policy`, `sandbox_policy`, `permission_profile`, `network: TurnContextNetworkItem`, `file_system_sandbox_policy`, `model`, `personality`, `collaboration_mode`, `realtime_active`, `effort`, `summary`, `user_instructions`, `developer_instructions`, `final_output_json_schema`, `truncation_policy`.

**Important:** `cwd` may change mid-session via `TurnContextItem.cwd`. Treat as authoritative for that turn forward.

### 3.5 `type: "event_msg"` — runtime telemetry

Payload is the `EventMsg` enum (`protocol.rs:1305-1511`), ~70 variants. Internally tagged `{"type": <snake_case>, ...payload fields}`. **Only a subset is persisted** — see persistence policy in `codex-rs/rollout/src/policy.rs:91-181` (`event_msg_persistence_mode`).

**Always persisted (`Limited` mode = default):**

- `user_message` (`UserMessageEvent`)
- `agent_message` (`AgentMessageEvent`)
- `agent_reasoning` (`AgentReasoningEvent`)
- `agent_reasoning_raw_content` (`AgentReasoningRawContentEvent`)
- `patch_apply_end` (`PatchApplyEndEvent`)
- `token_count` (`TokenCountEvent`)
- `thread_name_updated`
- `context_compacted`, `entered_review_mode`, `exited_review_mode`
- `thread_rolled_back`, `turn_aborted`
- `task_started` (alias `turn_started`), `task_complete` (alias `turn_complete`)
- `image_generation_end`
- `item_completed` **only when** `item.type == "plan"` (plan updates only)

**Also persisted in `Extended` mode:**

- `error`, `guardian_assessment`
- `web_search_end`, `exec_command_end`, `mcp_tool_call_end`, `view_image_tool_call`
- `collab_agent_spawn_end`, `collab_agent_interaction_end`, `collab_waiting_end`, `collab_close_end`, `collab_resume_end`
- `dynamic_tool_call_request`, `dynamic_tool_call_response`

**Never persisted to disk** (live-only): all `*_begin`, deltas, `hook_started`, `hook_completed`, `request_user_input`, `apply_patch_approval_request`.

Codex rollout files give **completed-only** snapshots — no streaming begin/output-delta events. Hook events (`hook_started`/`hook_completed`) exist as live event types but never persist.

`recorder.rs:200-214` truncates `ExecCommandEnd.aggregated_output` to 10,000 chars in `Extended` mode and clears `stdout`/`stderr`/`formatted_output` before persisting.

## 4. Resume / fork / sub-agent semantics

### Resume (same file)

`codex exec resume <id>` finds the file via `find_thread_path_by_id_str(codex_home, id)` (`list.rs:1318-1323`):
1. Tries SQLite state DB lookup.
2. Falls back to filename substring search using UUID.

Then opens with `RolloutRecorderParams::Resume { path }` (`recorder.rs:710-724`) — appends to existing file. **No new SessionMeta line** (`meta = None`, lines 720-722).

### Fork (new file)

`codex fork` creates a new file with its own UUID. The new `SessionMeta.forked_from_id = Some(parent_id)`. Parent file untouched. Walk `forked_from_id` to reconstruct lineage.

### Sub-agents (new file per spawn)

Each sub-agent gets its own rollout file. Linkage in `SessionMeta.source`:

```rust
pub enum SubAgentSource {
    Review,
    Compact,
    ThreadSpawn {
        parent_thread_id: ThreadId,
        depth: i32,
        agent_path: Option<AgentPath>,
        agent_nickname: Option<String>,
        agent_role: Option<String>,
    },
    MemoryConsolidation,
    Other(String),
}
```

`forked_from_id` and sub-agent linkage are **separate** mechanisms — forks are branch points; sub-agents are delegation.

Spawn site: `codex-rs/core/src/agent/control.rs:251-289`.

## 5. Enumeration

Two paths kept in sync (`recorder.rs:352-573` `list_threads_with_db_fallback`):

1. **Filesystem-first scan** — walk `$CODEX_HOME/sessions/YYYY/MM/DD`, parse filename for `(timestamp, uuid)`. Optionally read head with `read_head_for_summary` (`list.rs:1156-1192`) — up to 10 lines, returns `SessionMetaLine` + initial `ResponseItem`s. Skips compacted/turn_context/event_msg in head summary.

2. **SQLite state DB** — `$CODEX_HOME/state.db` (or `$CODEX_SQLITE_HOME`). Indexes `id`, `cwd`, `git_branch`, `git_sha`, `first_user_message`, `created_at`, `updated_at`, `source`. **Read-repairs from filesystem** if missing/stale.

**For a third-party reader (Cassy): don't bother with the SQLite DB** — it's reproducible from the JSONL and Codex itself rebuilds it via `metadata::backfill_sessions`. Just glob the rollouts folder and parse heads.

**Hard caps in enumerator:** `MAX_SCAN_FILES = 10_000` (`list.rs:105`); per-file head limit `HEAD_RECORD_LIMIT = 10` lines.

**Sidecar:** `$CODEX_HOME/session_index.jsonl` (`session_index.rs:17`) — append-only log of `{id, thread_name, updated_at}` for human-readable thread names. Newest-wins. Optional for parsing; needed for display.

## 6. Wire schema export (TypeScript via `ts-rs`)

Almost every rollout struct has `#[derive(... TS)]` — `RolloutItem`, `RolloutLine`, `SessionMeta`, `SessionMetaLine`, `GitInfo`, `CompactedItem`, `TurnContextItem`, `EventMsg`, `ResponseItem`, `ContentItem`. Codex emits TS types.

**Note:** `codex-rs/exec/src/exec_events.rs` is a **different, higher-level schema** for the `codex exec --json` output:

`ThreadEvent` (`exec_events.rs:11-37`): `thread.started`, `turn.started`, `turn.completed`, `turn.failed`, `item.started`, `item.updated`, `item.completed`, `error`.

`ThreadItemDetails` (`exec_events.rs:104-130`): `agent_message`, `reasoning`, `command_execution`, `file_change`, `mcp_tool_call`, `collab_tool_call`, `web_search`, `todo_list`, `error`.

**These are NOT the same as on-disk events.** The exec-events stream is a UI-friendly projection. To build a Cassy adapter, **parse the rollout schema (sections 2-3), not exec-events**.

## 7. Ephemeral mode

`codex exec --ephemeral` (`codex-rs/exec/src/cli.rs:27-28`). `core/src/session/session.rs:387-457` short-circuits — no rollout recorder, no thread store, no SQLite writes:

```rust
let thread_persistence_fut = async {
    if config.ephemeral {
        Ok::<_, anyhow::Error>(None)
    } else { ... }
};
let state_db_fut = async {
    if config.ephemeral { None } else { ... }
};
```

**Effect: zero on-disk artifacts.** Cassy will never see ephemeral runs.

## 8. Codex vs Claude Code: side-by-side schema

| Concern | Claude Code | Codex |
|---|---|---|
| Root | `~/.claude/projects/` | `~/.codex/sessions/` (override `$CODEX_HOME`) |
| Project grouping | Per-project: `<encoded-cwd>/<session-uuid>.jsonl` | **Date-nested**, NOT per-project: `YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl` |
| Filename | `<uuid>.jsonl` | `rollout-YYYY-MM-DDThh-mm-ss-<uuid>.jsonl` |
| Session ID | `sessionId` field on every line | `id` (`ThreadId`, UUID) on first SessionMeta line only |
| Parent linkage | `parentUuid` per-line (causal chain inside session) | No per-line parent; structural via `SessionMeta.forked_from_id` (forks) and `SessionSource::SubAgent::ThreadSpawn::parent_thread_id` (sub-agents) |
| `cwd` | Per-line `cwd` field | `SessionMeta.cwd` once + `TurnContextItem.cwd` per turn (can change mid-session) |
| `version` (CLI) | Per-line `version` field | `SessionMeta.cli_version` once |
| Message shape | `message: {role, content: [{type, text\|tool_use\|tool_result, ...}]}` | `ResponseItem::Message { role, content: Vec<ContentItem> }` where `ContentItem` is `input_text` / `input_image` / `output_text` |
| Tool calls | `content[].type == "tool_use"` with `name`, `input` | `ResponseItem::FunctionCall { name, arguments: String, call_id }` (string-encoded JSON args) |
| Tool results | `content[].type == "tool_result"` with `tool_use_id` matching | `ResponseItem::FunctionCallOutput { call_id, output }`; `call_id` is join key |
| Reasoning | `content[].type == "thinking"` | `ResponseItem::Reasoning { summary, content, encrypted_content }` |
| Tool lifecycle | tool_use + tool_result, no separate begin/end | `FunctionCall` + `FunctionCallOutput`, AND optionally `EventMsg::ExecCommandEnd` / `McpToolCallEnd` (Extended mode) |
| File patches | `tool_use` for Edit/Write tools | `ResponseItem::FunctionCall { name: "apply_patch", ... }` + `EventMsg::PatchApplyEnd` (always persisted) |
| Web search | `tool_use` with web search tool name | `ResponseItem::WebSearchCall { action }` + `EventMsg::WebSearchEnd` (extended) |
| Plans | (none — plan tool emits tool_use) | `EventMsg::ItemCompleted { item: TurnItem::Plan(...) }` (only `ItemCompleted` persisted) |
| Sub-agents | `isSidechain: true` per-line, same file | **Separate file** with `SessionSource::SubAgent::ThreadSpawn { parent_thread_id, depth, agent_role, agent_nickname, agent_path }` |
| Hooks | (none on disk) | Live-only `EventMsg::HookStarted/Completed` — **not persisted** |
| Compaction | Compacted into messages; meta marker | `RolloutItem::Compacted { message, replacement_history }` line + `EventMsg::ContextCompacted` |
| Turn boundaries | Implicit | Explicit: `EventMsg::TurnStarted` / `EventMsg::TurnComplete` |
| Token usage | Per-message `usage` field | `EventMsg::TokenCount { ... }` events |
| Resume | Append to same file | Append to same file (resume by UUID) |
| Fork | New file, no schema parent link (UUID lineage) | New file, `SessionMeta.forked_from_id` set |
| Index | None — list-dir | Optional SQLite (`$CODEX_HOME/state.db`) + sidecar `session_index.jsonl` for thread names |
| Ephemeral mode | None | `codex exec --ephemeral` writes nothing |

## 9. Adapter checklist for `CodexSessionStore`

1. **Resolve home:** `$CODEX_HOME` (env) else `$HOME/.codex`. Sessions = `<root>/sessions`. Archived = `<root>/archived_sessions`.

2. **Glob:** walk `<sessions>/*/*/*/rollout-*.jsonl`. Parse filename via right-to-left UUID strategy (`parse_timestamp_uuid_from_filename`) to extract `(timestamp, uuid)`.

3. **Fast list:** for each file, read up to 10 non-empty lines; first must deserialize as `RolloutLine` whose `item` is `SessionMeta`. Index by `meta.id`.

4. **Full load:** `tokio::fs::read_to_string` + `for line in lines() { serde_json::from_str::<RolloutLine>(line) }`. Mirror `RolloutRecorder::load_rollout_items` (`recorder.rs:854-921`):
   - Skip empty lines.
   - Log + skip parse errors (don't fail whole session).
   - Strip legacy `ghost_snapshot` lines via `strip_legacy_ghost_snapshot_rollout_line`.

5. **Tail-watch:** file is flushed line-by-line. inotify `IN_MODIFY` + tracked byte offset works. New sessions don't materialize until first item recorded — periodically rescan day directory.

6. **Lineage:**
   - Forks: `SessionMeta.forked_from_id`.
   - Sub-agents: `SessionSource::SubAgent::ThreadSpawn { parent_thread_id, .. }`.
   - Resumes: same file, no extra link.

7. **Common message mapping** (Codex → Cassy abstract event):
   - `ResponseItem::Message{role, content}` → user/assistant/system/developer message (role is freeform string).
   - `ResponseItem::Reasoning` → assistant thinking block.
   - `ResponseItem::FunctionCall` ↔ `ResponseItem::FunctionCallOutput` joined by `call_id` → tool call + result.
   - `ResponseItem::CustomToolCall` / `CustomToolCallOutput`, `LocalShellCall`, `WebSearchCall`, `ImageGenerationCall`, `ToolSearchCall`/`ToolSearchOutput` → typed tool calls (some have no Claude equivalent — fall back to generic tool_call).
   - `EventMsg::PatchApplyEnd` → file-change event.
   - `EventMsg::ExecCommandEnd` → command-execution event (note truncation in Extended mode).
   - `EventMsg::ItemCompleted` for `TurnItem::Plan` → plan/todo update.
   - `RolloutItem::Compacted` → compaction marker.
   - `RolloutItem::TurnContext` → metadata-only update (cwd/model/policy changes); Cassy may want to surface "model changed mid-session".

## 10. Minimum serde structs to vendor (~150 LOC)

For a parser-only adapter, the minimum viable set:

- `RolloutLine { timestamp: String, #[serde(flatten)] item: RolloutItem }`
- `RolloutItem` (5-variant tagged enum: `session_meta` / `response_item` / `compacted` / `turn_context` / `event_msg`)
- `SessionMetaLine` (flattened `SessionMeta` + `git`)
- `SessionMeta` (id, forked_from_id, timestamp, cwd, originator, cli_version, source, agent_*, model_provider, base_instructions, memory_mode)
- `SessionSource` enum (Cli/VSCode/Exec/Mcp/Custom/Internal/SubAgent/Unknown) + `SubAgentSource::ThreadSpawn`
- `ResponseItem` (12 variants — but use `#[serde(other)] Other` for forward-compat, like Codex itself does)
- `ContentItem` (input_text / input_image / output_text)
- `CompactedItem`, `TurnContextItem` (cherry-pick fields you need)
- `EventMsg`: declare `#[serde(tag = "type")]` with **only** the variants you handle (`agent_message`, `user_message`, `agent_reasoning`, `task_started`/`turn_started`, `task_complete`/`turn_complete`, `token_count`, `patch_apply_end`, `exec_command_end`, `mcp_tool_call_end`, `web_search_end`, `plan_update`, `context_compacted`, `thread_name_updated`, `item_completed`) and a `#[serde(other)] Unknown` catch-all. The 70-variant enum is mostly never persisted.

Plus the legacy `ghost_snapshot` skip preprocessor (one extra line).

## 11. Reference implementation: `codex-rs/external-agent-sessions/`

Codex's own crate that **reads Claude Code sessions from `~/.claude/projects/`**. The symmetric problem to what Cassy wants. Worth reading end-to-end as a model:

- `detect.rs:19-26` — discovery walker pattern.
- `records.rs:153-168` — Claude session line parser.
- `ledger.rs` — "import each session only once" tracking.

Cassy's `CodexSessionStore` should mirror this structure.
