# Style harvest

Before generating, inspect the project rather than guessing its visual
language. Read `DESIGN.md` or the output of the `design-spec` skill first;
then inspect Tailwind/theme configuration, CSS custom properties, existing logo
and palette assets, and a few approved images. Do not copy a prompt from an
unrelated project.

Distill the evidence into one short block and reuse it verbatim in every
generation request:

```text
STYLE TOKENS
palette: navy #0B3D91, gold #F2A900, off-white #F8F7F2
typography: bold geometric sans-serif for headings; quiet humanist sans-serif for body
motifs: ascending lines, generous whitespace, restrained geometric overlays
tone: premium, calm, editorial, precise
lighting: soft diffuse morning light; shadows fall down-right
framing: medium distance; leave the left third open for copy
references: assets/brand/logo-approved.svg, assets/brand/cover-approved.png
```

The block is an executable prompt input, not a report paragraph. Tie hex values
to named objects (for example, “gold #F2A900 ribbon”) and describe typography by
attributes rather than an unavailable font name. Preserve the project's actual
case and values. If no design system exists, say so in the block and choose a
small, explicit palette and tone with the operator rather than inventing brand
history.

For a reference image, state its role and the invariant: “use the approved
cover for palette and framing; change only the subject.” Nano Banana supports
multiple reference images, so add each approved logo, product shot, or prior
asset with a relationship instruction. Remove sensitive or unrelated images
from the request.

## Harvest record

Save the final block beside the generated assets or in the project's existing
design documentation. The asset note should include the source files read,
model tier, prompt, references, and review date. This makes a later revision a
controlled edit instead of a new style decision.
