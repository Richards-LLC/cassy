# Accounting entries

A bookkeeping entry is a journal grid, including in CPA briefs. These are presentation and
verification rules; they do not decide an account classification or advise a posting.

## The entry block

- Heading: **date · transaction type + document number · customer/vendor name**.
- Columns: **Account | Debit | Credit**. Debit rows first, flush left; credit rows follow,
  indented. Right-align amounts; use tabular figures and two decimals. Put the currency in the
  caption/header. Leave the unused side blank; never put an account, prose, condition or
  recommendation in an amount cell.
- Use the ledger's exact account name (fully qualified for subaccounts). Show a chart-of-accounts
  number only when the saved ledger preference enables numbers and the account has `AcctNum`.
  QBO `Id` is an internal object identity: never label or display it as an account number.
  Match every displayed account against the saved chart export, not a guessed label.
- End **every** grid with debit and credit totals. Validate in exact cents **before writing** the
  report; unequal totals stop rendering. Do not rely on browser JavaScript or visual inspection.
- Put one short, single-line memo below the grid. Conditions, variants and recommendations go
  below it; distinct variants get separate small balanced grids, not prose in numeric cells.
- Label an unposted entry **Illustration**. A void is an **Illustration — reversal view**, explicitly
  not a separately posted reversing entry; QuickBooks does not create that separate entry.
- Trace each actual amount to a saved invoice/payment/journal snapshot. Hypothetical amounts
  are labelled as such. In a revision, retain the prior journal model, compare **every** displayed
  amount (including totals), and show changed, added or removed values below the grids. Carry
  forward and diff dollar figures elsewhere in the report too; the journal checker covers grids.

## Validate while rendering

Use [scripts/accounting-journals.py](../scripts/accounting-journals.py) for any report with entries.
It has no third-party dependencies and reads local saved data only. Run it before emitting the
report HTML; a nonzero exit aborts the report build. Its `json` format is a machine-readable
validation receipt, not a human report. It validates account identities, amount precision,
per-entry balance, actual-source amounts and revision diffs before opening an output file.

```bash
python3 <skill>/scripts/accounting-journals.py entries.json --format json > journal-check.json
python3 <skill>/scripts/accounting-journals.py entries.json --format markdown --output journal-grids.md
python3 <skill>/scripts/accounting-journals.py entries.json --format html --output journal-grids.html
```

Write the report Markdown first, inserting the validated Markdown grids; use the same saved model
for its static HTML grids. Copy the memo, caveats, provenance and revision changes too. These are
fragments for the report, not replacements for its verdict, brief, source Markdown or visual QA.
No content depends on JavaScript when the reader opens the HTML.

The model has `currency`, a relative `chart` path, `revision` (initially `1`) and `entries`.
For `revision > 1`, `previous` is the relative path to the saved prior model. Keep stable entry
and line `id` fields across editions; IDs are comparison keys, never chart-of-account numbers.
Each entry supplies `date` (ISO calendar date), `type`, `document`, `name`, `memo`, explicit
`posted` / `hypothetical` flags and `lines`; `void: true` marks an unposted reversal view.
Each line supplies its stable `id`, exact `account` name and a positive `debit` **or** `credit`
amount. Use decimal strings. Actual lines also supply:

```json
{"source": {"path": "saved-invoice.json", "pointer": "/TotalAmt"}}
```

The JSON Pointer must select the amount in that saved record; the checker compares it exactly.
Use the actual posting-line amount, not a transaction total that differs from that line. Never
change a source snapshot just to make validation pass. The chart snapshot is JSON with an
`accounts` list preserving `Name`, `FullyQualifiedName` when present, `AcctNum` when actually
configured, and optionally internal `Id`. Capture `account_numbers_enabled` from the company's
saved preference; omitted/unknown means names only. A stale `AcctNum` is hidden when disabled.
Normalize a CSV export into this shape without inventing names or numbers, and retain the export.

Use this CSS in the report's design language; do not encode debit/credit meaning by color:

```css
.journal-grid { width: 100%; border-collapse: collapse; }
.journal-grid th { text-align: left; }
.journal-grid td, .journal-grid thead th:not(:first-child) {
  text-align: right; font-variant-numeric: tabular-nums;
}
.journal-grid .credit th { padding-inline-start: 1.5rem; }
.journal-grid tfoot { font-weight: 700; border-top: 2px solid var(--line-strong); }
```

## Before and after: simple and compound entries

[Open the rendered example](examples/accounting-journals.html), its
[Markdown source](examples/accounting-journals.md) and
[design/proof sidecar](examples/accounting-journals.why.md).
The simple Before reproduces the rejected Credit-cell pattern from GH #1868. The compound
Before demonstrates the same failure. After grids are generated from the accompanying saved
[model](examples/accounting-journals.json) and [fictional chart](examples/accounting-chart.json),
using the same $2,250.00 amount. Both are labelled illustrations; no client posting is implied.
The chart has internal `Id` `233`, disabled numbers and a stale `AcctNum`: After displays the
account's **Hosting Income** name, never “account 233” or that disabled number.

Sources: [OpenStax, journal entries](https://openstax.org/books/principles-financial-accounting/pages/3-5-use-journal-entries-to-record-transactions-and-post-to-t-accounts),
[Intuit's Account API (`Id` and `AcctNum`)](https://developer.intuit.com/app/developer/qbo/docs/api/accounting/all-entities/account).
