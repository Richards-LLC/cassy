---
name: cas-technical-drawing
description: Use when a project needs a technical or shop drawing of a physical object — an orthographic set, isometric, section, exploded view, joinery detail, part card, parts list or DXF for a build guide, plan, or review; renders every view from one parts-and-joints model with `scripts/draft.mjs`, enforces drafting conventions in code, and gates the result with mechanical checks plus a likeness critique.
managed_by: cas
---

# Technical drawings

Never draw geometry by hand. Every view is a projection of one solid model; correctness is measured, not eyeballed. A page titled "30° isometric" once shipped with 17° axes, 25% mismatched axis scales and its fence lying flat — rules with no check are decoration.

## Steps

1. **Write the concept brief into the model, before any render.** In `model.json` (schema: [model-schema.md](references/model-schema.md)) record the builder's contract: overall envelope, every part with its finished `size` and world placement, every joint with its stated width and depth, the assembly order (`explode`), the named visual `reference` with 3–5 identity features, and the print contract (`sheet`, `units`). Done when `node <skills-dir>/cas-technical-drawing/scripts/draft.mjs check model.json` reports the model checks (`cut-size`, `joints`, `interference`, and `cut-list` when a cut list exists) and every FAIL is either fixed or written down as a source-plan finding.
2. **Render the set.** `draft.mjs render model.json --out drawings/ --png` writes an orthographic sheet (third-angle FRONT/TOP/RIGHT with an isometric inset, balloons, cutting-plane marks), an isometric sheet, one sheet per `sections` entry, an exploded sheet, one teaching sheet per distinct joint (separated / assembled / dimensioned section), part cards, and a parts list. Add `--variant name` for configurations (doors open), `--only ortho,iso,joints` to subset, `--grid` for a ¼-unit counting grid, `--dxf` for outline export. Done when every sheet exists and `draft.mjs check model.json` ends with `ALL CHECKS PASS` for the drawing checks (`projection`, `axis-scale`, `proportion`, `collision`, `dimensions`, `text-size`).
3. **Choose views by what each must teach**, and drop the rest:

   | Question the reader has | View |
   | --- | --- |
   | What am I making, how big is it? | isometric sheet, overall dimensions |
   | Where does every part go, what are the controlling sizes? | orthographic set + variant sheets |
   | What is hidden inside (grooves, webs, buried parts)? | section through the relationship |
   | In what order and along which axis does it go together? | exploded sheet |
   | How do I cut this joint? | joint teaching sheet |
   | What do I cut, from what, how many? | part cards + parts list |

4. **Run the likeness critique** ([likeness-critique.md](references/likeness-critique.md)) on the isometric: render `--plain`, thumbnail it at 200 px, give a zero-context agent only the image and ask "what is this object?", then point to each identity feature in the pixels and score silhouette, proportion and part identification. Done when the score meets the floor and the scorecard is saved beside the drawings.
5. **Deliver the print contract.** Sheets are letter or A4 landscape in millimetres at a stated 1:N scale; embed the SVG inline (it carries `width`/`height` in mm) or ship the 300-dpi PNG. Commit `model.json` beside the drawings and treat it as the source of truth; a change to the object is a change to the model followed by a re-render, never an edit to the SVG.

## Conventions the renderer enforces

Line weights (object 0.7 mm, hidden 0.35 dashed, dimension/extension 0.25, centre 0.25 chain, section outline 0.7 with 45° hatch), third-angle placement, dimensions outside the view in stacked rows with gapped extension lines and filled arrowheads, unidirectional text, tight values pushed clear of every extension line, units stated once in the title block, a scale bar with its expected printed length, one value per face orientation in pictorials, alignment chain lines on exploded parts, item balloons that match the parts list. The rationale and each check's tolerance are in [drafting-conventions.md](references/drafting-conventions.md).

## Auditing a drawing you did not generate

`draft.mjs check drawing.svg --print-width-mm 180` measures any SVG: the length-weighted angle histogram of a claimed isometric, text crossing strokes or other text, dimension values written inside an outline, and the smallest text height at the stated print width. Use it to review hand-drawn build-guide figures before rework; quote its summary line in the review.
