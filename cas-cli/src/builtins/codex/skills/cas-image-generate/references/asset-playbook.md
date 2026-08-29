# Asset playbook

Research basis: the current [image-generation dossier](../../../../../../.cas/artifacts/cas-1c67/research/image-generation-dossier.md)
was compiled 2026-08-29. The hosted route in this skill is Google Nano Banana;
the dossier's other providers are retained as explicitly unwired references in
[providers.md](providers.md).

## Routing and presets

All rows use Nano Banana. Pick NB2 for a draft or iteration and NB Pro for a
final, cover, or dense in-image copy. The requested dimensions belong in the
prompt and in the output review; the helper sends references and decodes the
returned image but does not silently resize it.

| Asset | Tier | Suggested output | Prompt emphasis |
|---|---|---|---|
| Logo / logomark | NB2 → NB Pro | square PNG, 1024px or larger | flat, simple mark, the wordmark TEXT (spelled exactly, no quotation marks), high contrast, generous clear space; manual vectorization required |
| Icon set | NB2 → NB Pro | one square grid sheet, 1024–2048px | one locked 24px-grid spec sentence, uniform stroke, padding, one color; manually trace approved glyphs |
| Hero image | NB2 → NB Pro | 16:9, up to 1920px wide | focal subject, camera, lighting, and negative space for copy |
| Background / texture | NB2 | 16:9 or tileable square, 2K | low contrast, no focal object, exact palette, usable behind text |
| OG / social card | NB Pro | 1200x630 PNG | exact quoted copy early, safe area, brand contrast, no gibberish |
| Report cover / section art | NB Pro | A4 portrait or section banner, 2K+ | restrained palette, title placement, print-safe whitespace |
| Spot illustration | NB2 → NB Pro | square 800–1600px PNG | simple silhouette, transparent-look isolation only if verified; consistent family style |
| Favicon / app icon | derive from approved logo | 1024px square master, then 16/32/48/180/192/512px derivatives | centered mark inside the safe area; never generate six independent variants |

## Prompt templates

Replace every `{style_tokens}` slot with the complete block produced by
[style-harvest.md](style-harvest.md). Keep the asset-specific sentence stable
across a set and vary only `{subject}` or `{copy}`.

### NB2 draft / iteration

```text
Create a {asset_type} for {project}.
Subject: {subject}. Context: {context}. Composition: {composition}.
Style tokens: {style_tokens}.
Make this an exploratory draft with clean edges, coherent lighting, and no
unrequested text. Preserve the supplied reference relationships: {references}.
```

### NB Pro final / text-heavy

```text
Create the final {asset_type} for {project}.
Use these exact style tokens: {style_tokens}.
Subject and action: {subject_action}. Context: {context}.
Composition and safe area: {composition}. Exact displayed copy: {copy} (spelled exactly; no quotation marks unless explicitly requested).
References and what must stay unchanged: {references_and_invariants}.
Return a polished raster asset with correct spelling, deliberate edges, and no
extra lettering or decorative objects.
```

### Logo and icon limitation template

```text
Create a flat, high-contrast raster master for manual vectorization: {subject}.
Use {style_tokens}; two colors maximum, simple geometric paths, centered on a
plain background, generous clear space, no gradients, no shadows, and no extra
lettering. If a wordmark is required, render the wordmark TEXT (spelled {copy}, no quotation marks).
```

### Locked icon-set sentence

```text
Create a single grid sheet of line icons: each glyph uses a 2px stroke,
rounded caps and joins, a 24x24 grid with 2px padding, no fill, one color,
centered composition, and the same {style_tokens}. Subjects: {subject_list}.
```

## Consistency rules

- Generate related icons as one sheet where possible, then use the approved
  sheet as a reference for later batches.
- Keep the literal style token block, camera/framing language, and lighting
  sentence unchanged across a set. Models do not carry state between calls.
- Use a new aspect ratio as a crop or edit of an approved master, not a fresh
  unrelated generation.
- Lay the set out at true size and check backdrop tone, shadow direction, then
  crop distance for drift.
