---
source: [hub-web/src/tokens.css, docs/design/design-tokens.json]
inherits: petrastella
theme: dual
colors:
  bg: "--bg-root #F7F4EE / #12141A"
  surface: "--bg-panel #FFFFFF / #191C24"
  surface-raised: "--bg-raised color-mix(in srgb, var(--bg-panel) 96%, var(--text-hi)) / color-mix(in srgb, var(--bg-panel) 96%, var(--text-hi))"
  border: "--line-subtle #DAD3C7 / #2B3040"
  border-strong: "--line-strong #8F8371 / #6B7390"
  text: "--text-hi #1B1D24 / #E9E6E0"
  text-muted: "--text-mid #5A5F6E / #A3A7B4"
  primary: "--color-action #2E3A9F / #A9B3FF"
  accent: "--color-verdict #2E3A9F / #A9B3FF"
  focus: "--color-focus #2E3A9F / #A9B3FF"
  success: "--state-ok #226845 / #5FC492"
  warning: "--state-warn #7F5504 / #E2B14D"
  danger: "--state-crit #B3261E / #EF7B72"
  idle-mark: "--color-series-neutral #6B7280 / #9AA1AF"
  terminal: "--bg-terminal #0C0E13 / #0C0E13"
typography:
  families:
    display: "--font-display \"Iowan Old Style\", \"Palatino Linotype\", Palatino, \"Book Antiqua\", Georgia, \"Times New Roman\", serif"
    body: "--font-ui Inter, ui-sans-serif, system-ui, -apple-system, \"Segoe UI\", Roboto, sans-serif"
    mono: "--font-mono \"JetBrains Mono\", \"IBM Plex Mono\", ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"
  scale:
    eyebrow: "--fs-xs 12px / 16px / 600"
    meta: "--fs-meta 13px / 18px / 400"
    caption: "--fs-base 14px / 20px / 400"
    ledger: "--fs-md 15px / 22px / 400"
    lede: "--fs-lg 21px / 30px / 400"
    verdict: "--fs-verdict clamp(24px, 3vw, 34px) / 1.1 / 400"
    terminal: "--fs-terminal 13px / 1.35 / 400"
  weights: "--weight-regular 400, --weight-medium 500, --weight-semibold 600"
  tracking: "--tracking-label 0.08em"
  control-line-height: "--line-ui 1.43"
spacing:
  base: "4px"
  steps: "--space-1 4px, --space-2 8px, --space-3 12px, --space-4 16px, --space-6 24px, --space-8 32px, --space-12 48px, --space-16 64px"
radius:
  control: "--radius-card 4px"
  panel: "--radius-pane 8px"
  dot: "--radius-pill 999px"
elevation:
  overlay: "--shadow-overlay 0 24px 80px rgba(18,20,26,0.40)"
geometry:
  rail: "--machine-rail-width 48px"
  drawer: "--machine-drawer-width 280px"
  context: "--context-panel-width 320px"
  header: "--session-header-height 44px"
  pane-header: "--pane-header-height 32px"
  button: "--button-height 40px"
  dialog: "--dialog-width 520px"
  fleet-container: "--fleet-board-max-width 1120px"
  phone-rail-target: "--rail-item-min 44px"
breakpoints:
  phone: "(max-width: 53rem), (max-height: 30rem) and (pointer: coarse)"
  landscape-phone: "(max-height: 30rem) and (pointer: coarse)"
  compact: "(max-width: 53rem)"
  narrow: "(max-width: 500px)"
---
## Overview

Cassy Commander is a plain TypeScript console with a Petrastella light/dark shell around dark terminal wells.
`hub-web/src/tokens.css` is generated from `docs/design/design-tokens.json` by `hub-web/scripts/generate-tokens.mjs`; `docs/design/hub-web/token-map.md` records each mapping and retained console measurement.
The scheme follows the OS with light as the fallback; `commander.scheme` stores `system`, `light` or `dark`, and `hub-web/src/scheme.ts` applies `html[data-scheme]`.
This document records the token foundation. The fleet figure, attention timeline and connection verdict follow `docs/design/hub-web/concept-brief.md` in the subsequent screen units; their current component forms below remain until those units land.
Ghostty's ANSI palette stays in `hub-web/src/terminal/ghostty-adapter.ts`; it is independent of the application palette.

