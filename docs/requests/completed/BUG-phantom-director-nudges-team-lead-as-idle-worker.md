---
from: gabber-studio team (factory supervisor zen-hawk-97 / supervisor@gabber-studio-quiet-panda-24, pippenz @ /home/pippenz/Petrastella/gabber-studio)
date: 2026-06-26
priority: P2
---

# Phantom "director" coordinator messages the team-lead as an "idle worker" and tells it to self-assign tasks — on a single-session run the user never started a teammate for

## Resolution (cas-c9f0, 2026-07-02)

Resolved and moved to completed after verifying each acceptance criterion against current code.

- Role-gated idle nudges / no supervisor self-assignment: already fixed and hardened by cas-c9f0. The event detector excludes the supervisor/team lead from `WorkerIdle` tracking in `cas-cli/src/ui/factory/director/events.rs:615`-`623` and `events.rs:936`-`949`; the prompt layer suppresses supervisor idle events in `cas-cli/src/ui/factory/director/prompts.rs:207`-`214` and suppresses supervisor registration nudges in `prompts.rs:308`-`311`. Idle assignment guidance uses the worker's display name (canonical — `task mine` matches on it; a session-id assignee is silently normalized back to the display name by cas-dbbb, `update.rs:176`-`186`), not the session id, in `prompts.rs:199`-`294`, with `test_worker_idle_assignee_uses_display_name` at `prompts.rs:913`-`933`.
- Config-only director identity/color: already fixed in current code. The director member is explicitly documented as the daemon's system/auto-prompt identity in `cas-cli/src/ui/factory/daemon/runtime/teams.rs:16`-`30`; it is registered with `agent_type: "director"`, `color: DIRECTOR_AGENT_COLOR`, and `backend_type: None` so it is not presented as a live tmux peer in `teams.rs:436`-`461`. Inbox delivery uses the supplied color when writing messages in `teams.rs:633`-`710`, keeping the envelope aligned with the config record.
- Ready-count mismatch: fixed by cas-c9f0. The idle and registration nudge templates no longer embed the director snapshot count; they direct the supervisor to live `task action=ready` in `cas-cli/src/ui/factory/director/prompts.rs:263`-`290` and `prompts.rs:336`-`348`. Coverage is `test_worker_ready_prompt` and `test_worker_ready_no_tasks` in `prompts.rs:737`-`774`.
- Config-only member visibility: treated as nice-to-have and not required to resolve the unsafe nudge. The hazardous behavior was the peer-style, self-assignment nudge; current code uses non-interactive director metadata (`backend_type: None`) and role gates prevent the lead from being targeted as an idle worker.

A factory/team session launched in tmux team-lead mode received, **at session start**, a coordination message from a `director` teammate instructing it to assign ready tasks **to itself**:

> Worker zen-hawk-97 is idle with no assigned tasks.
> There are 14 ready tasks available.
> Assign work: `mcp__cas__task action=update id=<task-id> assignee=zen-hawk-97`
> To respond: `mcp__cas__coordination action=message target=zen-hawk-97 message="..."`

Three things are wrong with this message, and a fourth thing is wrong with the sender existing at all:

1. **Role category error.** The recipient is the **team-lead / supervisor**, not a worker. The nudge calls it "Worker zen-hawk-97" and tells it to set `assignee=zen-hawk-97` — i.e. a supervisor is told to self-assign implementation tasks. That's exactly the thing a supervisor must not do.
2. **The sender has no live process.** `director` is a config-only team member; nothing is running behind it. The message presents as a live peer teammate.
3. **Stale count.** It claims "14 ready tasks"; `task action=ready` actually returns **25**.
4. **User-visible alarm.** The operator (single working session, nothing else open) reasonably read an unsolicited "teammate" message as a rogue/extra session: *"its weird cause i have no other sessions open at all."*

## Affected version

`cas 2.22.0 (0f63093-dirty 2026-06-26)`, factory/team (tmux) mode.

## Evidence (same session)

**Process table — only ONE claude process and the daemon:**
```
187551  claude --dangerously-skip-permissions --session-id 4def0f9b-...
        --team-name gabber-studio-quiet-panda-24
        --agent-id supervisor@gabber-studio-quiet-panda-24
        --agent-name supervisor --agent-color green --agent-type team-lead
        --teammate-mode tmux
187605  cas serve
```
No `director` process exists. No tmux server is even running (`tmux ls` → no server). The only other process is the `cas serve` daemon.

**Team config (`~/.claude/teams/gabber-studio-quiet-panda-24/config.json`) registered TWO members at creation (08:25), one of which never runs:**
```json
"members": [
  { "agentId": "supervisor@...", "name": "supervisor", "agentType": "team-lead",
    "color": "green", "isActive": true },          // ← this session (leadSessionId match)
  { "agentId": "director@...",   "name": "director", "agentType": "director",
    "color": "white" }                              // ← no isActive, no process, 2-byte empty inbox
]
```
Both inboxes (`inboxes/director.json`, `inboxes/supervisor.json`) are 2 bytes (empty).

