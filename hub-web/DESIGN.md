---
version: alpha
name: Cassy Commander
description: Glass, the one look of Cassy Commander, the hub web client; frosted panels over a still violet-to-teal aurora, light and dark.
source: [hub-web/src/tokens.css, hub-web/src/glass.css, hub-web/public/cassy-tokens.css]
inherits: petrastella
colors:
  primary: "#5B3FE0"
  primary-dark: "#A897FF"
  on-primary: "#FFFFFF"
  on-primary-dark: "#120E2A"
  bg: "#EFECFB"
  bg-dark: "#0E0C20"
  surface: "#FFFFFF"
  surface-dark: "#1A1830"
  surface-raised: "#F7F5FF"
  surface-raised-dark: "#23203E"
  glass: "rgba(255, 255, 255, 0.6)"
  glass-dark: "rgba(22, 20, 44, 0.54)"
  glass-strong: "rgba(255, 255, 255, 0.78)"
  glass-strong-dark: "rgba(26, 23, 52, 0.74)"
  line: "rgba(76, 61, 160, 0.16)"
  line-dark: "rgba(190, 180, 255, 0.14)"
  line-strong: "rgba(60, 48, 130, 0.42)"
  line-strong-dark: "rgba(200, 190, 255, 0.4)"
  ink: "#1A1830"
  ink-dark: "#EEEBFF"
  ink-muted: "#4A4762"
  ink-muted-dark: "#B8B3D9"
  evidence: "#4A4762"
  evidence-dark: "#B8B3D9"
  verdict: "#5B3FE0"
  verdict-dark: "#A897FF"
  verdict-soft: "rgba(91, 63, 224, 0.14)"
  verdict-soft-dark: "rgba(180, 165, 255, 0.18)"
  action: "#5B3FE0"
  action-dark: "#A897FF"
  focus: "#5B3FE0"
  focus-dark: "#B9ABFF"
  good: "#226845"
  good-dark: "#5FC492"
  warning: "#7F5504"
  warning-dark: "#E2B14D"
  danger: "#B3261E"
  danger-dark: "#EF7B72"
  idle: "#6B7280"
  idle-dark: "#9AA1AF"
  supervisor: "rgba(255, 255, 255, 0.9)"
  supervisor-dark: "rgba(36, 33, 70, 0.92)"
  on-supervisor: "#1A1830"
  on-supervisor-dark: "#EEEBFF"
  you: "#5B3FE0"
  you-dark: "#6A4FF0"
  on-you: "#FFFFFF"
  on-you-dark: "#FFFFFF"
  ask: "#FFC46E"
  ask-dark: "#FFC46E"
  on-ask: "#2A1A05"
  on-ask-dark: "#2A1A05"
  ask-deep: "#A1401F"
  ask-deep-dark: "#7A3410"
  crit: "#B3261E"
  crit-dark: "#EF7B72"
  on-crit: "#FFFFFF"
  on-crit-dark: "#12141A"
  crit-tint: "#B3261E"
  crit-tint-dark: "#3A1F1E"
  terminal: "#0C0E13"
  terminal-dark: "#0C0E13"
typography:
  brand:
    fontFamily: '"Iowan Old Style", "Palatino Linotype", Palatino, "Book Antiqua", Georgia, "Times New Roman", serif'
    fontSize: 21px
    fontWeight: 500
  title-md:
    fontFamily: 'Inter, ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif'
    fontSize: 15px
    fontWeight: 600
  body-md:
    fontFamily: 'Inter, ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif'
    fontSize: 14px
    fontWeight: 400
    lineHeight: 1.43
  label-md:
    fontFamily: 'Inter, ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif'
    fontSize: 13px
    fontWeight: 400
  eyebrow:
    fontFamily: 'Inter, ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif'
    fontSize: 12px
    fontWeight: 600
    letterSpacing: 0.08em
  mono-xs:
    fontFamily: '"JetBrains Mono", "IBM Plex Mono", ui-monospace, SFMono-Regular, Menlo, Consolas, monospace'
    fontSize: 12px
    fontWeight: 400
  terminal:
    fontFamily: '"JetBrains Mono", "IBM Plex Mono", ui-monospace, SFMono-Regular, Menlo, Consolas, monospace'
    fontSize: 13px
    fontWeight: 400
    lineHeight: 1.35
