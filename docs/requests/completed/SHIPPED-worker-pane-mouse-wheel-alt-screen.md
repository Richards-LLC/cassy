---
from: pippenz (Penguinz)
date: 2026-05-07
resolved: 2026-06-01
priority: P1
cas_task: cas-d5fa, cas-f93a
status: SHIPPED
---

# Worker pane: mouse wheel / touch scroll does nothing while inner TUI is in alt-screen

Filed previously without a fix landing — re-filed with deeper instrumentation, then resolved in two passes.

## Resolution

Shipped in two commits:

1. **`53a7bf4` (cas-d5fa, 2026-05-07)** — added alt-screen detection on `Pane::feed` and a wheel-event PTY-forwarding path in `cas-cli/src/ui/factory/app/sidecar_and_selection.rs::alt_screen_scroll_input`. Initially forwarded `ESC[A` × 3 (arrow keys), which did NOT fix the user-visible symptom — arrow keys land in Claude Code's prompt-input box and cycle previous-prompt history, not the transcript.
2. **`678f75b` (cas-f93a, 2026-06-01)** — swapped the wheel payload to `ESC[5~` / `ESC[6~` (PgUp / PgDn). Empirical A/B by the user confirmed PgUp/PgDn correctly scrolls Claude Code's transcript. Wheel now rides the same byte payload as the existing PgUp/PgDn forwarding path.

Effective from cas builds containing `678f75b` (post-v2.17.3 dev / next tagged release). Penguinz-side note: previously-referenced phantom task `cas-c08d` did not exist in the cas-src Cassy instance and has been removed from frontmatter.

## Affected version

`cas 2.13.0 (7450278 2026-05-06)`

## Symptom

Clicking on a worker pane focuses it (correct), but **mouse wheel on desktop and two-finger swipe on Termux/SSH from a phone scroll nothing** in the worker output (Claude Code transcript). PgUp/PgDn also fail to expose pre-visible history. The in-app F1 help promises the opposite:

> Mouse: Click pane → Focus pane / Scroll → Scroll focused pane

## Reproduction

1. `cas` (factory mode), focus a worker pane running Claude Code.
2. Click the worker pane — focus indicator changes (confirms click is captured).
3. Try to scroll up:
   - **Desktop (Konsole):** mouse wheel. Profile already has `Allow terminal applications to handle clicks and drags` and `Enable Alternate Screen buffer scrolling` enabled — Konsole IS forwarding wheel events to Cassy as SGR mouse events.
   - **Termux (SSH from Android):** two-finger swipe (Termux maps this to wheel in mouse-mode apps).
4. Result: nothing visibly scrolls. Same on PgUp/PgDn.

Identical behavior in two unrelated terminal environments → not a terminal-config issue, not Termux-specific.

## What's already ruled out

- **Terminal isn't forwarding events** — works in both Konsole (with the right profile flags shown above) and Termux. Mouse capture is enabled in Cassy (`crossterm::event::EnableMouseCapture` symbol present in binary).
- **Click landed on the wrong pane** — focus indicator confirms click registers; `handle_mouse_click` runs.
- **Help text is just lying** — the binary contains real handler symbols `handle_scroll_up`, `handle_scroll_down`, `scroll_focused_pane`, `mc_scroll_up`, `mc_scroll_down`, `Pane::scroll`, plus error strings like `Failed to scroll focused pane:` and `Failed to scroll terminal: code` — meaning code paths exist and fail at runtime.

## Architecture (from `strings ~/.local/bin/cas` analysis)

**Client — `cas::ui::factory::app::FactoryApp`** in `cas/src/ui/factory/app/sidecar_and_selection.rs`:
- `handle_mouse_click` — routes click events
- `handle_scroll_up` / `handle_scroll_down` — wheel events
- `mc_scroll_up` / `mc_scroll_down` — main-content/mouse-click variants
- `scroll_focused_pane` — delegate that does the actual scroll RPC
- `sidecar_scroll_down` — sidecar pane has its own path
- Error log: `Failed to scroll focused pane: …`

**Pane mux — `cas_mux::pane::Pane::scroll`** in `crates/cas-mux/src/pane/mod.rs` (binary references lines 407, 417, 638, 715, 738, 803, 818):
- Wraps a `ghostty_vt` terminal per pane
- Symbols include `ghostty_vt_terminal_scroll_viewport`, `_scroll_viewport_top`, `_scroll_viewport_bottom`, `_take_viewport_scroll_delta`, `_scrollback_info`
- Error log: `Failed to scroll terminal: code …`

**Daemon — `cas::ui::factory::daemon::FactoryDaemon`:**
- `build_scrollback` — assembles scrollback content for the GUI client
- `SessionState` carries `request_scrollback: bool`, `scrollback`, `pane_id`
- Trace logs: `: scroll complete, after: offset=` / `: scroll delta=`

**Wire protocol — `ClientMessage` enum variants:**
- Input, InputFocused, Focus, Resize, ResizePane, SpawnShell, KillShell, SpawnWorkers, ShutdownWorkers, Inject, Attach
- **No dedicated `Scroll` / `Wheel` variant** → scroll requests piggyback on Input or rely on the SessionState `request_scrollback` poll.

## Single most likely root cause

