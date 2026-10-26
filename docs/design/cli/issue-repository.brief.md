# Project issue repository reads

| Field | Contract |
| --- | --- |
| First two lines | Plain get immediately supplies the usable tracker; a filtered config list identifies its section before the selected key/value row. |
| Scannable | Section headings group config keys, `*` marks modified values, and the origin note stays adjacent to the issue repository value. |
| Readable | Modified keys and values keep their status meaning while using foreground-safe colors; the origin note explains an inferred tracker. |
| Machine output | Plain get remains owner/repo; JSON get/list/describe/search add source and configured for issues.repo while retaining existing value fields. |
| Omitted | No network lookup, persistence action, new command or unrelated config detail is added. |

## Critique

terminal-qa: PASS issue-repo-origin-merged · 12 runs · 0 fail · 0 warn · 0 allowed · &lt;artifacts&gt;/cas-828c/terminal-qa/issue-repo-origin-merged/report.json

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | The issues.repo row identifies the effective tracker and its unset origin in one scan; plain get yields the actionable scalar immediately. |
| Fit | 4 | Provenance stays adjacent to its value; the selected issues row fits both 80 and 120 columns. |
| Craft | 4 | The existing config grammar gains one muted suffix; JSON retains its value fields and uses explicit source/configured fields. |
| Theme safety | 5 | All 12 terminal-QA runs pass with zero findings across dark, light and both Solarized palettes at 80/120 columns, pipe, NO_COLOR, C locale and JSON. Existing chip foreground guard still protects saturated fill colors. |
| Machine contract | 5 | JSON parses as one document; scalar get remains exactly owner/repo; read/override/reset tests preserve config bytes. |

Scored by vivid-robin-62 on 2026-10-15 after the contrast fix. The installed
baseline and initial candidate failures remain preserved under the cas-f398
artifact root; the final no-allowlist receipt is preserved under cas-828c and
was run against merged tip ee30320db.
