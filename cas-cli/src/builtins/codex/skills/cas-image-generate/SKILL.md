---
name: cas-image-generate
description: Generate style-matched raster assets for apps, websites, and reports through Google Nano Banana. Use when a project needs a hero, background, logo, icon, card, illustration, or report artwork.
managed_by: cas
---

# Style-aware image generation

Use this skill when an image should belong to an existing product, site, or
report rather than look like an unrelated stock asset. It covers logos, icon sets,
heroes, backgrounds, textures, OG/social cards, report art, illustrations,
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

Choose the asset medium before invoking Nano Banana. For geometric, flat,
limited-palette icons, marks, dividers, patterns, and favicons, write
agent-authored SVG directly; the standards, worked examples, and routing table
are in [svg-web-assets.md](references/svg-web-assets.md). Use Nano Banana for
photographic, painterly, or complex illustration work. For an organic mark such
as a ribbon-C, generate an approved raster first and follow the raster-to-vector bridge
in that reference when a vector deliverable is required. If no local
vectorizer is installed, follow its manual vectorization fallback. Nano Banana
still produces raster output, not editable SVG, so do not promise production
SVG paths or perfect small-size legibility from a generated raster.

Do not use Imagen: Google retired that image path on 2026-08-17. Recraft,
OpenAI, Ideogram, and hosted FLUX remain documented but unwired optional
add-ons; do not invent a second credential or silently route a request to one.
