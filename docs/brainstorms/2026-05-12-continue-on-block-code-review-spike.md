# Spike: continueOnBlock for cas-code-review at task.close autofix path

**Date:** 2026-05-12  
**Task:** cas-8655  
**Author:** rapid-parrot-78  
**Verdict:** Not applicable — code review runs inside the MCP handler, not through PostToolUse hooks.

---

## 1. Call-site map

### a. cas-code-review skill invocation paths

| Path | File | Mechanism |
|---|---|---|
| `owner=worker` autofix at close | `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs` L~1180 | Worker passes `code_review_findings` JSON in `TaskCloseRequest`; `run_code_review_gate()` is called inline within the MCP `task.close` handler |
| `owner=supervisor` lightweight lint at close | `close_ops.rs` L~1084–1180 | `run_lightweight_structural_lint()` called inline; on pass, task transitions to `PendingSupervisorReview` |
| Supervisor interactive review | Supervisor invokes `cas-code-review` skill manually | LLM skill orchestration; no hook involvement |
| EPIC-merge integration sweep | Supervisor triggers at cherry-pick / epic merge | Same — skill-level orchestration |

### b. Cassy PostToolUse hook configuration

Registered by `cas init` via `cas-cli/src/cli/hook/config_gen.rs` L~224–241:

```json
{
  "PostToolUse": [{
    "matcher": "Write|Edit|Bash",
    "hooks": [{
      "type": "command",
      "command": "cas hook PostToolUse",
      "timeout": 30,
      "async": true
    }]
  }]
}
```

Key observations:
- **`async: true`** — fires in the background, cannot block execution, cannot return feedback.
- **Matcher is `Write|Edit|Bash`** — does NOT match `mcp__cas__task` (the tool name for `task.close`). The PostToolUse hook never fires for task close calls.
- `handle_post_tool_use()` (`cas-cli/src/hooks/handlers/handlers_middle/post_tool.rs`) handles observation recording only; it has no code-review logic.

---

## 2. Key question: does the autofix path use Claude Code PostToolUse semantics?

**No.** Code review runs entirely inside the MCP `task.close` handler.

The sequence for `owner=worker` (legacy autofix mode):

```
Worker LLM → invokes mcp__cas__task(action=close, code_review_findings=<JSON>)
  → MCP handler: run_code_review_gate(task, req, project_root)
      → evaluates findings JSON inline
      → if P0 found: returns MCP error string (tool output)
      → if clean: proceeds to close
  → Worker LLM sees the outcome as normal tool output
```

The sequence for `owner=supervisor` (default):

```
Worker LLM → invokes mcp__cas__task(action=close)
  → MCP handler: run_lightweight_structural_lint()
      → if lint fails: returns MCP error string
      → if lint passes: updates task status → PendingSupervisorReview → returns success
  → Worker LLM sees lint failure as normal tool output
```

In both cases, the review outcome is an **MCP tool result** — Claude already sees it as part of the normal tool invocation response. There is no PostToolUse hook in the path.

---

## 3. What is `continueOnBlock` and where it applies

`continueOnBlock: true` (Claude Code 2.1.139) is a PostToolUse hook field. When a synchronous (non-`async`) PostToolUse hook returns a non-zero exit code (blocking rejection), normally Claude Code hard-stops the tool turn. With `continueOnBlock: true`, the rejection reason is instead fed back to Claude as tool feedback, and the turn continues — allowing Claude to potentially fix the issue and retry.

Prerequisites for `continueOnBlock` to be meaningful:
1. A synchronous (non-`async`) PostToolUse hook that **blocks** (returns non-zero).
2. The hook matches the tool being called (e.g., its `matcher` covers the tool).
3. The hook's rejection reason is something Claude can act on to retry successfully.

Cassy's current setup fails all three prerequisites for `task.close`:
- The PostToolUse hook is `async: true` (cannot block).
- The matcher doesn't cover `mcp__cas__task`.
- Code review outcome is already delivered as MCP tool output — Claude sees it without any hook involvement.

---

## 4. Hypothetical integration design (not recommended)

If Cassy were to register a *synchronous* PostToolUse hook matching `mcp__cas__task` with `continueOnBlock: true`:

```json
{
  "PostToolUse": [{
    "matcher": "mcp__cas__task",
    "hooks": [{
      "type": "command",
      "command": "cas hook PostToolUse --check-close-review",
      "timeout": 30,
      "continueOnBlock": true
    }]
  }]
}
```

The hook could inspect the MCP response for code-review rejection markers and feed them back. However, this would:

1. **Duplicate logic**: The review outcome is already in the MCP tool output; Claude already sees it. Adding a hook that re-parses the same output adds complexity without new information.
2. **Race condition risk**: The PostToolUse hook fires after the MCP response; duplicating the gating decision in two places risks desync.
3. **Wrong layer for the autofix loop**: The `autofix` loop (Unit 7 in the skill) already orchestrates multi-round review + fix at the LLM level, which is more powerful than a single `continueOnBlock` hook feedback cycle.
4. **`owner=supervisor` mode makes this even more moot**: Workers now close to `pending_supervisor_review`; there is no P0-blocking review at worker close time under the default config.

---

## 5. Tangential value: lightweight lint feedback

The one place where `continueOnBlock` could add marginal value is the **lightweight structural lint failure** path (supervisor-owned mode). When lint fails, the error is returned as an MCP tool error. Claude already sees this and can retry. `continueOnBlock` on a PostToolUse hook would provide the same feedback through a different channel — no improvement.

The lint failure message is explicit and actionable already:
```
⚠️ LIGHTWEIGHT LINT FAILED
Worker close (supervisor-review mode) rejected by structural lint.
<specific lint message>
Fix the violations above and retry close.
```

No improvement from routing this through a hook instead.

---

## 6. Recommendation

**Do not integrate `continueOnBlock` into the cas-code-review close path.** The reasons:

1. Code review runs inline in the MCP handler, not through PostToolUse hooks. `continueOnBlock` is architecturally mismatched.
2. Claude already sees code-review outcomes as MCP tool output — the feedback loop is already working.
3. The `owner=supervisor` default (v2.13.0+) means workers don't hit P0-blocking reviews at close time at all; the use case is vanishingly narrow.
4. Implementing this would require Cassy to register a new synchronous PostToolUse hook on `mcp__cas__task`, duplicating gating logic that already lives in the MCP handler.

**No follow-on feature task filed.** The investigation is closed.

---

## 7. Related context

- `continueOnBlock` is more applicable to Cassy's **PreToolUse** hook path, where Cassy can block filesystem writes or dangerous Bash commands. If a rejection reason from that hook could guide Claude to rephrase the command, `continueOnBlock` could be valuable. This is a separate, future investigation.
- cas-b51a / cas-cac3 (v2.13.0): switched default to `owner=supervisor`. Workers no longer run the full multi-persona review inline — making the close-time autofix path legacy opt-in only.
