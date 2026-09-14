Focused hosted pane: show the child cursor at its reported pane-relative position and preserve its visibility and shape.
Remedy: emit the host cursor only for the focused PTY pane; suppress it for sidecar/modal/mission-control views and non-PTY panes.

## Concept brief

### Scannable

- Factory panes remain bordered in full view and borderless in compact view.
- A focused PTY pane owns the host cursor at its terminal-reported position,
  translated into the pane's visible content rectangle.
- Unfocused panes remain visible without moving or showing the host cursor.
- Raw terminal clients and hosted/relay clients keep the same cursor protocol;
  modal overlays, sidecar focus, and Mission Control hide the host cursor.

### Readable

Pane text and existing focus borders remain unchanged. Cursor movement and
shape are terminal state, not extra prose; no cursor diagnostics are rendered
in the normal operator view.

### Machine output

This surface is an ANSI terminal stream, not a JSON command. Cursor position,
visibility, and shape are represented by standard terminal control sequences.

### Omitted

Raw escape bytes are not shown in the pane. Debug traces and scoped terminal
captures may record them for verification, but operator output should expose
only the resulting cursor state.

## Acceptance notes

The implementation must retain resize geometry, pane focus transitions,
unfocused behavior, and modal suppression. The live Codex operator flow is
reported separately if the environment cannot provide a running Codex harness;
fixture or snapshot evidence is not a substitute for that flow.

## Critique receipt

Implementation receipt:

`terminal-qa: PASS cas-31fa-version-surface · 11 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-31fa/terminal-qa-version2/report.json`

The real built binary (`target/debug/cas`, revision `a2e6338` plus the task
changes) was exercised at 80 and 120 columns, all four palette hints, C
locale, and `NO_COLOR`. The feature-specific factory TUI requires an
interactive harness/PTY and was not available to this worker; the scoped
operator-flow regression therefore remains the durable Rust render capture,
not a live Codex observation. An exploratory `cas --help` capture was
rejected by terminal QA for pre-existing help-surface overflow/locale output
and is not used as evidence for this cursor change.

| Surface | Verdict | Evidence |
| --- | --- | --- |
| Ghostty raw CSI visibility/DECSCUSR parsing | PASS | `test_cursor_state_tracks_visibility_shape_and_blink` |
| Full hosted focused pane | PASS | `focused_pane_cursor_is_forwarded_to_the_host_terminal` |
| Compact hosted focused pane | PASS | `compact_supervisor_cursor_uses_borderless_content_origin` |
| Hidden child and modal overlay | PASS | `hidden_child_cursor_stays_hidden_on_the_host`; `modal_overlay_suppresses_host_cursor` |
| Pane + host resize | PASS | `focused_cursor_survives_pane_and_host_resize` |
| Live Codex PTY operator flow | NOT EXERCISED | No harness/interactive PTY was available in this worker |
