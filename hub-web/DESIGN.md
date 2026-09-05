---
source:
  - hub-web/src/styles.css
theme: dark-first
colors:
  bg: "--bg-root #101318"
  surface: "--bg-panel #151922"
  surface-raised: "--bg-raised #1B202B"
  surface-hover: "--bg-hover #222836"
  surface-active: "--bg-active #2A3142"
  terminal: "--bg-terminal #0C0E13"
  border: "--line-subtle #232936; focused pane --line-strong #38415A"
  text: "--text-hi #E8EBF2"
  text-muted: "--text-mid #9AA3B5; tertiary --text-lo #5C6577"
  primary: "--state-info #6CA7F2 (running state and keyboard focus only; never a fill)"
  success: "--state-ok #4CC38A"
  warning: "--state-warn #E5B454; --tint-warn rgba(229,180,84,.09)"
  danger: "--state-crit #E5645E; --tint-crit rgba(229,100,94,.10)"
  idle: "--state-idle #5C6577 (same value as --text-lo, on purpose)"
  overlay: "--overlay-backdrop and --overlay-shadow-color = color-mix(--bg-terminal 72%, transparent)"
typography:
  families:
    ui: "--font-ui Inter, ui-sans-serif, system-ui, sans-serif"
    mono: "--font-mono \"JetBrains Mono\", \"IBM Plex Mono\", monospace"
  scale:
    xs: "--fs-xs .6875rem (11px) / eyebrows, chips, pane chrome, metadata"
    sm: "--fs-sm .78125rem (12.5px) / card prose, secondary controls"
    base: "--fs-base .84375rem (13.5px) / body, buttons, session names in cards"
    terminal: "--fs-terminal .8125rem (13px) at --line-terminal 1.35, clamped 12–16px by Ghostty"
    md: "--fs-md .9375rem (15px) / section titles, transcript reading view"
    lg: "--fs-lg 1.125rem (18px) / the open session title and the pairing code"
  weights: "--weight-regular 400, --weight-medium 500, --weight-semibold 600; nothing heavier"
  line-height: "--line-ui 1.4"
  tracking: "--tracking-label .06em on uppercase eyebrows"
spacing:
  base: "4px"
  steps: "--space-1 4, --space-2 8, --space-3 12, --space-4 16, --space-5 20, --space-6 24, --space-8 32, --space-10 40"
radius:
  card: "--radius-card 6px"
  pane: "--radius-pane 8px"
  pill: "--radius-pill 999px"
elevation:
  root: "--bg-root, no shadow"
  panel: "--bg-panel, no shadow"
  raised: "--bg-raised, no shadow"
  overlay: "--shadow-overlay 0 24px 80px var(--overlay-shadow-color); dialog and #toast only"
geometry:
  rail: "--machine-rail-width 48px; --rail-item-min 44px per phone-rail control"
  drawer: "--machine-drawer-width 280px"
  context: "--context-panel-width 320px; --landscape-attention-rail-width 80px"
  header: "--session-header-height 44px; --pane-header-height 32px"
  buttons: "--button-height 40px; --button-compact-height 28px"
  dialog: "--dialog-width 520px, max-height min(88dvh, 720px)"
  cards: "--terminal-state-width 360px; --fleet-card-min-width 260px; --fleet-board-max-width 1120px"
  phone: "--mobile-context-pill-width 152px; --mobile-attention-label-width 200px; --mobile-drawer-max-height 520px"
  motion: "--chrome-motion-duration 120ms; --attention-motion-duration 150ms; --connection-spin-duration 800ms"
breakpoints:
  phone: "(max-width: 53rem), (max-height: 30rem) and (pointer: coarse) — PHONE_MEDIA_QUERY in hub-web/src/viewport.ts"
  landscape_phone: "(max-height: 30rem) and (pointer: coarse)"
  compact: "(max-width: 53rem) — column floor and transcript default only"
  narrow: "(max-width: 500px) — header chips and ⌘K trigger drop"
---

## Overview

