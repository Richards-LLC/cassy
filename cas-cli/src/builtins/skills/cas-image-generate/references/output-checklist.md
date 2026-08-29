# Output conventions and review

## Where and how to save

Keep generated files in the consuming project, normally
`assets/generated/<asset-type>/<slug>.<ext>`, unless that project already has
an asset convention. Use lowercase kebab-case names and keep the approved
master separate from derived crops:

```text
assets/generated/hero/analytics-workspace-v1.png
assets/generated/logo/cassy-mark-v1.png
assets/generated/og/2026-performance-review-v1.png
assets/generated/hero/analytics-workspace-v1.style.txt
```

Record the model (`gemini-3.1-flash-image` or `gemini-3-pro-image`), prompt,
style token block, references, output dimensions, and review date in the asset
note. Never commit an API key or a provider response containing credentials.

## Format and size guidance

- Use PNG when alpha or lossless edges matter; verify that a requested
  transparent background is real alpha, not a baked checkerboard.
- Use WebP or AVIF for web photography and large scenes after review; retain
  the approved PNG master when practical. Use JPEG only where the consumer
  requires it.
- Logos and icons are raster in this wired path. Ask for a simple, flat master
  suitable for manual vectorization; manually create SVG/ICO derivatives from
  the approved mark when production requires them.
- Use 1200x630 for OG/social cards. Keep copy inside a safe area and check the
  rendered spelling at actual share-card size.
- Use 16:9 or wider for heroes with explicit negative space for UI copy; keep
  the focal subject away from the overlay region.
- Use A4/Letter portrait and at least 2K for report covers; use 2K or larger
  for print-ish artwork after confirming the document's actual resolution.
- Derive favicon.ico, favicon.svg, apple-touch-icon 180x180, and manifest
  icons 192x192/512x512 from one square logo master. Keep the glyph inside the
  small-size safe area.

## Post-generation checklist

1. Compare the asset against the style token block: palette hexes, optical
   weight, motif, lighting, crop, and contrast.
2. Inspect text at target size for spelling, punctuation, safe-area placement,
   and accidental lettering. Recompose rather than silently editing generated
   copy.
3. Confirm dimensions, color profile, file type, and genuine alpha where
   requested. A checkerboard painted into pixels is not transparency.
4. For a set, review all assets together at true size for backdrop, shadows,
   framing, and stroke drift.
5. For logos/icons, check legibility at 16px/24px/32px and manually vectorize
   only after the raster master is approved.
6. Add a license/usage note: record the provider, model, account/project, date,
   references, and any restrictions. Do not assume generated output is free of
   trademark or likeness obligations.
