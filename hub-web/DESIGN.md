---
source:
  - hub-web/src/styles.css
theme: dark-first
colors:
  bg: "--bg-root #101318"
  surface: "--bg-panel #151922"
  surface-raised: "--bg-raised #1B202B"
  terminal: "--bg-terminal #0C0E13"
  border: "--line-subtle #232936; focused --line-strong #38415A"
  text: "--text-hi #E8EBF2"
  text-muted: "--text-mid #9AA3B5; tertiary --text-lo #5C6577"
  primary: "--state-info #6CA7F2 (state and focus only)"
  success: "--state-ok #4CC38A"
  warning: "--state-warn #E5B454; --tint-warn rgba(229,180,84,.09)"
  danger: "--state-crit #E5645E; --tint-crit rgba(229,100,94,.10)"
typography:
  families:
    ui: "--font-ui Inter, ui-sans-serif, system-ui, sans-serif"
    mono: "--font-mono JetBrains Mono, IBM Plex Mono, monospace"
  scale:
    xs: "--fs-xs 11px / uppercase metadata and badges / 400–600"
    sm: "--fs-sm 12.5px / card body and secondary UI / 400–600"
    base: "--fs-base 13.5px / primary UI and buttons / 400–600"
    terminal: "--fs-terminal 13px / 1.35 / 400–600"
    md: "--fs-md 15px / pane and section titles / 400–600"
    lg: "--fs-lg 18px / current session title only / 400–600"
rail:
  item: "--rail-item-min 44px / one container treatment for every phone-rail control"
spacing:
  base: "4px"
  steps: ["4px", "8px", "12px", "16px", "20px", "24px", "32px", "40px"]
radius:
  card: "--radius-card 6px"
  pane: "--radius-pane 8px"
  pill: "--radius-pill 999px"
elevation:
  root: "--bg-root, no shadow"
  panel: "--bg-panel, no shadow"
  raised: "--bg-raised, no shadow"
  overlay: "--shadow-overlay only"
breakpoints:
  phone: "(max-width: 53rem), (max-height: 30rem) and (pointer: coarse)"
  landscape_phone: "(max-height: 30rem) and (pointer: coarse)"
  compact: "max-width: 53rem"
---

## Overview

Cassy Commander is a dense, dark-only mission-control console built with plain TypeScript and CSS.
Its sole application token source is `hub-web/src/styles.css`; component rules consume those custom properties rather than declaring colours or type sizes.
Cool graphite surfaces make the terminal canvas recede, while saturated colour communicates health, severity, focus, or connection state only.
Desktop and compact layouts share the same 4px rhythm; the compact surface keeps one readable terminal primary rather than shrinking the desktop grid.
Ghostty's ANSI palette in `hub-web/src/terminal/ghostty-adapter.ts` is terminal content and stays separate from application tokens.

## Colors

- `--bg-root` is the shell gutter and page background; it must remain visible between regions instead of being replaced by full-height separator borders.
- `--bg-panel` is quiet chrome: sidebars, toolbar, and pane title bars. Do not use it for terminal pixels.
- `--bg-raised` is for cards, list rows, inputs, and buttons. Hover moves one step to `--bg-hover`; active selection moves to `--bg-active`.
- `--bg-terminal` is reserved for terminal panes, terminal mounts, and code wells so machine output remains the deepest layer.
- `--line-subtle` is available for true hairline separators, but ordinary cards and columns separate through surface steps and spacing.
- `--line-strong` is the focused-pane border. Do not put it around every pane or use it as a decorative accent.
- `--text-hi` is primary copy; `--text-mid` is labels and explanations; `--text-lo` is timestamps, disabled metadata, and unfocused pane chrome.
- `--state-ok` marks connected or healthy state with dots/text only; it never fills a card.
- `--state-info` marks running/informational state and keyboard focus with dots/text/outlines only; it never fills a card.
- `--state-warn` may use `--tint-warn` behind degraded or action-needed content; no unrelated amber decoration.
- `--state-crit` may use `--tint-crit` and a 2px state rule for hard failures; routine dismiss actions must not borrow it.
- `--state-idle` is intentionally the same resolved value as `--text-lo`; idle should recede instead of reading as an alert.

