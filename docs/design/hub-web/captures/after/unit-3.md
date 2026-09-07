# Fleet board and shell — Unit 3 proof

The fleet is a state-track dot plot beside a serif verdict, followed by a ruled session ledger.
The critical session leads the figure and carries the verdict ring; working sessions form a column
inside the shaded band. At 390px the verdict stacks above the plot; eight 28px session rows fit.
Phase words sit inline beside fixed-position dots, with hairlines at row boundaries. Narrow
figures use numbered columns and an unbroken full-word legend. Names stay on one line with
ellipsis and a full-name title/accessible label; normal codenames fit in the wider phone column.

| Surface | Light 1280 / 390 | Dark 1280 / 390 |
| --- | --- | --- |
| Built app, no paired machines | PASS / PASS | PASS / PASS |
| Production fleet renderer, three sessions | PASS / PASS | PASS / PASS |
| Production fleet renderer, eight sessions | PASS / PASS | PASS / PASS |

Captures use 1280×800 and 390×844. `fleet-empty` is the real built app; `fleet-board` and
`fleet-eight` use the production FleetBoardRenderer in a minimal shell. Their model has two
machines, one blocked session, and working/idle sessions. No live fleet or private session data
is captured. The plot shows each session's animal codename; full names and source metadata are
in the ledger and accessible row names. The catalog supplies no activity timestamp: runtime
Working/Idle uses the reported phase; no timestamp is fabricated as activity.

Strict command, repeated for each scheme and viewport:

```sh
node scripts/visual-qa.mjs --strict --scheme light --viewport 390x844 \
  --allowlist /home/pippenz/.cas/artifacts/cas-0a8e/allowlist.json \
  --artifact-dir /home/pippenz/.cas/artifacts/cas-0a8e/correction-qa-light-390x844 \
  http://127.0.0.1:4186/commander/ \
  http://127.0.0.1:4186/commander/fleet-proof.html \
  'http://127.0.0.1:4186/commander/fleet-proof.html?count=8'
# PASS
```

All four commands exited 0. The only exception is the approved `javascript-disabled-loss`
class: Commander needs JavaScript for pairing/live state/terminals, and its noscript line says so.
There are no contrast, clipping, overlap, viewport or print exceptions.

After rebasing onto epic 8dc1b283 (including Units 2, 4 and 6), `npm run visual-qa --
--artifact-dir /home/pippenz/.cas/artifacts/cas-0a8e/correction-fixture-final` exits 0:
`PASS 9 fixtures x 2 schemes x 2 viewports`. The report contains 36 captures, zero findings,
and eight existing plan allowances (four JavaScript dependency, two collapsed-pane overflow,
two terminal clipping). No new allowlist entry was added. Unit 7 owns the assembled critique.

## Interaction and implementation evidence

`browser-proof.mjs` passes light/dark ×1280×800/390×844: Appearance dark→light→system,
storage persistence after reload, palette keyboard reopen and focus return, drawer
closed→open→closed with matching inert/aria-hidden, keyboard opening a ledger session,
eight plot rows visible above the fold, no horizontal overflow, and print expansion.
The drawer check exercises real DOM state; the unpaired shell hides its navigation.

The corrective round adds `correction-geometry.mjs`: 3/8 sessions × light/dark ×1280/390,
with every table cell and name measured at 28px; name and dot centers aligned; all working dots
sharing one x coordinate; phase text inside the working band and clear of every hairline;
full track words on one line; all eight rows, legend and caption visible without scrolling.
The script refreshes all 12 PNGs above. `correction-geometry.json` retains the measured boxes.
The empty-state PNGs were recaptured and remain byte-identical. Fresh `npm run typecheck` and
`npm test` both exit 0 (31 files, 439 tests, including 14 fleet tests and 61 invariants).

FleetBoardRenderer owns derived fleet state and its region key. main.ts supplies existing catalog
and attention data; shellSignature/renderDecision and terminal rendering are unchanged.
Catalog receipt time patches text without replacing a focused session button. Unknown liveness
falls back to Stale. The palette follows the shell scheme; terminal wells remain dark.
No raster assets, fonts or network requests were added. No motion was added. The local build
measured about 67 KB gzip of JavaScript including the fleet module and 10 KB gzip CSS;
Ghostty's existing WASM/font assets remain outside this unit. LCP/INP field data is unavailable.

## Critique

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Serif verdict, warm paper/graphite, ringed outlier and working band replace equal cards. |
| Fit | 4 | One dot outside the working band identifies the session that needs the operator. |
| Hierarchy | 4 | Verdict and plot lead, then a 3px rule, provenance and the quieter ledger; eight rows fit on phone. |
| Craft | 4 | Four strict owned-screen PASS receipts and the integrated strict PASS; browser measurements prove 28px alignment, inline phase clearance and full-word phone legend. |
| Accessibility | 4 | Semantic table/text states, full accessible session names, keyboard flow, both schemes and print pass; the JS dependency is stated. |

Corrective round scored by cosmic-dragon-35 on 2026-09-07 after reviewing the refreshed
light/dark desktop/phone captures. This supersedes the earlier craft score, which missed phase
and hairline collisions. Scope is Unit 3; Unit 7 owns the whole-app critique.

Durable JSON, logs, runnable proof sources and the four strict reports:
`/home/pippenz/.cas/artifacts/cas-0a8e/` (`correction-qa-*`, `correction-geometry.*`,
`correction-interactions.log`, `correction-fixture-final/`, `correction-tests.log`,
`correction-typecheck.log`, `correction-build.log`, `fleet-proof.*`, `build-proof.mjs`).
Preview servers are stopped at delivery.
