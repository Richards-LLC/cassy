# Patch proposal for the Woodworking project — dartboard cabinet drawings

Scope: `projects/dartboard-cabinet/` and the project-local `shop-drawings` skill. Nothing in the Woodworking repo was modified; this proposal is for the plan owner.

## 1. Replace hand-drawn figures with generated sheets

- Add `projects/dartboard-cabinet/drawing-model.json` (copy of `dartboard-cabinet.json` here) as the single geometry source; regenerate with `draft.mjs render drawing-model.json --out drawings/ --png` after every plan change and run `draft.mjs check drawing-model.json --cutlist cutlist.json` in the guide's verification checklist. The check's summary line is the collision-gate evidence the shop-drawings skill already asks for, without `data-collision="allow"` budgets.
- In `build-guide.html`, replace the "Finished piece", "Orthographic set", "Part drawings" and the carcass rabbet / divider dado / back groove / door stub-tenon / drawer corner joinery figures with the generated sheets inlined (`<svg>` with `width`/`height` in mm, `style="width:100%;height:auto"`). Keep the guide's prose, cut sequences, right/wrong pairs and machine-setup figures — the renderer does not draw those.
- Keep the operator's grid convention where the reader counts squares: pass `--grid` (¼ in minor, 1 in major on orthographic views). The scale bar on every sheet states the printed length at 1:N, which is the guide standard's own requirement.

## 2. Plan corrections the model surfaced

| Item | README / cut list says | Consequence | Proposed change |
| --- | --- | --- | --- |
| Drawer divider depth | 9-1/4 in, full depth | interpenetrates the sliding 1/4 in back (back sits at 1/4…1/2 in from the rear) | rip the divider to 8-3/4 in (front plane to back face, the depth the README already uses for the drawer bay), or cut it the same 1/4 × 1/4 back groove as top and bottom and say so in the carcass section |
| Drawer box front/back length | 26 in "between sides; gives 27-1/2 outside width" | true only for a butt joint; the decided locking rabbet puts a 3/8 in tongue into each side | 26-3/4 in; update `cutlist.json` and re-run the board layout (poplar 1×6 stock has the length) |
| Drawer-lock geometry | side groove 1/4 W × 3/8 D; front tongue "3/8-thick retained" | a 3/8 in tongue cannot enter a 1/4 in groove | state one of: 1/4 in tongue into a 1/4 in groove (drawn here), or a 3/8 in groove; the joint-library plate should be the source and the README should quote it verbatim |

## 3. shop-drawings skill amendments (project-local)

- Replace the "generate from 3D coordinates" recommendation with a requirement: pictorials come from `draft.mjs` or an equivalent committed generator; hand-authored `d=` parallelograms are not accepted for any view labelled isometric.
- Replace `check-pictorial.py` and `check-collisions.py` invocations with `draft.mjs check drawing.svg --print-width-mm <figure width>`; it measures the same angle histogram (the router-table page reads 17°/−30° under it too), plus text-in-outline, text-over-stroke, dimension chains and print text size, and needs no allow budget.
- Adopt the likeness critique from `cas-technical-drawing/references/likeness-critique.md` as the "cold look" step the skill already describes, with its scorecard and floor.

## 4. Drawing-layout items still owed by the renderer

- Dowels and pocket screws are schematic (square-section holes, symbol only); hinge cups, LED channel and slide hardware are not modelled — add them as `overlays` on the relevant views if the guide needs them located.