## Typography

- Human-written labels, buttons, explanations, titles, and attention summaries use `--font-ui`.
- Machine-generated session names, agent codenames, task IDs, paths, JSON, timestamps, connection states, and pairing codes use `--font-mono`.
- Session names such as `fast-badger-22` remain mono in `.session-name`, `.toolbar-session-title`, `.pane-title`, attention group headers, and status rows.
- Use `--fs-xs` for uppercase eyebrows and badges with `--tracking-label`; use `--fs-sm` for secondary UI and card prose.
- `--fs-base` is the default interactive scale; `--fs-md` is for pane/section titles; `--fs-lg` is the one current-session title scale.
- Ghostty reads `--font-mono` and `--fs-terminal` at mount time; its size clamps to 12–16px and defaults to 13px at 1.35 line height.
- The transcript reads at `--fs-md`/1.5 in `--font-mono`: it exists to be read at UI size, so it does not follow the `--fs-terminal` clamp. Its hanging indent is expressed in `ch`, which only lines up in the mono face.
- Use `--weight-regular`, `--weight-medium`, or `--weight-semibold`. Weight above 600 is prohibited, including ANSI bold rendering.
- Hierarchy comes from scale and text colour, not from heavier slabs or an extra display face.

## Layout

- All padding and gaps use the `--space-*` 4px scale. Cards and rows use 12px; panels use 16px; pane and shell gaps use 8px.
- Desktop chrome is centralized as a 48px `--machine-rail-width`, a 212px `--machine-drawer-width` (260px expanded navigation), and a 320px `--context-panel-width`.
- `.shell` exposes `--bg-root` through its 8px gaps; no full-height left/right column borders may recreate the old boxed grid.
- `.session-header` is exactly 44px. The supervisor receives 65% of the pane grid and the worker strip 35%; collapsed worker bars are exactly 32px.
- The collapsed attention region is the same 48px rail width, while the expanded operations panel occupies the 320px context column.
- A phone is `(max-width: 53rem), (max-height: 30rem) and (pointer: coarse)` — either axis too small for desktop chrome, driven by a finger. Width alone is a narrow-window test, not a phone test: a rotated Pixel 7 is 915px wide and would take the desktop grid onto a 412px-tall screen. The pointer clause keeps a mouse-driven desktop with a short window on the desktop layout. This exact string is `PHONE_MEDIA_QUERY` in `hub-web/src/viewport.ts`, and the stylesheet and every `matchMedia` call must use it, so rotation can never put the layout and the pane-mounting logic in different modes.
- `compact` stays width-only and separate. It answers how many columns fit across a mount — the 80-column PTY floor and the transcript default — and a phone in landscape genuinely has the width for a wider grid.
- In portrait, navigation becomes a 48px bottom rail, drawers open above it, and workers scroll horizontally below the readable supervisor terminal.
- In landscape, the chrome moves to the long edges: machines on the left, the collapsed attention rail on the right, both 48px, each honouring its safe-area inset. The terminal keeps the full height, the machine drawer opens as a left sheet, and the expanded operations panel is a right-hand sheet over the terminal — never a row taken from it. The worker strip is capped at 30dvh there instead of 40dvh.
- Every control in that rail — Machines, each machine chip, Pair, the attention summary, the message button — shares one container treatment at `--rail-item-min` (44px) on `--bg-raised` with `--radius-card`. The rail is one bar, so it may not mix bordered, filled, and bare controls in a single row.
- The collapsed context pill floats over the rail: it stays transparent, clips its own contents, and lets its two controls carry the shared rail-item surface, so it never paints a second surface or spills across Pair.
- At `max-width: 53rem` a pane defaults to the transcript reading view and its PTY is held at a floor of 80 columns; the canvas then sizes to that grid instead of the mount, and terminal view pans horizontally. Above the breakpoint nothing changes: no column floor, terminal view by default.
- The selected session's `.talk-supervisor` action occupies its own 48px row immediately above the phone rail; it is never moved into the top-bar overflow or a drawer.
- The phone message composer makes `#message-mic` the full-width first action, with Keyboard and explicit Send beneath it; desktop leaves the textarea keyboard-primary and keeps the mic secondary.
- Compact pane-order controls use 24px geometry; primary buttons remain 40px high.
- `.shell` owns `100dvh`; interior regions scroll independently and no page-height content sits beside it.

