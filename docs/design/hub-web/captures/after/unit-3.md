# Fleet board and shell — Unit 3 proof

The fleet is a state-track dot plot beside a serif verdict, followed by a ruled session ledger.
The critical session leads the figure and carries the verdict ring; working sessions form a column
inside the shaded band. At 390px the verdict stacks above the plot; eight 28px session rows fit.

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
  --artifact-dir /home/pippenz/.cas/artifacts/cas-0a8e/qa-light-390x844 \
  http://127.0.0.1:4186/commander/ \
  http://127.0.0.1:4186/commander/fleet-proof.html \
  'http://127.0.0.1:4186/commander/fleet-proof.html?count=8'
# PASS
```

All four commands exited 0. The only exception is the approved `javascript-disabled-loss`
class: Commander needs JavaScript for pairing/live state/terminals, and its noscript line says so.
There are no contrast, clipping, overlap, viewport or print exceptions.

After rebasing onto Unit 6 at epic a39919fc, `npm run visual-qa` produced 36 captures.
Fleet-populated and fleet-empty have zero findings; print-loss is zero on all nine fixtures.
The complete matrix exits 1 for 16 findings in Unit 4's unmerged surfaces: pane-title contrast
(4), attention-repeat contrast (2), invisible dismiss (4), attention detail overflow (2), clipping
(2), and truncation (2). These are recorded, not allowlisted. Unit 7 owns the integrated receipt.

## Interaction and implementation evidence

`browser-proof.mjs` passes light/dark ×1280×800/390×844: Appearance dark→light→system,
storage persistence after reload, palette keyboard reopen and focus return, drawer
closed→open→closed with matching inert/aria-hidden, keyboard opening a ledger session,
eight plot rows visible above the fold, no horizontal overflow, and print expansion.
The drawer check exercises real DOM state; the unpaired shell hides its navigation.

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
| Craft | 4 | Four strict owned-screen PASS receipts; zero clipping/overlap/overflow. Long track labels wrap on phone. |
| Accessibility | 4 | Semantic table/text states, full accessible session names, keyboard flow, both schemes and print pass; the JS dependency is stated. |

Scored by proud-falcon-90 on 2026-09-07. Scope is Unit 3; Unit 7 owns the whole-app critique.

Durable JSON, logs, runnable proof sources and the four strict reports:
`/home/pippenz/.cas/artifacts/cas-0a8e/` (`qa-*`, `browser-proof.*`, `fixture-qa-final/`,
`fleet-proof.*`, `build-proof.mjs`). Preview servers are stopped at delivery.
