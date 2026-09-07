# Commander fixture visual-QA CLI brief

## Concept

The command is a strict, deterministic release check for the Commander fixture
harness. It renders nine screen fixtures in both `light` and `dark` schemes at
desktop and phone widths, then emits one compact verdict while writing detailed
evidence for review.

## First two lines

PASS 9 fixtures x 2 schemes x 2 viewports
Artifacts: visual-qa.json and visual-qa.md (screenshots linked from the report)

On failure, the first line is `FAIL <finding type> <element path>` with the
fixture URL, scheme, and viewport. Subsequent lines contain one finding each.

## Scannable

- The stdout verdict leads with the matrix count and does not repeat build logs.
- Failure rows preserve the finding type, element path, fixture, scheme, and
  viewport so a reviewer can locate the defect without opening a browser.
- The Markdown report groups screenshots and finding details under stable
  fixture/scheme/viewport headings.

## Readable

Finding classes are the visual-QA vocabulary already used by the shared runner:
contrast, invisible-text, content-overflow, clipped-content,
truncated-container, print-loss, and javascript-disabled-loss. The allowlist
contains one narrowly scoped no-JavaScript harness reason; all other findings
remain strict failures.

## Machine output

The process exit code is `0` only for a strict PASS, `1` for visual findings,
and `2` for runner/configuration errors. The JSON artifact provides stable
`status`, `exitCode`, `schemes`, `viewports`, `urls`, `findings`,
`infoFindings`, `suppressed`, `screenshots`, and count fields. There is no
separate JSON stdout mode; CI consumes the exit code and the artifact path.

## Omitted

Vite build chatter, browser launch details, and raw accessibility snapshots are
kept out of the person-facing verdict. They remain available in the generated
artifacts or CI logs when debugging is required.

## Critique

Terminal QA receipt: `terminal-qa: PASS hub-web-visual-qa · 11 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-211c/terminal-qa-final4/report.json`.

Scores (1–5): Hierarchy 5 (verdict and artifact path lead); Fit 5 (failure
rows retain type/path/scheme/viewport); Craft 4 (stable JSON plus Markdown
evidence); Width 5 (80/120-column runs pass); Unicode 5 (C-locale run passes
with ASCII receipt markers).