rounded:
  card: 4px
  pane: 8px
  glass: 22px
  full: 999px
spacing:
  space-1: 4px
  space-2: 8px
  space-3: 12px
  space-4: 16px
  space-6: 24px
  space-8: 32px
  space-12: 48px
  space-16: 64px
components:
  canvas: { backgroundColor: "{colors.bg}", textColor: "{colors.ink}" }
  glass-panel: { backgroundColor: "{colors.glass}", rounded: "{rounded.glass}", padding: 8px }
  dialog: { backgroundColor: "{colors.glass-strong}", textColor: "{colors.ink}", rounded: 24px }
  card: { backgroundColor: "{colors.surface-raised}", textColor: "{colors.ink}", rounded: "{rounded.pane}" }
  button-primary: { backgroundColor: "{colors.primary}", textColor: "{colors.on-primary}", rounded: "{rounded.full}", height: 40px }
  project-badge: { backgroundColor: "{colors.verdict-soft}", typography: "{typography.title-md}", rounded: "{rounded.card}" }
  bubble-supervisor: { backgroundColor: "{colors.supervisor}", textColor: "{colors.on-supervisor}" }
  bubble-operator: { backgroundColor: "{colors.you}", textColor: "{colors.on-you}" }
  ask-card: { backgroundColor: "{colors.ask}", textColor: "{colors.on-ask}" }
  ask-tray: { backgroundColor: "{colors.ask-deep}" }
  blocker-card: { backgroundColor: "{colors.crit}", textColor: "{colors.on-crit}" }
  blocker-tint: { backgroundColor: "{colors.crit-tint}", textColor: "{colors.on-crit}" }
  meta-text: { textColor: "{colors.ink-muted}", typography: "{typography.label-md}" }
  evidence-note: { textColor: "{colors.evidence}", typography: "{typography.mono-xs}" }
  link: { textColor: "{colors.action}" }
  verdict-rule: { backgroundColor: "{colors.verdict}", height: 3px }
  focus-ring: { backgroundColor: "{colors.focus}", width: 2px }
  hairline: { backgroundColor: "{colors.line}", height: 1px }
  rule-strong: { backgroundColor: "{colors.line-strong}", height: 1px }
  status-ok: { backgroundColor: "{colors.good}", rounded: "{rounded.full}" }
  status-warn: { backgroundColor: "{colors.warning}", rounded: "{rounded.full}" }
  status-crit: { backgroundColor: "{colors.danger}", rounded: "{rounded.full}" }
  status-idle: { backgroundColor: "{colors.idle}", rounded: "{rounded.full}" }
  raw-output: { backgroundColor: "{colors.terminal}", typography: "{typography.terminal}" }
