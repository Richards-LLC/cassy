# `cas accounts list`

| Field | Contract |
| --- | --- |
| First two lines | Count the accounts and flagged entries, then identify the Claude group. |
| Scannable | Provider groups contain an account/plan/login table; each account has indented folder, usage/reset and flag rows, all within 80 columns. |
| Readable | Plain words distinguish never logged in, rejected credentials and an unknown login; unavailable usage reads `usage unknown`. |
| Machine output | `--json` returns one object with an `accounts` array of provider, name (legacy alias), folder, email, display_name, identity, plan, login_state, windows (name, used_percent, resets_at as UTC RFC3339) and flags; no credentials, banner or colour. |
| Omitted | Long human values are truncated; `cas accounts list --json` exposes every full value and reset timestamp. |

Identity uses provider account and organization/workspace IDs, falling back to email only when IDs are unavailable. Duplicate flags never cross providers. The single four-second collection deadline leaves incomplete probes and usage unknown. Existing folder aliases continue to select the same directory.
