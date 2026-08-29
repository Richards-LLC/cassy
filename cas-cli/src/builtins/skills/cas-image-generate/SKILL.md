---
name: cas-image-generate
description: Generate style-matched raster assets for apps, websites, and reports through Google Nano Banana. Use when a project needs a hero, background, logo, icon, card, illustration, or report artwork.
managed_by: cas
---

# Style-aware image generation

Use this skill when an image should belong to an existing product, site, or
report rather than look like an unrelated stock asset. It covers logos, icon
sets, heroes, backgrounds, textures, OG/social cards, report art, illustrations,
and favicons. Read the relevant
references before generating; keep the style decision reusable and inspect the
result at its intended size.

## Provider and invocation

Google Nano Banana is the only wired hosted provider. It handles every raster
asset type:

- **NB2** (`gemini-3.1-flash-image`) for drafts, iteration, and ordinary scenes.
- **NB Pro** (`gemini-3-pro-image`) for finals, report covers, and text-heavy
  compositions.

The helper uses the `GEMINI_API_KEY` environment variable and never accepts a
key as a command-line argument. No CAS-managed image-provider secret store is
currently available, so configure the key in the process environment (for
example, from the operator's local secret manager). If it is absent, the
helper stops before network access and prints the exact Google AI Studio setup
guidance. Run it from the project root:

```bash
bash <harness-config-dir>/skills/cas-image-generate/scripts/generate-image.sh \
  --tier draft --prompt "$PROMPT" --output assets/generated/hero.png
```

Use `--reference path/to/approved.png` once per reference image. Use
`--dry-run` to validate routing and key presence without calling the API. The
helper is a plain-curl adapter; see [providers.md](references/providers.md) for
the request shape and the unwired alternatives.

## Workflow

1. Harvest the project's design context using
   [style-harvest.md](references/style-harvest.md). Produce a concrete style
   token block before writing the final prompt; reuse that style token block in
   every request.
2. Choose the asset row and preset in
   [asset-playbook.md](references/asset-playbook.md). Put the token block in
   every prompt; reference approved assets when consistency matters.
3. Start with NB2 for a draft. Switch to NB Pro for the approved final or any
   composition where exact copy matters. State what must remain unchanged when
   editing a reference.
4. Store the result using the naming and format rules in
   [output-checklist.md](references/output-checklist.md), then complete its
   review checklist. Record model, tier, prompt, references, and any seed or
   style identifier in the project's asset note.

Nano Banana produces raster output, not editable SVG. For logos, favicons, and
icon sets, request a flat, high-contrast raster master with generous padding so
manual vectorization is practical; do not promise production SVG paths or
perfect small-size legibility. Derive favicon sizes from one approved master
rather than regenerating each size.

Do not use Imagen: Google retired that image path on 2026-08-17. Recraft,
OpenAI, Ideogram, and hosted FLUX remain documented but unwired optional
add-ons; do not invent a second credential or silently route a request to one.
