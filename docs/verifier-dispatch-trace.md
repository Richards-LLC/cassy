# Verifier dispatch trace — `cas_task_close` regression memo

Task: cas-09f3 (epic cas-cd8b)
Author: ready-sparrow-3
Date: 2026-04-08

## TL;DR

`cas_task_close` **never dispatches a verifier**. On the "no verification yet"
branch it sets `pending_verification=true` on the task, emits a
`⚠️ VERIFICATION REQUIRED` tool_error containing instructional text, and
returns. Actual verifier execution depends entirely on the host harness
(Claude Code) reading that text and the agent voluntarily calling
`Task(subagent_type="task-verifier", ...)`. In factory-worker sessions the
`VERIFICATION_JAIL_BLOCKED` enforcement that used to make this unavoidable
was removed on 2026-04-03 (commit `bba6fbf`), so nothing now compels the
worker to spawn the verifier — close just keeps failing with the same
warning and the task stays stuck in `pending_verification=true`.

## 1. File:line references

### Warning emission (the `⚠️ VERIFICATION REQUIRED` text)

- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs:261-287`
  — `Self::tool_error(format!("⚠️ VERIFICATION REQUIRED …"))` inside the
  `Ok(None) | Ok(Some(_))` arm (no approved verification found).
- Sets `task.pending_verification = true` at line 200 before returning.
- Sibling warning for already-rejected verifications at
  `close_ops.rs:151-182` (`⚠️ VERIFICATION FAILED`).

### Verifier spawn site (there isn't one)

- **None in `close_ops.rs`.** Grep for `"task-verifier"` across
  `cas-cli/src` returns only:
  - String literals used to format instructions to the human/agent
    (`close_ops.rs:102,104,163,173,246,255,273,281`,
    `mcp/server/mod.rs:669-670`,
    `task/lifecycle.rs:289,295`,
    `agent_coordination/task_claiming.rs:160,166`).
  - Hook handlers that *react* to the harness having spawned a verifier:
    `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs:164-242`
    (detect `Task`/`Agent` tool-use with
    `subagent_type == "task-verifier"`, write an unjail marker, clear
    `pending_verification`).
  - `handlers_state.rs:487-508` SubagentStart hook, same detection.
  - Builtin asset at
    `cas-cli/src/builtins/agents/task-verifier.md` — the agent
    definition the harness loads, not a dispatcher.
- Dispatch model: out-of-band. The MCP server only sets a flag and
  returns text; the Claude Code harness is expected to invoke the
  subagent via its own `Task`/`Agent` tool. There is no in-process
  "spawn verifier, await result, return" path.

### Close handler

- Entry: `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs:8`
  — `CasCore::cas_task_close`.
- Verification gate begins at `close_ops.rs:94`
  (`if verification_enabled && !assignee_inactive`).
- Worktree-merge jail (unrelated but adjacent): `close_ops.rs:296-349`.
- Actual mutation path (after gate passes): `close_ops.rs:351-574`.

### Jail enforcement (the path that WAS the `VERIFICATION_JAIL_BLOCKED` signal)

- `cas-cli/src/mcp/server/mod.rs:617-676`
  `CasCore::authorize_agent_action`.
- Error string at `server/mod.rs:667-671`
  — `"VERIFICATION_JAIL_BLOCKED: …"`.
- Factory-worker exemption at `server/mod.rs:646-655`
  (`is_factory_worker → return Ok(())`).
- Supervisor exemption at `server/mod.rs:640-644`.

## 2. Regression commit

`bba6fbf29a89cf64ffe5b682fc2576fc5f2c9863` — "Fix verification jail
cascade + close 9 resolved issues + fix test env leaks" (2026-04-03).

Relevant hunk (`cas-cli/src/mcp/server/mod.rs`, inserted before the
agent-jail check):

```rust
+ // Factory workers are exempt — they may have multiple tasks and must continue
+ // working while one awaits verification. close_ops.rs still blocks re-closing
+ // unverified tasks, so data integrity is maintained.
+ let is_factory_worker = std::env::var("CAS_AGENT_ROLE")
+     .map(|r| r.eq_ignore_ascii_case("worker"))
+     .unwrap_or(false)
+     && std::env::var("CAS_FACTORY_MODE").is_ok();
+ if is_factory_worker {
+     return Ok(());
+ }
```

The commit message's stated safety net — *"close_ops.rs still blocks
re-closing unverified tasks, so data integrity is maintained"* — is only
half-true. `close_ops.rs` does still block re-close, but it does so by
returning `⚠️ VERIFICATION REQUIRED` text without any mechanism that
causes a verifier to actually run. Before `bba6fbf`, the hard jail in
`authorize_agent_action` was what forced the harness (via
`pre_tool.rs:164-242`) to take the verifier-spawn path. Removing the
jail for factory workers removed the only load-bearing lever.

No single commit *introduced* the text-based dispatch — that design has
existed since `1b7889b` (initial release). The *regression* for factory
workers is `bba6fbf` removing enforcement; the deeper architectural
issue is that dispatch was always out-of-band and only worked because
the jail incidentally forced compliance.

`4ecf61b` (2026-04-03, "Improve factory task lifecycle") is adjacent in
time but only touched the epic-subtasks-complete daemon event in
`close_ops.rs:505-521` — not the verification gate. Not the regression.

## 3. Recommended fix direction

**Synchronous verify-and-return, invoked from `cas_task_close` itself.**

When `cas_task_close` reaches the "no approved verification found" arm
(`close_ops.rs:184`), it should:

1. Run the verifier in-process against the task and the proposed
   `close_reason`, write a `Verification` record via
   `verification_store.add(...)`, and then fall through to the normal
   close path (or return a structured rejection with issues).
2. Not set `pending_verification=true` and not return instructional
   text. The verifier's decision is the close's decision.

### Why sync over async

- **Eliminates the out-of-band dispatch problem.** Today's design
  assumes the host harness will read a tool_error string, parse it, and
  voluntarily spawn a specific subagent. That coupling is fragile: it
  breaks in any non-Claude-Code harness, in factory workers that now
  bypass the jail (`bba6fbf`), and in any flow where the agent
  misreads the instruction or silently gives up.
- **Matches the contract callers already expect.** `cas_task_close`
  returns one of: success, `MERGE REQUIRED`, `WORKTREE MERGE REQUIRED`,
  `VERIFICATION FAILED`. All of those are terminal decisions made
  inside the handler. Verification being the sole exception — a
  "please go run something and call me back" — is what every incident
  in recent memory has tripped over.
- **Factory-worker jail exemption stops mattering.** With sync verify,
  there is no pending-verification window where a worker needs
  mutating access *and* is expected to spawn a subagent. The
  `bba6fbf` exemption becomes a vestigial safeguard rather than the
  load-bearing lever.
- **Observable status is still achievable** — write the
  `Verification` record before returning, and the existing UI/task
  panels see the outcome. Async adds a state machine
  (`pending → running → approved|rejected`) with extra timeout, lease,
  and recovery logic for no practical gain in this code path; verify
  runs are short and bounded.

Async would only be justified if verification becomes expensive enough
to block the MCP call budget (multi-minute LLM runs, multi-file
analysis at scale). Nothing in the current `task-verifier` agent
suggests that; it reads the task, the diff, and records a verdict.

## 4. Complications

1. **There is no in-process verifier.** `task-verifier` is a Claude
   Code *subagent* definition
   (`cas-cli/src/builtins/agents/task-verifier.md`). It is LLM-driven
   and currently reachable only by the harness spawning
   `Task(subagent_type="task-verifier", ...)`. A sync path needs
   either:
   - An MCP-callable verification entrypoint the harness can invoke
     *before* calling `task close` (and `cas_task_close` refuses if no
     fresh verification exists — which is what it already does), **or**
   - A verifier that can run headless from the cas server (e.g. a
     dedicated agent invocation through a cloud client / local LLM),
     which is a larger piece of infrastructure.
   The first option is the minimum viable fix and is essentially what
   the codebase was doing before `bba6fbf` — enforce via jail, let the
   harness spawn the verifier, let close succeed once
   `VerificationStatus::Approved` exists. Revisiting the jail removal
   is probably cheaper than building a headless verifier.

2. **Multiple hook surfaces need to stay in sync.** Even today the
   "spawned verifier" flow touches three layers: `close_ops.rs` (sets
   the flag), `authorize_agent_action` in `server/mod.rs` (the jail),
   and `pre_tool.rs` + `handlers_state.rs` (detect the subagent, clear
   the flag, write the unjail marker at
   `pre_tool.rs:224, 310`). Any fix must leave these consistent; the
   `bba6fbf` change broke that consistency by exempting workers from
   only one of the three layers.

3. **Supervisor self-assignee path** (`close_ops.rs:86-92, 244-251`)
   and **orphaned-task supervisor bypass** (`close_ops.rs:66-82`)
   already allow close-without-verification in specific cases — any
   redesign has to preserve those exits or it will re-introduce the
   deadlocks fixed in `bb47138` and `c60d269`.

4. **`is_worker_without_subagents_from_env()`** (referenced at
   `close_ops.rs:95,161,271`) implies there are harnesses where the
   worker *cannot* spawn a subagent at all. In those harnesses the
   current design degrades to "ask supervisor to verify for you",
   which also relies on the out-of-band flow working. A sync fix
   needs to decide what happens for these workers: probably "message
   supervisor automatically via daemon event, block close", which is
   closer to the pre-`bba6fbf` jail behavior.

## 5. Close-reason content filter

**There is no Rust-side content filter.** Grep for `"remaining"`, `"beyond scope"`, `"will need to"`, `"admits incomplete"` across `cas-cli/src` returns only:

- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs:145` — string literal inside the `VERIFICATION FAILED` message, instructing the *agent* what language to avoid on resubmit.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs:215` — string literal inside the `VERIFICATION REQUIRED` message, instructing the *verifier subagent* to reject such language.
- `cas-cli/src/builtins/agents/task-verifier.md:33, 329, 346` — the same rule, written as prompt instructions to the LLM subagent.
- Identical copies in `cas-cli/src/builtins/codex/agents/task-verifier.md`.

The "filter" is a natural-language rule baked into the task-verifier agent's system prompt. `cas_task_close` itself performs **no string analysis** on `req.reason`. This reinforces §3's root cause: every part of the verification gate that was supposed to catch bad closes lives inside the subagent, so if the subagent never runs, nothing is enforced. For cas-7c37 downstream: the fix-site is either (a) port the rejection rules into Rust inside `cas_task_close` (cheap, deterministic, loses nuance), or (b) ensure the subagent actually runs (the real fix).

## 6. Open questions

1. **Was bba6fbf's factory-worker exemption a mistake or intentional scope creep?** The commit message claims "close_ops.rs still blocks re-closing unverified tasks, so data integrity is maintained" — but that claim only holds if the worker eventually spawns a verifier. In practice nothing triggers that spawn once the jail is gone. Did anyone verify the end-to-end flow after that commit, or was it closed based on the "other tools work again" symptom?
2. **Do factory workers in OpenClaw / prisma-phase2 even have the `Task` tool available in their harness?** The prisma-phase2 log shows them successfully calling `Task(subagent_type="task-verifier", ...)`. The OpenClaw log shows four close attempts with no matching Task call. Is the harness config different (e.g. sub-agent dir not mounted into the worker's `.claude/`), or did the worker just ignore the text instruction? cas-09f3 can't answer this from cas-cli source alone.
3. **Should the sync fix revive `VERIFICATION_JAIL_BLOCKED` for workers, or move to an in-handler verifier invocation?** Reviving the jail is a near-revert of `bba6fbf` (plus a real fix for whatever "verification cascade" bug motivated the removal). In-handler invocation requires a headless verifier runtime. Supervisor needs to pick; I've recommended revive-the-jail as the MVP in §3/§4 but it's not clearly dominant.
4. **What was the "verification cascade" that `bba6fbf` was trying to fix?** The commit message says "Factory workers were blocked from ALL tools when any single task triggered verification jail." Is the underlying bug that `check_pending_verification` returns jail for *any* in-progress leased task, even ones the worker isn't currently closing? If so, the sync fix can be narrower: only jail the specific `close` call, not unrelated mutations. That's probably the correct regression fix *and* preserves data integrity without removing the lever.
5. **`is_worker_without_subagents_from_env()` branch** (`close_ops.rs:95`) — what harness sets that? The text path it emits asks the worker to message supervisor. Does the supervisor have any automation that picks up `worker_verification_blocked` daemon events (emitted at `close_ops.rs:231-239`) and acts on them? If yes, that's a viable alternate dispatch path worth preserving; if no, that code branch is also dead.

## Appendix: call path summary

```text
mcp__cas__task action=close id=X
  └─ cas_task_close (close_ops.rs:8)
      ├─ get(task)                                      :14
      ├─ Epic unmerged-branch check                     :22-44
      ├─ verification_enabled?                          :46-62
      ├─ assignee_inactive / supervisor_is_assignee     :64-92
      ├─ if verification_enabled && !inactive:          :94
      │    open_verification_store                      :98
      │    get_latest_for_task(_by_type)                :108-112
      │    match:
      │      Approved  → fall through to close          :115
      │      Rejected  → return "VERIFICATION FAILED"   :118-182
      │      None/other→ set pending_verification=true  :184-200
      │                   emit daemon WorkerActivity    :231-239
      │                   return "VERIFICATION REQUIRED":261-287
      │                   *** NO VERIFIER SPAWNED ***
      ├─ Worktree merge jail                            :296-349
      └─ Close + notes + unblock + epic rollup          :351-574
```

```text
any mutating MCP tool call
  └─ authorize_agent_action (server/mod.rs:617)
      ├─ supervisor? exempt                              :640-644
      ├─ factory worker? exempt  ← bba6fbf regression    :646-655
      └─ check_pending_verification → VERIFICATION_JAIL_BLOCKED :663-673
```
