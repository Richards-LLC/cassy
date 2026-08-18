# Spike: Drop Claude Code agent-teams enrollment for Cassy factory workers

**Task:** cas-19fe (P3 spike, characterization-first)
**Author:** mighty-stork-36 (factory worker)
**Date:** 2026-05-01
**Status:** Complete — verdict below

## TL;DR — Verdict

**DROP** (with one carve-out, see §6).

Agent-teams enrollment provides exactly one load-bearing capability today: **the harness polls `~/.claude/teams/<team>/inboxes/<agent>.json` and surfaces inbox messages into the active conversation**. Every other claimed capability is either (a) implemented natively by Cassy, (b) cargo-cult metadata that no caller reads, or (c) actively harmful (the SendMessage interceptor hook, the team-mode UG9 deadlock, and the per-role settings hack that only exists to work around UG9).

The one load-bearing capability is itself flaky per multiple Anthropic issues — `#23415` (closed `NOT_PLANNED` 2026-03-20, i.e. won't-fix; bug still real), `#34668` (open), `#51959` (open) — and we already have a working substitute on the codex code path (PTY-stdin injection gated on pane-readiness, see `cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs:241–252`). Folding Claude workers onto the same path costs us one harness-side push channel but buys us: deletion of the SendMessage hook, deletion of the per-role settings allowlist hack (because the deadlock only exists in team-mode), exit from an experimental Anthropic surface that is being actively re-shaped, and ~700 lines of `teams.rs` reduced to a thin worktree-boundary write.

Recommend a follow-on EPIC to execute the drop. Sketch in §7.

---

## 1. Sources

### Local code (read 2026-05-01)
- `cas-cli/src/ui/factory/daemon/runtime/teams.rs` — `TeamsManager` (1358 lines total: ~800 production + ~558 tests; `#[cfg(test)]` starts at line 801)
- `crates/cas-pty/src/pty.rs` lines 41–354 — `TeamsSpawnConfig`, `PtyConfig::claude` argv assembly, `PtyConfig::codex` (no teams flags)
- `cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs` lines 220–500 — daemon delivery loop with `if let Some(ref teams) … else mux.inject` branching
- `cas-cli/src/ui/factory/daemon/runtime/lifecycle.rs` lines 96–124 — `TeamsManager` is created **unconditionally** even though the comment claims "Claude CLI only" (latent bug on codex)
- `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs` lines 114–143, 857–983 — `SendMessage` PreToolUse interceptor (`auto_route_send_message`)
- `cas-cli/src/mcp/tools/service/agent_search_system/message.rs` — `mcp__cas__coordination action=message` implementation (writes to `prompt_queue_store`)
- `cas-cli/src/cli/factory/mod.rs:882–886`, `cas-cli/src/cli/factory/daemon.rs:78–79` — supervisor entry points calling `TeamsManager::build_configs_for_mux`
- `crates/cas-mux/src/harness.rs` — `SupervisorCli::{Claude,Codex}` + capability table
- `~/.claude/teams/cas-src-fast-hawk-50/config.json` — observable team config for this very session
- `~/.claude/teams/cas-src-fast-hawk-50/inboxes/mighty-stork-36.json` — observable inbox file for this worker (2 messages, both from director, both showing the harness IS surfacing them as conversation turns)
- `~/.claude/teams/cas-src-fast-hawk-50/mighty-stork-36-settings.json` — observable per-worker settings file with `permissions.allow` + `cas hook PreToolUse|PermissionRequest`

### git archaeology (`git log --all -- <path>`)
- `bba7685` (2026-04-12) — first introduction of teams flag plumbing alongside the Slack-bridge salvage; commit body explicitly notes both PTY injection AND inbox delivery were broken at the time
- `509b308` / `d320148` / `43eaacb` / `ffb76df` / `959e69b` — chain of fixes to the per-role settings hack, all of which exist solely to work around the team-mode UG9 escalation deadlock
- `efe67b3` — inbox dedup + retention + unread-preservation (cas-7f57) — fix for an inbox-replay bug that does not exist on the PTY-injection path

### Anthropic docs (fetched 2026-05-01 via `claude-code-guide` subagent)
- https://code.claude.com/docs/en/agent-teams — agent-teams architecture, enumerated hooks (`TeammateIdle`, `TaskCreated`, `TaskCompleted`), explicit statement that *"the lead's conversation history does not carry over"*, explicit statement that team config *"is overwritten on the next state update — don't edit it by hand or pre-author it"*
- https://code.claude.com/docs/en/hooks — `PreToolUse` hook schema, `permissionDecision` shape

### GitHub issues (queried directly via `gh issue view -R anthropics/claude-code <num> --json number,title,state,closedAt,stateReason` 2026-05-01)

State legend: `OPEN` = currently open; `NOT_PLANNED` = closed without fix (Anthropic acknowledged but won't address; the underlying bug is still real); `DUPLICATE` = closed in favour of another tracking issue.

| # | State | Date | Title | Why it matters |
|---|----|------|-------|----|
| [#23415](https://github.com/anthropics/claude-code/issues/23415) | CLOSED `NOT_PLANNED` 2026-03-20 | 2026-02-05 | Teammates don't poll inbox — messages never delivered (tmux backend, macOS) | Closed won't-fix — Anthropic confirmed the bug but does not plan to fix the tmux-backend inbox-polling failure. Disproves the only load-bearing capability on the platform where we ship cas factory. |
| [#51959](https://github.com/anthropics/claude-code/issues/51959) | OPEN | 2026-04-22 | Lead agent requires manual stdin input to process teammate notifications — breaks unattended orchestration | The exact failure mode `bba7685` commit body called out 7 months ago. |
| [#34668](https://github.com/anthropics/claude-code/issues/34668) | OPEN | recent | Teammates intermittently stop receiving SendMessage after extended polling (default in-process mode) | Inbox polling unreliable even on default backend. |
| [#44080](https://github.com/anthropics/claude-code/issues/44080) | OPEN | 2026-04-06 | [BUG] SendMessage no longer available | Tool surface drift. |
| [#47021](https://github.com/anthropics/claude-code/issues/47021) | CLOSED `DUPLICATE` 2026-04-16 | 2026-03 | SendMessage tool referenced but not available at runtime | Closed as duplicate of an upstream tracker; bug class still active per the duplicates' chain. |
| [#42999](https://github.com/anthropics/claude-code/issues/42999) | OPEN | 2026-04-03 | SendMessage silently fails when using agent name — only agent ID works | Silent-fail mode hidden behind our hook interceptor. |
| [#48160](https://github.com/anthropics/claude-code/issues/48160) | CLOSED `DUPLICATE` 2026-04-18 | 2026-04-14 | Spawned subagents cannot originate SendMessage | Closed as duplicate; asymmetry in agent-teams comms still unaddressed in the parent issue. |
| [#27555](https://github.com/anthropics/claude-code/issues/27555) | CLOSED `NOT_PLANNED` 2026-03-22 | 2026-03 | Teammate messages render with `⏺ Human:` prefix | Closed won't-fix; cosmetic UX bug. |
| [#25135](https://github.com/anthropics/claude-code/issues/25135) | CLOSED `NOT_PLANNED` 2026-03-23 | 2026-03 | SendMessage silently succeeds when recipient name doesn't match team-lead's inbox polling target | Closed won't-fix; the silent-success delivery hazard is acknowledged but unfixed. |
| [#51431](https://github.com/anthropics/claude-code/issues/51431) | OPEN | 2026-04-21 | [Hooks] Missing hook events for inter-teammate notifications | Hook surface incomplete. |
| [#52251](https://github.com/anthropics/claude-code/issues/52251) | OPEN | 2026-04-23 | Agent-Teams sub-agents with model:opus cannot call SendMessage / TaskCreate / TaskUpdate (tmux backend) | Tool gating regressions on the exact backend we use. |
| [#53896](https://github.com/anthropics/claude-code/issues/53896) | OPEN | 2026-04-27 | SendMessage concurrent writes lose messages via array rewrite | Same bug class as our cas-7f57 inbox-replay fix — i.e. we are forking + carrying CC's inbox bugs. |

**Reading note on the four `NOT_PLANNED`/`DUPLICATE` closures:** none of these were closed because the bug was *fixed* — `NOT_PLANNED` is GitHub's "won't-fix" reason and `DUPLICATE` rolls the report into another (still-open or also-closed) tracker. The closure pattern itself reinforces the verdict: agent-teams bugs are getting acknowledged-and-shelved rather than addressed.

### context7
Not used — agent-teams is Claude Code-internal, not a third-party library.

### Cassy memory (read via `mcp__cas__search`)
Repo-side memory `project_session_start_truncation` confirms the SessionStart additionalContext payload (which arrives via the team-mode harness bridge in 2.1.x) silently truncates >2KB. Memory `project_claude_code_bun_pin` documents that we pin Claude Code to 2.1.116 because newer versions added Ink Box/Text crashes in agent-teams mode. Both reinforce that agent-teams is an unstable surface for us.

---

## 2. Inventory: what agent-teams enrollment actually provides today

These are the observable effects of writing `~/.claude/teams/<session>/config.json` + spawning `claude` with `--team-name --agent-id --agent-name --agent-color --agent-type --teammate-mode tmux --parent-session-id --settings <path>` + env `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`.

| # | Capability | Mechanism | Citation |
|---|----|----|----|
| **A** | **Daemon→Worker push: harness ingests inbox JSON file and surfaces messages as conversation turns** | `--teammate-mode tmux` activates inbox polling; daemon writes via `TeamsManager::write_to_inbox` | `crates/cas-pty/src/pty.rs:195–209`; `cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs:326–338`; observed in `~/.claude/teams/cas-src-fast-hawk-50/inboxes/mighty-stork-36.json` (this very session) |
| **B** | **`SendMessage` tool auto-surfaced to the agent** | Anthropic docs: *"Team coordination tools such as `SendMessage` and the task management tools are always available to a teammate"* | https://code.claude.com/docs/en/agent-teams (fetched 2026-05-01) |
| **C** | **Per-role settings file path (`--settings <path>`) load-bearing for filesystem auto-approve** | `--settings` is itself NOT agent-teams-specific — but the file lives under the team dir today and its `permissions.allow` exists *solely* to work around the team-mode "UG9" leader-escalation deadlock | `cas-cli/src/ui/factory/daemon/runtime/teams.rs:271–374`; `pre_tool.rs:81–92` (the `is_factory_agent` auto-approve hoist that exists to bypass the same deadlock when `cas_root=None`); observed at `~/.claude/teams/cas-src-fast-hawk-50/{supervisor,mighty-stork-36}-settings.json` |
| **D** | **TUI tmux pane layout with teammate panes side-by-side** | tmux is set up by `cas factory` itself, not by the harness — `--teammate-mode tmux` is what the *harness* expects to find, not what creates the panes | `crates/cas-pty/src/pty.rs:208`; cas-mux owns pane layout entirely (`crates/cas-mux/src/pane/`) |
| **E** | **Agent visibility / status-line entries** | Inherited from per-pane Claude Code session display; not gated on agent-teams | https://code.claude.com/docs/en/agent-teams §UI |
| **F** | **`TeammateIdle` / `TaskCreated` / `TaskCompleted` hooks** | Anthropic-only, agent-teams-exclusive | https://code.claude.com/docs/en/agent-teams §Hooks |
| **G** | **Idle-notification pop-ups in TUI** | Surface in `~/.claude/teams/<session>/inboxes/<agent>.json` writes from the harness back to the lead — claimed in docs, *unobserved on tmux backend per #23415* | https://code.claude.com/docs/en/agent-teams §Notifications; counter-evidence in #23415, #34668, #51959 |
| **H** | **Cross-teammate context inheritance** | None. Docs explicit: *"the lead's conversation history does not carry over"* | https://code.claude.com/docs/en/agent-teams §Architecture |
| **I** | **`config.json` `members[]` registry** | Used by harness to validate `SendMessage` recipients and look up agent IDs/colors | https://code.claude.com/docs/en/agent-teams §Architecture; observed config |
| **J** | **`--parent-session-id` analytics correlation** | Anthropic-internal telemetry only; not surfaced to caller | `crates/cas-pty/src/pty.rs:54` (`parent session ID for analytics correlation`) |

---

## 3. Cassy-equivalence map

For each capability above: does Cassy already have a native substitute?

| Cap | Cassy-native equivalent | File pointer | Status |
|---|---|---|---|
| **A** Daemon→Worker push | `mux.inject(&name, prompt)` writes to PTY stdin; gated on `pane_ready_for_injection` (5s output + grace window after first-byte) | `cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs:242–252, 397–404`; `crates/cas-mux/src/pane/` | **Yes — already used for codex workers; teams branch is the divergence** |
| **B** `SendMessage` tool | `mcp__cas__coordination action=message target=<name> message="..." summary="..."` — routes via `prompt_queue_store::enqueue_full` → daemon delivery loop | `cas-cli/src/mcp/tools/service/agent_search_system/message.rs`; `cas-cli/src/store/prompt_queue.rs` | **Yes — and is already the canonical path; the SendMessage hook just bridges agents who default to the wrong tool** |
| **C** Per-role settings allowlist | The `permissions.allow` block exists *solely* because `--teammate-mode` triggers the team-lead escalation. Without team-mode there is no UG9, no escalation, no allowlist requirement. The factory can still use `--dangerously-skip-permissions` (already passed) + the existing `cas hook PreToolUse` for path-based guards. | `crates/cas-pty/src/pty.rs:174` (already passes `--dangerously-skip-permissions`); `pre_tool.rs:81–92` (`is_factory_agent` auto-approve already exists) | **Yes — and the workaround would become unnecessary** |
| **D** Pane layout | Owned end-to-end by cas-mux | `crates/cas-mux/src/mux.rs`, `pane/mod.rs` | **Yes** |
| **E** Agent visibility | Owned by `cas-cli/src/ui/factory/` (TUI/GUI/web) | `cas-cli/src/ui/factory/app/render_and_ops/`, factory daemon `worker_status` | **Yes** |
| **F** TeammateIdle hook | `coordination.factory.worker_activity` + factory event-detector `last_state` resets surface idle/busy/wedged states; the supervisor's TUI shows them | `cas-cli/src/ui/factory/daemon/runtime/event_detector*` (observed via cas-7f57 dedup work); `mcp__cas__coordination action=worker_activity` | **Yes — partial; TaskCreated/TaskCompleted are Cassy-side events already** |
| **G** Idle notifications | Same as F — and Cassy already receives idle-state events via the pane buffer scanner, not via harness emission | factory event detector | **Yes — and Cassy-side detection works whereas #23415 / #51959 say harness-side does not** |
| **H** Context inheritance | Not provided by agent-teams either; n/a | — | **Tied** |
| **I** `members[]` registry | `cas_types::Agent` table + `agent_store` | `cas-cli/src/store/agent.rs`; `mcp__cas__coordination action=agent_list` | **Yes** |
| **J** Parent-session analytics | We don't consume this. | — | **N/a — Anthropic-side only** |

**Net:** Of the ten capabilities, **Cassy already has a native substitute for 9**. The tenth (parent-session analytics) is internal to Anthropic and we don't read it. The only one with a debatable gap is (A) push delivery — and Cassy already runs that gap closed for codex, with empirical proof in this very repo.

---

## 4. Lost-capability scoring

| Cap | Score | Rationale |
|---|---|---|
| **A** Daemon→Worker push (inbox-file polling) | **Nice-to-have** — not load-bearing because PTY injection is the documented fallback path used by codex factory mode and explicitly designed for in `pane_ready_for_injection` (`queue_and_events.rs:241–252`). Inbox path is preferred when it works (richer JSON, no readline-eating risk) but is broken per #23415, #51959 on the tmux backend we ship. **Cost-of-port: zero — the fallback is already there.** |
| **B** `SendMessage` tool auto-surface | **Negative-value** — when surfaced, agents pick it over `mcp__cas__coordination` because the harness's system-reminder tells them to. The PreToolUse hook then has to deny each call, producing a phantom "tool error" in the transcript (~2-300 tokens of agent context per occurrence) and training the agent that messaging produced an error. *Removing this is a strict win.* |
| **C** Per-role settings hack | **Negative-value** — only exists because `--teammate-mode` triggers UG9. No team-mode → no UG9 → no allowlist file required. Removing is a strict win. |
| **D, E, I** Pane layout / agent visibility / member registry | **Cargo-cult on the harness side** — Cassy owns these end-to-end. The harness-side equivalents are duplicated metadata that nothing reads. |
| **F, G** TeammateIdle / TaskCreated / TaskCompleted hooks + idle pop-ups | **Cargo-cult** — we don't currently subscribe to these hooks (no usage in `cas hook` handler), and they fire less reliably than Cassy event-detector signals. Per #23415, #51959, idle pop-ups don't even trigger on tmux backend. |
| **H** Context inheritance | **N/a** — agent-teams doesn't provide this either |
| **J** Parent-session analytics | **Cargo-cult** — Anthropic-side only |

**Score summary:** 1 nice-to-have (A) with a known-working substitute. 0 load-bearing. Everything else is negative-value, cargo-cult, or n/a.

---

## 5. Specific costs of the current bridge layer

These are concrete tax items that disappear if we drop enrollment:

1. **SendMessage interceptor hook** (`cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs:137–143, 857–983`, ~127 lines) — exists only because the harness surfaces `SendMessage`.
2. **SendMessage autoroute test suite** (`cas-cli/src/hooks/handlers/handlers_tests/send_message_autoroute.rs`) — ~lines of test code testing the bridge.
3. **Per-role settings file generator + 9 unit tests** (`cas-cli/src/ui/factory/daemon/runtime/teams.rs:271–374` + 200 lines of tests) — entire UG9 workaround.
4. **`teams.rs` itself** (1358 lines including tests) — `init_team_config`, `add_member`, `remove_member`, `cleanup_orphans`, `write_to_inbox` (with dedup + retention), `spawn_config_for`, `build_configs_for_mux`, the supervisor/worker settings synchronization.
5. **`TeamsSpawnConfig` + claude-side argv assembly** (`crates/cas-pty/src/pty.rs:41–225`) — flag construction that has no codex equivalent.
6. **Inbox replay/dedup machinery** (cas-7f57: `INBOX_DEDUP_WINDOW`, `INBOX_RETENTION`, ~120 lines including tests) — fixes a bug class that exists only on the inbox-file delivery path.
7. **Per-message phantom "tool error"** in every agent that defaults to SendMessage — ~200-500 tokens of context per agent per session. Multiplied across the 4-pane factory, this is one of the largest avoidable token costs we ship.
8. **Pinning Claude Code to 2.1.116** (project memory `project_claude_code_bun_pin`) because newer versions added Ink Box/Text crashes specifically in agent-teams mode. Drop agent-teams → unblock the upgrade.

---

## 6. The one carve-out: per-worker `--settings <path>` for filesystem auto-approve

The `--settings <path>` argv flag itself is *not* agent-teams-specific and is a useful capability for shipping a per-worker `permissions.allow` list. But its current contents (`permissions.allow` + `PreToolUse|PermissionRequest` hooks) only exist because team-mode triggers the UG9 deadlock. Once team-mode is gone:

- The factory already passes `--dangerously-skip-permissions` (`crates/cas-pty/src/pty.rs:174`), so all-tool auto-approve is in place.
- The Cassy `cas hook PreToolUse` handler already auto-approves filesystem tools for factory agents via env-var-driven `is_factory_agent` detection (`pre_tool.rs:81–92`), without needing a per-worker settings file.

**Recommendation:** keep `--settings <path>` capability in cas-pty for future use, but stop generating the per-role files as part of factory boot. They become opt-in for non-factory deployments that genuinely need a per-process permissions allowlist.

---

## 7. Sketch: follow-on tasks if DROP is approved

Implementation is **not** part of this spike. These are task titles + rough scope estimates so the supervisor can plan an EPIC.

1. **Remove agent-teams CLI flags from worker spawn argv (`PtyConfig::claude`)** — drop `--team-name`, `--agent-id`, `--agent-name`, `--agent-color`, `--agent-type`, `--teammate-mode`, `--parent-session-id`, `--settings`, and `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1` env. Rough scope: small. `crates/cas-pty/src/pty.rs:165–225` + adjust 5 unit tests.
2. **Make daemon delivery use PTY injection unconditionally (drop `if let Some(ref teams)` branches)** — collapse `queue_and_events.rs:326–404` to the inject path. Verify pane-readiness gate covers Claude as well as codex (it should — same readline init issue applies). Rough scope: small.
3. **Delete `TeamsManager` + `~/.claude/teams/` writes** — remove `teams.rs`, drop the `Option<TeamsManager>` field from `FactoryDaemon`, delete `init_team_config` callers in `process.rs`, `fork_first.rs`, `lifecycle.rs`. Rough scope: medium (~1300 LOC removed + cleanup of metadata.team_name + cloud sync references).
4. **Delete `SendMessage` PreToolUse interceptor + its tests** (`pre_tool.rs:137–143, 857–983` + `send_message_autoroute.rs`). Without enrollment, the harness no longer surfaces `SendMessage` to factory agents, so the hook becomes dead code. Rough scope: small. *Note:* docs reference (`#42737`, `#51071`) suggests CC sometimes surfaces SendMessage even without agent-teams via the Agent tool description — keep a defensive deny-with-guidance path but kill the autoroute machinery.
5. **Update worker-side coordination guidance** (`cas-cli/src/builtins/skills/cas-worker.md`, supervisor.md, codex twins) — remove "SendMessage is blocked" guidance since it'll no longer surface. Rough scope: tiny — already sized at one line in cas-worker.md and one in cas-supervisor.md.
6. **Drop `~/.claude/teams/` cleanup hook** in factory shutdown / orphan-cleanup paths (`teams::cleanup_orphans` callers). Rough scope: tiny.
7. **Verify Claude Code version pin can be relaxed** post-removal — test 2.1.117+ once the agent-teams Ink Box/Text trigger is no longer reachable. Spike. Rough scope: small.

The first three are an atomic merge unit (they each remove half of an unfinished refactor and won't compile separately without scaffolding). The last four are independent.

Total estimated LOC removed: ~1700 (teams.rs + tests + settings hack + autoroute hook + tests). Net: large simplification.

---

## 8. Alternative considered: PARTIAL

A "PARTIAL" verdict — keep enrollment but suppress the SendMessage hook overhead — was considered. Rejected because:

- The hook is the only thing standing between agents and a broken upstream surface (#42999 silent-success bugs would otherwise misroute messages).
- Suppressing the hook leaves the per-role settings hack and `teams.rs` overhead in place — i.e. it accepts the costs without buying the simplification.
- Anthropic's agent-teams surface is being actively re-shaped (#51071, #51431, hooks RFCs); coupling tightly to it carries ongoing breakage cost.

If Anthropic stabilizes agent-teams and fixes #23415 / #51959, the door is open to revisit. The verdict is "drop now, re-evaluate if upstream reaches GA."

---

## 9. Verdict

**DROP.** Plan: execute the 7-task sketch in §7 as a follow-on EPIC. Code-review-first for the three atomic-unit tasks (delivery path collapse) since they touch the daemon hot path; the rest are mechanical removals.
