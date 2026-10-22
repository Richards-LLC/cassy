# Root help wordmark

| Field | Contract |
| --- | --- |
| First two lines | Root help identifies Cassy with its locale-appropriate wordmark; existing command usage follows, without inventing a health verdict. |
| Scannable | Keep the existing compact UTF-8 wordmark on styled terminals, with a single ASCII Cassy label on plain or non-UTF-8 destinations. |
| Readable | Command descriptions and option usage remain the existing root help, fitted to 80 and 120 columns. |
| Machine output | Help is usage text even with --json; it never promises a JSON document. This change adds no machine fields. |
| Omitted | Block and box glyphs are omitted in C, legacy non-UTF-8, pipe and NO_COLOR output; no new banner or decoration replaces them. |

Use the existing locale policy (`LC_ALL`, then `LC_CTYPE`, then `LANG`) and
the existing output-mode detection. Limit the change to wordmark selection.

## Critique

Supervisor candidate `d2b71707852ab01fbe1898e70b4712360d66384d`, version
`cas 3.71.0 (d2b7170 2026-10-22)`, SHA-256
`d81b24e2cc5160d55e4f6839e53df0f5fafefa97d3fede6c383067cbf4e60e52`.
The project-scoped `cas-b596` task artifacts contain the six-cell real-build
`LEDGER.md`, raw terminal captures, four dark/light screenshots and receipts.

| Receipt | Runs | Failures | Warnings |
| --- | ---: | ---: | ---: |
| `terminal-qa: PASS root-help-UTF8` | 11 | 0 | 0 |
| `terminal-qa: PASS root-help-C` | 11 | 0 | 0 |
| `terminal-qa: PASS root-help-legacy` | 11 | 0 | 0 |
| `terminal-qa: PASS root-help-gc-C` | 11 | 0 | 0 |

All gates exercised 80/120 columns and four palettes, with no allowlists.
Raw assertions confirm C/ISO-8859-1 ASCII output, retained UTF-8 styled art,
and ASCII NO_COLOR/pipe output. The same C-locale gate using the prior real-build
candidate failed once on U+2588/U+2557; treatment has zero findings. That
baseline predates the exact merge base; the same named Rust regression also
ran red before the locale guard and green after it.

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 4 | Locale-appropriate identity precedes the unchanged usage and command list; this focused fix preserves the established UTF-8 help hierarchy. |
| Fit | 4 | Command rows remain scannable at both widths; C and legacy output replace six decorative rows with Cassy. |
| Craft | 4 | 80-column dark/light screenshots have no horizontal clipping, with clientWidth=scrollWidth=650; command and option indentation is retained. |
| Theme safety | 5 | All four palettes and NO_COLOR pass without findings; no new color or glyph meaning was introduced. |
| Machine contract | 5 | Piped help is plain ASCII with no cursor controls or project creation; help remains usage text and adds no JSON contract. |

The six runtime cells pass. No init, cleanup, integration or credential
operation was needed. Initial test-import and build-admission failures remain
recorded separately from the executed behavioral red/green evidence.