## Elevation & Depth

- Depth is the ordered surface ramp: `--bg-root` → `--bg-panel` → `--bg-raised`; terminals deliberately step back to `--bg-terminal`.
- Pane focus changes only the transparent reserved border to `--line-strong`; it does not add glow, shadow, or layout shift.
- Cards, buttons, sidebars, and terminal panes have no shadow. Surface colour and spacing carry their hierarchy.
- `dialog` and `#toast` are overlays and are the only elements allowed to use `--shadow-overlay`.
- The modal backdrop derives from `--bg-terminal` through `--overlay-backdrop`; it must not introduce another ad-hoc black.

## Shapes

- Cards, list rows, buttons, inputs, code wells, and state blocks use `--radius-card` (6px).
- Terminal panes and dialogs use `--radius-pane` (8px).
- Status dots, count badges, and compact connection pills use `--radius-pill` (999px), never `50%` or another radius.
- The default pane reserves a transparent `--line-width` border so focused state cannot move content; only `.pane.selected` makes it visible.
- Critical attention uses a `--state-rule-width` left rule because the rule communicates severity; ordinary cards stay borderless.
- Keyboard focus is the sole 2px outline and uses `--focus-ring-width` with `--state-info`.

## Components

- Shell/navigation: `.shell`, `.machine-navigation`, `.machine-rail`, `.machine-drawer`, `.context-panel`, and `.nav-item`; selected rows use `--bg-active`, not a saturated fill.
- Session row: `sessionButton()` in `hub-web/src/main.ts`; `.session-name` and `.session-meta` keep the codename, supervisor, worker count, and liveness mono.
- Enriched session card: `.session-summary-title`, `.session-summary-description`, and `.phase-chip` consume the single server-broadcast summary. The machine codename remains a mono eyebrow; stale active descriptions dim after ten minutes. Testing/building use info text, blocked alone may use the warning tint, and idle recedes. Enrichment is per-machine, default off, and must plainly warn that redacted terminal transcript excerpts are sent to the configured model provider.
- Header: `.session-header` in `hub-web/src/styles.css` and `render()` in `hub-web/src/main.ts`; only an active session receives `.toolbar-session-title` mono styling.
- Terminal pane: `.pane`, `.pane.selected`, `.pane-header`, and `.terminal-mount`; the terminal canvas owns its independent ANSI palette.
- Pane transcript: `.transcript`, `.transcript-line`, `.transcript-jump`, and `.pane-view-toggle`; the reflowed reading view of a pane, mono face at `--fs-md` over `--bg-terminal`, layered above the grid rather than replacing it. Colour comes from the same ANSI cells the canvas paints, so it stays outside the application palette.
- Attention counts: `.attention-count` is one badge on `--bg-active` for every severity, and the collapsed phone rail shows the single labelled `.attention-summary` instead of three bare numbers; severity is carried by text colour and the dot, never by a fill only some severities receive.
- Attention/status cards: `.attention-item` and `.status-row`; prose stays UI face while `.attention-group-label`, `.attention-ticket`, and `.status-identifier` isolate machine copy.
- Primary button: `.primary`; it uses `--bg-active` because blue is state/focus, not a decorative call-to-action fill.
- Pairing dialog: `dialog`, `.pair-flow`, `.pair-code`, and `.pair-details`; code, origins, URLs, and scopes are mono inside an otherwise UI-face flow.
- Supervisor messaging: `.talk-supervisor`, `.message textarea`, `.composer-actions`, and `openSupervisorComposer()` in `hub-web/src/main.ts`; the selected session supplies the exact supervisor target, the phone mic is primary only when feature detection succeeds, and Send remains explicit.
- Inputs: `dialog input` and `.message textarea`; raised or terminal fills replace permanent one-pixel boxes, with focus-visible outline for keyboard state.
- Terminal state: `.terminal-state`; warning tint is allowed because connection degradation is actionable state, and the retry remains a normal button.
- Toast: `#toast`; it is an overlay, so it may use the one soft overlay shadow but no saturated fill.

