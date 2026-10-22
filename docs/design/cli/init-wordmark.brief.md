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
the locale policy. R1 changed wordmark selection and the init text writer; R2 extends the same policy to the actual spinner and quick-start table.

## R1 critique — superseded coverage

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


## R2 complete-init evidence

Candidate `9e9210dd9693a2571716e68e0fc8e2dfa7e6d615`,
`cas 3.71.0 (9e9210d 2026-10-22)`, SHA-256
`3f7651748a0f79e53b850a809a29b344d73521ee00d802695357c95f20e3c4a5`.

The initial EOF-at-existing-config walk missed spinner and quick-start output.
R2 selects ASCII spinner frames and `Border::Ascii` whenever the shared styled
UTF-8 predicate is false, including the first frame with animation disabled.
The existing init regression now renders the actual quick-start table and
checks both missed decorations in the locale/mode cases. It fails before the
fix; root help remains covered by its existing adjacent regression.

Real TTY QA completes both `--yes --no-integrations` and interactive selection
1 (reconfigure), then `Proceed` confirmation, at 80/120 under C and UTF-8.
Each run reads back configuration, the SQLite tasks table, hooks and MCP files.
28 C/plain raw ANSI captures are ASCII, including overwritten spinner frames.
Four UTF-8 old/new ANSI comparisons are byte-identical: 6262 bytes for `--yes`
and 15470 for interactive, at both widths. The preserved baseline is ancestor
build `a8c1d0041`, not an asserted exact immediate-base build.

- `terminal-qa: PASS r2-yes-utf8-scoped · 11 runs · 0 fail · 0 warn · 269 allowed`
- `terminal-qa: PASS r2-yes-c-scoped · 11 runs · 0 fail · 0 warn · 249 allowed`
- `terminal-qa: PASS r2-interactive-utf8-scoped · 11 runs · 0 fail · 0 warn · 473 allowed`
- `terminal-qa: PASS r2-interactive-c-scoped · 11 runs · 0 fail · 0 warn · 413 allowed`

These passes use explicit supervisor approvals 4206333 and 4206384 for four
inherited findings: palette `cas-f15f`, factory-tooling overflow `cas-0ba9`,
piped phase redraws `cas-3075`, and the exact unchanged progress sentence
`Reconfiguring Cassy...` (with two leading spaces) falsely reported as
truncation (`cas-592b`).
All 190 exact distinct exception entries and their tasks are listed in the
ledger. Unfiltered reports remain FAIL; no blanket visual-quality pass is
claimed. The old 76-cell maximum applied only to R1's incomplete entry walk;
the full flow exposes an inherited 112-cell UTF-8 / 115-cell C tooling row.

| Dimension | Full-wizard score | Evidence / retained issue |
| --- | --- | --- |
| Hierarchy | 4 | Existing-config choice leads to one confirmation, then completed initialization and quick-start commands. |
| Fit | 2 | Changed logo/table fit 80; inherited tooling row overflows under approved `cas-0ba9` exception. |
| Craft | 3 | ASCII spinner/table are coherent; pre-existing Next steps box alignment remains unchanged. |
| Theme safety | 1 | Fixed palette still fails light themes; approved `cas-f15f` exception. |
| Machine contract | 3 | Full C/plain output is ASCII; inherited piped redraws remain under `cas-3075`. JSON unchanged, not exercised. |

The whole wizard remains below craft floors due to retained inherited issues;
this delivery proves the approved locale repair and UTF-8 preservation.
Inspected actual C80 dark/light PNGs and plain captures for both paths.
[Defaults capture](init-full-yes.ascii.capture.txt) and
[interactive capture](init-full-interactive.ascii.capture.txt) retain full text.

## Follow-up contract (cas-8bc8)

The Next steps box derives one width from its actual title and content; every
border and row shares that width and fits 80 columns. The command wording and
styled UTF-8 bytes outside this box stay unchanged. With all three locale
variables absent or empty, use the same ASCII decorations as C; explicit UTF-8
keeps the existing styled output. Published captures use `<project>` for the
project path so they remain portable. Full real-TTY defaults and interactive
reconfigure/confirm flows must complete with durable files at 80/120 columns.

## Follow-up evidence (cas-8bc8)

