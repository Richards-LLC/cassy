# Accounting entries in client reports

Use this branch when the report contains bookkeeping entries. It governs presentation and
verification, not account classification or advice about posting.

1. Resolve the installed `cas-html-reports` skill directory. Follow its
   [journal contract and saved-model schema](../../cas-html-reports/references/accounting-entries.md),
   including the chart export, saved source records and prior model for a revision.
2. Validate before writing report output; use the installed validator, not a browser balance check:

   ```bash
   python3 <cas-html-reports-dir>/scripts/accounting-journals.py <entries.json> --format json > <journal-check.json>
   ```

   A nonzero result stops the build. Render Markdown/HTML fragments from that same saved model
   with the validator's `--format markdown|html --output <file>` modes; do not retype amounts.
3. Keep one entry block per date/type/document/customer-or-vendor, with Account | Debit | Credit,
   debit lines before indented credits, right-aligned two-decimal amounts, declared currency and
   balanced totals. Use exact saved account names and only enabled, exported account numbers;
   an internal object ID is not an account number. Numeric cells contain amounts only.
4. Put the short memo, conditions, variants and recommendations below each grid. Label unposted
   entries as illustrations, give each variant a balanced grid, and disclose when a void's reversal
   view is illustrative rather than a separately posted entry.
5. Keep saved provenance or an explicit hypothetical label for every amount. Retain the previous
   model and disclose every changed, added or removed figure outside the grids, including totals.
   Reconcile figures elsewhere in the report too; the journal validator covers grid amounts.