## Colors

- `--bg-root` inherits house `bg` (warm paper / warm graphite); `--bg-panel` inherits `surface` for the rail, header, drawer and context panel.
- `--bg-raised` mixes 96% `surface` with `ink`; `--bg-hover` mixes 92%. These are the console's two derived overrides; selection uses house `verdict-soft` through `--bg-active`.
- `--text-hi` inherits `ink`; `--text-mid` inherits `ink-muted`. There is no tertiary text token; timestamps and pane roles use the readable muted value.
- `--color-action` is for controls and links; `--color-verdict` is for the decisive figure mark. Both inherit the house accent; neither is a running-status colour.
- `--color-focus` supplies the sole focus outline. `--state-ok`, `--state-warn` and `--state-crit` inherit `good`, `warning` and `danger`; info text is muted evidence.
- `--state-idle` and `--color-series-neutral` inherit `color.series-neutral`; idle text uses `--text-mid`, while dots use the neutral mark value.
- `--tint-warn` and `--tint-crit` inherit the corresponding house tints for actionable warnings and critical events. `.danger` actions remain text on a normal control surface.
- `--bg-terminal` stays #0C0E13 in both schemes. The generated dark-well scope supplies `color.dark.*` and `color.series-neutral.dark` to transcript, terminal mount, search/dialog inputs, pairing code and log/payload `pre` elements.
- Dark-well descendants inherit matching dark control surfaces, lines and foregrounds; the generated scope repeats the derived surface expressions so they resolve against its own dark roles.
- `--overlay-backdrop` derives from page `bg`; `--overlay-shadow-color` is extracted from `elevation.overlay`. Neither borrows an ANSI colour.

## Typography

- Display (`--font-display`) inherits the Iowan/Palatino/Georgia house serif for the fleet and connection verdict sentences; body (`--font-ui`) inherits Inter/system sans; identifiers (`--font-mono`) inherit JetBrains/IBM Plex/system mono.
- `--fs-xs` is the house 12px eyebrow, `--fs-base` the 14px caption, `--fs-md` the 15px ledger and `--fs-lg` the 21px lede. Pane/session metadata uses the retained 13px `--fs-meta` console step.
- `--fs-verdict` clamps the house title to 24px–34px at 3vw; the brief gives that slot 1.1 line-height and −.015em tracking. It is available for the screen units; existing headings still consume `--fs-lg` or `--fs-md`.
- `--tracking-label` inherits the house .08em eyebrow tracking. Weights are 400/500/600; the 500 hero-number weight is available for pairing code; ordinary copy stays 400 and headings top out at 600.
- `--line-ui` is caption line-height divided by size (20/14 → 1.43). Ghostty keeps `--fs-terminal` 13px, `--line-terminal` 1.35 and its 12–16px runtime clamp.
- Codenames, IDs, paths, timestamps, phases, scope names and JSON use mono even inside prose. Session codenames are never uppercased.

## Layout

- Spacing inherits the house 4px grid: 4/8/12/16/24/32/48/64px. Retired 20px gaps move to 24px; retired 40px control dimensions use `--button-height` so touch geometry stays 40px.
- Desktop `.shell` is 48px · minmax(0,1fr) · 320px; the drawer adds 280px; the collapsed context track is 48px. Pane headers are 32px and the session header is 44px.
- `.fleet-board` inherits the 1120px house container. Its existing card grid temporarily consumes the retained 260px minimum through `--mobile-pane-min-width`; the fleet screen unit removes the grid.
- The phone query is `(max-width: 53rem), (max-height: 30rem) and (pointer: coarse)` in both CSS and `hub-web/src/viewport.ts`; landscape uses the short-axis query so rotation keeps the same pane-mount policy.
- Portrait puts `main` above the 48px safe-area rail. Every rail target uses `--rail-item-min` 44px, and the attention/composer pill retains `--mobile-context-pill-width` 152px.
- Landscape puts the machine rail left and attention rail right, with sheets over the terminal; compact stays width-only for the 80-column PTY floor and transcript default.
- Below 500px the header drops ancillary machine/mode/latency chips. Interior regions scroll within the shell's `100dvh`; terminal mounts keep their own horizontal pan.