## Streaming text on narrow viewports (D15)

A 395px mount measures roughly 46 columns, and Commander used to hand exactly
that to the PTY. An 80-column agent TUI redrawn at half its width destroys its
own layout before Commander sees a byte: hanging indents collapse to column 0,
tags overrun the input rule, and every redraw reflows. Shrinking the font buys
a handful of columns and fixes none of it.

The fix has two halves.

- **Geometry.** `GhosttyTerminalSurface.setMinimumColumns` puts a floor under
  the columns reported to the PTY, applied by `main.ts` only below the compact
  breakpoint. The canvas backing store then sizes to the grid rather than to the
  mount, and `.terminal-mount` pans horizontally so "Show terminal" shows the
  real 80-column grid instead of a squeezed one. The floor holds in both views,
  so toggling never churns a PTY resize.
- **Reading view.** `TranscriptView` renders the pane's logical lines as
  wrapping text at UI size, with a per-line hanging indent so a wrap stays
  inside its own gutter.

**Seam decision — client-side, from the emulator snapshot.** The transcript is
built in the browser from `GhosttySnapshot.rowData`, which already carries
`isWrapContinuation` / `wrapsToNext` for soft wraps and per-cell colour, weight
and decoration for ANSI styling. That is everything a reflow needs, so there is
no hub-side "logical lines" projection and no new endpoint: a projection would
mean running a second emulator over the same bytes on the hub and inventing a
capability flag to negotiate it. `transcript.ts` holds the pure line model and
`transcript-view.ts` the DOM; the view follows the surface's own render tick
through the `onRender` callback rather than polling.

Consequences worth knowing:

- Transcript history is the emulator's scrollback, not a second buffer.
  Reaching the top of the transcript pages the terminal viewport back; "Jump to
  latest" returns it to the live tail.
- While the transcript is visible the canvas paint is skipped
  (`setCanvasPainting(false)`); the snapshot is still taken because the
  transcript is read from it.
- A tap on the transcript focuses the pane input, so the reading view never
  costs the operator the keyboard.

## Do's & Don'ts

- ✅ Add every application colour to the `:root` token block in `hub-web/src/styles.css` and consume it with `var(...)`.
- ❌ Never add a hex, `rgb()`, or `rgba()` colour to component rules, TypeScript markup, or `hub-web/index.html`.
- ✅ Keep the Ghostty 16-colour ANSI data in `hub-web/src/terminal/ghostty-adapter.ts` independent from the application palette.
- ❌ Never point ANSI entries at `--state-*`, or use ANSI RGB values for buttons/cards.
- ✅ Give critical and warning state the permitted low-alpha tints; show OK/info through dots, text, and focus outlines only.
- ❌ Never use green or blue card fills, gradient decoration, pulse glows, or coloured borders that do not encode state.
- ✅ Wrap session names, agent names, IDs, paths, timestamps, JSON, and connection state in a mono class even when embedded in UI prose.
- ❌ Never render a session codename in the UI face, including sidebar rows, header titles, pane bars, or attention grouping.
- ✅ Use only the 11/12.5/13.5/15/18px UI scale and 13px terminal default; clamp terminal controls to 12–16px.
- ❌ Never use font weight above 600 or add an unscaled one-off display size.
- ✅ Use 12px padding for cards/rows, 16px for panels, and 8px gaps between panes.
- ❌ Never add off-grid spacing or radii other than 6px, 8px, and 999px.
- ✅ Hide speech controls when secure-context/browser detection fails and keep the same editable textarea and explicit Send path after dictation.
- ❌ Never auto-send a speech transcript, nag after mic denial, or remove the one-tap keyboard fallback on phone.
- ✅ Separate shell regions with the graphite surface ramp and the 8px root gutter.
- ❌ Never restore full-height 1px column borders or shadows on non-overlay surfaces.
- ✅ Change `hub-web/src/styles.css`, then let the integration owner rebuild distribution assets once.
- ❌ Never hand-edit or commit generated `hub-web/dist` output from a factory lane.
