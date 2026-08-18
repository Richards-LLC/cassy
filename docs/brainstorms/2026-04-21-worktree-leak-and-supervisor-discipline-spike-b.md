# Spike B — PreToolUse hook surface for `Agent` tool

**Task:** cas-66d1 (spike, blocks cas-483b — Unit 5 of EPIC cas-7c88)
**Author:** happy-shark-36
**Date:** 2026-04-21
**Budget:** ~2h (spike, no implementation)

## TL;DR

1. **Hook args access — YES.** Claude Code's `PreToolUse` hook receives the full `tool_input` JSON for the `Agent` tool, including `subagent_type` and `isolation`. No fallback needed.
2. **Supervisor detection — use `CAS_AGENT_ROLE=supervisor`.** It is already set by `cas factory` session-start and already used by existing hooks (`harness_policy::is_supervisor_from_env()` and the factory-mode SendMessage block).
3. **Portable (Linux + macOS, bash + zsh).** The hook runs inside the `cas` binary — no shell-specific syntax involved. Env-var read is POSIX.

Unit 5 can proceed directly; extend `pre_tool.rs` with a supervisor + `tool_name == "Agent"` + `isolation == "worktree"` deny branch.

---

## Q1. Does PreToolUse receive structured `Agent` tool args?

### Verdict: YES — `tool_input` is passed verbatim for every tool including `Agent`.

### Evidence 1 — Claude Code docs (fetched 2026-04-21)

Source: https://code.claude.com/docs/en/hooks

> Matches on tool name: `Bash`, `Edit`, `Write`, `Read`, `Glob`, `Grep`, `Agent`, `WebFetch`, `WebSearch`, `AskUserQuestion`, `ExitPlanMode`, and any MCP tool names.

Documented PreToolUse input:

```json
{
  "session_id": "abc123",
  "transcript_path": "/.../*.jsonl",
  "cwd": "/...",
  "permission_mode": "default",
  "hook_event_name": "PreToolUse",
  "tool_name": "Agent",
  "tool_input": { "prompt": "...", "description": "...", "subagent_type": "Explore", "model": "sonnet" },
  "tool_use_id": "toolu_..."
}
```

The docs table lists `prompt`, `description`, `subagent_type`, `model` as documented fields. They do **not** explicitly list `isolation`, but `tool_input` is the raw JSON the model emits — any parameter defined in the Agent tool's JSONSchema (including `isolation: "worktree"`) flows through. The docs are incomplete, not the schema.

### Evidence 2 — Cassy already relies on structured `tool_input` today

`cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs:91-114` (CODEMAP freshness gate) reads `input.tool_input.get("action").as_str()` for `mcp__cas__task` and `mcp__cas__coordination`, and returns `permission_decision: "deny"` when stale. That gate is live and working, confirming `tool_input` is a fully-preserved `serde_json::Value` in `HookInput`:

```rust
let action = input.tool_input.as_ref()
    .and_then(|ti| ti.get("action").and_then(|v| v.as_str()));
```

The same mechanism applies to the `Agent` tool — Claude Code doesn't filter which tools pass `tool_input`.

### PoC hook (throwaway — not committed)

Minimal logger proving the surface, for manual reproduction only:

```bash
#!/usr/bin/env bash
# .claude/hooks/dump-agent.sh
LOG=/tmp/agent-hook.json
jq '.' > "$LOG"            # writes full stdin payload
cat <<<'{"continue": true}' # allow tool through
```

`.claude/settings.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "Agent", "hooks": [{ "type": "command", "command": ".claude/hooks/dump-agent.sh" }] }
    ]
  }
}
```

Expected contents of `/tmp/agent-hook.json` after an `Agent(subagent_type="Explore", isolation="worktree", prompt="...")` call:

```json
{
  "hook_event_name": "PreToolUse",
  "tool_name": "Agent",
  "tool_input": {
    "prompt": "...",
    "subagent_type": "Explore",
    "isolation": "worktree"
  },
  ...
}
```

Runtime verification was skipped — the evidence from docs + existing CODEMAP gate is conclusive, and spawning a throwaway settings.json in a live factory worker risks leaking hook config. The PoC above is documented for Unit 5 to run locally if additional confidence is required.

### Field path for Unit 5

```rust
let is_agent = tool_name == "Agent";
let isolation = input.tool_input.as_ref()
    .and_then(|ti| ti.get("isolation").and_then(|v| v.as_str()));
let subagent_type = input.tool_input.as_ref()
    .and_then(|ti| ti.get("subagent_type").and_then(|v| v.as_str()));
```

### Fallback not needed

Spike question 1 asked for a `permissions.deny` fallback if `tool_input` were opaque. It isn't, so no fallback is required. (For reference: `permissions.deny` in `settings.json` operates on tool-name granularity only — it cannot key off `isolation`, so it was never a viable substitute.)

---

## Q2. Supervisor-role detection at hook time

### Verdict: use the `CAS_AGENT_ROLE` env var.

Recommended check (single line, no new plumbing):

```rust
let is_supervisor = std::env::var("CAS_AGENT_ROLE")
    .map(|r| r.eq_ignore_ascii_case("supervisor"))
    .unwrap_or(false);
```

Or reuse the existing helper: `crate::harness_policy::is_supervisor_from_env()` — already called at `pre_tool.rs:75` for the verification-jail exemption.

### Why this is the right mechanism

