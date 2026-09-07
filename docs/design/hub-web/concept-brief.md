# Brief: Cassy Commander (`hub-web`)

Concept brief for the Hub Commander design pass. Every later unit implements this brief; the
token mapping is in [token-map.md](token-map.md), the visual-QA allowlist reasoning in
[visual-qa-plan.md](visual-qa-plan.md), and the 3.17.3 baseline renders under
[captures/before/](captures/before/). Written against `hub-web/DESIGN.md` (3.17.3, dark-only),
`docs/design/petrastella-design-language.md` and `docs/design/design-tokens.json`.

## Single idea

An operator opening the Commander knows in three seconds whether any session needs a hand,
which one, and that the rest are working — because the fleet is drawn, not listed.

## Hero form

**Dot plot on a work-state track** (form vocabulary: *dot plot* — compare positions with
precision; sorted categories on the y axis, one dot each, a hairline from axis to dot, a
`verdict-soft` band for the reference range).

The fleet board is the hero and its first screen is a verdict hero built from live state:

1. Eyebrow in mono: `FLEET · 1 machine · 7 sessions · 12:41`.
2. The verdict sentence in the display serif, composed by the renderer from the same model the
   board already has (`fleetBoardSignature()` fields plus attention severity per session), at
   most 22 words: *"One of seven sessions needs you; five are working, one has gone quiet."*
   When nothing needs the operator it says so: *"All seven sessions are working."*
3. The figure: one row per session (mono codename, sorted needs-you first, then quiet, then by
   last activity), a hairline from the name to a single dot placed on the track
   **Needs you · Working · Idle · Stale · Unreachable**. *Working* is the `verdict-soft` band; a
   session whose card summary carries a phase (`planning`, `editing`, `building`, `testing`,
   `reviewing`) prints that word beside its dot inside the band; `blocked` and any session with
   a critical attention item sit on *Needs you* and carry the 2.5px `verdict` ring; liveness
   `stale_metadata` → *Stale*, `missing_endpoint` → *Unreachable*, a live session with no
   phase and no recent activity → *Idle*. Status colour appears only on the dots that are on
   *Needs you* (danger) or *Unreachable* (warning); every other mark is `ink-muted`.
4. A 3px `verdict` rule, then one line of provenance in mono `ink-muted`: the machine, its
   connection phase, the hub version and the time of the last catalog refresh.

Why this geometry is the claim: a healthy fleet is a column of dots inside one band; a fleet
that needs the operator has a dot outside it, at the top, ringed. The reader sees the shape
before reading a single codename. Three cards in a row (the 3.17.3 board,
`captures/before/fleet-board.1280.dark.png`) show three equal boxes and no argument.

At 1280×800 the sentence and the figure share the first screen with the sentence on the left
and the figure beside it; at 390×844 the sentence stacks above the figure and both are visible
without scrolling for up to eight sessions (the row height is 28px; beyond eight the figure
scrolls inside its own container and the verdict still names the count).

## Emotional register

**Calm, certain, ruled.** Warm paper in light and warm graphite in dark (`bg`/`surface`), one
indigo mark per screen, hairlines instead of boxes, the serif reserved for sentences that
state an outcome, no shadow anywhere but the dialog and the toast, no spinner, no glow, no
counter that counts. The terminal wells stay the deepest, darkest thing on the page in both
schemes so the machine's own output is never dressed.

## Distinctive move

The fleet is a figure: every session is one dot on the work-state track under a serif sentence
that says what the dots show, and the session that needs you is the one dot outside the band.
The same move is echoed once below the fold — the attention rail is an annotated timeline whose
one `verdict`-marked event is the item the sentence pointed at.

## Deliberately omitted

- **The session card grid.** Cards are the anti-default for the hero; below the hero the
  sessions become an evidence ledger (one ruled row per session, machine as the group caption,
  supervisor as the source column), which is denser at 390px than three stacked cards.
