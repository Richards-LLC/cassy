---
from: Petra Stella Tools (pippenz @ /home/pippenz/Petrastella/petra_stella_tools)
date: 2026-04-21
priority: P1
---

# BUG: Factory supervisor Write/Edit still deadlocks on "Waiting for team lead approval" despite `f671890` fix

## Summary

The filesystem-tool auto-approve in `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs:768-775` (commit `52bd88d` / shipped in `f671890`) is *not* preventing the Claude Code team-mode leader-escalation deadlock for factory supervisors in real sessions. `Write(.claude/CODEMAP.md)` stalls at **"Waiting for team lead approval"** with the approval request being sent to the supervisor itself (`leadAgentId = supervisor@<team>`), producing the exact self-deadlock the fix was supposed to bypass.

`Bash(cp …)` succeeds under the same conditions — so the gap is narrowly scoped to the tools in `FACTORY_AUTO_APPROVE_TOOLS` that rely on the PreToolUse short-circuit.

## Environment

- Installed binary: `cas 2.0.0 (f671890 2026-04-21)` — contains the fix
- Team: `petra_stella_tools-crisp-octopus-83`
- Role: supervisor (I am the lead — `leadAgentId: supervisor@…`)
- Env at time of deadlock:
  - `CAS_AGENT_ROLE=supervisor`
  - `CAS_AGENT_NAME=jolly-falcon-60`
  - `CAS_FACTORY_MODE=1`
  - `CAS_ROOT=/home/pippenz/Petrastella/petra_stella_tools/.cas` (exists)
  - `CAS_SESSION_ID=2bbf124b-a044-4767-af91-e44dea96ffab`
- Harness bypass mode: **already on** at the time of the deadlock (Claude Code shows `⏵⏵ bypass permissions on`)
- Project `.claude/settings.json` PreToolUse hook registered with matcher `Read|Glob|Grep|Write|Edit|Bash|WebFetch|WebSearch|Task|SendMessage` ⇒ `cas hook PreToolUse`

## Repro

1. Spawn a factory session where the supervisor *is* the team lead (no separate `team-lead` member — standard Cassy factory layout).
2. As supervisor, call `Write` to create a file in the project.
3. Claude Code surfaces modal: `* Waiting for team lead approval` with `Permission request sent to team "<name>" leader`. The lead is the supervisor, which has no UX path to self-approve. Hangs indefinitely.
4. Working around with `Bash(cp /tmp/foo /home/.../foo)` succeeds immediately.

## What should have prevented it

```rust
// cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs:768
if is_factory_agent && FACTORY_AUTO_APPROVE_TOOLS.contains(&tool_name) {
    return Ok(HookOutput::with_pre_tool_permission(
        "allow",
        &format!("Factory agent auto-approve ({tool_name}) — bypasses …"),
    ));
}
```

All preconditions were met:
- `is_factory_agent(input)` → true (`CAS_AGENT_ROLE=supervisor` is non-empty)
- `tool_name == "Write"` ∈ `FACTORY_AUTO_APPROVE_TOOLS`
- `CAS_ROOT` resolves to a valid `.cas` directory

Yet Claude Code still surfaced the team-mode approval modal, implying either (a) `handle_pre_tool_use` returned `HookOutput::empty()` before reaching line 768, (b) the hook returned "allow" but Claude Code 2.1.116 ignored it in team-mode, or (c) the hook never ran.

## Suspected failure modes (unconfirmed — needs a maintainer with a debug build)

1. **Early return on `cas_root = None`** at `pre_tool.rs:61-64`: if the hook invocation resolves `cas_root` as `None` for any reason (Cassy not yet initialized in the supervisor's cwd at hook-dispatch time, symlink issue, race), the handler short-circuits to `empty()` before the factory bypass at line 768. The `Agent(isolation)` block above this check is hoisted specifically to avoid this class of miss — the factory-auto-approve block is not.
2. **Classifier order inside Claude Code**: the PreToolUse hook `permissionDecision:"allow"` may not pre-empt the team-mode leader-escalation path the way the fix assumes. The comment on line 749 already notes this suspicion for the settings-file path; the hook-level fix may share the same fate on some Claude Code builds.
3. **Hook transport**: the Claude Code harness may not propagate the `agent_role` field or read `CAS_AGENT_ROLE` into the hook process env on every invocation (tmux-backed long-lived sessions in particular).

## Ask

- Confirm reproducer on a fresh factory session.
- If hoisting line 768 above the `cas_root` check fixes it, that's a ~3-line patch.
- If PreToolUse "allow" isn't pre-empting team-mode in Claude Code 2.1.x as assumed, we need a Claude Code–side change or a different bypass surface (harness-level injection, `PermissionRequest` handler returning auto-approve for the same tool list, etc.). The `PermissionRequest` handler at `handlers_events/notifications.rs:3` already has auto-approve logic for task-context matches — extending it to cover factory-agent filesystem tools would be belt #3 and would catch the class of cases where PreToolUse short-circuiting fails.

## Workaround users are currently using

Use `Bash(cp ...)` or `Bash(cat > ...)` instead of `Write`/`Edit`. Acceptable for ad-hoc sessions but defeats the point of the tool split and confuses downstream hooks that key off tool name.

## Reference

- Commit with the intended fix: `52bd88d` / merged as `f671890`
- Existing memory: `project_cas_team_permission_escalation_bug`
- Related design notes in source: comments at `pre_tool.rs:729-767` and `handlers_tests/factory_auto_approve.rs:1-16`
