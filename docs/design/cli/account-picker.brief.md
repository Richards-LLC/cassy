# Claude and Codex account picker

| Field | Contract |
| --- | --- |
| First two lines | `? Choose Claude account`, then the first account; the cursor starts on the policy's recommended row, which carries a `✓ recommended` mark. |
| Scannable | One row per usable subscription: the login email (plus its folder alias when it differs) and short state tags, then exactly one weekly line: `% left` and the reset. |
| Readable | 5-hour windows appear only as the `⚠` limit that excluded a row; unknown capacity says why (`no recent report`, `login unknown`, `until login`) and never reads as spare capacity. |
| Machine output | Pickers do not add a JSON mode; `cas accounts list --json` is the shared typed account read model with account/workspace/email-fallback identity fields. |
| Omitted | Duplicate folders of a listed identity and never-used folders (no credential, email or identity, logged out) are hidden; both stay launchable by name and listed by `--list-profiles`, which keeps full 5-hour detail. |

Exact folder aliases win before unique display-email matches. Ambiguous and unknown names refuse before provider launch or worker spawn. Only an explicit login flow creates an account folder. `main` and the active folder are never hidden; an active duplicate moves the cursor to its canonical row.

Duplicate-alias exception: only a folder with the same provider identity as a listed folder is hidden. One email that owns two subscriptions (different account or workspace IDs) is two rows, each naming its folder alias. A hidden duplicate keeps its alias as a launch key (`cas claude <alias>`).

Type-ahead: the new-login email is read through the terminal's own line discipline, not inquire, whose reader keeps keys typed after Enter in a private queue. One canonical read is one line, so an answer typed together with the email stays queued, in order, for the duplicate-account question; echo, editing, Ctrl-C and Ctrl-D (EOF cancels, also after an unfinished line) are the terminal's. The email line has no inquire styling, Esc or arrow-key editing.

Colour sits on the two one-cell marks only: `✓` green (40,145,75) and `⚠` amber (176,112,16). Selection is the `>` cursor plus bold in the default foreground, replacing inquire's cyan, which measured 3.37:1 on white and 2.48:1 on Solarized light. `NO_COLOR` or a pipe emits no SGR; a non-UTF-8 locale uses `[OK]` / `[WARN]` and no em dash. After Enter the answer line repeats only the account.

Before (operator capture, 3.86): a `main` row duplicating daniel, a never-used `grok` row, 5-hour lines above weekly ones, and the recommendation as a third detail line below the fold of the row.

## Critique

terminal-qa: PASS account-picker-claude · 11 runs · 0 fail · 0 warn · 0 allowed · cas-715d/terminal-qa/after-claude/report.json

terminal-qa: PASS account-picker-codex · 11 runs · 0 fail · 0 warn · 0 allowed · cas-715d/terminal-qa/after-codex/report.json

Before, the same fixture on installed 3.86: FAIL claude 33 and codex 41 findings (selection and prompt contrast, cancel mark, literal ellipsis).

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | The cursor and `✓ recommended` land on the policy's row; each row's first detail is weekly `% left` and reset. |
| Fit | 4 | Four two-line account rows replace six of up to four lines; every line is at most 76 cells, so the new-login row stays on screen. |
| Craft | 4 | One identity per row; 5-hour data appears only as an excluding `⚠` tag; unknown capacity names its reason. |
| Theme safety | 5 | Four palettes, NO_COLOR (no SGR), pipe (no prompt) and C locale (`[OK]`/`[WARN]`) pass for both providers with no allowances. |
| Machine contract | 5 | Selection keys are unchanged: Enter launched the preselected folder and Down+Enter the next shown folder in real PTYs; pipes keep the ambient account. |

Scored on 2026-10-28 against candidate f708a9e (sha256 65f26f5c…) with a disposable-HOME fixture that reproduces the operator's screenshot: no credential files and no provider API calls. A pane that inherited `NO_COLOR=1 TERM=dumb` gets no SGR by design; `env -u NO_COLOR TERM=xterm-256color cas claude` restores colour for that one launch.
