# `cas codex --list-profiles`

| Field | Contract |
| --- | --- |
| First two lines | Show the existing launch syntax, then identify detected Codex profiles. |
| Scannable | Each profile keeps one header with its login and active state, followed by its folder and any usage detail. |
| Readable | Headers and folders fit 80 columns by shortening long displayed identities; login and active words remain complete. |
| Machine output | This listing remains plain text; `cas accounts list --json` is the existing structured account read model. |
| Omitted | Long labels and folders are shortened with ASCII `...`; `cas codex --list-profiles --full` exposes their exact values. |

The existing global `--full` flag is the escape for this listing. Its normal
layout stays within 80 columns even on a 120-column terminal. Aliases and
profile folders remain unchanged; shortening affects display only. Codex
currently has no proven non-secret email source, so its display label normally
equals the folder alias. Do not infer an email from credentials to reproduce
the historical email-plus-alias presentation.

## Critique

Pending exact-build baseline and treatment captures at 80/120 columns,
four palettes, C locale, pipe and NO_COLOR. No terminal PASS is claimed yet.
