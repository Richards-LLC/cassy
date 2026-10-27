# Account recommendation

| Field | Decision |
| --- | --- |
| First two lines | Ask which Claude or Codex account to use; the cursor and recommended mark explain the proposed choice before Enter. |
| Scannable | Preserve every account row and the new-login row, with one recommended mark beside the eligible subscription. |
| Readable | The recommendation gives the weekly reset interval and remaining percentage; excluded rows say why automatic selection skipped them. |
| Machine output | No new picker JSON mode; factory spawn receipts include the chosen directory and reason, and Commander profiles add recommendation or exclusion text. |
| Omitted | No automatic interactive launch or hidden account; full quota details stay in `cas accounts list`. |

Design: DESIGN.md, inheriting hub-web/DESIGN.md. Existing terminal colours and selection treatment are retained.

## Critique

terminal-qa: PASS account-recommendation · 11 runs · 0 fail · 0 warn · 29 allowed

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | The prompt asks; the proposed row carries recommended and its reason. |
| Fit | 4 | The real 80-column picker has no overflow; redundant usage is omitted beside the reason. |
| Craft | 4 | Claude and Codex share the renderer and cursor calculation. |
| Theme safety | 3 | Twenty existing inquire contrast findings remain under the scoped supervisor exception below. |
| Machine contract | 4 | The real piped launch has no colour or cursor controls. |

Supervisor approval #4211116 permits only the 29 individually cited baseline
findings for this delivery: 20 existing inquire contrast findings, 8 existing
literal-action ellipsis findings, and the C-locale em dash Note. Base f45193c
and head 83f1f11 reports are retained in the cas-8523 artifacts; each exception
names its rule, location and baseline citation. New head findings still fail.
Follow-up cas-715d owns contrast and ASCII fallback; no global allowlist changed.
Full report: `terminal-qa/final/report.json`; detailed critique: `CRITIQUE.md`.
Design: DESIGN.md@b43940e43; reviewed 2026-10-27.
