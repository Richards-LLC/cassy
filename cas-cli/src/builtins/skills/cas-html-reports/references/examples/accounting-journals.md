# Read the entry in one pass.

The same amounts, laid out as entries a CPA can scan. Both examples are hypothetical illustrations.

## Simple invoice

### Before — rejected Credit-cell pattern

| Debit | Credit |
| --- | --- |
| Accounts Receivable $2,250.00 | Hosting Income $2,250. A prepaid line can credit Deferred revenue. |

The old “account 233” label confuses an internal QBO object identity with a chart-of-accounts number.
This Before reproduces the failure described in the issue, not a client screenshot.

### After — validated illustration

### 2026-10-31 · Invoice INV-17 · Example Customer

Illustration — hypothetical amounts · USD

| Account | Debit | Credit |
| --- | ---: | ---: |
| Accounts Receivable | 2,250.00 |  |
| &emsp;Hosting Income |  | 2,250.00 |
| **Total** | **2,250.00** | **2,250.00** |

Illustrative hosting invoice.

Source: Hypothetical illustration

## Compound receipt

### Before — the same presentation failure

| Debit | Credit |
| --- | --- |
| Bank $2,190.00 plus Processing Fees $60.00 | Accounts Receivable $2,250.00 |

This counterexample demonstrates the failure; it is not a historical client entry.

### After — validated illustration

### 2026-11-01 · Payment PAY-17 · Example Customer

Illustration — hypothetical amounts · USD

| Account | Debit | Credit |
| --- | ---: | ---: |
| Bank | 2,190.00 |  |
| Processing Fees | 60.00 |  |
| &emsp;Accounts Receivable |  | 2,250.00 |
| **Total** | **2,250.00** | **2,250.00** |

Illustrative receipt after a processing fee.

Source: Hypothetical illustration

## Source and validation

The saved model is accounting-journals.json; the saved fictional chart is accounting-chart.json.
The chart has account numbers disabled. Its internal Id 233 and stale AcctNum 4000 are not account labels.
Each entry balances to USD 2,250.00; amounts, names, dates and document identities are illustrative.
Recommendations and alternative postings belong below an entry, never in its numeric cells.
The Before Credit-cell text comes from the reported failure; all After grids come from the validator.
