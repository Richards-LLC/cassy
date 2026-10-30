# `cas claude --list-profiles` and `cas codex --list-profiles`

| Field | Contract |
| --- | --- |
| First two lines | Show the provider's existing launch syntax, then identify its detected profiles. |
| Scannable | Each profile keeps one header with its login and active state, followed by its folder and any usage detail. |
| Readable | Headers and folders fit 80 display cells, including CJK and emoji identities; login and active words remain complete. |
| Machine output | Both legacy listings remain plain text under `--json`, which preserves exact values like `--full`; `cas accounts list --json` is the structured account read model. |
| Omitted | Long labels and folders use ASCII `...`; only a shortened listing shows the provider's `--list-profiles --full` escape. |

The existing global `--full` flag is the escape for both listings. Their normal
layout stays within 80 columns even on a 120-column terminal. Aliases and
profile folders remain unchanged; shortening affects display only. Codex
currently has no proven non-secret email source, so its display label normally
equals the folder alias. Do not infer an email from credentials to reproduce
the historical email-plus-alias presentation.

## Critique

### Provider parity follow-up

Baseline `2404ec18`: the same fixtures at 80 and 120 columns show Claude
headers of 196 (ASCII), 242 (CJK), and 202 (emoji) cells. Codex fits but wastes
display cells on multibyte names; it also prints the full-value hint for short
names, and its `--json` listing still truncates values. Baseline captures live
under `<artifacts>/cas-f972/baseline/`, with `baseline-RECEIPT.json` binding the
supervisor-built candidate. The historical critique below describes the prior
Codex-only change.

### Prior Codex-only change

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
