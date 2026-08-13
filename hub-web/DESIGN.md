---
source:
  - hub-web/src/styles.css
theme: dark-first
colors:
  bg: "page background #080b10"
  surface: "pane/input background #0d1117"
  surface-raised: "dialog/button background #161b22"
  border: "default border #30363d"
  text: "body text #c9d1d9"
  text-muted: "secondary text #8b949e"
  primary: "primary action #1f6feb with border #388bfd"
  accent: "focus/selection #58a6ff"
  success: "connected/pulse #3fb950 / #56d364"
  warning: "compatibility #d29922 / #a36c2c"
  danger: "destructive #da3633 / #ff7b72"
typography:
  families:
    body: "Inter, ui-sans-serif, system-ui, sans-serif"
    mono: "ui-monospace, monospace"
  scale:
    metadata: "11px / compact / regular"
    label: "12px / compact / regular"
    section: "14px / compact / regular"
    title: "18px / compact / regular"
    pairing-code: "30px / compact / 700"
spacing:
  base: "2px observed increment; no named token scale"
  steps: ["2px", "4px", "6px", "8px", "10px", "12px", "14px", "16px", "18px", "22px", "24px"]
radius:
  control: "7px–8px"
  pane: "10px"
  dialog: "14px"
  pill: "999px"
elevation:
  pane: "0 10px 32px rgba(0,0,0,.22)"
  dialog: "0 24px 80px #000"
---

## Overview

Commander is a dense, dark-first browser control surface built with plain TypeScript and CSS.
Its live visual source is `hub-web/src/styles.css`; there is no component framework or separate token package.
The four-column desktop shell prioritizes terminal space, while the compact layout keeps machines, sessions, and status simultaneously reachable.
New surfaces should look operational and calm: near-black layers, one-pixel borders, compact labels, and blue reserved for the current action or selection.
The stylesheet has a single `max-width: 850px` compact breakpoint rather than a named min-width breakpoint system.

## Colors

- Page background is `#080b10`; never use pure black for the body because the radial `#17243a` wash provides the only large-area depth cue.
- Working surfaces use `#0d1117`; use it for pane interiors, text inputs, textareas, and pairing-code wells.
- Raised controls and dialogs use `#161b22`; do not use this fill for the terminal canvas or it loses depth against panes.
- Default structure is a one-pixel `#30363d` border, with quieter shell divisions at `#21262d`.
- Body text is `#c9d1d9`; secondary explanation and metadata are `#8b949e`.
- Primary actions use `#1f6feb` with `#388bfd` border and white text; only one action per decision area should receive this fill.
- Selection, hover, and focused pane borders use `#58a6ff`; never use a filled accent panel for selection.
- Connected state uses `#3fb950` or `#56d364`; it is status, not a general confirm-button color.
- Warnings combine `#a36c2c` borders with a translucent fill and `#ffe2a8` copy.
- Destructive actions use `#da3633` borders and `#ff7b72` text; interrupt uses the softer `#ffa198` copy.

## Typography

- Body UI uses `Inter, ui-sans-serif, system-ui, sans-serif` from `hub-web/src/styles.css`.
- Machine codes, connection state, and pane headers use `ui-monospace, monospace` because they are copied or scanned as operational identifiers.
- Metadata is 11px, form labels and toolbar support copy are 12px, section headings are 14px, and primary screen titles are 18px.
- Pairing codes are 30px, weight 700, with `.12em` letter spacing; no other content uses display-scale typography.
- Sidebar section headings are uppercase with `.04em` tracking; dialog titles and toolbar titles remain sentence/title case.
- Keep explanatory dialog paragraphs regular weight and muted; use `strong` only for the live countdown.
- Do not introduce a second display or serif family: Commander is an instrument panel, not editorial content.

## Layout

- Desktop shell columns are `210px 245px minmax(420px, 1fr) 300px`, with the terminal workspace owning flexible width.
- Sidebars use 16px padding; the pane grid uses 8px padding and gap; toolbars use 12px by 18px.
- Terminal cards use `repeat(auto-fit, minmax(340px, 1fr))` and `minmax(280px, 1fr)` rows.
- Dialog width is `min(520px, calc(100vw - 24px))`, preserving a 12px edge gutter on narrow screens.
- Dialog forms and `.pair-flow` use a 12px grid gap; fieldsets use two columns until the outer compact layout takes over.
- At 850px and below, the shell becomes an 84px machine rail plus the flexible workspace.
- Compact sessions become a horizontal scroller, the context area becomes a horizontal card strip, and terminals use one column.
- The mobile machine action collapses to a visible `+`; retain an accessible text label in the button markup.
- Do not add fixed page-height content outside `.shell`; it owns `100dvh` and all interior regions scroll independently.

