# BUG: Codex factory worker stalls in task-lifecycle gates, produces zero code

## Resolution

Resolved in current code before this request was processed:

- `cas-bbc2` made Codex agents self-contained by spawn-injecting the CAS MCP server in `crates/cas-pty/src/pty.rs` via `mcp_servers.cs.command="cas"` and `mcp_servers.cs.args=["serve"]`, so workers no longer have to reverse-engineer or hand-launch `cas serve`.
- `cas-bbc2` also rewrote the Codex worker startup path to choose exactly one assigned task, start only that task, implement/commit/push/close it, and avoid batch-starting multiple assignments into the one-unverified-in-progress jail.
- `cas-3522` injects `CAS_SESSION_ID` into the Codex `cs` MCP server environment, allowing first-tool-call auto-registration and removing the old `session_start` dance.
- `cas-8aaf` injects `CAS_FACTORY_MODE=1` and `CAS_FACTORY_WORKER_CLI=codex` into the Codex `cs` MCP server environment, so verification-jail and close guidance use the Codex-capable `mcp__cs__*` aliases.
- `Pty::spawn` now preflights Codex spawns and refuses loudly when the spawn-injected `cas` MCP binary is not on `PATH`, instead of launching a tool-less worker.

Verification evidence:

- `crates/cas-pty/src/pty.rs` contains regression tests for Codex MCP injection, one-task startup wording, session-id injection, factory env injection, and the absence of `session_start`.
- `cas-cli/tests/mcp_tools_test/task_tools/verification_flow.rs` verifies Codex verification-jail guidance uses `mcp__cs__coordination`.

**Date:** 2026-06-23
**Reporter:** supervisor (factory session, epic cas-167a)
**Severity:** High — `cli=codex` factory workers are effectively non-functional for multi-task assignments
**Evidence:** `./BUG-codex-worker-stalls-in-task-lifecycle-gates.codex-rollout-2026-06-23.jsonl` (full Codex session rollout, 194 lines)

## Summary

A factory worker spawned with `cli=codex` (model **gpt-5.4 medium**) consumed an entire ~5+ minute
session on Cassy task-lifecycle coordination and produced **zero implementation** — no file edits, no
commits, and it never even reset its worktree to the assigned epic base. It marked two assigned tasks
`InProgress`, attempted to start a third, hit the "one unverified in-progress task" gate, interpreted
that expected gate as a hard blocker, and halted.

A Claude (`cli=claude`) worker on the sibling lane of the same epic completed 5 tasks end-to-end in the
same window. The failure is specific to the Codex worker integration.

## Spawn

```
mcp__cas__coordination action=spawn_workers count=1 cli=codex isolate=true worker_names=posthog-codex
```

Three tasks were pre-assigned to `posthog-codex` (cas-5192, cas-e582, cas-dafb — all small, well-specified,
file-level instructions provided via supervisor message).

## What happened (from the rollout transcript)

- Tool-call breakdown for the whole session: ~22 `exec_command` + 8 `write_stdin` + 1 `_get_user_login`.
  **Zero `apply_patch`/edit/write calls. Zero git commits. No branch pushed to origin.**
- The worker's own final message:
  > "Right now I'm not implementing code yet. I completed Cassy worker startup and task sync. … `cas-e582`
  > is `InProgress`, `cas-5192` is `InProgress`, `cas-dafb` is still `Open`. I attempted to start
  > `cas-dafb`, but Cassy blocked it because there is already an unverified in-progress task (`cas-e582`)."
- Git ground truth at shutdown: worktree HEAD still at `903401e1c` (staging) — it **never reset to the
  assigned epic branch** despite instructions to do so; `factory/posthog-codex` never pushed.

## Root causes (hypotheses)