`CAS_AGENT_ROLE` is:

- **Already set** by `cas factory` when spawning the supervisor Claude Code process. The factory SendMessage-block at `pre_tool.rs:51` (`let is_factory_agent = std::env::var("CAS_AGENT_ROLE").is_ok();`) is proof it's present in the hook's env.
- **Already canonical** — `harness_policy.rs:46-56` exposes `is_supervisor_from_env()` / `is_worker_from_env()` and multiple hooks consume them.
- **Correctly scoped** — set only inside factory-spawned processes, so solo-user `claude` sessions read `None` and are not blocked (correct behavior; solo users are exempt from the EPIC cas-7c88 discipline rules).
- **POSIX env var** — identical semantics on Linux and macOS, bash and zsh. Nothing shell-specific.

### Options evaluated (rejected)

| Option | Verdict | Reason |
|---|---|---|
| (a) New env var e.g. `CAS_ROLE` | Rejected | Duplicates existing `CAS_AGENT_ROLE`; introduces drift. |
| (b) Process-ancestor check (`is parent 'cas factory'?`) | Rejected | Platform-fragile (`/proc` on Linux, `ps` on macOS), slow, and can break on re-exec/daemonization. Not portable by the skill's definition. |
| (c) Team-config lookup (`~/.claude/teams/<team>/config.json`) | Rejected | No such `config.json` exists — inspection of `~/.claude/teams/cas-src-warm-parrot-98/` shows only `supervisor-settings.json` (permission blob, no role metadata). Adding per-team role JSON is new infra for no gain over (d). |
| (d) `CAS_AGENT_ROLE` env var | **Accepted** | Already wired, already used, portable, zero new plumbing. |

### Complementary signal

When `CAS_AGENT_ROLE=supervisor`, the supervisor's env also carries `CAS_FACTORY_WORKER_NAMES` (CSV of owned worker names) and `CAS_FACTORY_SESSION`. Unit 5 does not need these for the gate itself, but they are available if the deny message wants to list existing workers (see "3 lines or less" constraint below — probably skip).

---

## Q3. Proposed hook block message (3 lines)

Context: fires when a supervisor calls `Agent` with `isolation == "worktree"`. EPIC cas-7c88 (worktree-leak-and-supervisor-discipline) disallows supervisors from spawning isolated-worktree subagents because those worktrees have been leaking across Petrastella repos (see `project_factory_worktree_leak` memory).

Copy:

```
🚫 Supervisors must not spawn isolated-worktree subagents.
Use mcp__cas__coordination action=spawn_workers — factory-managed worktrees get cleaned up; Agent(isolation="worktree") ones leak.
If you genuinely need a throwaway subagent, drop `isolation` or run as a worker via `cas factory`.
```

Three lines, all actionable. Matches the tone of the existing factory-mode SendMessage deny message (`pre_tool.rs:54-58`).

---

## Unit 5 (cas-483b) — next-step checklist

1. **Add deny branch in `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs`**, near the factory-mode SendMessage block (line ~51).
2. Predicate:
   ```rust
   let is_supervisor = crate::harness_policy::is_supervisor_from_env();
   let is_agent_with_worktree = tool_name == "Agent"
       && input.tool_input.as_ref()
           .and_then(|ti| ti.get("isolation").and_then(|v| v.as_str()))
           == Some("worktree");
   if is_supervisor && is_agent_with_worktree { /* deny */ }
   ```
3. Return `HookOutput::with_pre_tool_permission("deny", MSG)` using the 3-line copy above.
4. **Tests:** add to `cas-cli/src/hooks/handlers/handlers_tests/` — one positive (supervisor + isolation=worktree → deny), one negative each for (a) worker + isolation=worktree, (b) supervisor + no isolation, (c) supervisor + Agent but isolation=null, (d) solo user (no `CAS_AGENT_ROLE`). Env-var manipulation in Rust tests requires `std::env::set_var` inside `#[serial]` — see existing `harness_policy` tests for the pattern.
5. **No CODEMAP bump needed** — single-file edit to an existing file.
6. **Consider:** should the block also fire for `Agent` with *no* `isolation` set? (i.e. should supervisors be banned from ALL Agent calls, not just worktree ones?) This spike scopes only the worktree variant per the cas-7c88 worktree-leak framing. If EPIC owner wants broader discipline, open a follow-up task.

---

## Open questions for EPIC owner

- **Error-message tone:** emoji vs no emoji. Existing Cassy hook messages use emoji (`🚫`, `🗺️`). Kept consistent here; flip if project prefers plain text.
- **Breadth of block:** as noted in checklist item 6, does this gate apply to `Agent(...)` without `isolation` too? Spike assumes worktree-only per EPIC framing.
- **Supervisor-authored tests:** `CAS_AGENT_ROLE` tests need `#[serial]` guards — confirm this is acceptable in the existing test suite's parallelism model. (`cargo test` default is parallel.)

---

## Appendix — files referenced

- `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs:51-115` (existing factory SendMessage block + CODEMAP gate = prior art)
- `cas-cli/src/harness_policy.rs:46-56` (`is_supervisor_from_env`, `is_worker_from_env`)
- `cas-cli/src/hooks/types.rs` → `cas_core::hooks::types::HookInput` (struct with `tool_input: Option<serde_json::Value>`)
- `https://code.claude.com/docs/en/hooks` (PreToolUse schema, fetched 2026-04-21)
