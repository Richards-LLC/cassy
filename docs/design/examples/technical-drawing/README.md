# Technical drawings — before / after on the dartboard cabinet

The Woodworking project's dartboard-cabinet build guide draws its elevations and joinery by hand as inline SVG. This directory re-generates the same object with the vendored `cas-technical-drawing` skill (`scripts/draft.mjs`) from one parts-and-joints model, keeps the originals for comparison, and records what the mechanical checks and the likeness critique measured on each.

| | Before (hand-authored, `projects/dartboard-cabinet/build-guide.html`) | After (`dartboard-cabinet.json` → `draft.mjs`) |
| --- | --- | --- |
| Source of geometry | `d=` strings and `<rect>`s typed per view | 32 parts placed once in a world frame; 40 joints derived from part overlap |
| Views | closed/open front elevations, side + plan, flat part cards, joint "separated/assembled" boxes | third-angle set with isometric inset (closed and doors-open variants), true 30° isometric, two hatched sections, exploded assembly with alignment lines, 12 joint teaching sheets (separated / assembled / dimensioned section), 8 part-card sheets, parts list — 26 sheets |
| "30° isometric" | an oblique: 39 % of stroke length is horizontal, 53 % vertical, 6 % at 150° (`before/check-output.txt`) | every axis-tagged edge at 30°/150°/90° within 0.5°, axis scales equal to 0.1 % |
| Dimension text | values written inside the parts (`9-1/4″ W × 3/4″ T`), labels over outlines and arrows | outside the views in stacked rows; chains sum to their overall (checked); tight values pushed beyond the chain end |
| Smallest print text | 1.04 mm (part card) and 0.49 mm (rabbet teaching sheet) at the guide's 180 mm figure width | 2.0 mm floor, 3 mm dimension text, letter sheet at a stated 1:N |
| Title block, scale bar, item balloons, cutting-plane marks | none / grid-square counting | on every sheet |

## Files

- `before/` — the two operator screenshots (`cabinet-elevations.png`, `cabinet-joinery.png`), five figures extracted from the build guide as standalone SVG, and `check-output.txt` from `draft.mjs check <svg> --print-width-mm 180`.
- `dartboard-cabinet.json` — the model, written from `projects/dartboard-cabinet/README.md` and `cutlist.json`.
- `after/` — all 26 sheets as SVG (`dartboard-cabinet-<kind>.svg`) with 150-dpi PNGs of the key sheets, the doors-open orthographic variant, the caption-free isometric used for the likeness test (`dartboard-cabinet-iso-plain.*`, `likeness-thumb-200px.png`), and `check-output.txt` from `draft.mjs check dartboard-cabinet.json --cutlist …`.
- `likeness.md` — the cold-look answer, feature point-check and scorecard.
- `woodworking-patch-proposal.md` — what to change in the Woodworking project and the two source-plan findings the model surfaced.

## Reproduce

```bash
S=cas-cli/src/builtins/skills/cas-technical-drawing/scripts/draft.mjs
cd docs/design/examples/technical-drawing
node $S check dartboard-cabinet.json --cutlist /path/to/Woodworking/projects/dartboard-cabinet/cutlist.json
node $S render dartboard-cabinet.json --out after --png
node $S render dartboard-cabinet.json --out after --png --variant doors-open --only ortho
node $S render dartboard-cabinet.json --out after --plain --only iso && rsvg-convert -w 200 after/dartboard-cabinet-iso-plain.svg -o after/likeness-thumb-200px.png
```

## What the checks say

`after/check-output.txt` ends with two FAIL lines. Both are findings about the source plan, not the drawing, and are left standing on purpose:

1. **Drawer divider** — the cut list's 9-1/4 in deep divider collides with the sliding 1/4 in back (the back passes behind the divider at y = 1/4…1/2). The model uses 8-3/4 in, the same "front plane to back face" depth the README states for the drawer bay; the README specifies neither a divider groove nor the shorter rip.
2. **Drawer box front/back** — a 26 in front/back placed between two 3/4 in sides makes a 27-1/2 in box only as a butt joint. With the decided locking rabbet (3/8 in tongue into each side) the front/back must be 26-3/4 in; the drawer bottom's 26-7/16 in already assumes the 27-1/2 in box.

Every other line is PASS: 32 parts match their cut sizes, 40 joints match their stated width and depth, no undeclared interference, and on all 26 sheets projection, axis scale, proportion, dimension chains and text size pass. The shipped example `examples/shelf-box.json` passes clean (`ALL CHECKS PASS`).
