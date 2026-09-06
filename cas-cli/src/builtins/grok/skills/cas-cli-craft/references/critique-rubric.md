# Critique rubric

Score every human-facing command before it merges. Five dimensions, each 1–5, the same shape as
`cas-ui-craft` so one reviewer can score both surfaces. A command merges only when **hierarchy,
fit, and craft are each ≥ 4** and no mechanical zero applies. Theme safety and machine contract
below 4 are defects fixed in the same change.

## Mechanical zeros

Any of these sets the whole score to 0 until fixed; the gate detects each one:

- a table row or any line wider than the column count (`overflow`), or a token split across
  lines (`word-split`);
- a coloured run under 3:1 for a mark or 4.5:1 for text on **any** of the dark, light, or
  Solarized palettes (`contrast`);
- an ellipsis with no `--full` escape mentioned (`truncation-without-escape`);
- glyphs or box drawing under `LC_ALL=C` (`unicode-without-fallback`), SGR under `NO_COLOR`
  (`color-under-no-color`), redraws when piped (`control-when-piped`), or anything but one
  document from `--json` (`json-contract`).

## Dimensions and anchors

| Dimension | 1 | 3 | 5 |
| --- | --- | --- | --- |
| **Hierarchy** — do the first two lines carry the verdict and the next action, and is there exactly one verdict? | the command name is line one; the verdict is a count at the bottom | verdict first, but remedies are buried in prose or repeated per finding | glyph + word + number first, remedy as one command, receipts last, nothing competes |
| **Fit** — does the shape match how the reader uses it: scan, read, or pipe? | key/value dump for a scan task; paragraph for a warning; JSON with a banner | right shape, wrong grain (every healthy check gets a row; every instance gets a paragraph) | healthy groups collapse to a line, findings expand, repeats collapse to a count, JSON is one document |
| **Craft** — alignment, units, wrapping, width, consistency across commands | ragged columns, units per cell, wraps at 80, two heading styles | aligned and fitted with one visible seam (a wrapped remedy, a unit twice) | right-aligned numbers, one unit header, hanging indents, identical grammar to sibling commands |
| **Theme safety** — colour survives light, dark, Solarized, `NO_COLOR`, 16-colour | whole lines coloured; near-white text on the light palette | marks coloured correctly with one weak pair on one palette | gate passes four palettes; monochrome output reads identically |
| **Machine contract** — `--json`, exit codes, piped behaviour | `--json` mixed with log lines; progress redraws in a pipe | clean JSON but human render derived separately and drifting | one document, stable fields, exit code is the verdict, piped output appends |

## Procedure

1. Run the gate: `node scripts/terminal-qa.mjs --label <command> [--json-flag --json] -- <command …>`.
   Read `report.md`; fix every finding or allowlist it with a reason a reviewer would accept.
2. Open the 80-column HTML capture and read it on the light and the dark palette; then read the
   `.txt` capture as a pipe consumer would.
3. Score each dimension against the anchors with one sentence of evidence naming a line of
   output. "Looks clean" is not evidence.
4. Append the receipt line and the score table to the brief under `## Critique`.
5. If a floor fails, change the brief's *first two lines* or *scannable* field first, re-render,
   re-score. Two failed rounds means the command is doing two jobs; split it.

## Common failures and the dimension they hit

- Command name as the first line → hierarchy 2.
- One paragraph per warning, repeated per instance → fit 2, craft 2.
- Every healthy check on its own row → fit 3.
- Whole-line status colour → theme safety 1 (and usually a contrast zero on light).
- Right-aligned labels with left-aligned values (`        Project: …`) → craft 2.
- Sentence that describes a command instead of printing it → hierarchy 3.
- Banner or progress on stdout under `--json` → machine contract 1 (zero).
