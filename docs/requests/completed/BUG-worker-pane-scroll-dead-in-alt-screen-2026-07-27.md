---
filed: 2026-05-07
refiled: 2026-07-27
resolved: 2026-07-27
priority: P2
cas_task: cas-3f23
status: DUPLICATE
duplicate_of: docs/requests/completed/SHIPPED-worker-pane-mouse-wheel-alt-screen.md
---

# BUG: Worker pane mouse wheel / touch scroll does nothing while inner TUI is in alt-screen (Claude Code)

**Filed:** 2026-05-07 (originally as Penguinz task cas-c08d, P1; moved to this inbox 2026-07-27 — CAS-tooling issue, wrong board)
**Affected version:** `cas 2.13.0 (7450278 2026-05-06)`
**Status:** RESOLVED — duplicate, see Resolution below.
**Prior history:** the "prior fix attempt [that] did not land the behavior" referenced by the original 2026-05-07 report is `53a7bf4` (cas-d5fa) — it forwarded arrow-key bytes, which landed in Claude Code's prompt-input history instead of scrolling the transcript. It was superseded seven days later by `678f75b` (cas-f93a), which switched to PgUp/PgDn bytes and was empirically A/B confirmed by the user to work. This report was re-filed on 2026-07-27 reusing the original's stale `cas 2.13.0` text without checking whether f93a had already landed — see Resolution.

## Resolution (2026-07-27, cas-3f23)

**Duplicate of `docs/requests/completed/SHIPPED-worker-pane-mouse-wheel-alt-screen.md`.** This report was filed against a `cas 2.13.0` binary using `strings`-derived file:line notes; those notes are stale. On current `main` (v2.29.0, commit `8879819`), the exact behavior requested here is already implemented and covered by automated tests:

- Alt-screen detection: `crates/cas-mux/src/pane/mod.rs` scans the raw PTY byte stream for DEC private-mode 1049/47/1047 (`update_alt_screen`, `is_in_alt_screen`); exposed fleet-wide via `Mux::focused_is_in_alt_screen()` (`crates/cas-mux/src/mux.rs:1168`).
- Dispatch: `FactoryApp::handle_scroll_up`/`handle_scroll_down` (`cas-cli/src/ui/factory/app/sidecar_and_selection.rs:574`/`607`) return `ScrollAction::AltScreen` when the focused pane is in alt-screen and no dialog/help/Mission-Control overlay suppresses forwarding; otherwise they fall through to local `Pane::scroll` (host scrollback) — no regression for shell panes.
- Forwarding: on `ScrollAction::AltScreen`, wheel and PgUp/PgDn both forward harness-aware bytes to the inner PTY via the existing `Mux::send_input` path (`cas-cli/src/ui/factory/daemon/runtime/client_input.rs:157-194` for wheel, `:722-753` for PgUp/PgDn) — PgUp/PgDn (`\x1b[5~`/`\x1b[6~`) for Claude/Codex, SGR wheel for Grok (`alt_screen_wheel_bytes`, `sidecar_and_selection.rs:60`).
- F1 help text (`cas-cli/src/ui/factory/app/render_and_ops/rendering/dialogs.rs:1156`, "Scroll → Scroll focused pane") is accurate for current behavior.
- `"Failed to scroll terminal: code …"` (`crates/ghostty_vt/src/lib.rs:39`) is a genuine `ghostty_vt` error variant, not the alt-screen no-op case — that case is now intercepted before reaching `Pane::scroll`.

Verified locally: `cargo test -p cas --lib sidecar_and_selection` (25/25 pass, including `pgup_dispatch_fires_when_alt_screen_active`, `pgdn_dispatch_fires_when_alt_screen_active`, `wheel_scroll_no_regress_when_not_in_alt_screen`, `scroll_changes_dialog_blocks_alt_screen_forwarding_cas_72c3`) and `cargo test -p cas-mux --lib pane::` (35/35 pass, including the `update_alt_screen_*` DEC-sequence scanner suite). No code changes made — closing as duplicate/already-shipped.

**Manual verification still required** (cannot be automated — see task cas-3f23 notes for the human test plan): confirm on a build ≥ commit `678f75b` at an actual Konsole terminal and over Termux/SSH that wheel-up/PgUp scroll a focused Claude Code worker pane's transcript, and that a plain shell pane still scrolls its own host-side scrollback.

## Summary

Clicking a worker pane focuses it (correctly), but **mouse wheel on desktop and touch/two-finger swipe on Termux/SSH do nothing** — the worker output (Claude Code transcript) cannot be scrolled. PgUp/PgDn also do not scroll back. The in-app F1 help promises the opposite:

> Mouse: Click pane → Focus pane / Scroll → Scroll focused pane

## Reproduction

1. `cas` (factory mode), focus a worker pane running Claude Code (the default).
2. Click the worker pane (focus indicator changes — click is captured).
3. Try to scroll up:
   - Desktop terminal: mouse wheel
   - Konsole: profile has `Allow terminal applications to handle clicks and drags` AND `Enable Alternate Screen buffer scrolling` enabled — Konsole IS forwarding wheel as SGR mouse events.
   - Termux (SSH from Android): two-finger swipe (Termux maps to wheel in mouse-mode apps).