Candidate `cd6681c37336946636a544283025bc30265f41e2`, `cas 3.71.0 (cd6681c 2026-10-22)`,
SHA256 `88ce51031aa702b1b6fa2b3bd981b7e8b579562413c7d9a223f14e0d9aa243cc`. Real TTY defaults, fresh interactive and
existing-config reconfigure/Proceed flows finish at80/120. Defaults/reconfigure
also run underC,UTF8 and all3locale variables absent. Readbacks confirm config,
tasks,hooks andMCP;77final init captures have8aligned61cell box rows.
All tested C/plain/unset rawANSI is ASCII. Six styledUTF8 before/after captures
are byte-identical outside the permitted box; no other normalization is used.
The4published init captures use `<project>` rather than a machine scratch path.

- terminal-qa: PASS yes-utf8-scoped · 11 runs · 0 fail · 0 warn · 269 allowed · &lt;artifacts&gt;/cas-8bc8/terminal-qa/yes-utf8-scoped/report.json
- terminal-qa: PASS yes-c-scoped · 11 runs · 0 fail · 0 warn · 249 allowed · &lt;artifacts&gt;/cas-8bc8/terminal-qa/yes-c-scoped/report.json
- terminal-qa: PASS yes-unset-scoped · 11 runs · 0 fail · 0 warn · 249 allowed · &lt;artifacts&gt;/cas-8bc8/terminal-qa/yes-unset-scoped/report.json
- terminal-qa: PASS fresh-utf8-scoped · 11 runs · 0 fail · 0 warn · 457 allowed · &lt;artifacts&gt;/cas-8bc8/terminal-qa/fresh-utf8-scoped/report.json
- terminal-qa: PASS interactive-utf8-scoped · 11 runs · 0 fail · 0 warn · 473 allowed · &lt;artifacts&gt;/cas-8bc8/terminal-qa/interactive-utf8-scoped/report.json
- terminal-qa: PASS interactive-c-scoped · 11 runs · 0 fail · 0 warn · 413 allowed · &lt;artifacts&gt;/cas-8bc8/terminal-qa/interactive-c-scoped/report.json
- terminal-qa: PASS interactive-unset-scoped · 11 runs · 0 fail · 0 warn · 413 allowed · &lt;artifacts&gt;/cas-8bc8/terminal-qa/interactive-unset-scoped/report.json
- terminal-qa: PASS root-help-unset · 11 runs · 0 fail · 0 warn · 0 allowed · &lt;artifacts&gt;/cas-8bc8/terminal-qa/root-help-unset/report.json

These passes retain only the4exact inherited exception sets and tasks already
approved4206333/4206384, reaffirmed4206624. All190entries remain byte-identical.
The ledger preserves the automation-path mismatch and obsolete analysis-count
failure; final reports pass without changing any allowance. Unfiltered reports
remain FAIL. No whole-wizard visual-quality or JSON pass is claimed.

| Dimension | Full-wizard score | Evidence / retained issue |
| --- | --- | --- |
| Hierarchy | 4 | Existing choices lead to Proceed, completed initialization and copyable commands. |
| Fit | 2 | Assigned box fits80/120 at61cells; inherited tooling row still overflows undercas-0ba9. |
| Craft | 4 | All8box rows/borders share one edge; documented project paths are portable. |
| Theme safety | 1 | Inherited light-palette contrast remains undercas-f15f; UTF8 colors unchanged. |
| Machine contract | 3 | C/unset/plain are ASCII; pipeCR remains undercas-3075. JSON not exercised. |

Changed-box geometry scores5; full-wizard floors remain below target because of
explicit inherited exceptions. Inspected80column dark/light PNGs and plain text
for all3locales and both interactive entries. Task ledger lists every exception,
binary identity, retained failures, readbacks and final reports. Targeted Rust
proof is bound to the final documentation tip separately.

Published evidence: `art-9712ab8d` (`cas-8bc8`).

## Palette contract (cas-f15f)

| Field | Contract |
| --- | --- |
| First two lines | The compact Cassy welcome introduces the existing-config choice without changing its wording or ordering. |
| Scannable | The six-row logo, frame, section rules and aligned guidance keep their geometry; bold provides hierarchy without assuming a dark background. |
| Readable | Descriptions, paths, commands and table text inherit the terminal foreground; only short semantic status marks receive palette-safe color. |
| Machine output | Init JSON retains one result document and bypasses the splash; NO_COLOR and pipes retain the same readable text without SGR. |
| Omitted | No hardcoded bright body, logo or border colors; locale fallback and compact dimensions remain covered by existing controls. |

Real-build gates must pass all four palettes at80/120, C and NO_COLOR with
all cas-f15f contrast allowances removed. The three previously approved exact
exceptions for cas-0ba9, cas-3075 and cas-592b remain scoped to their findings.

