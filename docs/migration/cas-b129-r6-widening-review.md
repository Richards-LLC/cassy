# cas-b129 R6 widening — supervisor review artifact

Required by the **E1 ruling**: the token list and the full quarantine list must be
reviewed by a supervisor before any apply. This is the run-2 (widened) review.

Generated from a dry run against fresh `VACUUM INTO` copies of both live
databases. **No live store was read-write at any point.**

## Token list

Nine tokens added to `SUBSTRING_TOKENS` (`memory_migration/routing.rs`),
case-sensitive substring over `title` and `content` — the same match class as the
original five. The original five (`QBO`, `TNTAP`, `FONCE`, `FAE 183`,
`Journal Entr`) and the two word-tokens (`1040`, `1065`) are unchanged.

| Added token | Class |
|---|---|
| `Roark` | Entity name |
| `Realty` | Entity name |
| `JRPW` | Entity name |
| `Renovo` | Lender name |
| `Leake` | Property name |
| `Moultrie` | Property name |
| `Radnor` | Property name |
| `Old Hickory` | Property name |
| `HUD-1` | Settlement document type |

### Rejected candidates, and why

**Rejected as false-positive generators** — each matches genuine cas-src prose in
the real corpus:

| Candidate | Real cas-src string it matches |
|---|---|
| `Property` | `Object.getOwnPropertyNames(ItemsService.prototype)` (`2026-04-23-3`) |
| `Lease` | `LeaseNotFound` typed error, task cas-6cb0 (`2026-04-23-2`, `-12`) |
| `Richards` | `Richards-LLC` GitHub org / Vercel team — 10 marginal rows, **all** legitimate infrastructure memories |

**Rejected as redundant** — measured marginal contribution of **zero** rows over
the proper nouns above: `Escrow`, `Mortgage`, `Loan`, `Settlement Statement`,
`Warranty Deed`, `Quitclaim`, `Promissory`, `ALTA`, `Seller Note`,
`Center Street`, `Schedule L`, `K-1`, `1098`, `CSL`, `Ingram`, `Rearden`.

Both directions are locked by unit tests in `memory_migration/routing.rs`.

## Effect

| Measure | Run 1 (original R6) | Run 2 (widened) |
|---|---|---|
| R6 quarantined | 84 | **123** |
| Pages written | 181 (126 proj + 55 glob) | **146 (107 + 39)** |
| migrate-to-page | 160 | 125 |
| carry-verbatim (locked) | 21 | 21 |
| Loss audit | 1700/1700, unaccounted 0 | 1700/1700, unaccounted 0 |

Newly quarantined: **39**. Released from quarantine: **0**
— the widening is monotone; nothing previously protected was un-protected.

## The 39 newly-quarantined rows

Quarantine is **stay-entry in place**: these rows are not deleted and not paged.
Rows marked 🟡 are the ones worth an explicit decision — genuine cas-src memories
caught because the other project's names appear in their *bodies*.