- **The KPI pair in the rail.** The `4` / `75` count badges at the top right of every screen
  are two numbers in boxes; the verdict sentence carries the count, and the rail keeps a single
  labelled *Needs you* figure.
- **The spinner and rising counter on connection failure.** Replaced by a stated outcome and an
  annotated timeline of the attempts (see Connection state below); the house motion rule forbids
  looping animation and `--connection-spin-duration` is retired.
- **A scheme toggle button in the chrome.** Scheme follows the operating system by default; the
  override lives in the command palette (`Ctrl/⌘ K` → *Appearance*), so no control competes with
  the verdict.
- **Any change to pairing copy or step order** (see *Not changing*).

## Forms per screen

| Screen | Reader's task | Form (from `form-vocabulary.md`) | What changes |
| --- | --- | --- | --- |
| Fleet board (hero) | know if the fleet needs me and where | **Verdict hero** with a **dot plot** on the work-state track; **evidence ledger** of sessions beneath it | `FleetBoardRenderer` draws the figure and the ledger; cards and the `.fleet-sessions` grid go |
| Shell: rail, drawer, header | move between machines and sessions | chrome — no vocabulary form; tokens, hairlines and the mono/sans split only | rail count badges become one labelled figure; drawer rows become ledger rows; chips take `radius.chip` |
| Session canvas with panes | watch and steer the work | the terminal wells **are** the figure; pane chrome is an eyebrow row; the worker strip is a **ruled list** of 32px rows | no decoration; focus ring `focus`; the disconnected banner becomes a one-line stated outcome |
| Transcript | read what the emulator holds | reading column at `ledger` type (15/22 mono, tabular), 68ch measure, hanging indent in `ch`; *Jump to latest* as a **callout rail** item | line-wrap and the 390px reflow are Unit 4's mechanical fixes (see visual-qa-plan.md) |
| Attention rail | see what needs me, in order, and the decisive one | **annotated timeline** per session group: `line-strong` spine, mono timestamps, elapsed time printed on the spine, the critical item marked `verdict`, repeats printed as `×N` | stacked cards, per-card dismiss glyphs and the `time` eyebrow go; actions stay as text buttons on the event |
| Connection failed / retry | know whether it will recover and when | **verdict sentence** (serif, states the outcome) over an **annotated timeline** of the attempts; the connection log is an **evidence ledger** | spinner, elapsed counter and amber step line go; `fatal` reads *not retrying* on the sentence |
| Pairing dialog | complete a capability exchange safely | step 1 unchanged in structure; `.pair-details` rendered as an **evidence ledger** (term rows, hairlines, no box); the pairing code as a **hero-number** | copy, order, buttons and live regions are unchanged (cas-8051 / cas-7d55 contracts) |

No two adjacent sections share a form; the ledger appears under the hero and again inside the
pairing dialog and the connection log, never as the first element of a screen.

## Light and dark policy

The house language is light-first with a dark counterpart. The Commander gets **both**:

- `color-scheme: light dark` on `:root`; tokens are generated for both schemes from
  `design-tokens.json` (Unit 2) and switched by `prefers-color-scheme`.
- **Default follows the operating system.** A terminal-heavy surface is used in whatever room
  the operator is in; the OS knows the room. When the browser states no preference the page
  is light, which is the house default.
- The override is persisted (`localStorage`, `commander.scheme` = `system | light | dark`)
  and applied through a `data-scheme` attribute on `<html>` so a stored choice never flashes.
- **Terminal wells are dark in both schemes.** `--bg-terminal` stays a hub-only token at
  `#0C0E13`; Ghostty's ANSI palette is unchanged and would not survive a light well. In light
  the panes read as deep wells cut into warm paper; in dark they are the darkest step of the
  ramp, as today.
- Both schemes are verified at 1280×800 and 390×844 by the strict visual-QA row (Unit 6).

## Typography

Three families, all system stacks, from `typography.family`:

