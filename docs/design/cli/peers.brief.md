# Peer awareness

| Field | Contract |
| --- | --- |
| First two lines | The count of visible supervisors and live supervisors comes first, followed by the repository scope. |
| Scannable | Each supervisor groups machine, session, epic focus, claims, and live workers with branch and task rows. |
| Readable | Claim titles and locally synced task status explain the held work; claim age is time since the last renewal. |
| Machine output | `coordination action=peers json=true` and `cas peers --json` return one object with `canonical_id`, `peers`, and `detail`; each detail peer contains `supervisor`, `user_id`, `team_id`, `project_id`, `claims_visibility`, `branch`, `claims`, and `workers`. Missing synced titles, statuses, or branches are null; claims visibility is `same_account`, `different_account` or `unknown`. |
| Omitted | Human output truncates long values to 80 cells; JSON preserves full values. Workers are read-only observations and never become message or spawn targets. |

## Critique

terminal-qa: PASS peers · 12 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a/cas-559c/terminal-qa/peers/report.json

terminal-qa: PASS peers-long · 12 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a/cas-559c/terminal-qa/peers-long/report.json

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | Line one states one peer and one live; line two names the repo scope. |
| Fit | 4 | Supervisor groups separate identity, claims and live workers; the summary omits expanded work. |
| Craft | 4 | Claim ages align right with one seconds heading; long values truncate with a JSON escape. |
| Theme safety | 5 | Both gates pass dark, light and Solarized, NO_COLOR and C locale without colors or controls. |
| Machine contract | 5 | One typed document carries full values, visibility and identity; failure JSON is valid and exits nonzero. |

Scored by golden-gazelle-73 on 2026-10-13 at fc7f32515. Captures use the actual built CLI with an isolated fake cloud; production proof is cas-c7c7.
