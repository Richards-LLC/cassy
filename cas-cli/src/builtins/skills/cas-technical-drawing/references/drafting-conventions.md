# Drafting conventions and their checks

Each rule below is enforced by `draft.mjs` when rendering and measured by `draft.mjs check` on the emitted SVG. The check reads the file, not the generator's intent, so a hand-edited or foreign SVG is audited the same way.

## Line hierarchy (mm on paper)

| Meaning | Weight | Pattern |
| --- | --- | --- |
| Visible object edge | 0.7 | solid, round caps |
| Section outline (material cut by the plane) | 0.7 | solid; cut faces hatched at 45°, 2.5 mm pitch, 0.25 |
| Hidden edge | 0.35 | dash 1.6 / gap 0.8; drawn in orthographic views, omitted in pictorials and sections |
| Clipped end of a part on a teaching sheet | 0.35 | solid ("break", not an edge) |
| Dimension, extension, leader | 0.25 | solid; filled 3 × 1 mm arrowheads |
| Centre line, cutting-plane mark, alignment line | 0.25–0.6 | chain 6 / 1.5 / 1.5 / 1.5 |
| Sheet border | 0.5 | solid |

## Views

- Third-angle: TOP above FRONT, RIGHT SIDE to the right, aligned; the projection symbol sits beside the title block.
- Isometric: `u = (x − y)·cos 30°`, `v = (x + y)·sin 30° − z`; every axis-parallel edge lies at 30°, 150° or 90° and the same unit projects to the same length on all three axes. Faces take one neutral value per orientation (top lightest, front mid, right darkest) so the solid reads without colour; a `dark` material inverts the scale, never the rule.
- Hidden-line resolution is exact: each edge is split where other edges cross it on paper and every piece is ray-cast against the solid model toward the viewer.
- Sections keep the material behind the plane, hatch only what the plane cuts, and put "A"-labelled arrows on the parent view pointing the way the section looks. Cutting-plane marks are ends-only so they never cross a dimension.
- Exploded parts move only along their true assembly axis; dash-dot alignment lines reconnect the assembled position.
- Joint sheets show the pair separated along the assembly axis, assembled, and a hatched section through the joint with the depth chain (remaining material + cut = female thickness) and the width or tenon chain.
- Part cards show the largest face with its edge views (third angle), the L/W/T chain, and every cut's position and depth; each card states its own scale.

## Dimensions

- Outside the view, in rows 10 mm from the object then 11 mm apart; extension lines start 1.2 mm off the object and pass the last row by 2 mm.
- Unidirectional text, 3 mm high, above a horizontal line or in a break of a vertical line; a value whose span is under 8 mm gets arrows outside and its text beyond the chain end (first/last in the row) or staggered clear of neighbouring extension lines (mid-chain).
- Units once, in the title block; inch values are mixed fractions to 1/64, mm values to 0.1.
- Overall sizes on the outermost row; contiguous detail dimensions on one row form a chain that must sum to the overall spanning it.
- Balloons carry item numbers that match the parts list; leaders end in a dot on the part.

## Sheet

Letter or A4 landscape, 10 mm border, ANSI-style title block (project, sheet title, scale, units, sheet n of m, date, revision), drawn-by line, third-angle symbol, a scale bar labelled with its expected printed length, notes and the named reference on sheet 1.

## Print contract

The SVG's `width`/`height` are in mm and the viewBox is the sheet in mm, so an unscaled print is exact; PNG export is the sheet at 300 dpi. The smallest text is 2.0 mm (about 5.7 pt); dimension text is 3 mm (8.5 pt). For inline HTML use `width: 100%; height: auto` and keep one sheet per figure so the scale bar stays with its views.

## Checks and tolerances

| Check | Measures | Fails when |
| --- | --- | --- |
| `projection` | angle of every `data-axis` edge in an isometric group (or the length-weighted angle histogram of an untagged pictorial) | any tagged edge off its axis by > 0.5°; untagged: < 60% of stroke length on 30°/150°/90° |
| `axis-scale` | median mm per model unit along x, y, z | any axis differs from the view scale by > 1% |
| `proportion` | bounding box of each part's drawn lines vs its declared extent × scale | > 1% and > 0.15 mm off (parts with occluded silhouettes are reported, not judged) |
| `collision` | text vs strokes outside its own group, text vs text, numeric text inside a filled outline | any hit after a 0.3 mm inset |
| `dimensions` | label value vs measured span, from/to vs value, chain sums vs overall | > 1/128 in or 0.05 mm |
| `notes-block` | every line of a declared text block (notes, reference) against the block bounds and the scale bar | any line outside its block or touching the scale bar (the renderer never drops a line; it shrinks pitch and font to the 2.0 mm floor) |
| `text-size` | smallest text and smallest dimension text in mm on paper | < 2.0 mm / < 2.5 mm (configurable) |
| `cut-size` | part extents vs `size` | > 1/64 in or 0.5 mm |
| `joints` | stated vs derived width/depth, rabbet at edge, dado interior, through cuts, tenon shoulders | > 1/64 in; slack > 1/8 in behind the male |
| `interference` | undeclared shared volume between parts | any |
| `cut-list` | size and quantity per part name vs a cut-list JSON | any mismatch or unmapped row |

A FAIL from `cut-list`, `joints` or `interference` is often a defect in the source plan rather than the drawing; report it to the plan owner with the check line quoted rather than editing the model to hide it.