Cassy Commander is a dark-only mission-control console: plain TypeScript, one stylesheet, no framework.
Every application colour, size and duration is a custom property in the `:root` block of `hub-web/src/styles.css`; component rules consume tokens and an invariant test rejects any hex or `rgb()` below that block.
Cool graphite surfaces recede so terminal pixels stay the deepest thing on screen; saturated colour appears only where it encodes health, severity, focus or connection state.
Desktop is three columns (rail · canvas · context panel). A phone in portrait is one column over a 48px bottom bar; a phone on its side puts the rails on the long edges.
The canvas with machines paired and nothing open is the fleet board — machines and their sessions as cards — not an empty card pointing at a drawer.
Ghostty's 16-colour ANSI palette in `hub-web/src/terminal/ghostty-adapter.ts` is terminal content, separate from these tokens.

## Colors

- `--bg-root` is the page and the 8px gutter between shell regions; regions separate through the surface ramp and that gutter, never through full-height borders.
- `--bg-panel` is quiet chrome: rail, drawer, header, context panel, phase chips, `.pair-details`. Never behind terminal pixels.
- `--bg-raised` is every card, row, input and button, and the desktop machine tile. Hover moves to `--bg-hover`; selection, the primary button, the active tab and the active machine tile move to `--bg-active`.
- `--bg-terminal` is reserved for terminal mounts, the transcript, code wells, dialog inputs and the connection log.
- `--line-subtle` is a hairline for search and palette inputs only. `--line-strong` is the focused pane border and the selected context tab underline; the phone block contains no `--line-strong` at all (D7 invariant).
- `--text-hi` is primary copy and every identifier the operator acts on; `--text-mid` (6.4:1 on `--bg-raised`) is labels, prose, eyebrows, session metadata and empty-state guidance — anything the operator is meant to read; `--text-lo` (2.8:1) is only timestamps, disabled metadata and unfocused pane chrome.
- `--state-ok`, `--state-info` and `--state-idle` show through dots, text and outlines only. `--state-warn` and `--state-crit` may sit on `--tint-warn`/`--tint-crit` behind actionable content: the warning attention card, the blocked phase chip, the stale status notice, the browser-unsupported line; critical cards add a 2px left rule.
- `.danger` (Interrupt, remove machine, explicit dismiss) is red text on a normal surface, never a red fill.
- Only two `box-shadow` declarations exist — `dialog` and `#toast` — and the test suite counts them.

## Typography

- Human sentences, buttons, titles, headlines and explanations use `--font-ui`. Anything a machine minted — session names, agent codenames, task IDs, paths, JSON, timestamps, connection phases, pairing codes, scope names — uses `--font-mono`, even inside UI prose.
- The one `--fs-lg` title is the open session name in `.session-header h1.toolbar-session-title`; "Fleet overview" is the same slot in the UI face. Panels and dialogs title at `--fs-md` semibold.
- Eyebrows (`.session-eyebrow`, `.picker-machine`, `.status-section-label`, `.fleet-machine-phase`, `.pair-details dt`, `.pane-role`) are `--fs-xs`, uppercase, `--tracking-label`; those that name something use `--text-mid`, `.pane-role` alone stays `--text-lo`. Codenames are never uppercased.
- Chips (`.phase-chip`, `.status-chip`, `.mode-badge`) are `--fs-xs` mono uppercase on `--bg-panel` with `--radius-pill`; only blocked/control borrow a state colour.
- Card prose (`.attention-detail`, `.status-activity`, `.session-summary-description`) is `--fs-sm` and wraps with `text-wrap: pretty` or a two-line clamp; it never ellipsises after four words.
- Ghostty reads `--font-mono` and `--fs-terminal` at mount and clamps 12–16px. The transcript reads at `--fs-md`/1.5 mono because it exists to be read, and its hanging indent is in `ch`, which only lines up in the mono face.
- Weight above 600 is prohibited everywhere, including ANSI bold in the renderer.

## Layout

