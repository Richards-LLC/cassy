---
from: Ozer Health factory (operator + Grok Build supervisor)
date: 2026-07-11
resolved: 2026-07-11
priority: P1
type: BUG
component: Grok Build TUI — turn cancel / Stop control / keyboard interrupt
project: ozer-health (Richards-LLC/ozer-health)
for_team: cas-src (Grok Build harness / TUI)
cas_task: cas-7f6f
status: FIXED
---

# BUG: Grok TUI — Stop does nothing; Esc does nothing mid-turn (cannot abort a running turn via UI)

**Label:** `grok-tui` · `cancel` · `stop` · `keyboard` · `operator-control` · **P1**

Please treat this as a **Grok Build TUI / harness** bug (cas-src or the Grok Build client tree you own for factory sessions), not an Ozer product bug.

---

## Situation (plain language)

During a live Grok Build supervisor turn on the Ozer factory session, the operator could not stop the in-flight turn:

1. Clicking the **Stop** control in the TUI does **nothing** (no cancel, no status change, turn keeps running).
2. Pressing **Esc** does **nothing** mid-turn (no cancel).

Result: a runaway or unwanted turn cannot be aborted from the UI path the operator expects. The only remaining options are killing the process externally or waiting it out — which is unacceptable during factory coordination (merge races, wrong-epic work, director misroutes, etc.).

---

## Expected behavior

While a turn is running:

| Control | Expected |
|---------|----------|
| **Stop** (mouse / UI button) | Immediately request cancel of the current turn; UI should show cancelling → cancelled; tools in flight should be aborted or cleanly interrupted per product policy. |
| **Esc** | Either cancel the turn **or** surface a clear affordance that cancel is `Ctrl+C` (not a silent no-op that feels broken). |
| Documented cancel path | If Esc is intentionally a no-op mid-turn (see 0.2.93 breaking change), **Stop** and/or **Ctrl+C** must still work reliably. Silent failure of Stop is the defect. |

---

## Actual behavior

| Control | Actual |
|---------|--------|
| **Stop** click | No effect. Turn continues tool calls / streaming. |
| **Esc** | No effect mid-turn. |

Operator observation (verbatim intent): *"I can't stop a Grok turn — clicking Stop does nothing; pressing ESC does nothing."*

---

## Environment

| Field | Value |
|--------|--------|
| Date | 2026-07-11 |
| Host / project | Ozer Health (`/home/pippenz/Petrastella/ozer`) |
| Client | Grok Build TUI (`grok`) |
| Grok version | `0.2.93 (f00f96316d) [stable]` |
| Cassy version (co-running factory) | `2.27.0 (9f86e08-dirty 2026-07-10)` |
| Session context | Supervisor / factory session on Ozer (long tool-using turns, MCP `cas__*` calls) |
| Observed active Grok session (same host, same day) | `019f5306-9f48-7e21-ac1e-1696bbda7ac0` · cwd ozer · pid ~435771 (and concurrent session in worktree) |

---

## Why this is more than a convenience gap

Factory supervisors and workers routinely run multi-minute turns (merge, verification, code review). When the director misroutes epic-completion to the wrong supervisor, or a turn is about to do the wrong destructive action (`shutdown_workers count=0`, wrong epic close), the operator **must** be able to stop the turn in seconds.

Related same-day incident (different bug, same need for stop):
`docs/requests/BUG-director-epic-complete-misrouted-supervisor-2026-07-11.md` — operator interrupt was the only thing that prevented a non-owning supervisor from racing close/shutdown. If Stop is dead, that safety valve is gone.

---

## Notes from shipped Grok docs (possible related change)

Grok **0.2.93** changelog / keyboard guide:

- **Breaking:** Esc no longer cancels a running turn; use **Ctrl+C** instead.
- Keyboard table: mid-turn Esc is a **swallowed no-op**; cancel is `Ctrl+C` (empty prompt; non-empty draft clears first).

So Esc doing nothing may be **intentional** after 0.2.93 — but that makes the **Stop** control failure the real P1, and it creates a UX trap:

1. Operator hits Esc (old muscle memory / Claude Code habit) → nothing.
2. Operator clicks Stop → nothing.
3. Operator concludes cancel is broken entirely (may never try Ctrl+C, or Ctrl+C may also fail — not confirmed in this report).

**Please verify on repro:**

1. Mid-turn **Stop** button / palette cancel.
2. Mid-turn **Ctrl+C** (empty prompt).
3. Mid-turn **Esc** (document whether intentional).
4. Mid-turn cancel while a long MCP tool or shell tool is in flight (factory sessions are tool-heavy).
5. Mid-turn cancel while status shows streaming vs waiting on tool result.

---

## Suggested fix direction

1. **Make Stop always work** while `turn_running` / `turn_cancelling` — wire the button to the same cancel path as Ctrl+C; never swallow the click.
2. **Visible feedback** on cancel request: "Cancelling…" → "Cancelled" (or error if cancel failed).
3. **If Esc stays a no-op mid-turn**, show a one-line status hint on first Esc: "Press Ctrl+C to cancel" (or flash the Stop control) so operators aren't gaslit by silent swallow.
4. **Tool-in-flight cancel:** ensure cancel aborts the active tool/MCP wait, not only the model stream after the tool returns.
5. Add a regression test: start a long-running tool turn → fire cancel via Stop + Ctrl+C → assert turn ends within N seconds and no further tool side effects after cancel ack.

---

## Impact if unfixed