| DB | Legacy id | Token | Title |
|---|---|---|---|
| global | `2026-03-18-1` | `Roark` | Roark Realty - PSA & Loan Details Confirmed (March 2026) |
| global | `2026-03-18-4` | `JRPW` | JRPW GP Note Details — Confirmed from Forbearance Agreement + Payoff Docs |
| global | `2026-03-18-7` | `Roark` | Roark Realty — Open Questions and Blocked Items |
| global | `2026-03-19-10` | `Roark` | Old Hickory Bargain Sale — FULL PSA DETAILS (03/19/2026) |
| global | `2026-03-19-2` | `JRPW` | 814 Ingram Note — Full Timeline Documented (Loan Extension + Payoff) |
| global | `2026-03-19-3` | `Roark` | All Institutional Loans — Complete Details from Lender Portals (03/19/2026) |
| global | `2026-03-19-4` | `Leake` | 105 Leake Ave #44 — Original Purchase Settlement (08/31/2022) |
| global | `2026-03-19-5` | `Renovo` | 202 Moultrie Park — Original Purchase HUD-1 (08/18/2022) — CSL Loan $1.33M |
| global | `2026-03-19-9` | `Roark` | Bargain Sale PSA — Friends of Radnor Lake, $1,825,000 for both OH properties |
| global | `2026-03-20-10` | `Renovo` | CSL 806 Old Hickory 1098 Data (2023-2025) |
| global | `2026-03-20-3` | `Roark` | Renovo Loans Confirmed Interest-Only — Ben 03/20/2026 |
| global | `2026-03-20-8` | `Renovo` | Session Summary — Epic cas-12b8 Completed (03/20/2026) |
| global | `2026-03-25-22` | `Renovo` | Complete 1098 / Loan Data — All Institutional Loans Confirmed |
| global | `2026-03-25-23` | `Roark` | Session 03/24-25 Complete Summary |
| global | `2026-03-30-13` | `Roark` | Roark 2022 Amendment — Authoritative Final Numbers |
| global | `2026-03-30-15` | `Roark` | Roark 2022-2023 Carryforward Verified |
| global | `2026-03-30-16` | `Roark` | Roark Property Basis and Assessor Splits |
| global | `2026-04-24-1` | `Roark` | TN franchise tax repeal (Public Chapter 950) — property measure only, not full repeal |
| project | `2026-03-18-1` | `Roark` | Roark Realty - PSA & Loan Details Confirmed (March 2026) |
| project | `2026-03-18-4` | `JRPW` | JRPW GP Note Details — Confirmed from Forbearance Agreement + Payoff Docs |
| project | `2026-03-18-7` | `Roark` | Roark Realty — Open Questions and Blocked Items |
| project | `2026-03-19-10` | `Roark` | Old Hickory Bargain Sale — FULL PSA DETAILS (03/19/2026) |
| project | `2026-03-19-2` | `JRPW` | 814 Ingram Note — Full Timeline Documented (Loan Extension + Payoff) |
| project | `2026-03-19-3` | `Roark` | All Institutional Loans — Complete Details from Lender Portals (03/19/2026) |
| project | `2026-03-19-4` | `Leake` | 105 Leake Ave #44 — Original Purchase Settlement (08/31/2022) |
| project | `2026-03-19-5` | `Renovo` | 202 Moultrie Park — Original Purchase HUD-1 (08/18/2022) — CSL Loan $1.33M |
| project | `2026-03-19-9` | `Roark` | Bargain Sale PSA — Friends of Radnor Lake, $1,825,000 for both OH properties |
| project | `2026-03-20-10` | `Renovo` | CSL 806 Old Hickory 1098 Data (2023-2025) |
| project | `2026-03-20-3` | `Roark` | Renovo Loans Confirmed Interest-Only — Ben 03/20/2026 |
| project | `2026-03-20-8` | `Renovo` | Session Summary — Epic cas-12b8 Completed (03/20/2026) |
| project | `2026-03-25-22` | `Renovo` | Complete 1098 / Loan Data — All Institutional Loans Confirmed |
| project | `2026-03-25-23` | `Roark` | Session 03/24-25 Complete Summary |
| project | `2026-03-30-13` | `Roark` | Roark 2022 Amendment — Authoritative Final Numbers |
| project | `2026-03-30-15` | `Roark` | Roark 2022-2023 Carryforward Verified |
| project | `2026-03-30-16` | `Roark` | Roark Property Basis and Assessor Splits |
| project | `2026-04-24-1` | `Roark` | TN franchise tax repeal (Public Chapter 950) — property measure only, not full repeal |
| project | `2026-04-27-6` | `Roark` | 🟡 Local Cassy project layout — 30+ projects across $HOME, identified by .cas/cas.db |
| project | `2026-04-27-7` | `Roark` | 🟡 Cassy Cloud auth env vars + cas-login wrapper |
| project | `2026-04-28-4` | `Roark` | 🟡 Ben Richards (Roark Realty managing partner) — Slack communication profile |

## Verification that this fixes the run-1 regression

Applied to the copies, then scanned **all 146 page titles** for
`roark|realty|leake|moultrie|radnor|jrpw|renovo|old hickory|hud-1|escrow|deed|
1098|1040|1065|qbo|tntap|fonce|lender|psa|assessor|ingram|bargain|carryforward`
— **no matches**.

In the SessionStart index ordering `(page_type, title, id)`, which decides what
wins the 600-token budget:

| Position | Run 1 (what the operator rejected) | Run 2 (widened) |
|---|---|---|
| 1 | 105 Leake Ave #44 — Original Purchase Settlement (08/31/2022) | 2026-07-06 triple release: team-scoping, session isolation, model steering |
| 2 | 202 Moultrie Park — Original Purchase HUD-1 — CSL Loan $1.33M | 2026-07-30 harness diary sweep completed |
| 3 | 2026-07-06 triple release… | 2026-08-06 handoff: cas-2dd1 on main @ 5cf566c4 |

The two documents the cutover was rolled back for occupied positions 1 and 2.
They are gone.
