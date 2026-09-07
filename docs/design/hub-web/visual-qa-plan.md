# Visual-QA plan for the Commander

What `scripts/visual-qa.mjs` finds on the 3.17.3 Commander today, class by class, with the
justification each allowlist entry will carry in Unit 6 (cas-211c) — or **must fix**, naming the
unit that owns it. Unit 7 (cas-b296) shows the delta against the counts below.

## How the baseline was measured

- The live local hub (3.17.3, `http://127.0.0.1:4173/commander/`, the embedded `dist/`) was
  paired from a fresh headless Chrome profile with a one-time `cas hub pair` invitation, so every
  screen shows real fleet state, not a fixture.
- `captures/capture-before.mjs` walked the screens at 1280×800 and 390×844, dark scheme, and ran
  the verbatim `PAGE_INSPECTION` function from `scripts/visual-qa.mjs` on each one (same
  thresholds: 4.5:1 text, 3:1 large text, 1px box tolerance). Counts are in
  `captures/before/baseline-visual-qa.json` with every finding's selector, sample and ratio (committed run: 2026-09-07 12:48–12:54Z, fleet of three sessions on one machine, session `cas-src-sharp-tiger-49` with three workers open).
- The script proper was run once, unmodified, on the unpaired page — the only state it can reach
  without credentials — for the page-level classes it alone reports (`print-loss`,
  `javascript-disabled-loss`): `captures/before/visual-qa.unpaired.dark.md`.
- Counts are a snapshot of a live console: the transcript and attention classes scale with how
  much text the emulator held and how many attention items the hub had (119 info, 8–9 critical
  at capture time). The classes and their causes are stable; the exact numbers are not, and the
  delta Unit 7 reports is per class and per cause, not per node.

## Baseline counts per screen (dark)

| Screen | Viewport | contrast | invisible-text | clipped-content | content-overflow | truncated-container | outside-viewport | overlapping-text | Capture |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| pairing-dialog-step1 | 1280 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | [pairing-dialog-step1.1280.dark.png](captures/before/pairing-dialog-step1.1280.dark.png) |
| pairing-dialog-step1 | 390 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | [pairing-dialog-step1.390.dark.png](captures/before/pairing-dialog-step1.390.dark.png) |
| fleet-board | 1280 | 1 | 11 | 0 | 0 | 0 | 0 | 0 | [fleet-board.1280.dark.png](captures/before/fleet-board.1280.dark.png) |
| fleet-board | 390 | 0 | 12 | 0 | 0 | 0 | 0 | 0 | [fleet-board.390.dark.png](captures/before/fleet-board.390.dark.png) |
| session-canvas | 1280 | 14 | 12 | 1 | 1 | 1 | 0 | 0 | [session-canvas.1280.dark.png](captures/before/session-canvas.1280.dark.png) |
| session-canvas | 390 | 4 | 12 | 333 | 4 | 3 | 1 | 0 | [session-canvas.390.dark.png](captures/before/session-canvas.390.dark.png) |
| transcript | 1280 | 13 | 12 | 321 | 1 | 1 | 0 | 0 | [transcript.1280.dark.png](captures/before/transcript.1280.dark.png) |
| transcript | 390 | 4 | 12 | 319 | 4 | 3 | 1 | 1 | [transcript.390.dark.png](captures/before/transcript.390.dark.png) |
| attention-rail | 1280 | 129 | 111 | 305 | 153 | 96 | 0 | 0 | [attention-rail.1280.dark.png](captures/before/attention-rail.1280.dark.png) |
| attention-rail | 390 | 120 | 111 | 21 | 12 | 8 | 0 | 0 | [attention-rail.390.dark.png](captures/before/attention-rail.390.dark.png) |
| connection-disconnected | 1280 | 152 | 132 | 377 | 189 | 118 | 0 | 0 | [connection-disconnected.1280.dark.png](captures/before/connection-disconnected.1280.dark.png) |
| connection-disconnected | 390 | 143 | 132 | 23 | 13 | 9 | 0 | 0 | [connection-disconnected.390.dark.png](captures/before/connection-disconnected.390.dark.png) |
| connection-failed | 1280 | 142 | 132 | 376 | 188 | 117 | 0 | 0 | [connection-failed.1280.dark.png](captures/before/connection-failed.1280.dark.png) |
| connection-failed | 390 | 142 | 132 | 21 | 10 | 6 | 0 | 0 | [connection-failed.390.dark.png](captures/before/connection-failed.390.dark.png) |
| **all screens** | | **864** | **821** | **2097** | **575** | **362** | **2** | **1** | |

