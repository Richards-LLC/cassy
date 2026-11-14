# Why it works — accounting-journals.html

Type: accounting presentation example. Audience: practitioner.
Design: DESIGN.md@b43940e432, inheriting hub-web/DESIGN.md@3577bff4a1.

## Plan alignment

GH #1868 requests readable simple and compound journal blocks, real account identities and
pre-render balance validation. The simple Before quotes the rejected Credit-cell pattern from
that issue; the compound Before is a labelled counterexample. Neither is a client screenshot.
The After grids are actual renderer output from the saved, explicitly hypothetical model and
fictional chart. This example teaches presentation; it does not choose a client's posting.

| Criterion | Evidence | Assessment |
| --- | --- | --- |
| Dated type/document/name heading; Account/Debit/Credit; debit then indented credit | Both generated After blocks | Met |
| Two-decimal, aligned amounts, totals and one-line memo | Rendered simple/compound grids; validator rejects imbalance before output | Met |
| Exact ledger names; no internal Id used as account number | Saved chart has disabled numbers and a stale AcctNum; After uses names | Met |
| Illustrations, void views, saved sources and revision changes | Accounting reference and executable validator; behavior fixtures | Met |
| A short entry contract and a before/after a CPA can read | Skill section, linked reference, this page and its Markdown source | Met |

## Concept brief

- Single idea: the entry is legible when each posting gets its own row.
- Form: paired rejected/validated journal tables; the column geometry exposes the difference.
- Register: a composed workbook, with no accounting recommendation hidden in a number column.
- Distinctive move: indented credit names and a strong, matching totals rule make both sides
  reconcile at a glance. The second example repeats it for a compound entry.
- Omitted: charts and KPI cards that add nothing to this accounting representation; private
  client names, exports and screenshots. At phone width, Before and After stack in reading order.

## Reproduce the data view

```bash
python3 <skill>/scripts/accounting-journals.py <skill>/references/examples/accounting-journals.json --format json
python3 <skill>/scripts/accounting-journals.py <skill>/references/examples/accounting-journals.json --format markdown
python3 <skill>/scripts/accounting-journals.py <skill>/references/examples/accounting-journals.json --format html
```

The HTML page composes those validated fragments with the rejected/counterexample cells from
its Markdown source. Its inline CSS, fonts and mark resolve the repository DESIGN.md through
`cas deliverable brand`. The report reads offline with no JavaScript.

## Critique

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Ruled postings and visibly indented credits carry the workbook form; the mark and type use the project brand |
| Fit to argument | 5 | The same amounts become separate balanced rows; Before and After show the exact readability problem |
| Hierarchy | 4 | One verdict leads, then simple and compound comparisons, then source detail; phone keeps each pair in order |
| Craft | 4 | Strict visual QA PASS: desktop/phone in light/dark, zero findings; all four captures inspected |
| Accessibility | 5 | Semantic headers/captions, tabular numbers, print and JS-disabled content retained; no color-only meaning |

The small visual-QA receipt is beside this file. Full captures and a print/JS-disabled preview
are retained in the delivery's proof bundle, not installed as skill assets.
