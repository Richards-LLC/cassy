# Commander table rendering

head_sha: 46b823b1781cd0d14ce644eafd777d7b50aa5948
frontend at walk start/end: committed hub-web/dist from 46b823b1781cd0d14ce644eafd777d7b50aa5948; served locally at /commander/
backend at walk start/end: protocol double; no backend Git revision, live daemon or Cloud claim
original selected run: 910500855acd5bb4b0289872dabb92fb8754323f; frontend dist tree 8c3c8e23c4d21ef504b659156cd7408010e56689; table renderer/CSS byte-identical at control tip
budget: 30 minutes for delivery QA; original affected run 13.2 minutes; evidence-only correction performs no new walk
cells: 33
PASS: 30
FAIL: 3
NOT EXERCISED: 0
label split: 8 real-build (rendered table cells), 25 fixture (protocol-model journey results)
sweep: not configured
scope: Read two- and three-column supervisor markdown tables with prose, formatted text and compact identifiers on desktop and phone; retain the original affected journey outcomes.
surface: Commander conversation body
artifacts root: /home/pippenz/.cas/artifacts/cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a/cas-8bb3
portable original evidence: art-a62f5e35 (table-evidence.tar.gz)
design: hub-web/DESIGN.md@ce0d8adc9

Counts cover the 8 table cells and 25 original journey rows below, not the individual native tests. Matrix subtotal: 8 PASS / 0 FAIL. Journey subtotal: 22 PASS / 3 FAIL. Isolated controls are described inside observed cells and do not replace failed rows or increase headline counts.

The table cells exercise the actual compiled frontend; their real-build label proves rendering only. All server transport is doubled. Journey rows are explicitly fixture evidence for modeled server behavior, never live-backend proof. No assertion here claims production pairing, daemon delivery or Cloud behavior.

Expected for every table cell: words stay whole (including Daniel and C2), prose uses left-aligned body type, inline formatting produces DOM elements, the table owns horizontal overflow, and the page stays within its viewport. Before and after use the same named production-bundle browser regression and inputs. Screenshots are real browser captures.

## Cells

| id | cell | expected | observed | verdict | label | evidence path | defect task |
| --- | --- | --- | --- | --- | --- | --- | --- |
| M01 | Tab / State, 1280×800, light | readable words, safe bold/italic/code/link | Daniel Open Hours stays whole; strong/em/code/HTTPS-link DOM present; prose is left-aligned in body font; overflow-x auto; document width equals viewport. | PASS | real-build | before/table-2-1280-light.png; after/table-2-1280-light.png | — |
| M02 | # / What / Starts, 1280×800, light | C2 whole; prose left aligned | C2 stays whole; strong/code DOM present; prose is left-aligned in body font; overflow-x auto; document width equals viewport. | PASS | real-build | before/table-3-1280-light.png; after/table-3-1280-light.png | — |
| M03 | Tab / State, 1280×800, dark | same readable rendering in dark | Daniel Open Hours stays whole; strong/em/code/HTTPS-link DOM present; prose is left-aligned in body font; overflow-x auto; document width equals viewport. | PASS | real-build | before/table-2-1280-dark.png; after/table-2-1280-dark.png | — |
| M04 | # / What / Starts, 1280×800, dark | same readable rendering in dark | C2 stays whole; strong/code DOM present; prose is left-aligned in body font; overflow-x auto; document width equals viewport. | PASS | real-build | before/table-3-1280-dark.png; after/table-3-1280-dark.png | — |
| M05 | Tab / State, 390×844, light | word wrapping; horizontal overflow local | Daniel Open Hours stays whole; strong/em/code/HTTPS-link DOM present; prose is left-aligned in body font; overflow-x auto; document width equals viewport. | PASS | real-build | before/table-2-390-light.png; after/table-2-390-light.png | — |
| M06 | # / What / Starts, 390×844, light | C2 whole; no page overflow | C2 stays whole; strong/code DOM present; prose is left-aligned in body font; overflow-x auto; document width equals viewport. | PASS | real-build | before/table-3-390-light.png; after/table-3-390-light.png | — |
| M07 | Tab / State, 390×844, dark | readable phone rendering in dark | Daniel Open Hours stays whole; strong/em/code/HTTPS-link DOM present; prose is left-aligned in body font; overflow-x auto; document width equals viewport. | PASS | real-build | before/table-2-390-dark.png; after/table-2-390-dark.png | — |
| M08 | # / What / Starts, 390×844, dark | readable phone rendering in dark | C2 stays whole; strong/code DOM present; prose is left-aligned in body font; overflow-x auto; document width equals viewport. | PASS | real-build | before/table-3-390-dark.png; after/table-3-390-dark.png | — |
| HUB-J1 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J1/bundle.json | — |
| HUB-J2 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J2/bundle.json | — |
| HUB-J3 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J3/bundle.json | — |
| HUB-J4 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original table setup failed before rendering: no socket open at hub-double.ts:703. Fix 46b823b waits for visible Live; separate control.log records 10/10 passing. Original outcome remains FAIL. | FAIL | fixture | journeys/HUB-J4/bundle.json | cas-1c92 |
| HUB-J5 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J5/bundle.json | — |
| HUB-J6 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J6/bundle.json | — |
| HUB-J7 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J7/bundle.json | — |
| HUB-J8 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J8/bundle.json | — |
| HUB-J9 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J9/bundle.json | — |
| HUB-J10 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J10/bundle.json | — |
| HUB-J11 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J11/bundle.json | — |
| HUB-J12 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original network-switch.journey.ts:1197 timed out in ProtocolClock.advance. Isolated network-control.log: 1 PASS; original outcome remains FAIL. | FAIL | fixture | journeys/HUB-J12/bundle.json | cas-c971 |
| HUB-J13 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J13/bundle.json | — |
| HUB-J14 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J14/bundle.json | — |
| HUB-J15 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J15/bundle.json | — |
| HUB-J16 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J16/bundle.json | — |
| HUB-J17 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J17/bundle.json | — |
| HUB-J18 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J18/bundle.json | — |
| HUB-J19 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J19/bundle.json | — |
| HUB-J20 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J20/bundle.json | — |
| HUB-J21 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J21/bundle.json | — |
| HUB-J22 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J22/bundle.json | — |
| HUB-J23 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J23/bundle.json | — |
| HUB-J24 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original paste-content.journey.ts:143 timed out clicking Send. Isolated paste-control.log: 1 PASS; original outcome remains FAIL. | FAIL | fixture | journeys/HUB-J24/bundle.json | cas-c971 |
| HUB-J25 | Original affected journey (protocol model) | Complete the registered journey with its rendered assertions and protocol-model read-back. | Original selected journey assertions passed against the protocol model; bundle retains screenshots and modeled-state evidence. | PASS | fixture | journeys/HUB-J25/bundle.json | — |

