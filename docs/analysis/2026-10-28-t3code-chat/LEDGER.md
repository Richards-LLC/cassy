# T3 Code report acceptance ledger

Scope: the standalone research report and the actual read-only research workflow; no Commander runtime change.
Surface: local single-file HTML, its PDF editions and captured shell workflow.
Budget: 30 minutes for report QA; original captures retained and reused after this ledger-only correction.
Build revision: report inputs at b1f4808e322bc5eb44ae6660a24eb8c6ca1bd126, contained in correction base 2fcf5175c. No compiled application binary is exercised; the actual report is the build.
Correction: ver-ee4f9738669a findings addressed in this ledger; HTML/PDF inputs and their receipts unchanged.

Headline counts: cells=19; PASS=5; FAIL=0; NOT EXERCISED=14.
Demo table counts: cells=5; PASS=5; FAIL=0; NOT EXERCISED=0.
Outside-demo journey table counts: cells=14; PASS=0; FAIL=0; NOT EXERCISED=14.
Evidence-label split: real-build=5; source-inferred=14.
sweep: not configured

## Demo and report acceptance table

The five cells below evaluate the report and workflow. The 14 Commander journeys sit outside this demo table and are separately accounted for below. Expectations come from the task ACs and the pre-render concept brief. This is implementation evidence, not an independent QA verdict.

| id | cell | expected | observed | verdict | label | evidence path | defect task |
| --- | --- | --- | --- | --- | --- | --- | --- |
| D01 | Open brief and choose follow-ups | 8–15 techniques, citations, effort, ranked top five | 12 techniques; 25 linked ranges checked; five ranked slices; actual rendered report inspected | PASS | real-build | /home/pippenz/.cas/artifacts/cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a/cas-b4e8/visual-qa-desktop/docs-analysis-2026-10-28-t3code-chat-report-html-light-1280x800.png | none |
| D02 | Phone/desktop in both schemes | Readable verdict/figure, no clipping or contrast defects | Strict desktop and phone light/dark receipts PASS with zero findings; full figure at 440px/540px | PASS | real-build | /home/pippenz/.cas/artifacts/cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a/cas-b4e8/phone-dark-fold.png | none |
| D03 | JavaScript disabled and offline | Report content remains complete without JavaScript/network resources | JS-disabled phone capture and strict comparison PASS; acceptance.json confirms complete source text and offline dependencies | PASS | real-build | /home/pippenz/.cas/artifacts/cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a/cas-b4e8/phone-light-fold.png | none |
| D04 | Print adjacent surface | Complete print editions, readable bounds and portable links | A4 8 pages and Letter 10; both check-pdf receipts zero bounds/local-link/footer defects; all Markdown text chunks preserved | PASS | real-build | /home/pippenz/.cas/artifacts/cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a/cas-b4e8/report-A4.pdf | none |
| D05 | Trust/license and no upstream execution | Saved original command record, pinned clone, license/read manifest | 54 captured shell requests audited; six upstream-cwd requests use read-only commands only. Clean pinned clone manifest and MIT/source manifest retained. See trust-audit.md | PASS | real-build | /home/pippenz/.cas/artifacts/cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a/cas-b4e8/command-log.md | none |

D01 companion: `acceptance.json` validates source citations and Markdown/HTML/PDF fidelity. D02–D03 companions: `visual-qa/visual-qa.json`, `visual-qa-desktop/visual-qa.json`; all four `*-fold.png` captures were made with JavaScript disabled. D04 companions: `report-Letter.pdf`, `pdf/A4/receipt.json`, `pdf/Letter/receipt.json`. D05 companions: `command-log.json`, `clone-manifest.json`, `trust-audit.md`, `search-manifest.md`. All companion paths resolve beside this ledger in the task artifacts and complete portable bundle.

## Commander journeys outside the demo table

These 14 rows are outside the five-cell demo table. They are not report demo checks and none is claimed PASS. The authoritative delivery impact is docs-only with no changed live application journey. They remain here to account explicitly for the task’s predicted journey list; they are included in the global 19-cell headline counts.