## Palette evidence and critique (cas-f15f)

Candidate `f3aecb67acffb7284fe33cefee0dbe6789c48071`,
`cas 3.71.0 (f3aecb6 2026-10-22)`, SHA256
`2f6ca7525f2b8824779d911d46c7028cb2518f0a6e36bba41b19900c787a39bf`.

Eight real-build gates pass: defaults and reconfigure under UTF8/C/unset,
fresh interactive confirmation under UTF8, and adjacent unset-locale help.
Each gate covers80/120, four palettes, NO_COLOR, pipe and C. All178cas-f15f
exceptions are removed; zero contrast findings remain, including allowed
findings. Same-command baseline reports fail256contrast defaults /452contrast
reconfigure. Source-equivalent ancestor buildcd6681c supplies the baseline.
105durable init readbacks confirm config/store/tasks/hooks/MCP;22init children
and11help children have all3locale variables absent. All tested raw C/plain/
unset output is ASCII. Six UTF8 defaults/fresh/reconfigure ×80/120 comparisons
preserve visible text and layout byte-for-byte after stripping SGR only.

Body, paths, commands, logo and rules now use terminal foreground. Bold
provides logo/header hierarchy. Only short UTF8 marks use mid-tones, measured
minimum3.71:1 success,3.30:1 warning,3.59:1 error; ASCII status words stay in
default foreground. The quick-start palette override belongs only to init.

- terminal-qa: PASS fresh-utf8-scoped · 11 runs · 0 fail · 0 warn · 13 allowed · &lt;artifacts&gt;/cas-f15f/terminal-qa/fresh-utf8-scoped/report.json
- terminal-qa: PASS interactive-c-scoped · 11 runs · 0 fail · 0 warn · 21 allowed · &lt;artifacts&gt;/cas-f15f/terminal-qa/interactive-c-scoped/report.json
- terminal-qa: PASS interactive-unset-scoped · 11 runs · 0 fail · 0 warn · 21 allowed · &lt;artifacts&gt;/cas-f15f/terminal-qa/interactive-unset-scoped/report.json
- terminal-qa: PASS interactive-utf8-scoped · 11 runs · 0 fail · 0 warn · 21 allowed · &lt;artifacts&gt;/cas-f15f/terminal-qa/interactive-utf8-scoped/report.json
- terminal-qa: PASS root-help-unset · 11 runs · 0 fail · 0 warn · 0 allowed · &lt;artifacts&gt;/cas-f15f/terminal-qa/root-help-unset/report.json
- terminal-qa: PASS yes-c-scoped · 11 runs · 0 fail · 0 warn · 13 allowed · &lt;artifacts&gt;/cas-f15f/terminal-qa/yes-c-scoped/report.json
- terminal-qa: PASS yes-unset-scoped · 11 runs · 0 fail · 0 warn · 13 allowed · &lt;artifacts&gt;/cas-f15f/terminal-qa/yes-unset-scoped/report.json
- terminal-qa: PASS yes-utf8-scoped · 11 runs · 0 fail · 0 warn · 13 allowed · &lt;artifacts&gt;/cas-f15f/terminal-qa/yes-utf8-scoped/report.json

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Existing-config choice leads to bold confirmation, then completed init and copyable commands. |
| Fit | 2 full /5 welcome | Compact welcome/frame and61cell box fit80/120; inherited tooling row overflow retains exact cas-0ba9 exceptions. |
| Craft | 4 | Consistent default text, bold hierarchy, aligned borders and short colored status marks. |
| Theme safety | 5 | All4palettes pass with0contrast findings and no cas-f15f exceptions; actual before/after PNGs inspected. |
| Machine contract | 3 | NO_COLOR/C/unset clean; inherited9piped redraws remain under cas-3075. JSON unchanged, not exercised. |

The12retained exact exception entries cover only cas-0ba9, cas-3075 and the
literal two-space `Reconfiguring Cassy...` truncation false positive cas-592b.
The ledger lists every entry and preserves baseline FAIL reports. No blanket
full-wizard fit/machine pass is claimed. Corrected screenshot capture waits
for fonts and a2000px viewport; original paint/crop artifacts are retained.
No CLI gate was replayed for screenshot correction. [Actual welcome/choice
capture](init-welcome.palette.capture.txt) uses a portable project placeholder.
Task artifact LEDGER.md and qa/pairs hold the complete real-build evidence;
final-tip Rust receipts are recorded separately.

Published palette evidence: `art-01be6bdf` (`cas-f15f`).