- All padding and gaps are `--space-*`: cards and rows 12px, panels 16px, pane and shell gaps 8px, in-card gaps 4–8px.
- Desktop: `.shell` is `48px · minmax(0,1fr) · 320px`; an open drawer widens the first track by 280px; a collapsed context panel narrows the third to 48px. The `.session-header` is exactly 44px; the supervisor pane takes 65fr and the worker strip 35fr; collapsed worker bars are 32px tall.
- The fleet board (`.fleet-board`) sits at the top of the canvas, centred at up to 1120px, one `.fleet-machine` section per machine with `.fleet-session` cards on `repeat(auto-fill, minmax(260px, 1fr))`.
- The phone rule is `PHONE_MEDIA_QUERY`, verbatim in the stylesheet and in every `matchMedia` call, so rotation can never put CSS and pane-mounting logic in different modes. `compact` stays width-only: it decides the 80-column PTY floor and the transcript default, which a landscape phone genuinely has the width to skip.
- Portrait phone: `main` over a 48px + safe-area bottom bar. The bar holds exactly four labelled controls at `--rail-item-min`: Machines and Pair share the left, the attention summary and the envelope share a 152px pill on the right. Machine chips are hidden here; machines live in the drawer the bar opens and the header names the open one.
- An expanded phone panel takes a row of `min(45dvh, 520px)` above the bar. Its rail is hidden; the tab row (48px) carries Attention, Workers & Tasks and the close control.
- Landscape phone: rails on the long edges, 48px machines left (initials) and an 80px labelled attention column right, both honouring safe-area insets; the drawer is a left sheet and the expanded panel a right-hand sheet over the terminal, never a row taken from it. Worker strip capped at 30dvh instead of 40dvh.
- Below `max-width: 500px` the header drops the machine, mode and latency chips and the ⌘K trigger. Above it, those chips render only while a session is open — on the fleet board they described nothing.
- On a phone only the primary pane mounts a terminal; every other pane is a 40px tappable row that opens as primary, so the reorder glyphs are hidden and only the view toggle and Find remain in the 32px pane header.
- `.talk-supervisor` is the selected session's own 48px row above the bar; it hides itself while the composer is already on screen (`:has()` on the open status tab) and returns when the panel or tab changes.
- `.shell` owns `100dvh`; interior regions scroll independently.

## Elevation & Depth

- Depth is the ordered ramp `--bg-root` → `--bg-panel` → `--bg-raised` → (`--bg-hover`, `--bg-active`); terminals deliberately step back to `--bg-terminal`.
- Pane focus turns the reserved transparent 1px border to `--line-strong` and nothing else: no glow, no shadow, no layout shift.
- Cards, tiles, buttons, sidebars and panes have no shadow. `dialog` and `#toast` are the only overlays and the only shadows; the modal backdrop derives from `--bg-terminal` through `--overlay-backdrop`.
- Phone sheets (drawer, expanded landscape panel) are `position: fixed` surfaces on `--bg-panel` with `--radius-pane`, still without shadow; their depth reads from the darker terminal beneath them.

## Shapes

- `--radius-card` (6px): cards, rows, buttons, inputs, chips that are not pills, code wells, the machine tile, the rail item.
- `--radius-pane` (8px): terminal panes, dialogs, the empty/connecting card, fleet session cards, phone sheets.
- `--radius-pill` (999px): status dots, count badges, phase and status chips, the back control, header chips. Never `50%`.
- Borders: 1px reserved-transparent on panes, 1px `--line-subtle` on search inputs, 1px `--text-mid` on the observer badge, 2px `--state-crit` left rule on critical cards. Focus is the sole 2px outline, `--state-info`, offset 2px.
- Icons are stroke glyphs at 20px (`.commander-mark-icon`) or single characters; status dots are 8px.

## Components

