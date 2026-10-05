# Brief: Conversations as the only surface (cas-0546)

## Single idea
The operator works in one place, the conversation. The two session controls that used to need the Terminal view sit in its header: Interrupt, and Raw output for reading the supervisor's terminal.

## Hero form
These sit in the conversation header beside the identity:
- **Raw output** is a quiet text pill with a terminal glyph. On a phone it is icon-only.
- **Interrupt** is the one control in the `--state-crit` tone, with a stop glyph and its word. On a phone it shows the word only.

Raw output opens a read-only dark well: a 640px sheet on the right on desktop, and a bottom sheet on a phone.

## Emotional register
Calm and in control. Nothing on screen is stale or shown by default. Interrupt is unmistakable but not alarming, and a takeover is always said out loud: "Took control from Studio iPad."

## Distinctive move
Interrupt is the only critical-toned control in Commander's header. Raw output is the only dark well, and it shows only on request. Neither control is ever silently missing. When one can't act, it stays in place, marked unavailable, with the reason in plain words.

## Deliberately omitted
The following are not included:
- the Terminal view: machine rail, fleet board and Session ledger, the fleet-wide Attention feed, the pane grid
- typing into the terminal
- observer and Take control chrome
- pane reordering
- the side composer
- "Show workers"

Only asks and blockers count as waiting on the operator. Awaiting-merge and lifecycle events never do. The supervisor's surface stays mounted in a hidden, inert host so Raw output has text. It never paints.

## Critique
Scored by steady-stork-35 on 2026-10-05. Sources:
- the built dist in HUB-J18 at 1280 light/dark and 390 light/dark
- the Android Chrome emulator (Pixel, API 34) on the conversation, raw-output and interrupt-unavailable fixtures
- strict visual QA scoped against b86ec0c2e

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Interrupt is the header's single `--state-crit` control and Raw output its single dark well. Each is readable at a glance in both schemes (HUB-J18 J02/J04). |
| Fit | 4 | The two controls the operator still needed now live where the operator works. The removed view's stale alerts and duplicate composer are gone. A takeover names the other device. |
| Hierarchy | 4 | The actions sit right-aligned above the identity row and never compete with the project title. On a phone the header stays one row, and there is exactly one bottom bar, the composer (Android capture). |
| Craft | 4 | Phone targets are 44px. The drawer fades in place instead of sliding past the viewport edge. Strict visual QA adds no finding over the base after the label and animation fixes. |
| Accessibility | 4 | Both controls keep full accessible names. An unavailable action stays focusable, with `aria-disabled` and a reason that is read once from a hidden description node. Escape returns focus from Raw output. The hidden host is `inert`/`aria-hidden`, so Tab never stops in it. |