## Elevation & Depth

- Base shell regions separate through `#21262d` borders and translucent `rgba(13,17,23,.75–.88)` fills, not shadows.
- Terminal panes earn `0 10px 32px rgba(0,0,0,.22)` because they are interactive work surfaces above the shell.
- The selected pane adds a `#58a6ff` outline while retaining the same shadow; selection must not change pane position.
- Dialogs are the highest surface with `0 24px 80px #000` and a `rgba(0,0,0,.72)` backdrop.
- Pairing-code wells use border and fill only; a code is important content inside a dialog, not another elevation layer.
- Toasts use a flat `#30363d` surface plus motion/opacity; do not give them dialog-level shadow.

## Shapes

- Standard buttons, inputs, status rows, and attention cards use 7–8px radii with one-pixel borders.
- Terminal panes and pairing-code wells use 10px radii to read as substantial working objects.
- Dialogs use 14px radii; nested controls must remain at the smaller control radius.
- Badges and connection pills use `999px` only when the content is a short count or state.
- Primary and secondary buttons share a 40px minimum height; small acknowledge/remove controls explicitly reduce this to 26–28px.
- Selected/focused state adds border color or a one-pixel outline, never a size-changing border width.
- Commander currently has no icon library; use concise text and existing typographic symbols rather than introducing one-off SVG styles.

## Components

- Shell/navigation: `.shell`, `.machines`, `.sessions`, `.context`, and `.nav-item` in `hub-web/src/styles.css`; markup is rendered by `render()` in `hub-web/src/main.ts`.
- Primary button: `.primary` in `hub-web/src/styles.css`; full-width in sidebars and auto-width only inside `.dialog-actions`.
- Dialog: `dialog`, `dialog::backdrop`, `dialog form`, and `.pair-flow` in `hub-web/src/styles.css`; state-specific markup lives in `pairDialogMarkup()`.
- Pairing code: `.pair-code` in `hub-web/src/styles.css`; always paired with exact Commander origin, scopes, and a countdown.
- Detail list: `.pair-details` in `hub-web/src/styles.css`; 120px term column plus wrapping value column for origins and URLs.
- Input: `dialog input` and `.message textarea` in `hub-web/src/styles.css`; dark inset surface, default border, inherited type.
- Terminal card: `.pane`, `.pane.selected`, `.terminal-mount` in `hub-web/src/styles.css`; created in `renderSessionState()`.
- Connection and compatibility status: `.connection` and `.compatibility-warning`; status colors must keep their current semantic meanings.
- Attention/status cards: `.attention-item`, `.status-row`, `.attention-open`, and `.ack`; operational lists stay compact and left aligned.
- Toast: `#toast` with `.visible`; use generic safe messages and never include credentials, invitations, or relay secrets.

## Do's & Don'ts

- ✅ Escape every origin, URL, label, code, and scope before inserting generated dialog markup with `escapeHtml`/`escapeAttr`.
- ❌ Never interpolate relay secrets, invitations, credentials, or server error bodies into Commander UI.
- ✅ Display the exact kebab-case relay scopes; they are the hub wire contract and should not be cosmetically converted.
- ❌ Never make elevated scopes selectable in page-initiated pairing; its visual contract is the three read-only scopes.
- ✅ Preserve an open pairing dialog across background fleet renders so polling and connection events do not dismiss the task.
- ❌ Never rebuild or dispose the live terminal grid merely to update pairing status; retain the existing preserved-grid path.
- ✅ Keep code, origin, scopes, hub URL, machine label, and countdown visible together when authorization arrives.
- ❌ Never use a toast as the only home for expiry or authorization state; pairing status belongs inside the dialog.
- ✅ Use `#58a6ff` for hover/focus/selection and `#1f6feb` only for the primary decision.
- ❌ Never add light surfaces, pure-white panels, or filled accent selections to the dark operational shell.
- ✅ Verify both the desktop four-column shell and the `max-width: 850px` compact rule after dialog changes.
- ❌ Never edit `hub-web/dist/app.css` by hand; update `hub-web/src/styles.css` and regenerate the checked-in bundle.
