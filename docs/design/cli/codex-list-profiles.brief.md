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

Baseline `9b1c55f3`: `terminal-qa: FAIL codex-list-profiles · 11 runs · 8 fail · 0 warn · 0 allowed`.
The four 80-column palette captures each contain a 100-cell header and a
90-cell folder line; the 120-column captures fit.

Treatment `e608e09f4`: `terminal-qa: PASS codex-list-profiles · 11 runs · 0 fail · 0 warn · 0 allowed`.
The same fixture now fits 80 columns in every palette, with complete login and
active state. NO_COLOR, C locale and pipe remain plain. `--full` preserves
the exact long alias and folder; the adjacent account JSON retains the alias.

Captures and build receipts: `<artifacts>/cas-d2fe/terminal-baseline/`,
`terminal-qa/`, `baseline-RECEIPT.json`, and `candidate/RECEIPT.json`.
The 80-column HTML was visually inspected in all four palettes.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Launch syntax leads, followed by the detected profiles and their states. |
| Fit | 4 | Each profile keeps a header and folder; only long displayed values shrink. |
| Craft | 4 | Header states remain whole, folders stay indented, and the full-value escape is explicit. |
| Theme safety | 5 | Four palettes, NO_COLOR, C locale and pipe pass without SGR or new colors. |
| Machine contract | 4 | Listing remains plain text; the existing account JSON and full listing preserve exact aliases. |