maps:
  primary: "--color-action"
  on-primary: "--accent-fg"
  bg: "--bg-root"
  surface: "--bg-panel"
  surface-raised: "--bg-raised"
  glass: "--look-glass"
  glass-strong: "--look-glass-strong"
  line: "--line-subtle"
  line-strong: "--line-strong"
  ink: "--ink"
  ink-muted: "--ink-mid"
  evidence: "--ink-mid"
  verdict: "--color-verdict"
  verdict-soft: "--bg-active"
  action: "--color-action"
  focus: "--color-focus"
  good: "--state-ok"
  warning: "--state-warn"
  danger: "--state-crit"
  idle: "--state-idle"
  supervisor: "--sup-bg"
  on-supervisor: "--sup-fg"
  you: "--you-bubble-bg"
  on-you: "--you-bubble-fg"
  ask: "--ask-bg"
  on-ask: "--ask-fg"
  ask-deep: "--ask-deep"
  crit: "--crit-bg"
  on-crit: "--crit-fg"
  crit-tint: "--crit-tint"
  terminal: "--bg-terminal"
  typography.brand: "fontFamily --font-display, fontSize --fs-lg, fontWeight --weight-medium"
  typography.title-md: "fontFamily --font-ui, fontSize --fs-md, fontWeight --weight-semibold"
  typography.body-md: "fontFamily --font-ui, fontSize --fs-base, fontWeight --weight-regular, lineHeight --line-ui"
  typography.label-md: "fontFamily --font-ui, fontSize --fs-meta, fontWeight --weight-regular"
  typography.eyebrow: "fontFamily --font-ui, fontSize --fs-xs, fontWeight --weight-semibold, letterSpacing --tracking-label"
  typography.mono-xs: "fontFamily --font-mono, fontSize --fs-xs, fontWeight --weight-regular"
  typography.terminal: "fontFamily --font-mono, fontSize --fs-terminal, fontWeight --weight-regular, lineHeight --line-terminal"
  rounded.card: "--radius-card"
  rounded.pane: "--radius-pane"
  rounded.glass: "--glass-radius"
  rounded.full: "--radius-pill"
---
## Overview

Commander (hub-web) is a plain TypeScript client with one surface, Conversations: a list, a thread and a context rail, in the **Glass** look. Glass is the only look; there is no look switch.
A still violet-to-teal aurora fills the window and the chrome floats over it as frosted glass: the conversation list, the conversation heading, the context rail and the composer. Reading surfaces (bubbles, cards, dialogs) stay nearly opaque. Send glows violet, and an open question is the warmest thing on screen, an amber-to-coral card.
Light and dark both ship. The scheme follows the OS with light as the fallback; Appearance stores `system`, `light` or `dark` in `commander.scheme`, and `hub-web/src/scheme.ts` sets `html[data-scheme]`.
Token sources, in cascade order: `hub-web/src/tokens.css` (generated from `docs/design/design-tokens.json` by `hub-web/scripts/generate-tokens.mjs`; the house roles), then `hub-web/src/glass.css` (imported after `styles.css`), whose token blocks re-colour most of them. The frontmatter records the value that wins in each scheme; `maps` names the variable that carries each role (a `spacing` step `space-N` is `--space-N`), and `hub-web/src/tokens.test.ts` fails when either drifts.
Petrastella roles kept: the type pairing (display serif, Inter, JetBrains Mono), the 4px grid, `good`/`warning`/`danger` and the dark terminal well. Overridden: the warm-stone paper and Stella indigo become the Glass field and violet (`bg`, `verdict`, `action`). Not used: `surface-hero` (the aurora takes the hero's place; there is no sandstone surface) and `--fs-verdict` (a `clamp()`, which the format's dimension type cannot hold; only the connection title uses it).
Lint (`npx @google/design.md lint hub-web/DESIGN.md`) has no errors. Its remaining warnings are `orphaned-tokens`, one per `-dark` colour: the format has no scheme axis, so a component references the light value and the same component paints the `-dark` value in dark.

## Colors