| Role | Token | Where in the Commander |
| --- | --- | --- |
| display (Iowan Old Style … serif) | `--font-display` (new) | the fleet verdict sentence; the stated outcome on the connection-failed card; nothing else |
| body (Inter … sans-serif) | `--font-ui` | buttons, prose, dialog copy, headings |
| mono (JetBrains Mono … monospace) | `--font-mono` | everything a machine minted: codenames, IDs, paths, timestamps, phases, pairing codes, eyebrows, the transcript |

The size scale keeps the console dense but re-keys every step to a house step (`token-map.md`
rows 24–29):

| Step | Size / line | Family, weight | Use |
| --- | --- | --- | --- |
| eyebrow | 12 / 16 | mono 600, uppercase, tracking .08em | pane roles, section labels, `dt` terms, the hero eyebrow |
| meta | 13 / 18 | mono 400 | session meta, pane chrome, chips (hub-only step; the house has no 13) |
| caption | 14 / 20 | sans 400 | body of controls, card prose, dialog copy |
| terminal | 13 / 1.35 | mono 400 | Ghostty grid, clamped 12–16 by the renderer (unchanged) |
| ledger | 15 / 22 | mono 400, tabular | the transcript, ledger rows, the connection log |
| lede | 21 / 30 | mono 400 for the open session codename; sans for "Fleet overview" | the one `h1` |
| title | clamp(24px, 3vw, 34px) / 1.1 | display 400, tracking −.015em, max 22ch | the fleet verdict sentence and the connection outcome |

Weights stay 400 / 500 / 600; nothing heavier, including ANSI bold in the renderer.

## Not changing

- **Pairing flow structure and copy.** Step order, the one-primary-action rule, `.pair-status`
  as a live region, the cleanup step and its retry, the "asks for the machine's hub address"
  behaviour, and every sentence bound by the cas-8051 and cas-7d55 contracts and
  `docs/specs/2026-08-08-commander-security-architecture.md`. Unit 5 restyles the dialog's
  surfaces and the `.pair-details` list; it does not touch `pairing-*.ts` logic or copy.
- **Ghostty terminal rendering**: `hub-web/src/terminal/*`, the ANSI palette, the 12–16px
  clamp, the 80-column floor below `compact`, and the transcript reflow model.
- **The render model** (`renderDecision()`, `shellSignature()`, region updaters) and the phone
  layout invariants D5/D7/D8/D15 in `DESIGN.md`. Restyling happens inside the regions each unit
  owns; nothing new enters a signature.
- **The scheme of the terminal well** (dark in both schemes, above).
- **The `dist/` bundle** is rebuilt once by the integration owner; no unit commits it.

## Critique

Appended by Unit 7 (cas-b296) after the render, using `critique-rubric.md`; the baseline
scored here for the delta:

| Dimension | Before (3.17.3) | After (epic tip `698dbaba`) | Evidence (before) |
| --- | --- | --- | --- |
| Distinctiveness | 2 | **4** | dark graphite, all-sans, rounded cards, count badges — house tokens absent |
| Fit to argument | 1 | **5** | the fleet board is three cards; no figure, no sentence |
| Hierarchy | 2 | **4** | "Fleet overview", the `4`/`75` badges and the cards share weight above the fold |
| Craft | 0 | **4** | attention `time` eyebrows and `.pane-role` under 4.5:1; attention prose clipped by a fixed height; transcript lines escape the viewport at 390 (`captures/before/baseline-visual-qa.json`) |
| Accessibility | 0 | **4** | contrast pairs below 4.5:1 in dark; no light scheme; content lost with JS disabled (expected, allowlisted per class in `visual-qa-plan.md`) |

Scored by agile-octopus-74 on 2026-09-07 from the before captures; floor fails on four rows.
After column scored by watchful-jaguar-3 on 2026-09-07 from the strict run on the integrated tip;
floor holds (4 / 5 / 4, no 0) — evidence, receipts, the before/after figure and four non-blocking
send-backs are in [critique.md](critique.md).
