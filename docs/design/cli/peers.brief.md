# Peer awareness

| Field | Contract |
| --- | --- |
| First two lines | The caller's registration state and focus open the view, then the peer/live count starts the next group; repository scope follows. |
| Scannable | Each supervisor groups machine, session, epic focus, claims, and live workers with branch and task rows. |
| Readable | Claims use plain task IDs and owner names; synced titles/status explain the work, and missing titles explicitly say they are not synced. Claim age is time since renewal. |
| Activation | For this registration, show Cloud's strict-activation state counts after the repository identity. `activation` in JSON retains the original binding and fence receipt; an unavailable read is named and never gates claims or peer discovery. No local activation predicate or cache is added. |
| Machine output | `cas peers --json` returns one object with `canonical_id`, `peers`, and `detail`; top-level `cloud_presence` holds the typed last recorded state, `cloud_presence_message` the current display (including opt-out), `epic_splits` the recorded splits with stored `state` and effective `display_state` (including expiry), and `mailbox_status` only mailbox health; each detail peer contains `supervisor`, `user_id`, `team_id`, `project_id`, `claims_visibility`, `branch`, `claims`, and `workers`; claims expose plain `task_id`, raw `lock_key`, `agent_id` and `agent_name`. Missing synced titles, statuses, or branches are null; claims visibility is `same_account`, `team_project`, `different_account` or `unknown`. |
| Omitted | Human output fits 72 cells, ordered presence, peers/detail, splits, mailbox; JSON preserves full values. Locally registered agents are excluded from peers even in a plain terminal. Workers are read-only observations and never become message or spawn targets. |

## Critique

terminal-qa: PASS peers · 12 runs · 0 fail · 0 warn · 0 allowed · <artifacts>/cas-559c/terminal-qa/peers/report.json

terminal-qa: PASS peers-long · 12 runs · 0 fail · 0 warn · 0 allowed · <artifacts>/cas-559c/terminal-qa/peers-long/report.json

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Presence is first as required by the supervisor; peer/live count follows, then scope and details, recorded splits, with mailbox last. |
| Fit | 4 | Supervisor groups separate identity, claims and live workers; the summary omits expanded work. |
| Craft | 4 | Claim ages align right with one seconds heading; presence, splits and mailbox use shared renderers; all lines fit 72 columns and long values name the JSON escape. |
| Theme safety | 5 | Both gates pass dark, light and Solarized, NO_COLOR and C locale without colors or controls. |
| Machine contract | 5 | One typed document preserves raw lock keys and owner IDs; split raw/effective states and shares are structured, mailbox health separate; failure JSON is valid and exits nonzero. |

Scored by golden-gazelle-73 on 2026-10-13 at f0b866541. Captures use the actual built CLI with an isolated fake cloud and local supervisor/split records; independent final production re-check belongs to subtle-cobra-75.

### Activation addition (cas-f82cb)

terminal-qa: PASS peers-activation · 12 runs · 0 fail · 0 warn · 0 allowed · <artifacts>/cas-f82cb/terminal-qa/peers-activation/report.json

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Peer verdict and repository remain the opening lines; Slack activation follows the scope it describes. |
| Fit | 4 | One state-count line summarizes the registration; full bindings and fence receipt remain in JSON. |
| Craft | 4 | Existing 72-cell renderer bounds and sanitizes the added line. |
| Theme safety | 5 | Twelve gate captures pass; dark/light inspection shows identical legible text and no new color. |
| Machine contract | 5 | The actual built CLI returns one JSON document with the original activation receipt. |

Captures use the CLI built by the capped integration test at 99e3a387f and an isolated local Cloud fixture, never a live Slack read. The renderer and CLI inputs are unchanged by the subsequent body adapter.

Published evidence: `art-0abf9796` (`cas-f82cb`).
