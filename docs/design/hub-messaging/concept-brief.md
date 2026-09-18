# Brief: 2026-09-18-hub-messaging-study.html

Concept brief for the hub messaging design study (task cas-c3c5), written before the markup, per
`cas-ui-craft/references/concept-brief.md`. Tokens are the house set in
`docs/design/design-tokens.json` as mapped by `hub-web/DESIGN.md` and `hub-web/src/tokens.css`;
the mockups add one house token the hub does not yet map (`surface-hero`, for the *ask* kind) and
one status tint (`good-tint`) — both already exist in the token file.

## Single idea

The hub should show the operator only the turns the supervisor addresses to them — six typed
kinds on a ruled timeline (the Ledger), with the pane one tap away — because roughly six of every
seven things the pane prints are tool traffic and worker mail, not a message to the operator; two
other surfaces (Desk, Brief) are drawn in full so the operator can pick.

## Hero form

**Waffle plot** (form vocabulary: *show a share of a whole; every unit visible*). One hundred
squares, each one per cent of the 5,155 content blocks three real supervisor sessions printed to
their pane; the fifteen squares that are the supervisor's own prose sit in `verdict`, the three
that are the operator's directives in `action`-tinted outline, and the remaining eighty-two —
tool calls, tool results, worker and director mail — in `line`. The shape is the claim: a thin
indigo band in a field of grey is what "mirror the pane" delivers to a phone. A second, smaller
waffle of the same population by character count (2.4 % prose) sits in the margin as the
worst-case reading.

## Emotional register

**Calm, exact, unhurried.** Warm paper and one indigo mark above the fold; the verdict in the
serif; the measurement in tabular mono with its method printed under it; status colour appears
only inside the mockups where a blocker is a blocker. No reference screenshot appears before the
argument; the gallery is a ruled contact sheet below the fold, not a hero.

## Distinctive move

The proposed conversation is drawn as an **annotated timeline, not a chat**: mono timestamps run
down a left spine, the elapsed time between a directive and its answer is printed on the spine,
each supervisor turn carries a kind eyebrow (ANSWER · RECEIPT · STATUS · BLOCKER · WAITING ON YOU
· EVIDENCE), and the one unanswered *ask* is the single `verdict`-ruled, sandstone element on the
screen. The same spine reappears once in the seam section, where the message path is drawn as a
timeline of file:line hops.

## Deliberately omitted

- **Chat bubbles for the supervisor.** Bubbles equalise every turn; the supervisor's turns are
  not equal — a receipt is a ledger, a status is a line, an ask is a decision.
- **Streaming the pane into the thread.** The pane stays available as an escape hatch (a
  toggle, and a per-turn "pane line N ▸" anchor) and never as the default reading surface.
- **A KPI row of the measurement** (five percentages in boxes). The waffle carries the numbers;
  the evidence ledger beneath it carries the exact counts and the method.
- **Hotlinked or described references.** Every one of the reference frames is a real capture
  saved under `refs/` and embedded as a data URI, so the report opens offline.
- **A spinner for "still working".** A hollow ring and a mono sentence derived from the pane's
  heartbeat and phase, refreshed in place; nothing loops.

## Forms per section

| Section | Reader's task | Form | Why |
| --- | --- | --- | --- |
| Verdict hero | get the conclusion | verdict sentence + waffle plot | share of a whole; every unit visible |
| Today | see what the operator gets now | before/after pair (four real captures) | the change is judged on the artifact itself |
| References | scan 22 UIs for stealable patterns | ruled contact sheet, three lines per frame | parallel, equally weighted items; the one grid on the page |
| Taxonomy | look up each kind's treatment | evidence ledger | exact lookup, auditability |
| Three options | judge three surfaces at both widths | paired plates per screen (hub-mobile plate format), 24 renders | same session, same minute; the layout carries the comparison |
| Choosing | pick one | evidence ledger with the recommended row banded | the decision is spatial |
| Seam | follow the message path | annotated timeline of file:line hops | causality is an ordering claim |
| Proposal | see the minimal change | ledger of field → change → cite | contradiction is spatial |

## Evidence and deliberate costs

- The HTML embeds 25-plus reference captures, 12 today-state captures (4 shown) and 24 option renders as WebP data URIs; it will exceed the report contract's ~500 KB guide, as the hub-mobile study did (1.16 MiB), because real captures are the evidence. It still opens offline with zero requests.
- Option renders are full-page captures at 390 and 1280 px width so every kind is visible in one image; the live surfaces would scroll inside a fixed shell.
- Names, times, SHAs and run numbers in the mockups are illustrative; the seam cites and the pane measurement are real.

## Critique

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | serif verdict on sandstone over a waffle whose fifteen indigo squares are the argument; the seam is an annotated timeline of file:line hops; the three options are paired plates in the hub-mobile format |
| Fit to argument | 4 | the waffle's shape is the claim (a thin band of message in a field of traffic); the recommendation among options is carried by the comparison ledger below, not by the hero |
| Hierarchy | 4 | verdict, why, figure, 3px rule, provenance, then everything a step down; the document title above the eyebrow is a second, muted line |
| Craft | 4 | `node scripts/visual-qa.mjs … --strict` PASS (light + dark, 1280 + 390, 0 findings, 0 allowlisted); 844×390 without overflow; print reflows the hero to one column; the option plates are full-page captures that need enlarging at 390 |
| Accessibility | 4 | JS-off text identical (51,103 characters), one `h1`, `details` open in the markup, 0 external requests, keyboard-reachable viewer dialog; physical phones untested; 2.3 MB is slow on a weak link |
Scored by calm-stork-91 on 2026-09-18 from the strict run and the renders in `/home/pippenz/.cas/artifacts/cas-c3c5/`; floor holds (4 / 4 / 4, no 0). Per-option scores (Ledger 4/5/4/4/4, Desk 4/3/5/4/4, Brief 5/3/4/4/4) are in the report's Critique section; the supervisor supplies the independent second reading.