**Cassy registry vs team layer disagree on who this agent is:**
- `coordination whoami` / `agent_list` → `zen-hawk-97 (primary/supervisor) [active]`
- team `config.json` → `supervisor@... (team-lead, green)`
- the `director` message → addresses it as **"Worker zen-hawk-97"**

So the *same single process* is simultaneously a Cassy **primary/supervisor**, a team **team-lead**, and — per the nudge — a **worker** to be assigned to. The director collapsed all three into "idle worker."

**No live workers exist to assign to anyway** — every other agent in `agent_list` is `[shutdown]` (golden-gopher-43, mason, scout, a re-registered Primary). The "assign 14 ready tasks" instruction has no valid target even in principle.

**Envelope/identity mismatch.** The delivered teammate-message envelope tagged `director` with `color="green"`, but `config.json` says director is `white` and the supervisor is `green`. The sender's advertised color doesn't match its config record.

## Why it happens (hypothesis)

- The team scaffolding **auto-registers a `director`-typed coordinator member** even for what is effectively a single-agent (team-lead-only) run, and that coordinator (or the monitor acting on its behalf) emits a generic **"worker idle → assign work to yourself"** nudge **without gating on the recipient's `agentType`**. The template is worker-shaped (`Worker <name> is idle … assignee=<name>`) and gets fired at a `team-lead`.
- The idle nudge is generated from a **stale/independent task count** (14) that isn't reconciled against `task ready` (25) at send time.
- A **config-only member with no live process / heartbeat is still allowed to originate messages** that render in the recipient's inbox as a normal peer teammate, with no "automated/system" framing — so to the user it looks like a second live session they never launched.

## Impact

- **Unsafe supervisor behavior if obeyed.** A supervisor that trusts the nudge would self-assign implementation tasks (role violation) and/or try to dispatch to a worker pool that is entirely `[shutdown]` — here that queue includes live billing P0/P1 incidents (`cas-812c` double-grant, `cas-8f8c` paying-customers-no-credits), so a blind "assign to yourself and go" is a real foot-gun.
- **User trust / confusion.** A solo session surfaced an unsolicited "director" teammate message; the operator concluded something rogue was running. Investigation (`whoami`, `agent_list`, `ps`, team config) was required to prove it was just scaffolding.

## Suggested fixes

1. **Role-gate the idle nudge.** Never send a "worker idle → assign tasks to yourself" message to an agent whose `agentType` is `team-lead` / `supervisor` / `director`. For a supervisor with an empty queue, the correct nudge is "N ready tasks, 0 live workers — spawn workers or stand down," **not** `assignee=<self>`.
2. **Don't instruct self-assignment.** An idle nudge whose remediation is `assignee=<recipient>` is wrong for any coordinating role; the recipient and the assignee should never be the same agent in a "go do work" prompt aimed at a lead.
3. **No messages from non-live members, or label them as system.** A team member with no running process / lapsed heartbeat should not originate peer-style messages. If automated coordinator nudges are desired, render them with explicit `system`/`automated` framing so the user doesn't read them as a second live session.
4. **Reconcile the nudge's task count** with `task ready` at send time (said 14, actual 25).
5. **Fix envelope identity/color** — `director` advertised `green` while configured `white`; the message envelope and team config must agree.
6. **Reconsider auto-registering a phantom `director`** for single-lead runs, or make its non-running state unmistakable in `agent_list`/FACTORY views (it currently appears only in `config.json`, not in `agent_list` at all).

Related: `BUG-factory-liveness-signals-disagree.md` (same family — team/registry/process views not reconciled; agents misclassified across surfaces).

## What we did to recover

Took **no action** on the nudge. Verified ground truth (`whoami`, `agent_list`, `ps`, tmux, team `config.json`/inboxes), confirmed the `director` is a config-only phantom with no process, and reported back to the operator. No tasks assigned, no replies sent into the loop.

## Repro sketch

1. Launch `claude` in factory team (tmux) mode with `--agent-type team-lead` and a team `config.json` that also lists a `director`-typed member with no running process.
2. Start the session with 0 active tasks assigned to the lead.
3. Observe an unsolicited `director → lead` message: `Worker <name> is idle … Assign work … assignee=<name>`, with a task count that doesn't match `task ready`, sent to an agent that is a team-lead (not a worker), from a member with no live process.

## Acceptance criteria

1. Idle nudges are **never** sent to `team-lead`/`supervisor`/`director`-typed agents instructing them to self-assign tasks (`assignee=<self>`).
2. A team member with no live process/heartbeat cannot originate peer-style messages; any automated coordinator output is labeled `system`/`automated`.
3. The idle nudge's task count matches `task action=ready` at send time.
4. A given agent's advertised identity/color in a delivered message matches its team `config.json` record.
5. (Nice-to-have) `agent_list` surfaces config-registered-but-not-running members (e.g. the phantom `director`) rather than them being visible only in `config.json`.