## Elevation & Depth

- Root, panel and raised surfaces separate regions by colour and the 8px shell gutter. The only shadows remain the `dialog` and `#toast` declarations in `hub-web/src/styles.css`.
- Both shadows consume house `elevation.overlay` through `--shadow-overlay`; the shadow is identical in both schemes. Phone drawer and attention sheets stay shadowless.
- Pane selection changes the reserved transparent border to `--line-strong`; it does not change geometry or add a glow.

## Shapes

- `--radius-card` now inherits house chip radius 4px for controls; `--radius-pane` inherits panel radius 8px for panes/dialogs. Current rows still consume these until their screen units introduce ruled ledgers.
- `--radius-pill` stays 999px for dots and existing count/chip shapes; it is a console exception, not a new house radius.
- Hairlines use house `chart.hairline` through `--line-width` 1px. Critical rules stay 2px; `--rule-verdict` inherits `chart.mark-decisive` 2.5px and `--rule-hero` is the brief's 3px verdict rule.
- Focus uses `--focus-ring-width` 2px and `--color-focus`. Motion is house chrome 120ms/reveal 200ms with `--motion-easing`; the retired connection spin duration and animation declaration are gone.

## Components

- Shell, rail and drawer: `render()` in `hub-web/src/main.ts`; `.machine-icon` uses raised/active surfaces and `.rail-control` gains a raised hover surface. Open/selected states retain their existing DOM contract.
- Fleet: `FleetBoardRenderer` in `hub-web/src/fleet-board.ts`; current machine sections and session cards consume the new palette. The next unit replaces that form with the brief's dot plot and evidence ledger.
- Header and pickers: `hub-web/src/main.ts`; `.session-picker-entry[aria-current]` uses verdict-soft, and `.session-picker-current` uses action ink. `setScheme()` is ready for the command-palette Appearance entry.
- Panes and transcript: `hub-web/src/main.ts` and `hub-web/src/transcript-view.ts`; terminal wells remain dark while pane chrome follows the shell. Transcript/find controls inside a well inherit dark control roles.
- Connection surface: `hub-web/src/connection-state-view.ts`; existing connecting/failed/retry markup remains. Its log `pre` is a dark well; the screen unit owns the outcome sentence and attempt timeline.
- Attention: `hub-web/src/attention-view.ts`; critical/warning cards use semantic tints, info uses muted evidence, and payload `pre` remains dark. The timeline conversion follows in the attention unit.
- Workers/tasks and composer: `hub-web/src/main.ts`; mono identifiers, muted supporting copy and green in-progress status use the new roles; Send stays explicit.
- Pairing dialog: `pairDialogMarkup()` in `hub-web/src/main.ts` and cancel semantics in `hub-web/src/pairing-dialog.ts`; inputs and code wells get dark foregrounds, while the dialog and detail terms follow the page scheme.
- Buttons and inputs: `hub-web/src/styles.css`; full controls retain 40px height and compact pane controls 28px. Keyboard focus uses the house focus role; disabled copy uses muted text.
- Toast: body-level `#toast` in `hub-web/src/main.ts`; raised surface and the house overlay shadow, above the phone rail.

## Do's & Don'ts

