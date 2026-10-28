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

## Round 2: idle and repeated accounts

Rolled-over capacity windows count as unused; a missing weekly reset is eligible
behind known weekly resets. Unknown live usage and unusable authentication remain
excluded. Duplicate folders say `duplicate of <alias>` and only the canonical
folder can be recommended. A row outside the anchored reset group says
`resets later than <recommended>`. Each row stays manually selectable. No machine
schema or interaction changes. Commander profile probing can still take up to
four seconds (QA F02, deferred by supervisor #4211163).

## Missing reset metadata

A reported nonzero percentage stays authoritative when its reset time is
missing: 59% used means 41% left, and 95% used is excluded at the 90% threshold.
Only a missing/zero percentage without a reset is unopened; a past timestamp
still proves rollover. A losing unknown weekly reset says `weekly reset unknown;
known resets first`. The short reason fits the existing bounded picker row.

## Chooser presentation (2026-10-28)

The recommendation policy and its reasons are unchanged; the terminal chooser
now presents them through [account-picker.brief.md](account-picker.brief.md).
The recommended row shows a `✓ recommended` mark and its weekly line instead of
a third reason line. Duplicate and never-used folders are hidden from the
chooser rather than labelled. Comparative reasons (`resets later than`,
`known resets first`) are implied by the weekly lines and no longer printed;
threshold exclusions remain as a `⚠` tag. Commander profile text is unchanged.