- Shell and rail: `.shell`, `.machine-navigation`, `.machine-rail`, `.machine-icon` (raised tile, `--bg-active` when selected), `.rail-control` (transparent glyph, raised on hover) — `render()` in `hub-web/src/main.ts`.
- Fleet board: `.fleet-board`, `.fleet-machine-header` (dot · name · phase eyebrow), `.fleet-session` (mono name, phase chip, two-line summary, `--text-mid` meta) — `FleetBoardRenderer` in `hub-web/src/fleet-board.ts`, fed by `renderFleetBoard()` in `hub-web/src/main.ts`; keyed on the board element plus a phase-only signature, so a shell rebuild refills the new container and a heartbeat never rebuilds it.
- Drawer and session rows: `.machine-row`, `.nav-item`, `sessionButton()`; the codename stays mono in `.session-name` and the enriched card puts the title in the UI face with the codename as a mono eyebrow.
- Header: `.session-header`, `.session-back`, `.session-picker-toggle`, `.machine-chip`, `.mode-badge`, `.connection-summary`; only an open session gets the mono `.toolbar-session-title` and the three chips.
- Terminal pane: `.pane`, `.pane.selected`, `.pane-header`, `.pane-layout-controls`, `.terminal-mount`, `.transcript`, `.transcript-jump` — `renderSessionState()` in `hub-web/src/main.ts`, `hub-web/src/transcript-view.ts`.
- Canvas states: `.empty-pane-slot` (no machine / no session / no panes) and `.terminal-state` (connecting, failed, retry) are the same centred 360px raised card with `--radius-pane`; the connecting card stacks spinner, mono title, elapsed, amber step and actions — `renderConnectionSurface()` in `hub-web/src/main.ts`.
- Attention: `.attention-panel-header`, `.attention-group-header` (session name is the heading, `--text-hi` mono), `.attention-item` with `--critical`/`--warning` modifiers; cards omit `.attention-session` when the group already names it — `hub-web/src/attention-view.ts`.
- Workers & Tasks: `.status-section-label`, `.status-row.status-agent`/`.status-task`, `.status-line` (mono identifiers + `.status-chip`), `.status-activity`/`.status-task-title` prose — `renderStatus()` in `hub-web/src/main.ts`.
- Composer: `.message`, `#message-text`, `.composer-actions`, `#message-mic` (full-width primary on phone only when feature detection succeeds), explicit `#message-send` — `openSupervisorComposer()` in `hub-web/src/main.ts`.
- Pairing dialog: `dialog`, `.pair-flow`, `.pair-details` (uppercase eyebrow terms, mono values, stacked on phone), `.pair-code`, sticky `.dialog-actions` — `pairDialogMarkup()` in `hub-web/src/main.ts`, cancel semantics in `hub-web/src/pairing-dialog.ts`. `.pair-status` is always in the markup and is a live region with the submit/create/close buttons, so a failed exchange re-enables Pair under the focused field; `.pair-cleanup` is the "Could not finish cancelling" step with Retry cleanup. Each step has one `.primary`: Create pairing code with no invitation (no Pair control exists there; `.pair-alternative` names the link path), Pair on the confirmation form an invitation opens directly. `.pair-details` leads with a plain capability line (`scopeSummary()` in `hub-web/src/pairing-scopes.ts`) above the exact origin and exact scopes; the legacy form asks for the machine's hub address with `.field-hint` guidance and never seeds it from the page origin (`#pair-use-page-origin` fills it on request). Success toasts "Access saved — connecting to <machine>…" and only a live connection toasts "<machine> connected".
- Pickers: `.command-palette`, `.session-picker`, `.palette-command`, `.session-picker-entry[aria-current]`.
- Toast: `#toast`, body-level overlay above the phone bar.

## Do's & Don'ts

