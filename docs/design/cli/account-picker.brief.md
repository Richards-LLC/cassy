# Claude and Codex account picker

| Field | Contract |
| --- | --- |
| First two lines | Identify the provider and offer distinguishable email/alias choices before asking the operator to launch. |
| Scannable | Each row names the login email and, when different, its actual folder alias in parentheses; login and usage follow. |
| Readable | Ambiguous emails name their aliases; unknown names suggest an existing alias and the explicit login command. |
| Machine output | Pickers do not add a JSON mode; `cas accounts list --json` is the shared typed account read model with account/workspace/email-fallback identity fields. |
| Omitted | Credentials and implicit folder creation are absent; full folder paths remain available through list-profiles and accounts list JSON. |

Exact folder aliases win before unique display-email matches. Ambiguous and unknown names refuse before provider launch or worker spawn. Only an explicit login flow creates an account folder. Existing recommendation/disabled marks and picker cursor policy are owned by their separate lanes.
