# Init wordmark and frame

| Field | Contract |
| --- | --- |
| First two lines | Init identifies Cassy before presenting the existing configuration choice; this locale fix preserves the wizard's entry hierarchy. |
| Scannable | Styled UTF-8 terminals retain the compact six-row splash and frame; C, legacy, pipe and NO_COLOR destinations receive the Cassy label and ASCII rules. |
| Readable | The existing welcome description and configuration choices keep their wording, indentation and 80-column fit. |
| Machine output | `cas init --json` retains its existing result document and bypasses this human splash; this change adds no JSON fields or status codes. |
| Omitted | Decorative blocks, box glyphs and bullet/arrow marks are omitted from non-styled or non-UTF-8 output; user-supplied Unicode content is retained. |

Use the same styled-output plus UTF-8 predicate as root help. Keep the raw
clap command's default banner ASCII so alternate help entry paths cannot skip
the locale policy. Limit changes to wordmark selection and init's text writer.

## Critique

Candidate `a8c1d0041a4ff2fb397338ed5a10d613a0150d61`,
`cas 3.71.0 (a8c1d00 2026-10-22)`, SHA-256
`d63e0630d816cc0b0d1868c978b6ebb093dc65387cb9bb892735938e1065de58`.
Actual binary, isolated project/home/config; stdout retains the gate's TTY or
pipe. Closed stdin stops at the existing-configuration choice before writes.
The final error in each capture is this deliberate EOF, not an init failure.

- `terminal-qa: PASS cas-init-utf8-scoped · 11 runs · 0 fail · 0 warn · 100 allowed`
- `terminal-qa: PASS cas-init-c-scoped · 11 runs · 0 fail · 0 warn · 40 allowed`
- `terminal-qa: PASS cas-root-help · 11 runs · 0 fail · 0 warn · 0 allowed`

These are scoped locale/fit passes, with an explicit existing palette defect.
Unfiltered init reports FAIL with 100 UTF-8 / 40 C contrast findings. All
findings are contrast; no locale, width, NO_COLOR or pipe findings. The narrow
allowlist matches 34 exact measured contrast tuples, citing `cas-f15f`.
Init colors and welcome source are byte-identical to base `343c75d1e`.
The task requires styled UTF-8 appearance unchanged, so palette repair remains
in that separate task. These receipts do **not** establish full theme safety.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Cassy identifies the welcome; existing-config status leads directly to one numbered choice. |
| Fit | 5 | Every init capture is at most 76 cells at both 80 and 120; ASCII collapses the logo to one line. |
| Craft | 4 | Six-row UTF-8 logo and frame retained; C uses aligned ASCII rules with readable Cassy text. |
| Theme safety | 1 | Light and Solarized-light contrast fails; tracked as `cas-f15f`, allowed only for this locale scope. |
| Machine contract | 4 | NO_COLOR has no SGR; piped welcome has no redraws. Unchanged JSON initialization was not exercised. |

Read the 80-column dark/light/Solarized HTML captures and PNGs, and the plain
pipe capture. Scoped review passes; full-command review retains the theme
failure. Task artifacts hold raw reports, allowed reports, exact exceptions,
source equality proof, candidate receipt, ledger and PNGs. Adjacent
[UTF-8](init-wordmark.utf8.capture.txt) and
[ASCII](init-wordmark.ascii.capture.txt) captures pin the actual welcome text.