- ✅ Change house values in `docs/design/design-tokens.json`, update the mapping in `hub-web/scripts/generate-tokens.mjs`, and run `npm run tokens`; commit the generated CSS.
- ❌ Never hand-edit `tokens.css`, add a handwritten `:root` palette to `styles.css`, or restore the retired token aliases; `tokens.test.ts` checks drift and every CSS/TypeScript consumer.
- ✅ Add a generated dark-well scope when retaining a dark background beneath a light shell; source foregrounds from `color.dark.*`.
- ❌ Never combine light-scheme ink with `--bg-terminal` or wire Ghostty ANSI entries to application state tokens.
- ✅ Use `applyScheme()` at boot and `setScheme(system|light|dark)` for Appearance; storage denial still permits a page-local choice.
- ❌ Never put scheme state in `shellSignature()` or remount terminals merely to change chrome colours.
- ✅ Keep machine text mono, focus outlines visible and the two overlay shadows as the only shadow consumers.
- ❌ Never use accent as an info status, add a looping connection animation, or restore a low-contrast tertiary text step.
- ✅ Let the integration owner rebuild `hub-web/dist` once; validate lane builds with a separate worktree output directory.
- ❌ Never hand-edit or commit generated `hub-web/dist` output from a factory lane.

## Behavioural constraints
<!-- keep -->

These are engineering decisions the visual system sits on. They survive redesigns.

**Render model.** `render()` chooses one of three paths via `renderDecision()` in `hub-web/src/render-model.ts`: *regions* (default, the only path a heartbeat may take — `renderRegions()` writes into nodes already on screen), *shell* (full rebuild, only when `shellSignature()` changed), or *defer* (signature changed while a form control has focus; flushed by `DeferredRenderScheduler` in `hub-web/src/deferred-render.ts` after focus leaves and the pointer gesture has delivered its click — a macrotask, because the click is dispatched in the same task as pointerup). If a value appears in shell markup it belongs in `shellSignature()` or `applyLiveRegions()`; per-heartbeat data must never enter the signature; `applyLiveRegions()` writes only into existing nodes; anything a region re-creates (rail, drawer tree, session picker, fleet board) binds its own handlers; lease identity is deliberately structural.

**Pairing cancellation.** Cancel discards the invitation: `cancelPendingPairing()` invalidates the in-flight operation, clears the pending store and resets the draft; reopening offers the create-code flow and says pairing needs a fresh URL from `cas hub pair`. A pairing invitation is a one-time capability and Cancel is the operator saying the request must not proceed — including when the link went somewhere it should not have. The dialog closes only once the page can vouch that the cancellation is durable; otherwise it stays on a cleanup step with a retry that never resumes the invitation (`cancellationOutcome()` in `hub-web/src/pairing-cleanup.ts`). A cancellation owns that step through `PairingCancellationTracker` in `hub-web/src/pairing-cancellation.ts`: a rollback that rejects after Cancel still lands on the step, retries run one at a time and report rejection inside the dialog, and any replacement flow supersedes the cancellation so a late result can never close or rewrite it. Browser cancel blocks this browser only; it does not revoke the machine's invitation.

**Narrow-viewport text (D15).** Below `compact`, `GhosttyTerminalSurface.setMinimumColumns` floors the PTY at 80 columns; the canvas sizes to the grid and `.terminal-mount` pans, so "Show terminal" shows the real grid. The transcript (`hub-web/src/transcript.ts` model, `hub-web/src/transcript-view.ts` DOM) reflows the emulator's logical lines from `GhosttySnapshot.rowData` in the browser — no hub-side projection, no new endpoint. Its history is the emulator's scrollback; reaching the top pages the viewport back and "Jump to latest" returns. While the transcript is visible canvas paint is skipped but the snapshot is still taken; a tap on the transcript focuses the pane input.

**Phone layout invariants.** The phone rule keys on the short axis and pointer, not width alone (D5); every drawer/attention `.shell` class combination is listed in the phone block because a media query adds no specificity; only the primary pane mounts a terminal on a phone; the collapsed context pill paints no surface of its own and never spills across Pair (D7); severity is carried by text colour and the dot, never by a fill only some severities receive (D8).
<!-- /keep -->