- ✅ Add every colour, size and duration to the `:root` block of `hub-web/src/styles.css` and consume it with `var(...)`.
- ❌ Never write a hex, `rgb()` or `rgba()` outside `:root`, in TypeScript markup, or in `hub-web/index.html` — the invariant test fails the build.
- ✅ Keep Ghostty's ANSI data in `hub-web/src/terminal/ghostty-adapter.ts` independent of application tokens.
- ❌ Never point ANSI entries at `--state-*` or reuse ANSI RGB for buttons and cards.
- ✅ Wrap every codename, ID, path, timestamp and connection phase in a mono class, even mid-sentence.
- ❌ Never render a session codename in the UI face or uppercase it in an eyebrow.
- ✅ Use the 11/12.5/13.5/15/18px scale, weights 400–600, radii 6/8/999 and the 4px spacing steps.
- ❌ Never add a third shadow, a coloured card fill for ok/info, a gradient, a pulse glow or a decorative coloured border.
- ✅ Give the phone rail one container treatment (`--rail-item-min`, `--bg-raised`, `--radius-card`) for every control in it.
- ❌ Never put `--line-strong` inside the phone media block; it is the focused-pane border and the phone has no focusable pane chrome.
- ✅ Keep state that changes every heartbeat (latency, counts, stale age) out of `shellSignature()` and out of region-updater signatures; the fleet board keys on connection phase, not latency.
- ❌ Never let a region updater call `innerHTML` into the live shell; it owns and clears its own container and binds its own handlers.
- ✅ Keep Send explicit, keep the dictated text editable, keep Cancel destroying a pairing invitation.
- ❌ Never auto-send a transcript, hide the keyboard fallback, or make a cancelled invitation retrievable.
- ✅ Change `hub-web/src/styles.css` and `hub-web/src/main.ts`, then let the integration owner rebuild `hub-web/dist` once.
- ❌ Never hand-edit or commit generated `hub-web/dist` output from a factory lane.

## Behavioural constraints
<!-- keep -->

These are engineering decisions the visual system sits on. They survive redesigns.

**Render model.** `render()` chooses one of three paths via `renderDecision()` in `hub-web/src/render-model.ts`: *regions* (default, the only path a heartbeat may take — `renderRegions()` writes into nodes already on screen), *shell* (full rebuild, only when `shellSignature()` changed), or *defer* (signature changed while a form control has focus; flushed by `DeferredRenderScheduler` in `hub-web/src/deferred-render.ts` after focus leaves and the pointer gesture has delivered its click — a macrotask, because the click is dispatched in the same task as pointerup). If a value appears in shell markup it belongs in `shellSignature()` or `applyLiveRegions()`; per-heartbeat data must never enter the signature; `applyLiveRegions()` writes only into existing nodes; anything a region re-creates (rail, drawer tree, session picker, fleet board) binds its own handlers; lease identity is deliberately structural.

**Pairing cancellation.** Cancel discards the invitation: `cancelPendingPairing()` invalidates the in-flight operation, clears the pending store and resets the draft; reopening offers the create-code flow and says pairing needs a fresh URL from `cas hub pair`. A pairing invitation is a one-time capability and Cancel is the operator saying the request must not proceed — including when the link went somewhere it should not have. The dialog closes only once the page can vouch that the cancellation is durable; otherwise it stays on a cleanup step with a retry that never resumes the invitation (`cancellationOutcome()` in `hub-web/src/pairing-cleanup.ts`). A cancellation owns that step through `PairingCancellationTracker` in `hub-web/src/pairing-cancellation.ts`: a rollback that rejects after Cancel still lands on the step, retries run one at a time and report rejection inside the dialog, and any replacement flow supersedes the cancellation so a late result can never close or rewrite it. Browser cancel blocks this browser only; it does not revoke the machine's invitation.

**Narrow-viewport text (D15).** Below `compact`, `GhosttyTerminalSurface.setMinimumColumns` floors the PTY at 80 columns; the canvas sizes to the grid and `.terminal-mount` pans, so "Show terminal" shows the real grid. The transcript (`hub-web/src/transcript.ts` model, `hub-web/src/transcript-view.ts` DOM) reflows the emulator's logical lines from `GhosttySnapshot.rowData` in the browser — no hub-side projection, no new endpoint. Its history is the emulator's scrollback; reaching the top pages the viewport back and "Jump to latest" returns. While the transcript is visible canvas paint is skipped but the snapshot is still taken; a tap on the transcript focuses the pane input.

**Phone layout invariants.** The phone rule keys on the short axis and pointer, not width alone (D5); every drawer/attention `.shell` class combination is listed in the phone block because a media query adds no specificity; only the primary pane mounts a terminal on a phone; the collapsed context pill paints no surface of its own and never spills across Pair (D7); severity is carried by text colour and the dot, never by a fill only some severities receive (D8).
<!-- /keep -->
