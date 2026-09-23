# Brief: 2026-09-23-harness-diary-report.html

Type × audience: investigation / audit × practitioner (supervisor and operator). The status/release
summary of the sweep is nested after the never-addressed evidence.

## Single idea

A third of what the harness diaries flagged for Cassy was never acted on (52 of 146 items), and a
handful of those can fail a factory silently.

## Hero form

The hero is small multiples of unit waffles on one shared unit, one waffle per harness (Claude
Code, Codex, Grok):

- Each square is one audited diary item.
- Squares fill in class order: never addressed (solid indigo, the decisive mark), then in flight
  (hatched), then addressed (hollow outline), then no longer applicable (dot).

The waffle's shape is the claim. The solid block in each panel is the unaddressed share, and it is
visible before any number is read. The shared unit keeps Grok's 72 items and Codex's 32 honest
against each other: Grok has the most never-addressed items, and the most in flight.

## Emotional register

The register is sober and accountable, earned by these choices:

- a serif verdict on the sandstone hero surface;
- one accent colour, indigo, used only for the never-addressed marks;
- no status reds above the fold, because this is an audit, not an alarm;
- counts printed as direct labels, not in a legend.

## Distinctive move

Each panel's "never addressed" count is printed as a large serif numeral directly under its solid
block. The same numerals reappear as the row-group totals in the classification table, so the
figure and the ledger read as one object.

## Deliberately omitted

- **No KPI card row** (146 / 60 / 32 / 52 in boxes). It states totals and shows no argument; the
  waffles carry the same numbers with the share visible.
- **No per-version timeline of the sweep.** Part 1 is secondary to the headline and is served by a
  compact ledger.
- **No status colour for addressed items.** Green squares would make the addressed majority the
  loudest mark on the page.
- **Petrastella tokens rather than neutral grey.** There is no white-label reason for grey.

## Critique

Renders reviewed:

- 1280×900 light and dark;
- 390×844 light and dark;
- print to A4 PDF (19 pages, figure and legend intact);
- JavaScript disabled (figure-data `details` stays open).

The receipt is `node scripts/visual-qa.mjs --strict` at 1280×800 and 390×800, in light and dark.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Serif verdict on sandstone with the indigo "52 of 146" numeral. The serif never-addressed counts under each waffle are echoed by the bold sum row of the classification ledger. |
| Fit to argument | 5 | The solid indigo block in each waffle is the unaddressed share, readable before any number. Grok's larger hatched block shows its in-flight dependence on one validation task. |
| Hierarchy | 4 | Verdict, then waffles, then the 3px verdict rule, then the ledgers. The figure-data table sits below the rule and is collapsed with JavaScript on, so it does not compete. |
| Craft | 4 | visual-qa.mjs --strict PASS: 0 findings, light and dark, desktop and phone. The first render broke narrow cells mid-word and dropped the print legend swatches; both were fixed. The long evidence tables scroll inside their container on phone. |
| Accessibility | 5 | Every text pair is from the sanctioned token set. Class and impact carry a glyph as well as colour (■ ▨ □ ·, ▲ △). The SVG has a title and desc plus a real data table. Print and JS-off lose nothing. |

Scored by factory worker true-otter-26 (task cas-df20) on 2026-09-23. The floor holds; no blind
second scorer was available.
