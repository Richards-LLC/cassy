# model.json — the drawing model

One JSON file describes the object; every sheet is derived from it. A complete small model is `examples/shelf-box.json` (two sides, a rabbeted top, a dadoed shelf, one section). Dimensions are numbers or fraction strings (`"9-1/4"`, `"3/8"`, `"17/32"`); in `mm` models use plain numbers.

## World frame

`x` runs left→right (width), `y` runs back→front (depth; the front face has the larger `y`), `z` runs up (height). The FRONT view looks toward −y, TOP looks down, RIGHT SIDE looks toward −x; the isometric shows the top, front and right faces with +x at 30°, +y at 150° and +z vertical on paper.

## Top level

```json
{
  "project": "Dartboard cabinet", "title": "Dartboard cabinet",
  "units": "in", "sheet": "letter", "date": "2026-09-06", "revision": "B", "author": "who or what",
  "reference": { "path": "reference/plan.png", "features": ["tall shallow wall box 30×38×9-1/4", "two overlay Shaker doors", "..."] },
  "notes": ["FRAMELESS: DOORS OVERLAY THE CARCASS", "..."],
  "parts": [ ... ], "joints": [ ... ], "sections": [ ... ], "dims": [ ... ],
  "overlays": [ ... ], "variants": { ... }, "explode": { ... }, "grid": false
}
```

`sheet` is `letter`, `a4`, `tabloid` or `a3` (landscape). `reference.features` are the 3–5 things a stranger would use to name the object; they are printed on sheet 1 and scored by the likeness critique.

## Parts

```json
{ "id": "side-l", "name": "Carcass side", "qty": 1, "material": "poplar 1x10",
  "size": ["38", "9-1/4", "3/4"], "at": ["0", "0", "0"], "dims": ["3/4", "9-1/4", "38"],
  "tone": "light", "group": "carcass", "cutlist": "Carcass side", "notes": "",
  "cuts": [ { "type": "notch", "name": "wire pass", "at": [x, y, z], "dims": [dx, dy, dz] } ] }
```

- `size` is the finished cut size `[L, W, T]`; `dims` are the part's world extents `[dx, dy, dz]` from its minimum corner `at`. The `cut-size` check requires the sorted values to agree.
- Parts with the same `name` share an item number and a parts-list row; `qty` on one entry lets a drawn piece stand for several identical pieces that are not modelled.
- `tone` is `light` (default), `mid` or `dark` and sets the neutral value scale in pictorials; assign it from the real material (black film-faced ply is `dark`).
- `cuts` are explicit subtractive boxes for features no joint produces (a notch, a hole modelled as a square pocket). Joints add their own cuts.

## Joints

Joints are derived from where placed parts overlap, so position the male `depth` into the female and declare the joint; the female receives the cut.

```json
{ "type": "rabbet", "female": "side-l", "male": "top", "width": "3/4", "depth": "3/8", "occurs": "4 corners" }
{ "type": "dado",   "female": "side-l", "male": "rail-upper", "width": "3-1/2", "depth": "3/8", "run": "y" }
{ "type": "groove", "female": "stile-l1", "male": "panel-l", "depth": "3/8", "run": "z" }
{ "type": "tenon",  "female": "stile-l1", "male": "rail-l-top", "tenon_thickness": "1/4", "depth": "3/8" }
{ "type": "dowel",  "parts": ["a", "b"], "axis": "x", "diameter": "3/8", "depth": "1", "at": [[x, y, z]] }
{ "type": "pocket-screw", "from": "a", "into": "b", "axis": "x", "angle": 15, "count": 2 }
```

- `rabbet`, `dado`, `groove`, `housing`: the cut is the overlap box. `run: "y"` extends it through the female along that axis (a through dado or a full-length groove). A stated `depth` larger than the male's penetration deepens the cut and records the clearance (a ¼-in groove holding a panel that sits 7/32 in). `axis` overrides the inferred depth axis; `entry` overrides the assembly axis used by exploded and teaching sheets.
- `tenon` (stub or full): the tenon is the overlap narrowed to `tenon_thickness` (and `tenon_width` if given); the male loses its shoulders, the female gains the mortise.
- `dowel` adds square-section holes in both parts (drawn hidden); `pocket-screw` is a drawing symbol only.
- `occurs`, `note`, `label` are printed on the joint's teaching sheet; `detail: false` suppresses that sheet.

Checks: `joints` compares stated width/depth with the overlap, rejects a rabbet that is not at an edge or a dado that is, and flags a cut through the female's full thickness. `interference` flags any two parts that share volume without a declared joint or explicit cut.

## Sections, dimensions, overlays, variants, explode

```json
"sections": [ { "name": "A", "axis": "x", "at": "15", "look": "-", "title": "on the centreline",
                "dims": [ { "axis": "z", "from": "0", "to": "3/4", "row": 1, "side": "left" } ] } ]
"dims": [ { "view": "front", "axis": "x", "from": "1/16", "to": "14-15/16", "row": 1, "side": "bottom", "variant": "doors-open", "label": "optional text" } ]
"overlays": [ { "view": "front", "variant": "doors-open", "type": "circle", "center": ["15", "21-13/16"], "r": "8-7/8", "label": "DARTBOARD Ø 17-3/4 (REF)" },
              { "view": "top", "type": "label", "at": [x, y], "text": "LED CHANNEL", "leader_to": [x, y] } ]
"variants": { "doors-open": { "omit": ["stile-l1", "..."], "move": { "door-l": [dx, dy, dz] } } }
"explode": { "side-l": [-6, 0, 0], "top": [0, 0, 5] }
```

- A section keeps the material on the side the viewer looks from (`look: "-"` puts the viewer on the +axis side looking toward −); cut material is hatched, and the cutting-plane marks appear on the orthographic sheet.
- `dims` are extra dimensions on a named view; the overall sizes are added automatically on the outermost row. Contiguous dimensions on one row form a chain that the `dimensions` check sums against the overall.
- `explode` vectors are explicit; without them parts separate along their joints' assembly axes (a male moves out, its female moves away).

## Commands

```
node draft.mjs render model.json --out drawings/ [--sheet a4] [--variant doors-open] [--only ortho,iso,section,exploded,joints,parts] [--scale 1:16] [--grid] [--plain] [--png] [--dpi 300] [--dxf]
node draft.mjs check  model.json [--cutlist cutlist.json] [--variant name] [--json]
node draft.mjs check  drawing.svg [--print-width-mm 180] [--min-text-mm 2] [--min-dim-text-mm 2.5] [--json]
node draft.mjs table  model.json [--format md|csv|json]
node draft.mjs dxf    model.json --view front -o front.dxf
```

`--cutlist` reads `{ "parts": [ { "name", "qty", "length", "width", "thickness" } ] }` and reports size or quantity parity per part name (`cutlist` on a part maps a different name). Exit code 1 on any FAIL.