- Operators cannot safely interrupt bad or misrouted factory turns.
- Increases reliance on `kill` / session restart, which loses in-flight state and can leave workers/epics half-merged.
- Erodes trust in Grok as a factory supervisor CLI relative to clients where Esc/Stop actually abort.

---

## Repro sketch (minimal)

1. Open Grok Build TUI in a project with MCP tools enabled.
2. Prompt a long turn (e.g. multi-file search + several tool rounds, or a deliberate slow shell tool).
3. While status shows a running turn / active tool:
   - Click **Stop** → observe whether turn continues.
   - Press **Esc** → observe no cancel.
   - (For completeness) Press **Ctrl+C** on empty prompt → record whether cancel works.
4. Expected: at least Stop and Ctrl+C cancel the turn; Esc either cancels or teaches Ctrl+C.

---

## Related

- Grok 0.2.93: "Esc no longer cancels a running turn; use Ctrl+C instead."
- `~/.grok/docs/user-guide/03-keyboard-shortcuts.md` — Escape / Ctrl+C cancel table.
- `docs/requests/BUG-director-epic-complete-misrouted-supervisor-2026-07-11.md` — operator interrupt as only safety valve.
- `docs/requests/BUG-grok-supervisor-misses-awaiting-merge-merge-queue-2026-07-10.md` — same Grok factory surface family.

---

## Resolution (cas-7f6f, 2026-07-11)

**Status:** FIXED in factory/hv-grok-tui (this commit)

### Root cause (verified against Grok 0.2.93 docs + Cassy factory code)

1. **Stop button dead under factory mouse capture.** Factory enables mouse capture and routes clicks only through `handle_mouse_click` for pane focus / tab chrome. Clicks were never forwarded as SGR 1006 mouse events into the already-focused Grok alt-screen PTY, so Grok's on-screen Stop never received the event.

2. **Esc mid-turn is a Grok intentional no-op (0.2.93 breaking change).** Factory forwarded raw Esc (`0x1b`) into the Grok PTY. Grok swallows mid-turn Esc; cancel is Ctrl+C. Operators with Claude muscle memory hit Esc → nothing, then Stop → nothing, and concluded cancel was fully broken.

3. **Programmatic turn-break was Claude-only.** `Pane::break_turn` / urgent `interrupt_and_inject` always sent Esc. That does not cancel a Grok turn.

### Fix (shared cancel path)

| Control | Before | After |
|---------|--------|-------|
| Stop click (focused Grok alt-screen) | Focus only | SGR left-click press+release at PTY coords |
| Esc (focused Grok pane) | Raw Esc (no-op mid-turn) | `break_turn` → Ctrl+C via harness-aware cancel |
| Esc (Claude/Codex) | Raw Esc | Unchanged |
| Urgent interrupt / `break_turn` | Always Esc | Harness-aware: Esc (Claude/Codex), Ctrl+C (Grok) |
| Idle / unfocused click | Harmless | Unchanged (first click still focuses only) |

Key symbols:
- `SupervisorCli::turn_cancel_bytes()` — cas-mux harness
- `Pane::break_turn` — uses pane harness
- `FactoryApp::handle_mouse_click` → `ClickAction::ForwardSgr` + `sgr_left_click_bytes`
- `client_input` Esc branch for Grok → `mux.break_turn`

### Proof

```
cargo test -p cas-mux turn_cancel_bytes
# 1 passed

cargo test -p cas --lib sidecar_and_selection::tests
# 19 passed (incl. cas-7f6f: sgr click shape, forward-only-when-focused-Grok-alt,
#            Claude no-forward, idle harmless, turn_cancel_bytes)
```

Exit 0 on both.

### Manual smoke (operator)

1. Rebuild cas; run factory with supervisor_cli=grok (or cli=grok worker).
2. Start a long turn on the Grok pane.
3. Focus the pane (click once if needed), click Stop → turn should cancel.
4. Or press Esc while focused → same cancel path (Ctrl+C into Grok).
5. Factory session remains alive; Claude panes keep Esc-cancel and no SGR click forward.

### Review follow-up (cas-7f6f P2)
1. Grok Esc only rewrites to cancel when the pane has recent PTY output (turn-active); idle forwards raw Esc.
2. Stop click geometry uses per-render `pty_content_areas` (full=bordered inner, compact=borderless content).

### Review follow-up 2 (cas-7f6f P2)
1. Authoritative `Pane.turn_in_flight` (set on inject/CR submit; cleared on break_turn/interrupt) — not output-timing. Quiet active stays cancelable; idle redraws do not mark in-flight.
2. Separate `full_pty_content_areas` / `compact_pty_content_areas`; mouse routing uses client view mode so concurrent full+compact clients keep independent Stop geometry.

### Review follow-up 3 (cas-7f6f)
1. Normal completion: mark_turn_completed + quiet-after-saw-output window (TURN_COMPLETE_QUIET); submit→quiet→complete→idle test.
2. Only true keyboard Enter/inject marks turn_in_flight; bracketed paste/drop never do.
3. Image drop uses pane_at_screen_for(geometry) so compact/full maps stay independent.

### Review follow-up 4 (cas-7f6f)
1. Removed 8s/output quiet completion — long MCP/tool waits false-completed active turns.
2. Authoritative Grok completion: `events.jsonl` `turn_ended` (offset after submit); quiet >8s stays cancelable.
3. Unified `Mux::deliver_user_input(data, UserInputKind::{KeyStream,StructuredPaste})` across terminal, GUI, WebSocket, relay; paste/drop are StructuredPaste only.
4. Generic SGR click never clears `turn_in_flight`; Stop completion via harness `turn_ended` / Esc via `break_turn`.