The worker pane runs Claude Code, which is a **fullscreen TUI in alt-screen mode**. Alt-screen has no scrollback. When wheel events arrive at the focused worker pane and Cassy routes them to `Pane::scroll → ghostty_vt::scroll_viewport`, there is nothing above the visible region to scroll into — the call no-ops (or trips the `Failed to scroll terminal: code …` path). The user sees nothing.

This is the well-known alt-screen scroll trap. Tmux faced the same call ([tmux#3705](https://github.com/tmux/tmux/issues/3705)) and Konsole's `Enable Alternate Screen buffer scrolling` is its workaround — it translates wheel events to up/down arrow keys when the inner app is in alt-screen so the inner app paginates itself. Mosh has a long-standing issue on the same topic ([mobile-shell/mosh#2](https://github.com/mobile-shell/mosh/issues/2)).

**Right behavior for Cassy:** when the focused worker pane's inner process is in alt-screen mode, **forward wheel events to the inner process as `MouseEvent::ScrollUp/ScrollDown`** (Claude Code consumes these and scrolls its own transcript), instead of consuming them for `Pane::scroll`.

Cassy already has a forwarding path for keyboard input (`ClientMessage::Input`); this is plumbing the existing path for mouse-wheel events when the inner TTY has alt-screen active.

### Concrete fix prescription

Two changes, in order:

1. **`cas_mux::pane::Pane` — expose alt-screen state per pane.** `ghostty_vt` already knows which screen is active (primary vs alternate). Surface that as a `Pane::is_alt_screen() -> bool` (or similar) so the client can gate its scroll-routing decision on the current pane mode rather than guessing from the pane class.

2. **`cas::ui::factory::app::FactoryApp::scroll_focused_pane` — branch on that flag.** Pseudo-shape:

   ```text
   fn scroll_focused_pane(&mut self, direction: ScrollDir) {
       let pane = self.focused_pane();
       if pane.is_alt_screen() {
           // Forward as input: serialize crossterm MouseEvent::ScrollUp/ScrollDown
           // to the SGR mouse-wheel sequence (CSI < 64 ; Cx ; Cy M for up,
           // 65 for down) and emit ClientMessage::Input. Inner TUI (Claude Code)
           // consumes it and paginates its own transcript.
           self.send_input(serialize_wheel(direction, pane.cursor_pos()));
       } else {
           // Primary screen — existing path: ask cas_mux to move the viewport.
           self.pane_scroll(pane.id(), direction);
       }
   }
   ```

   The `send_input` path already exists for keyboard events; this just funnels wheel events through it when alt-screen is active. The else branch is the current (broken-on-alt-screen, fine-on-primary) `Pane::scroll` path unchanged.

3. **Apply the same branch to PgUp/PgDn handlers** — same root cause when the focused pane is in alt-screen.

### Secondary issue (don't lose track)

Even when scrolling Cassy's own pane scrollback (e.g., shell panes, no alt-screen), the `Failed to scroll focused pane: …` log line implies `Pane::scroll` returns errors in some conditions — probably a `cas_mux` plumbing bug where the daemon → client `request_scrollback` round-trip doesn't deliver fresh content to the renderer, so the user sees no visual change even when ghostty's viewport offset moved. Worth confirming under tracing.

## Diagnostic recipe

Run the user-reported repro with:

```bash
RUST_LOG=cas_mux=trace,cas::ui::factory=trace cas 2>~/cas-scroll.log
```

Then in `~/cas-scroll.log` look for:

| Log line | What it tells you |
|---|---|
| `: scroll delta=` | `scroll_focused_pane` was reached |
| `: scroll complete, after: offset=` | ghostty viewport actually moved |
| `Failed to scroll focused pane:` | client-side failure path triggered |
| `Failed to scroll terminal: code` | ghostty returned an error code (likely "no scrollback in alt-screen") |

If `scroll delta=` is logged but `offset=` doesn't change → confirms the alt-screen no-op hypothesis.

## Acceptance criteria

1. **Worker pane (Claude Code, alt-screen TUI), focused, mouse wheel up:** Claude's transcript scrolls up. Same on PgUp.
2. **Worker pane, two-finger swipe in Termux over SSH:** same as #1 (Termux delivers wheel events, Cassy forwards them to Claude).
3. **Shell pane (no alt-screen), mouse wheel up:** Cassy pane scrollback scrolls up (existing behavior, must not regress).
4. **Sidecar pane, j/k:** existing sidecar scroll continues to work (must not regress).
5. **No `Failed to scroll focused pane:` or `Failed to scroll terminal: code …` log lines** under the above repros at `RUST_LOG=info`.
6. **F1 help text** continues to match observed behavior — current text "Click pane → Focus pane / Scroll → Scroll focused pane" is fine; just make it true.
7. **Manual test on at least two terminals**: Konsole (Linux) and Termux (Android over SSH). Both must show wheel/touch scroll working in worker panes.

## Demo statement (Definition of Done)

Open `cas` in factory mode, focus a worker pane running Claude Code, mouse-wheel up — Claude's transcript scrolls back through history. The same gesture works via two-finger swipe in Termux over SSH from an Android phone.

## References

- Ratatui alternate-screen tradeoffs: https://ratatui.rs/concepts/backends/alternate-screen/
- tmux discussion of the exact same UX trap: https://github.com/tmux/tmux/issues/3705
- mosh long-standing alt-screen+scrollback issue: https://github.com/mobile-shell/mosh/issues/2
