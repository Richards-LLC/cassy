# Claude and Codex account picker

| Field | Contract |
| --- | --- |
| First two lines | `? Choose Claude account`, then the first account; the cursor starts on the policy's recommended row, which carries a `✓ recommended` mark. |
| Scannable | One row per usable subscription: the login email (plus its folder alias when it differs) and short state tags, then exactly one weekly line: `% left` and the reset. |
| Readable | 5-hour windows appear only as the `⚠` limit that excluded a row; unknown capacity says why (`no recent report`, `login unknown`, `until login`) and never reads as spare capacity. |
| Machine output | Pickers do not add a JSON mode; `cas accounts list --json` is the shared typed account read model with account/workspace/email-fallback identity fields. |
| Omitted | Duplicate folders of a listed identity and never-used folders (no credential, email or identity, logged out) are hidden; both stay launchable by name and listed by `--list-profiles`, which keeps full 5-hour detail. |

Exact folder aliases win before unique display-email matches. Ambiguous and unknown names refuse before provider launch or worker spawn. Only an explicit login flow creates an account folder. `main` and the active folder are never hidden; an active duplicate moves the cursor to its canonical row.

Colour sits on the two one-cell marks only: `✓` green (40,145,75) and `⚠` amber (176,112,16). Selection is the `>` cursor plus bold in the default foreground, replacing inquire's cyan, which measured 3.37:1 on white and 2.48:1 on Solarized light. `NO_COLOR` or a pipe emits no SGR; a non-UTF-8 locale uses `[OK]` / `[WARN]` and no em dash. After Enter the answer line repeats only the account.

Before (operator capture, 3.86): a `main` row duplicating daniel, a never-used `grok` row, 5-hour lines above weekly ones, and the recommendation as a third detail line below the fold of the row.

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