4. Result: nothing scrolls. PgUp/PgDn same. Identical across two unrelated terminal stacks → not terminal config.

## Ruled out

- **Terminal not forwarding events** — ruled out (two environments; `crossterm::event::EnableMouseCapture` present in binary).
- **Wrong pane clicked** — ruled out (focus indicator confirms; `handle_mouse_click` runs).
- **Help text merely wrong** — ruled out: binary contains `handle_scroll_up`, `handle_scroll_down`, `scroll_focused_pane`, `mc_scroll_up`, `mc_scroll_down`, `Pane::scroll`, and error strings `Failed to scroll focused pane:` / `Failed to scroll terminal: code` — code paths exist and fail at runtime.

## Architecture notes (from `strings ~/.local/bin/cas`)

**Client** (`cas::ui::factory::app::FactoryApp`, `cas/src/ui/factory/app/sidecar_and_selection.rs`): `handle_mouse_click`, `handle_scroll_up/down`, `mc_scroll_up/down`, `scroll_focused_pane`, `sidecar_scroll_down`; error log `Failed to scroll focused pane: …`

**Pane mux** (`cas_mux::pane::Pane::scroll`, `crates/cas-mux/src/pane/mod.rs` ~lines 407, 417, 638, 715, 738, 803, 818): wraps `ghostty_vt` per pane; symbols `ghostty_vt_terminal_scroll_viewport`, `_scroll_viewport_top/_bottom`, `_take_viewport_scroll_delta`, `_scrollback_info`; error log `Failed to scroll terminal: code …`

**Daemon** (`FactoryDaemon`): `build_scrollback`; `SessionState { request_scrollback: bool, scrollback, pane_id }`; trace logs `: scroll complete, after: offset=` / `: scroll delta=`

**Wire protocol:** `ClientMessage` variants Input, InputFocused, Focus, Resize, ResizePane, SpawnShell, KillShell, SpawnWorkers, ShutdownWorkers, Inject, Attach — **no dedicated Scroll/Wheel variant** → scroll likely piggybacks Input or uses the `request_scrollback` poll.

## Hypothesis (single most likely cause)

Worker panes run Claude Code — a **fullscreen TUI in alt-screen mode**. Alt-screen has no scrollback, so wheel events routed to `Pane::scroll → ghostty_vt::scroll_viewport` have nothing to scroll into: silent no-op (or the error path). The classic alt-screen scroll trap — see tmux#3705 and Konsole's "Alternate Screen buffer scrolling" workaround (translate wheel → arrow keys so the inner app paginates itself).

**Right behavior for CAS:** when the focused pane's inner process is in alt-screen, **forward wheel events to the inner process as `MouseEvent::ScrollUp/ScrollDown`** (Claude Code consumes these and scrolls its own transcript) instead of consuming them for `Pane::scroll`. The keyboard forwarding path (`ClientMessage::Input`) already exists — this is plumbing wheel events through it, gated on alt-screen state (ghostty_vt exposes the active screen).

### Secondary issue (don't lose)

Even for non-alt-screen panes, `Failed to scroll focused pane: …` implies `Pane::scroll` errors in some conditions — likely the daemon→client `request_scrollback` round-trip not delivering fresh content to the renderer, so no visual feedback even when the ghostty viewport offset moved.

## Diagnostic next steps

1. `RUST_LOG=cas_mux=trace,cas::ui::factory=trace cas 2>~/cas-scroll.log`, reproduce; check for `: scroll delta=`, `: scroll complete, after: offset=`, `Failed to scroll focused pane:`, `Failed to scroll terminal: code`.
2. Detect alt-screen per pane in `cas_mux::pane::Pane` via ghostty_vt.
3. In `FactoryApp::scroll_focused_pane`, branch: alt-screen → emit `ClientMessage::Input` with SGR wheel sequence, skip local viewport scroll; else existing path.

## Acceptance criteria

1. Worker pane (Claude Code, alt-screen), focused, wheel up → Claude's transcript scrolls. Same for PgUp.
2. Termux two-finger swipe over SSH → same as #1.
3. Shell pane (no alt-screen), wheel up → CAS pane scrollback scrolls (no regression).
4. Sidecar j/k scroll → no regression.
5. No `Failed to scroll focused pane:` / `Failed to scroll terminal: code …` at RUST_LOG=info under these repros.
6. F1 help text matches observed behavior.
7. Manual test on Konsole (Linux) AND Termux (Android/SSH).

**Demo:** factory mode, focus Claude Code worker pane, wheel up → transcript scrolls back. Same via two-finger swipe in Termux.

## References

- ratatui alt-screen tradeoffs: https://ratatui.rs/concepts/backends/alternate-screen/
- tmux same UX trap: https://github.com/tmux/tmux/issues/3705
- mosh long-standing issue: https://github.com/mobile-shell/mosh/issues/2
