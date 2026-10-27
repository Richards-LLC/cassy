# Claude and Codex account picker

| Field | Contract |
| --- | --- |
| First two lines | Identify the provider and offer distinguishable email/alias choices before asking the operator to launch. |
| Scannable | Each row names the login email and, when different, its actual folder alias in parentheses; login and usage follow. |
| Readable | Ambiguous emails name their aliases; unknown names suggest an existing alias and the explicit login command. |
| Machine output | Pickers do not add a JSON mode; `cas accounts list --json` is the shared typed account read model with account/workspace/email-fallback identity fields. |
| Omitted | Credentials and implicit folder creation are absent; full folder paths remain available through list-profiles and accounts list JSON. |

Exact folder aliases win before unique display-email matches. Ambiguous and unknown names refuse before provider launch or worker spawn. Only an explicit login flow creates an account folder. Existing recommendation/disabled marks and picker cursor policy are owned by their separate lanes.

## Critique

terminal-qa: PASS claude-list-profiles · 11 runs · 0 fail · 0 warn · 0 allowed · cas-1e30/terminal-qa/claude-list-profiles/report.json

terminal-qa: PASS codex-list-profiles · 11 runs · 0 fail · 0 warn · 0 allowed · cas-1e30/terminal-qa/codex-list-profiles/report.json

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | The launch syntax is first; displayed emails resolve to real accounts and the aliases are explicit. |
| Fit | 4 | Same-email rows differ by alias; interactive choice text reserves alias/login space before capacity. |
| Craft | 4 | Picker text is bounded to 76 cells on the fixture; full expiry details occupy their own bounded listing lines. |
| Theme safety | 5 | Four palettes, NO_COLOR, pipe and C locale pass for both providers; 80-column captures were reviewed. |
| Machine contract | 5 | Shared registry identity is typed; exact aliases precede unique email matches and ambiguous/unknown names refuse. |

Scored by jolly-cardinal-26 on 2026-10-27 at 5a50c3c79. Both real PTY pickers showed distinct aliases, and Down+Enter selected the alt folder. Full capacity copy is available with --list-profiles; an expired window never retains a percentage as current. The prior 93-column Codex listing now fits 80 columns. Exact delivery-tip captures are refreshed in task artifacts after this documentation commit.
