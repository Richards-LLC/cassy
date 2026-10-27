# `cas accounts list`

| Field | Contract |
| --- | --- |
| First two lines | Count the accounts and flagged entries, then identify the Claude group. |
| Scannable | Provider groups contain an account/plan/login table; each account has indented folder, usage/reset and flag rows, all within 80 columns. |
| Readable | Plain words distinguish never logged in, rejected credentials and an unknown login; unavailable usage reads `usage unknown`. |
| Machine output | `--json` returns one object with an `accounts` array of provider, name (legacy alias), folder, email, display_name, identity, plan, login_state, windows (name, used_percent, resets_at as UTC RFC3339) and flags; no credentials, banner or colour. |
| Omitted | Long human values are truncated; `cas accounts list --json` exposes every full value and reset timestamp. |

Identity uses provider account and organization/workspace IDs, falling back to email only when IDs are unavailable. Duplicate flags never cross providers. The single four-second collection deadline leaves incomplete probes and usage unknown. Existing folder aliases continue to select the same directory.

## Critique

terminal-qa: PASS accounts-list · 12 runs · 0 fail · 0 warn · 0 allowed · cas-1e30/terminal-qa/accounts-list/report.json

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Account and finding counts lead; provider heading is line two. |
| Fit | 4 | Provider groups, email rows and short indented usage/reset rows; full values have one JSON escape. |
| Craft | 4 | Text columns align, usage numbers align right, maximum fixture line is 65 columns. |
| Theme safety | 5 | Four palettes, C locale, NO_COLOR and pipe pass; all meaning is plain text. |
| Machine contract | 5 | JSON is one typed accounts object, preserving absolute reset instants and full folder values. |

Scored by jolly-cardinal-26 on 2026-10-27 against 722bf2d27. The 80-column HTML was visually inspected on dark, light and both Solarized palettes; `accounts-list.capture.txt` is the pipe capture. Exact delivery-tip evidence is refreshed in the task artifact directory after this documentation commit.