| id | cell | expected | observed | verdict | label | evidence path | defect task |
| --- | --- | --- | --- | --- | --- | --- | --- |
| HUB-J1 | Commander application journey | Existing registered goal remains unchanged | Report/ledger-only delivery; application inputs unchanged; no runtime walk attempted | NOT EXERCISED | source-inferred | not applicable: outside report scope | none |
| HUB-J11 | Commander application journey | Existing registered goal remains unchanged | Report/ledger-only delivery; application inputs unchanged; no runtime walk attempted | NOT EXERCISED | source-inferred | not applicable: outside report scope | none |
| HUB-J12 | Commander application journey | Existing registered goal remains unchanged | Report/ledger-only delivery; application inputs unchanged; no runtime walk attempted | NOT EXERCISED | source-inferred | not applicable: outside report scope | none |
| HUB-J13 | Commander application journey | Existing registered goal remains unchanged | Report/ledger-only delivery; application inputs unchanged; no runtime walk attempted | NOT EXERCISED | source-inferred | not applicable: outside report scope | none |
| HUB-J14 | Commander application journey | Existing registered goal remains unchanged | Report/ledger-only delivery; application inputs unchanged; no runtime walk attempted | NOT EXERCISED | source-inferred | not applicable: outside report scope | none |
| HUB-J15 | Commander application journey | Existing registered goal remains unchanged | Report/ledger-only delivery; application inputs unchanged; no runtime walk attempted | NOT EXERCISED | source-inferred | not applicable: outside report scope | none |
| HUB-J17 | Commander application journey | Existing registered goal remains unchanged | Report/ledger-only delivery; application inputs unchanged; no runtime walk attempted | NOT EXERCISED | source-inferred | not applicable: outside report scope | none |
| HUB-J18 | Commander application journey | Existing registered goal remains unchanged | Report/ledger-only delivery; application inputs unchanged; no runtime walk attempted | NOT EXERCISED | source-inferred | not applicable: outside report scope | none |
| HUB-J2 | Commander application journey | Existing registered goal remains unchanged | Report/ledger-only delivery; application inputs unchanged; no runtime walk attempted | NOT EXERCISED | source-inferred | not applicable: outside report scope | none |
| HUB-J20 | Commander application journey | Existing registered goal remains unchanged | Report/ledger-only delivery; application inputs unchanged; no runtime walk attempted | NOT EXERCISED | source-inferred | not applicable: outside report scope | none |
| HUB-J3 | Commander application journey | Existing registered goal remains unchanged | Report/ledger-only delivery; application inputs unchanged; no runtime walk attempted | NOT EXERCISED | source-inferred | not applicable: outside report scope | none |
| HUB-J4 | Commander application journey | Existing registered goal remains unchanged | Report/ledger-only delivery; application inputs unchanged; no runtime walk attempted | NOT EXERCISED | source-inferred | not applicable: outside report scope | none |
| HUB-J5 | Commander application journey | Existing registered goal remains unchanged | Report/ledger-only delivery; application inputs unchanged; no runtime walk attempted | NOT EXERCISED | source-inferred | not applicable: outside report scope | none |
| HUB-J7 | Commander application journey | Existing registered goal remains unchanged | Report/ledger-only delivery; application inputs unchanged; no runtime walk attempted | NOT EXERCISED | source-inferred | not applicable: outside report scope | none |

## Constants vs expectation

No product constants, timers or thresholds changed. The effort scale is explicitly an estimate, not a runtime timer or measured duration.

## Contradictions

No conflicting runtime state claims were introduced. Proposed/source-derived sketches are labeled, have no fake working controls, and do not claim either live product was exercised. No performance or accessibility claim is made about upstream.

## Honesty

- Original ledger grammar/count/label defects are corrected here. Earlier published bundle art-4bf4f263 is superseded by the corrected bundle; HTML art-9a0fb906 and A4 art-936a8f44 remain valid unchanged artifacts.
- No new browser run: unchanged report inputs reuse strict light/dark desktop/phone receipts, including print and JS-disabled comparison. The saved PDFs and their complete-source/bounds receipts are unchanged.
- D05 now cites the saved actual command capture and clone manifest. The no-execution assertion is not inferred solely from reading source or missing node_modules. Upstream UI/performance/accessibility remains NOT VERIFIED.
- The first transcript extraction attempt used the wrong record type and returned zero records, then failed closed. The corrected extraction uses actual `custom_tool_call` records and yields 54 requests. Workdir parsing handles quoted metadata keys; six upstream-cwd requests are preserved.
- Product tests and application journeys are not applicable to the no-code report/ledger delivery. No upstream package manager, build, scripts or tests were run.
