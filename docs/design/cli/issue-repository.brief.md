# Project issue repository reads

| Field | Contract |
| --- | --- |
| First two lines | Plain get immediately supplies the usable tracker; grouped views expose whether origin supplied an unset setting. |
| Scannable | The existing config key/value rows add an adjacent origin note; doctor promotes the unset setting to a warning. |
| Readable | The origin note explains why the repository is usable while config remains unset; doctor names the effective tracker or the explicit-setting remedy. |
| Machine output | Plain get remains owner/repo; JSON get/list/describe/search add source and configured for issues.repo while retaining existing value fields. |
| Omitted | No network lookup, persistence action, new command or unrelated config detail is added. |

## Critique

terminal-qa: PASS issue-repo-origin · 12 runs · 0 fail · 0 warn · 12 allowed · /home/pippenz/.cas/artifacts/cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a/cas-f398/terminal-qa/issue-repo-origin/report.json

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | The issues.repo row identifies the effective tracker and its unset origin in one scan; plain get yields the actionable scalar immediately. |
| Fit | 4 | Provenance stays adjacent to its value; the selected issues row fits both 80 and 120 columns. |
| Craft | 4 | The existing config grammar gains one muted suffix; JSON retains its value fields and uses explicit source/configured fields. |
| Theme safety | 4 (scoped) | No added contrast finding across four palettes, NO_COLOR, pipe and C locale; twelve unchanged baseline findings are tracked by cas-828c and narrowly allowlisted. |
| Machine contract | 5 | JSON parses as one document; scalar get remains exactly owner/repo; read/override/reset tests preserve config bytes. |

Scored by calm-viper-79 on 2026-10-15 for the origin-provenance change. The
installed baseline and initial candidate failures are preserved under the task
artifact root; this receipt does not claim the existing theme defect is fixed.