Unpaired page, script run unmodified (`--scheme dark`, desktop + phone): `javascript-disabled-loss`
×2, nothing else — the empty state has no contrast, clipping or overflow finding, and print did
not lose it.

## Classes, causes and the allowlist position

| Class | Baseline cause (file:line) | Position | Owner |
| --- | --- | --- | --- |
| `contrast` | `--text-lo` #5C6577 on `--bg-raised` = 2.78:1 — `.attention-eyebrow time` (styles.css:1449), `.pane-role` (:929), `.pane-last-activity`, pane-header controls, `.terminal-connecting-elapsed`; `.attention-explicit-dismiss` and `.attention-count--critical` at 3.91:1 (`--state-crit` on `--bg-active`); `--text-lo` on `--tint-crit` 2.66 and on the toast 2.54 | **must fix** — no allowlist. `--text-lo` is retired (token-map row 11); danger text sits on `surface` or `danger-tint` where the sanctioned pair is ≥ 5.06 | Unit 2 retires the token; Units 3–5 move each consumer |
| `invisible-text` (`opacity-0`) | two hover-reveal patterns: `.attention-dismiss { opacity: 0 }` until hover/focus (:1532; one per attention item, so 100+ per attention screen) and the closed machine drawer keeping its rows in the DOM at `opacity: 0` (:373; `.session-name`, `.session-meta`, `#machine-drawer-close`, `#remove-machine` — the constant 11–12 per screen); `#toast` at rest (:2134) | **must fix** for the dismiss glyph — hover-only affordances fail the accessibility floor on a phone; the timeline form gives each event a visible text action. **Allowlist** for the closed drawer and the resting toast, scoped to `.machine-drawer[aria-hidden="true"] *` and `#toast:not(.visible)`: they are closed overlays whose content is off-screen by design and announced only when open. The entry is valid only because the matched node is also hidden from assistive technology — and at 3.17.3 the closed drawer is **not**: `.machine-drawer` (styles.css:373) is `opacity: 0; pointer-events: none` with no `aria-hidden`/`inert` (main.ts:1962), so its rows are reachable by a screen reader while invisible. Unit 3 adds `inert` + `aria-hidden="true"` to the closed drawer; until then the entry does not match and the class stays red | Unit 4 (dismiss); Unit 6 (two scoped entries); Unit 3 (`inert` on the closed drawer) |
| `clipped-content` + `content-overflow` + `truncated-container` on `.attention-detail` | the two-line `-webkit-line-clamp` with `overflow: hidden` (:1515–1525) — three classes fire on the same node; it is the largest `clipped-content` source on every screen that shows the attention panel | **must fix** — the house container rule allows a clamp only with a visible affordance and this one has none; the timeline shows the detail whole (the `▸ Details` disclosure keeps the payload). No allowlist | Unit 4 |
| `clipped-content` + `content-overflow` on `.attention-session`, `.attention-group-label` | `white-space: nowrap; text-overflow: ellipsis` on session codenames in the group header (:1462) — the second-largest `clipped-content` source on the attention screens | **must fix** — the codename is the heading of the group and is cut after eight characters at 320px (`gabber-st…`); the timeline gives the codename its own line at `meta` size and wraps | Unit 4 |
| `clipped-content` on `p.transcript-line > span`; `outside-viewport` on the same | the transcript reflow keeps emulator lines at their logical width inside an `overflow: hidden` column; at 390 the rows escape the document width (`outside-viewport`: 1 node per screen in the committed run, 106 in an earlier run when the emulator held a wide box-drawn table; `clipped-content` 319–333 per screen in the committed run, 780–854 in that earlier one, because the reflow column is narrower than the lines it holds) | **must fix** in the reading view (wrap at the measure; box-drawing runs may scroll inside their own container per `container.phone-wrap`); the terminal grid itself (`.terminal-mount`, panned below `compact`, D15) is **allowlisted** by selector: the 80-column floor is a deliberate pan and the emulator is out of scope | Unit 4 (transcript); Unit 6 (one entry for `.terminal-mount`) |
| `clipped-content` on `.session-picker-toggle > .session-picker-name` | ellipsis on the `h1` codename at 390 (`Penguinz-fierce-tig…`) | **allowlist** with a reason: the one `h1` is a single-line codename with `text-overflow: ellipsis` and a `title`, which the house container rule permits; the session picker shows it whole | Unit 6 |
| `content-overflow` / `truncated-container` on `section.pane.collapsed` | the collapsed worker bars (32px tall) clip the pane body they hide (one per collapsed pane) | **allowlist** scoped to `.pane.collapsed`: the clipped content is the unmounted terminal placeholder of a pane the operator collapsed; nothing textual is lost (the bar shows the codename and role) | Unit 6 |
| `overlapping-text` | one node per run: a box-drawing rule line in the transcript overlapping the following `span` at 390 (the reflow places two logical lines on one row) | **must fix** with the transcript wrap above | Unit 4 |
| `javascript-disabled-loss` | the Commander is a JavaScript application: `index.html` is an empty `#app` and a module script; without JS there is nothing to render, no pairing, no terminal | **allowlist**, whole class, with this reason — and the `<noscript>` line Unit 3 adds ("Cassy Commander needs JavaScript to reach your machines") is the one thing a script-less load shows, so the loss is stated, not silent | Unit 6 (class entry); Unit 3 (`noscript`) |
| `print-loss` | not observed on the unpaired page; expected on screens with terminals once a print stylesheet exists, because a `canvas` terminal has no printable text | **allowlist** per selector when it appears, never as a class: `.terminal-mount` is a canvas and prints as a labelled well; every text region (fleet ledger, transcript, attention timeline, pairing details) must print whole. Unit 6 adds the `@media print` sheet (there is none at 3.17.3) and a print run to the gate | Unit 6 |
| `unverifiable-contrast` (informational) | none in the baseline: every background resolves to a flat token colour | nothing to allow | — |
| `off-token-contrast` (informational) | none: the app has no `.tag`/`.status` classes the checker treats as status labels | nothing to allow | — |