- **Field and glass.** `--bg-root` is the field under the aurora (#EFECFB light, #0E0C20 dark). Chrome panels are `--look-glass` (white 0.6 / indigo 0.54) with `--look-blur`; search fields and dialogs are `--look-glass-strong`. `--bg-panel` is the opaque fallback for every glass surface under more contrast, reduced transparency or forced colours.
- **Cards.** `--bg-raised` is an opaque lavender-white (#F7F5FF) or deep indigo (#23203E): worker cards, the paired-machines row and dialog footers never read as grey slabs on glass. `--bg-hover` and `--bg-active` are violet washes, never grey.
- **Ink.** `--ink`/`--text-hi` (#1A1830 / #EEEBFF) and `--ink-mid`/`--text-mid` (#4A4762 / #B8B3D9). There is no tertiary text step; timestamps and meta use `--ink-mid`.
- **Violet is the one action colour.** `--color-action`, `--color-verdict` and `--color-focus` are the same violet (#5B3FE0 light; #A897FF and a lighter #B9ABFF focus in dark). Send, the compose button and every `.primary` carry the `--look-send` gradient (#7656FA → #5B3FE0 → #3E5BEA) with white text. Violet never marks status.
- **Status.** `--state-ok`, `--state-warn`, `--state-crit` and `--state-idle` are the house values; Glass leaves them alone. A message known **not sent** is critical (`--crit-bg` outline, glyph and label); one only **not confirmed** is caution (`--warn-text`), in the thread and on the dismissed-messages chip; a settled record drops to `--ink-mid`.
- **Ask and blocker.** An open question is `--look-ask` (light #FFD983 → #FFA184) in `--ask-fg` #2A1A05 over an ember `--look-ask-tray`, in both schemes; its answer chips are opaque and stay opaque on hover (`--tray-chip-hover`). A blocker is a `--crit-bg` flood in light and, in dark, a `--crit-tint` #3A1F1E card in ink with a 4px salmon `--crit-edge`.
- **Bubbles.** Supervisor bubbles are `--sup-bg` (0.9 alpha, no blur). The operator's bubble is the `--look-you` gradient; under more contrast it is the flat `--you-bubble-bg`.
- **Machine accents.** `.machine-accent-0…4` (indigo, green, violet, teal, rose; `hub-web/src/machine-accent.ts`, append only) colour only each machine's avatar. Glass resets their `--sup-bg`, `--accent-soft` and `--lift-sup`, so they never tint bubbles or the selected row.
- **Dark wells.** `--bg-terminal` #0C0E13 in both schemes. The generated dark-well scope (Raw output transcript, search and log `pre`, attention payloads) supplies dark ink so light ink never lands on the terminal colour.

### Shared tokens

`hub-web/scripts/build-cassy-tokens.mjs` (run by `npm run tokens` and the build) writes `hub-web/public/cassy-tokens.css`: the custom-property blocks of `tokens.css` and `glass.css`, with no component rules, machine accents or dark wells, under a `cassy-tokens-sha256:` header. The hub serves it at `/commander/cassy-tokens.css`. Explorer vendors it with `node hub-web/scripts/sync-cassy-tokens.mjs --to app/cassy-tokens.css` (`--check` in CI: exit 1 when edited or behind, 2 when the source hash is broken). It is also the palette every report and HTML document in this repo pastes (root `DESIGN.md`). `cassy-tokens.test.ts` pins that it is current, token-only and hash-intact. Change a token in `design-tokens.json` or `glass.css`, then run `npm run tokens`; never edit the artifact.

## Typography

- Three families. Display `--font-display` (Iowan Old Style, Palatino, Georgia; not bundled) sets the "Cassy Cloud" wordmark (500, 21px) and the connection verdict (`--fs-verdict`, 400, −.015em). Body `--font-ui` is Inter. Identifiers `--font-mono` is JetBrains Mono.
- Inter and JetBrains Mono ship as latin variable subsets (`hub-web/src/fonts.css`, OFL 1.1, `font-display: swap`), served as `/commander/inter.woff2` and `/commander/jetbrains-mono.woff2`.
- Scale: `--fs-xs` 12px (eyebrows, mono codenames, day labels), `--fs-meta` 13px (meta, the most used step), `--fs-base` 14px body with `--line-ui` 1.43, `--fs-md` 15px (project names, the project badge, app names), `--fs-lg` 21px (wordmark). The list title "Conversations" is a hand-set 22px/1.2 semibold with −.015em.
- Weights 400/500/600 only; headings top out at 600. Eyebrows are 600 uppercase with `--tracking-label` .08em.
- Codenames, IDs, paths, timestamps and JSON are mono even inside prose; session codenames are never uppercased. Raw output keeps `--fs-terminal` 13px with `--line-terminal` 1.35.

## Layout

- 4px grid: 4/8/12/16/24/32/48/64px (`--space-*`). Controls are `--button-height` 40px, compact pane controls 28px.
- Desktop: a floating 382px glass list (`--conversation-rail-width`; 374px of content inside an 8px `--glass-inset`), a reading column open to the aurora, and the glass context rail, which collapses to its 48px track.
- The phone query is `(max-width: 53rem), (max-height: 30rem) and (pointer: coarse)` in CSS and in `hub-web/src/viewport.ts`; landscape uses the same short-axis query, so rotation keeps the phone layout.
- Phone: list → full-width thread with an always-addressed composer. The list is full-bleed glass, the heading hangs from the top edge with rounded lower corners, every touch target is at least `--rail-item-min` 44px, and Tasks & progress, Waiting on you and Raw output open as sheets.
- Installed (standalone) and sideways phones: every top-anchored surface adds `env(safe-area-inset-*)` to its own padding; `e2e/installed-safe-area.spec.ts` and HUB-J9 in `phone.journey.ts` fail when a control sits under a status bar or notch. Interior regions scroll inside the shell's `100dvh`.

## Elevation & Depth

- **Aurora (level 0).** One still image behind the shell on a fixed layer: `--look-aurora`, a 320×200 JPEG that `hub-web/scripts/render-glass-aurora.mjs` paints from the `--look-aurora-source` gradients. It never moves; an animated backdrop re-blurs every frosted panel each frame (measured 30 fps instead of 59).
- **Glass (level 1).** List, heading, context rail and composer: `--look-glass`, `--look-blur` (16px, saturate 160%; 26px measurably slowed timing journeys), a 1px `--look-glass-edge` drawn as an inset shadow (never a border, so geometry matches the base) and `--look-shadow-float`.
- **Reading (level 2).** Bubbles and cards are nearly opaque and never blurred individually (a blur per bubble cost a quarter of a long thread's frame rate). Cards carry `--lift`; attention objects carry `--lift-strong`.
- **Overlay (level 3).** Dialogs are `--look-glass-strong` with `--look-shadow-float` over a blurred `--look-scrim`; the toast is a solid `--bg-panel` with `--shadow-overlay`. Full-screen phone sheets are opaque and square.
- Glass sets `--lift-edge` and `--lift-head` to `none`: the rail edge and thread header are separated by the glass itself, not a shadow.
- Text sits only on a known colour: opaque cards, ≥ 0.6-alpha panels, or (timestamps and meta only) the aurora's pale middle in light and its near-black field in dark. `glass.test.ts` measures each pair at ≥ 4.5:1 composited over the aurora's stops.
- Accessibility: all Glass decoration sits inside `@media (forced-colors: none)`. More contrast, reduced transparency or forced colours make every panel opaque `--bg-panel`, unblurred, with `--line-strong` edges; more contrast also darkens `--ink-mid` and drops the aurora. Glass adds no motion.

## Shapes

- Radii: `--radius-card` 4px (controls, project badge), `--radius-pane` 8px (panes), `--glass-radius` 22px (list and context rail), 20px (conversation heading), 24px (dialogs), 26px (composer bar), 23px (reply fields: the conversation composer's and the inbox reply composer's), 16px (conversation rows, search, app-switcher card), `--radius-pill` 999px (dots, chips, outlined dialog buttons).
- Bubbles are asymmetric: supervisor 8px 22px 22px 8px, operator 22px 8px 8px 22px.
- Lines: `--line-width` 1px hairlines, `--state-rule-width` 2px critical rules, `--rule-verdict` 2.5px and `--rule-hero` 3px (launch progress). The open conversation row has a 2px `--color-action` edge all round, never a left bar.
- Focus: `--focus-ring-width` 2px in `--color-focus`.

## Components

- **Shell.** `render()` in `hub-web/src/main.ts` builds the shell from `hub-web/src/conversation-shell.ts`; rows from `conversation-list.ts`, the thread from `conversation-view.ts` and `conversation-history.ts`.
- **Conversation row** (`conversation-list.ts`). Open: `--look-row-open` wash plus the 2px `--color-action` edge (5.7:1 light, 7.5:1 dark against the list). Hover is a faint `--bg-hover` wash with no edge, so hover can never look open.
- **Primary button** (`styles.css`, `glass.css`). `--look-send` with `--look-send-glow`; hover deepens to `--look-send-hover` and never brightens.
- **Dialog** (`glass.css`). Glass-strong sheet; its sticky action bar is a light `--look-glass-bar` frost with a hairline and a 12px gap; plain buttons (Close, Back) are outlined pills; a destructive Remove is a `--crit-bg` outlined pill.
- **Ask card and pinned ask** (`attention-view.ts`, `context-rail.ts`). The full stepped question lives once, in the thread; the "Waiting on you" bookmark points to it. Only asks and blockers count as waiting on the operator.
- **Conversation header.** Interrupt (`#conversation-interrupt`, `--state-crit` outline, `aria-disabled` with its reason when it cannot act) and Raw output (`dialog#raw-output`, a read-only `transcript-view.ts` on `--bg-terminal`: a 640px right sheet on desktop, a bottom sheet on a phone).
- **Project badge.** `--bg-active` fill, `--color-action` text, 15px semibold, wraps; named from the catalog `project_dir` with an honest missing state.
- **New session** (`#launch-dialog`, `launch-session.ts`). Chosen rows carry `--bg-active` and a `--color-action` edge (`.is-checked`); Attach and Start are its only `.primary`; progress is a `--rule-hero` rule with a quiet elapsed count, never a spinner.
- **Toast** (`#toast`, `main.ts`). Solid `--bg-panel`; slides but never fades through the page.
- **Inbox reply composer** (`form.operator-inbox-composer`, `inbox/inbox-view.ts`; `styles.css`). The offline inbox's reply field reuses the conversation composer's rounded field so a reply looks the same in both places: a column with a `--space-2` gap; the textarea is full width with `--line-width` `--line-strong` edge, 23px radius, `--panel` fill (white 0.84 light, indigo rgba(32, 29, 60, 0.82) dark, opaque `--bg-panel` under more contrast, reduced transparency or forced colours), `--ink` text, inherited font at line-height 1.55 and `resize: none`. Text is inset `12px 22px` so no line touches the rounded edge, including a long reply that scrolls. Unlike the conversation field it carries no `--lift` and keeps `--panel` rather than Glass's `--look-glass-strong`, because it already sits on the glass-strong dialog. HUB-J19 (`e2e/journeys/operator-inbox.journey.ts`) compares the field's fill, edge, radius, ink, inset, line height and resize against these tokens.

Work-step disclosures (`work-rows-view.ts`) use operation-first body text at `--fs-meta`, with named read/edit target lists, search-match scope, or command/working-directory/exit metadata. Exact tool input and output stays plain mono text inside the existing `--bg-raised` detail well; unknown wrapper tools keep their original label and raw record. Failures have words plus the critical dot. Routine turn-start/completion notices share a collapsed Turn activity disclosure; failed notices remain visible. Detail loads only on opening, and live metadata updates preserve focus and expansion in both the thread and worker timeline. See `src/work-rows.brief.md`.

### Brand

`hub-web/public/favicon.svg` is the one brand asset: three interlocking ribbons (white, aurora blue #E6ECFF → #A9C2FF, aurora teal #B8F5EA → #6FE0D0) on a 512px tile (radius 114) filled with the `--look-send` gradient, the same in both schemes. `hub-web/src/cloud-brand.ts` renders it as a 32px `<img>` beside the "Cassy Cloud" wordmark (`--font-display`, 500, 21px, `--space-2` gap); the context rail repeats a 14px wordmark with a 24px mark. `hub-web/scripts/render-brand-icons.mjs` paints the favicons, the 180px apple-touch icon and the 192/512 install and maskable icons from it. The manifest names the app **Cassy** (`theme_color` #5B3FE0, `background_color` #0E0C20). The list screen is the only place the phone shows the lockup; the phone conversation header stays one row (about 56px).

- ❌ Never recolour the mark with `--color-action` or `currentColor`, draw it as flat ink, put the tile on another violet fill, or render it below 16px. Maskable icons keep every ribbon inside the centred 80% circle.

### App switcher

The lockup at the top of the list is `button#app-switcher-toggle` ("Cassy Cloud apps"); below 380px it shows the mark and chevron only. It opens `nav#app-switcher`, a native popover: an opaque `--bg-panel` card, `--line-strong` edge, `--lift-strong`, 16px radius, 18rem wide, listing **Commander** then **Explorer** (15px semibold names, 13px `--ink-mid` hints, 44px rows). The current app is a `span[aria-current="page"]` with `--bg-active` and an inner 2px `--color-action` outline, never the Send gradient. The other app is a link to a named tab (`cassy-explorer` / `cassy-commander`) with `rel="noopener noreferrer"`, opening `<cloud origin>/explorer/tasks?project_id=<id>`. The URL carries only the origin, the path and `project_id`, never a key, code, session name or token (`app-switcher.test.ts`).

## Do's & Don'ts

- ✅ Change house values in `docs/design/design-tokens.json` and Glass values in `glass.css`'s token blocks, run `npm run tokens`, and update this frontmatter in the same commit; `tokens.test.ts` fails on any drift.
- ❌ Never hand-edit `tokens.css` or `public/cassy-tokens.css`, add a `:root` palette to `styles.css`, or restore retired aliases (`--text-lo`, `--fs-sm`, `--space-5`, …).
- ✅ Keep Glass's light block and its two dark blocks (the media block and `html[data-scheme="dark"]`) identical in shape; `glass.test.ts` pins pairs and the accessibility media rules.
- ❌ Never put a gradient, blur or animation outside `@media (forced-colors: none)`, animate anything behind glass, or reintroduce a look switch (`data-look`, `?look=`).
- ✅ Use violet only for actions, the selection edge and focus.
- ❌ Never use violet, a machine accent or `--bg-hover` to mean status, and never brighten a control on a gradient on hover (white fell to 4.27:1).
- ✅ Keep text on opaque or ≥ 0.6-alpha surfaces; give a new dark background a generated dark-well scope with `color.dark.*` foregrounds.
- ❌ Never put light-scheme ink on `--bg-terminal` or wire ANSI colours to application tokens.
- ✅ Make every `box-shadow` a token (`--lift*`, `--look-shadow-float`, `--shadow-overlay`); `invariants.test.ts` checks it.
- ❌ Never render conversation content inside `.pane-host`, give it a visible size, or add a second presentation beside Conversations.
- ❌ Never hand-edit or commit generated `hub-web/dist` from a factory lane; the integration owner rebuilds it once.

## Behavioural constraints
<!-- keep -->

These are engineering decisions the visual system sits on. They survive redesigns.

**Render model.** `render()` chooses one of three paths via `renderDecision()` in `hub-web/src/render-model.ts`: *regions* (default, the only path a heartbeat may take — `renderRegions()` writes into nodes already on screen), *shell* (full rebuild, only when `shellSignature()` changed), or *defer* (signature changed while a form control has focus; flushed by `DeferredRenderScheduler` in `hub-web/src/deferred-render.ts` after focus leaves and the pointer gesture has delivered its click — a macrotask, because the click is dispatched in the same task as pointerup). If a value appears in shell markup it belongs in `shellSignature()` or `applyLiveRegions()`; per-heartbeat data must never enter the signature; `applyLiveRegions()` writes only into existing nodes; anything a region re-creates (conversation list rows, rail sections, header actions) binds its own handlers; lease identity is deliberately structural.

**Browser installations.** Each Paired machines row opens an installation inventory for that named hub. The current browser leads with “This browser” and its full device ID; a ruled ledger shows generation, first paired, last use, origin, signing-key fingerprint and account enrollment. Origins and fingerprints wrap on phone; the body scrolls and Close remains reachable. Revoke confirms the exact device ID and ends its hub access; Remove from this browser is a separate local action. Other installations require hub-admin permission. Account enrollment currently displays Un-enrolled; display names never imply verified account identity.

Re-pairing proves the retained origin-local signing key and updates the same installation. Prepare, catalog stage, server commit and catalog activation are separate boundaries. Cancel confirms remote abort before restoring prior browser access. Uncertain cleanup stays visible with Retry cleanup and quarantines the affected hub until recovery is confirmed. Key replacement additionally proves the prior key. Tabs share a hub-scoped Web Lock and reload accepted generations from the same-origin catalog; broadcasts carry only invalidation, never secrets. Lost storage enrolls a distinct device and requires explicit revoke consent to replace an old one. The browser never merges devices by their display labels.

**Pairing cancellation.** Cancel discards the invitation: `cancelPendingPairing()` invalidates the in-flight operation, clears the pending store and resets the draft; reopening offers the create-code flow and says pairing needs a fresh URL from `cas hub pair`. A pairing invitation is a one-time capability and Cancel is the operator saying the request must not proceed — including when the link went somewhere it should not have. The dialog closes only once the page can vouch that the cancellation is durable; otherwise it stays on a cleanup step with a retry that never resumes the invitation (`cancellationOutcome()` in `hub-web/src/pairing-cleanup.ts`). A cancellation owns that step through `PairingCancellationTracker` in `hub-web/src/pairing-cancellation.ts`: a rollback that rejects after Cancel still lands on the step, retries run one at a time and report rejection inside the dialog, and any replacement flow supersedes the cancellation so a late result can never close or rewrite it. Browser cancel blocks this browser only; it does not revoke the machine's invitation.

**Raw output text (D15).** Commander never sizes or renders the PTY (cas-f285). Raw output pages the supervisor pane's rows from the daemon, which renders them at the pane's real grid (`ScrollbackRequest` → `ScrollbackPage`, at most 200 rows a page, the newest 600 kept), and re-reads the tail while new output arrives. The transcript (`hub-web/src/transcript.ts` model, `hub-web/src/transcript-view.ts` DOM) turns those rows and their style runs into lines in the browser. It is read-only and never focuses a pane input. An old daemon without paged rows is read from its recent output bytes, control sequences removed.

**Phone layout invariants.** The phone rule keys on the short axis and pointer, not width alone (D5); only the supervisor pane is attached, and its host never paints; the header keeps one row with Interrupt and Raw output reachable, and there is exactly one bottom bar (the composer) — never stacked bars; severity is carried by text colour and the dot, never by a fill only some severities receive (D8).
<!-- /keep -->

- Delivery and evidence copy is part of the surface; tests pin the behaviour. Reply receipts: **Forwarded · not stored on this device** until the reply is committed to this browser's IndexedDB journal, then **Stored on this device**; neither claims the operator read it.
- Kept commands use per-item IndexedDB transactions scoped to hub URL, installation and session; a sending or unconfirmed claim never expires into a second send; held messages keep the two-minute wait and explicit cancel. Local reply payloads expire after 90 days; remove or revocation deletes them.
- The Connection log shows the measured machine cause while that machine is Unsteady; NetworkInformation estimates never count as a changed route. A hub 503 `auth_store_unavailable` is its own cause, **Hub is temporarily unable to check pairings**, saying Commander retries by itself and this device's pairing is intact; it never reads as a refused or expired credential. An Attention notice with unchanged identity and payload keeps its open Details and Copy across outage-copy changes and shell rebuilds.
- While a machine is reconnecting or Unsteady, its catalog stays listed past its freshness instead of emptying the list: each row's status label gains **· last known**, and the footer qualifies only that share (**N conversations · K last known**, or **N conversations, last known** when every listed conversation is). A live machine's catalog still expires, and an empty catalog still empties the list.
