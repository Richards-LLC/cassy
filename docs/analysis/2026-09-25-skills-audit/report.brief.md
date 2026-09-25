# Brief: report.html (skills & prompts audit 2026-09, operator decision brief)

Markdown source: `SYNTHESIS.md` in this directory, at `5108c1de8`. Lane reports: `L1-findings.md` through `L6-findings.md`.
Cell: decision brief × executive/operator audience. The hero shows where the P0s come from, and the
closing sections are the ask (Wave A) and the decisions (D1–D13).

## Single idea

Twenty of the 27 ways Cassy misleads its agents are instructions its own code rejects. The fix is a
test that checks prose against the schema, not a rewrite of the skills.

## Hero form

The hero is a unit chart: one square per P0, 27 squares, grouped in rows by root-cause theme. The
T5 row ("prose not tied to what the code accepts") holds 20 squares in verdict indigo, and every
other row holds one or two muted squares. The row lengths make the argument before any label is
read: one theme is a long bar of units and the rest are stubs. A plain count would say "20" but
would not show that the other themes are marginal. The T3 (budget) row sits in the figure empty on
purpose, because budget overruns are P1s: they cost tokens, not correctness. The empty row answers
the obvious "isn't size the problem?" question without any prose.

## Emotional register

Calm and decided. The choices behind that:

- a sandstone hero surface with a serif verdict and no status colours above the fold;
- one indigo row, with everything else in ink-muted;
- the ask worded as a plain imperative ("Approve Wave A").

## Distinctive move

Each square is labelled with its master ID in mono, so the hero figure also serves as the index. The
27-row P0 ledger further down repeats the same IDs in the same theme order, and each ledger group is
headed by its row of squares from the hero. A reader can go from a square to its file:line without
a legend.

## Deliberately omitted

- **No KPI card row** (27 P0 / 39 P1 / 16 WPs / 13 decisions in boxes). Four numbers in boxes carry
  no argument. The counts appear once, in the provenance line.
- **No table of contents or navigation band.** The page is read top to bottom: verdict, ask,
  budget, decisions, evidence.
- **No per-lane breakdown chart.** Lane is an organisational fact, not a cause. Lanes appear only
  as links on each ledger row.
- **No P1/P2 detail.** It lives in `SYNTHESIS.md`. The page links to it rather than re-rendering
  80 rows.

## Theme assignment used by the hero (derived; the findings are unchanged)

Each P0 takes the first `SYNTHESIS.md` §3 theme that names it. Where §3 names it under two themes,
it takes the one describing the defect rather than a consequence (for example, M08 is named in T5 as
"names a non-existent script" and in T2 as a consequence, so it counts as T5). Where §3 does not
name it, it takes the theme matching the mechanism stated in its §2.1 row.

| Theme | P0 master IDs | Count |
| --- | --- | ---: |
| T5 Prose not tied to what the code accepts | M02 M03 M04 M07 M08 M09 M10 M11 M12 M13 M14 M15 M16 M17 M18 M20 M22 M23 M25 M26 | 20 |
| T6 Operator / cas-src content ships everywhere | M06 M19 | 2 |
| T1 Tool-prefix model | M01 | 1 |
| T2 Install lifecycle only ever adds | M05 | 1 |
| T4 Rules maintained as hand copies | M21 | 1 |
| T7 Wording lags model guidance | M24 | 1 |
| T8 Stores carry other projects' facts | M27 | 1 |
| T3 No owner for the always-loaded budget | — (its findings are P1: M30–M35, M62) | 0 |
| **Total** | | **27** |

Mechanism-assigned rows are the ones §3 does not name:

- M09: the prompt contract disagrees with the parser, so T5.
- M14, M15, M18, M20, M22, M25: the text disagrees with the code or the tool, so T5.
- M21: two copies of the format rules disagree, so T4.

## Critique

Scored on the `cas-ui-craft` rubric after the final render (visual-QA run `final`).

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | A serif verdict with an indigo italic clause sits on sandstone. The hero units are labelled with master IDs, and the same tile rows head each ledger group, so the figure doubles as the index. It is not a 5 because the section forms below the fold (ledger, dot plot, decision list, timeline) are house-standard. |
| Fit to argument | 5 | The row lengths are the claim: one long indigo row of 20 against stubs of 1–2 and an empty budget row. The sentence only names what the shape already shows. |
| Hierarchy | 4 | At 1280×800 and 390×844 the verdict and the complete figure are the only things above the 3px rule. The two-line sub-paragraph under the verdict takes a little weight from the figure on the phone. |
| Craft | 5 | `node scripts/visual-qa.mjs --strict` → **PASS**: light and dark, 1280 and 390, 0 findings, 0 allowlisted. The one-decimal share shows 99.9% instead of a misleading 100%. Tables stack into labelled rows at ≤820px instead of scrolling sideways. |
| Accessibility | 4 | Contrast passes in both schemes (strict PASS). A JS-disabled print render keeps all 5 sections, 27 P0 rows, 13 decisions and every tile (the page has no JS). Every figure has an aria text alternative and a real data table. Caveat: the stacked phone tables label cells with CSS `::before` text, and screen-reader support for that varies. |

Scored by calm-owl-92 on 2026-09-25; the floor holds (distinctiveness, fit and hierarchy are all ≥ 4, and no dimension scored 0).
Receipt: `~/.cas/artifacts/cas-248d7/visual-qa-final/visual-qa.md`, with the JSON, four screenshots and `print.pdf` beside it.
The committed copy is `report.visual-qa.md`.