Rules the allowlist file (`hub-web/visual-qa-allowlist.json`, Unit 6) must satisfy so it is a
list of decisions and not a mute button:

1. Every entry names a finding **type and a selector**; the only class-wide entry is
   `javascript-disabled-loss`, with the reason above.
2. Every entry carries the reason from this table, verbatim or tighter, and the unit that
   accepted it.
3. `contrast` is never allowlisted. A pair below 4.5:1 in either scheme is a 0 on the rubric.
4. An `invisible-text` entry is valid only while the matched node is inside an `aria-hidden`
   or closed (`[hidden]`, `:not([open])`) ancestor; the entry states that ancestor.
5. The strict run (`--strict`, light + dark, 1280 + 390) is green on every fixture screen with
   this list; a new finding class is a new row here before it is a new entry there.

## What Unit 7 reports

For each screen at each viewport and scheme: the class counts after, beside the baseline
counts above, and for every remaining allowlisted finding the entry that covers it. The delta
that matters is `contrast` → 0 without an allowlist, the three `.attention-detail` classes → 0,
`outside-viewport` → 0 at 390, and `invisible-text` reduced to the two scoped drawer/toast
entries.

## Unit 7 result (2026-09-07, epic tip `698dbaba`)

Reported in full in [critique.md](critique.md). Strict run: PASS 9 fixtures × 2 schemes × 2
viewports, 0 findings, 8 allowlisted. Against the counts above: `contrast` 864 → 0 with no
allowlist entry; `invisible-text` 821 → 0 (the two scoped drawer/toast entries this plan
anticipated were never needed — the fixtures carry no closed drawer or resting toast, and no
hover-only affordance remains); the three `.attention-detail` classes → 0; `outside-viewport`
2 → 0; `clipped-content` 2,097 → 0 with two allowlisted `h1` codename ellipses at 390 and
`content-overflow` 575 → 0 with the same two. Two amendments to this plan: the fixture runner's
phone viewport is 390×800, not the 390×844 stated here (send-back to Unit 6), and the
`connection-failed-retry` fixture is a hand-built stand-in rather than the production
verdict + timeline, so its PASS line covers the fixture markup and the production surface's
PASS is Unit 5's own receipt (send-back to Unit 6).