## Adjacent and safety proof

- Existing numeric-evidence unit case includes a bold numeric cell: mono/right classification includes 412, pass, 318 and 1 flake. npm test: 100 files, 1467 passed (vitest-final.log).
- Existing markdown-renderer tests cover safe link schemes and literal HTML. Tables call appendInline; no innerHTML was introduced.
- Control read-history spec: 10 passed at one worker (control.log), including all eight table capture cells. Typecheck exited 0 (typecheck-final.log).
- Strict snapshot visual QA: PASS, two URLs × two viewports × two schemes. Media snapshots: forced-colors, reduced-motion and contrast-more PASS. See qa/bundle.json.

## Constants vs expectation

| constant | location | visible contract | predictable? | defect task |
| --- | --- | --- | --- | --- |
| None added or changed in the table path | conversation-view.ts renderBody; markdown-renderer.ts appendInline; styles.css .evi | Table content renders immediately and owns horizontal overflow; no hidden timing threshold. | Yes | — |
| OPENING_MOTION_DELAY_MS (unchanged, outside table path) | conversation-view.ts:1180 | Existing conversation-opening indicator; unrelated to table content. | Not assessed in this table QA; no new claim | — |

Search for `MIN_`, `MAX_`, `_MINUTES`, `_MS`, `_SECS`, `THRESHOLD`, `GRACE`, `DEBOUNCE` and `RETRY` in the touched source files finds only existing connection-state imports and the existing opening-motion threshold. No table-path threshold is present; no control timing changed.

## Contradictions

| terminal state | surface | visible claim | conflicting surface | defect task |
| --- | --- | --- | --- | --- |
| Read-only rendered table | Before/after captures and final ARIA in qa/ | Text and inline formatting match the supervisor reply fixture; no delivery/completion claim is introduced. | No contradictory claim observed within the captured table surface; other lifecycle surfaces were outside this correction. | — |

## Honesty

Original failing results remain in red.log and red-playwright/. An intermediate green.log failure counted a legal hyphen break in time-tracking as a broken word; the assertion was corrected to Unicode word segments. Inline code/link wrapping was also made explicit. No original failure is relabeled as a passing run.

Automatic capture returned unavailable: affected journeys exceed max_shots and docs/qa/shots.json is absent. Manual before/after pairs cover all requested shapes/variants. Extracted HTML snapshots used by strict visual QA preserve captured DOM and built CSS; they are snapshots, not live backend evidence.

The original four-worker selected run remains 190 passed / 3 failed. Its aggregate receipt was refused because a readiness-only test commit advanced HEAD during execution. Native report.json and journey bundles are retained. The two adjacent timeout controls passed alone (network: 31.3s total; paste: 30.4s total), and cas-c971 tracks their load-sensitive cause. HUB-J4 setup failure and its existing readiness fix are recorded under cas-1c92.

This correction only restructures the existing ledger. No new QA walk or browser rerun occurred; no product, catalog, runner or test inputs change in this delivery. Evidence still names its original tested build, rather than claiming a fresh run at the evidence correction tip. All evidence paths are relative to artifacts root above. The repository copy is a reviewable receipt; the task artifact copy is the verifier input.