1. **Startup instructions drive coordination over implementation.** The injected Codex worker
   `developer_instructions` ("On startup run session_start … then `task mine`. For assigned tasks run
   `task show` then `task start` before coding.") lead the worker to enumerate and **`start` multiple
   assigned tasks up front** rather than implement one fully, then start the next. It batch-started 2,
   then gate-blocked on the 3rd.
2. **Expected gate read as a terminal blocker.** The "unverified in-progress task" gate is *normal*
   sequential-work backpressure. The Codex worker treated it as a stop condition and reported a blocker
   instead of proceeding to implement an already-in-progress task.
3. **Possible gate inconsistency.** Cassy allowed **two** tasks to be `InProgress` simultaneously for one
   worker but blocked the **third**, citing the first. If the intended invariant is one-in-progress-per-
   worker, the second start should have been blocked too; if multiple are allowed, the third should not
   have been blocked. Worth confirming the intended rule.
4. **No transition from coordination to work.** Nothing in the Codex path nudges the worker from
   "synced + started" into actually editing files; it spent its budget on `exec_command` round-trips.

## Impact

- `cli=codex` workers cannot be trusted with multi-task lanes; they stall before writing code.
- Wasted a full worker slot + ~5 min while an epic-critical lane (Lane B of cas-167a) made no progress.
- Required supervisor intervention: kill the Codex worker, reset the two falsely-`InProgress` tasks
  (`action=reset`), and reassign the lane to a Claude worker.

## Suggested fixes

1. Rework the Codex worker startup instructions to **start exactly one task, implement it to commit+push,
   then loop** — never `start` more than one task at a time.
2. Teach the Codex worker that the in-progress gate is expected: on hitting it, **continue implementing
   the current in-progress task** rather than reporting a blocker.
3. Add an explicit "after sync, your FIRST action is to edit code for task N" step so the worker leaves
   the coordination phase.
4. Confirm/repair the one-in-progress-per-worker invariant (it allowed 2, blocked the 3rd).
5. Consider a supervisor-owned-lifecycle mode for Codex workers (worker only codes/commits/pushes;
   supervisor owns start/close) — this was the operator mitigation that unblocked the session.

## Repro

1. `spawn_workers cli=codex isolate=true` with 3 pre-assigned tasks.
2. Observe the worker run startup + `task mine` + `task start` on multiple tasks, hit the in-progress
   gate, and halt without editing any files.
3. Confirm via git that the worktree was never reset to the assigned base and nothing was committed.

## Related

- `BUG-factory-liveness-signals-disagree.md` (same session also reproduced the [stale]-vs-live-process
  inversion — supervisor verified live workers with `ps` before any cleanup).

---

## SUPERVISOR ANALYSIS (2026-06-23, wild-falcon-36) — corrected root cause

I read the full 194-line rollout. **The reported "stalls in lifecycle gates" framing is a symptom, not
the disease.** The actual root cause is an MCP-wiring gap; the verification jail behaved exactly as
designed.

### What the transcript actually shows

The Codex worker **never had the CAS MCP tools mounted.** Its own narration is explicit:
- [16] "The Cassy tools weren't in the initial tool list"
- [28] "The requested MCP endpoints still aren't exposed in this session"
- [97] "the `cs`/`cas` MCP tools still aren't mounted in this session. I'm using the local `cas serve`
  MCP server directly … instead of faking state"

With no `mcp__cs__*` tools, the worker reverse-engineered the JSON-RPC wire protocol from the cas-src
Rust source + the `rmcp` crate, **hand-spawned its own `cas serve`** (exec_command, session 92703), and
drove raw `initialize` / `tools/list` / `tools/call` frames over stdin. That bootstrap consumed the
entire budget — hence zero edits/commits/push. Heroic, but the worker was solving a problem that should
never have existed.

### Root cause (confirmed in code)

`PtyConfig::codex` (`crates/cas-pty/src/pty.rs:250`) builds the worker command with model, effort,
`--config developer_instructions=…`, and a startup prompt — **but injects no MCP server config and sets
no `CODEX_HOME`.** Codex does not read Claude's `.mcp.json`; it discovers `[mcp_servers.cs]` from a
`.codex/config.toml`. That file is written only by `cas update` / `cas init`
(`configure_codex_mcp_server`, `config_gen.rs:526`), never at factory spawn.

This factory ran in **gabber-studio**, which has `.mcp.json` (Claude) but **no `.codex/` directory at
all** — it was integrated for the Claude harness, never for Codex. So the Codex worker had no reachable
Cassy server by any path. A Claude worker on the sibling lane worked because `.mcp.json` was present.

### Verdict on the report's hypotheses

- **#1 (startup prompt batch-starts tasks): VALID, secondary.** The worker startup prompt step 4 says
  "show/**start each** task." Even with MCP working, a multi-task lane would start task 1, then hit the
  one-unverified-in-progress jail on task 2. Worth fixing independently (start ONE → implement →
  commit/push → close → loop).
- **#2 (gate read as terminal blocker): INVALID.** The worker correctly understood the gate; it simply
  had no tools and no budget left.
- **#3 ("allowed 2, blocked 3rd" = gate inconsistency): essentially a NON-bug.** The worker pipelined
  all three `start` calls in one raw stdin write (ids 9/10/11) before reading any response. The
  "allowed 2" is a read-after-write race in its hand-rolled client; normal serialized tool calls can't
  reach it. The gate itself is correct — it properly blocked the later dafb retry (id 16) citing
  cas-e582. Low-priority hardening at most.
- **#4 (no coordination→work transition): symptom of the MCP gap, not a separate cause.**

The `[Cassy] Serve panic log: …` line is the normal `cas serve` startup banner, **not** a panic.

### Fix direction (in-repo — cas-src IS Cassy)

1. **Primary — spawn-inject the CAS MCP server into the Codex command** in `PtyConfig::codex`, parallel
   to `developer_instructions`: `-c mcp_servers.cs.command=cas`, `-c 'mcp_servers.cs.args=["serve"]'`,
   `-c mcp_servers.cs.env.CAS_CODEX_FALLBACK_SESSION=1`. Makes Codex workers self-contained regardless
   of downstream project integration state. (Note: the same gap hits a **Codex supervisor** too.)
2. **Belt-and-suspenders — spawn preflight:** if neither injection nor a project `.codex/config.toml`
   provides the server, **refuse loudly** ("run `cas init`/`cas update` to enable the Codex harness
   here") instead of spawning a worker that flails silently.
3. **Prompt fix (independent):** rewrite `CODEX_WORKER_INSTRUCTIONS` / startup prompt to start exactly
   one task at a time.

Tracking: see Cassy bug task created 2026-06-23 (linked from the team-lead report).
