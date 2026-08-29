---
title: SVG and web assets
---

# SVG and web assets

Use this reference when an asset can be a vector, when a generated raster must
be bridged to a vector, or when one approved master needs web derivatives. The
short version is: author simple SVG directly, generate raster for visual
complexity, and trace only an approved raster when the organic shape needs a
vector deliverable. Route to the existing
[output-checklist.md](output-checklist.md) for the shared review, naming, and
license record; this reference adds vector and web-pipeline details without
duplicating that checklist.

## Decide the medium first

Agent-authored SVG is a first-class output. Write the SVG directly for icons,
icon sets, simple logomarks, favicons, dividers, waves, blobs, grid or pattern
backgrounds, badges, and loading spinners when the design is geometric, flat,
or uses a limited palette. It is usually smaller, editable, deterministic, and
more legible at small sizes than a generated raster.

| Asset shape | Route | Acceptance signal |
|---|---|---|
| Geometric icon or icon set | Author directly | Paths fit a stated grid and share stroke rules. |
| Flat logomark, badge, favicon, divider, or pattern | Author directly | Palette and spacing are explicit; the `viewBox` survives reuse. |
| Photographic scene or soft texture | Generate raster | The visual depends on continuous tone, grain, or lighting. |
| Painterly or complex illustration | Generate raster | Many irregular details would make hand-authored paths brittle. |
| Organic mark, such as the Cassy ribbon-C | Generate approved raster, then raster-to-vector | A human checks the trace at target sizes and cleans its paths. |

Do not ask Nano Banana to return production SVG. It is the raster route. Do not
trace a raster merely because a file extension says `.svg`: an SVG containing a
single embedded bitmap is still a raster asset and does not meet the vector
requirement.

## Agent-authored SVG standards

Apply these rules to every directly written SVG:

- Declare `xmlns` and a meaningful, tight `viewBox`; add `width` and `height`
  only when the consumer needs a default rendered size. Keep coordinates in a
  simple stated grid (24px for interface icons).
- Keep IDs minimal and meaningful. Use an ID for a title or reusable path when
  it is referenced; remove editor-generated layer names, metadata, guides,
  transforms, and namespace cruft before shipping.
- Map harvested style tokens one-to-one to CSS custom properties such as
  `--color-brand`, `--color-surface`, and `--color-foreground`. Use
  `currentColor` for one-color icons or as a fallback in
  `var(--color-foreground, currentColor)` so consumers can recolor them.
- State stroke-width on the grid. A typical 24px icon uses a 2px stroke with
  round caps and joins, then keeps that optical weight across the set. Do not
  mix arbitrary half-pixel widths without a deliberate small-size reason.
- For inline SVG, include a concise `<title>` and `role="img"` when the image
  conveys meaning; connect the title with `aria-labelledby`. For decorative
  SVG, use `aria-hidden="true"` instead. A favicon can retain a title, but its
  browser-tab context does not replace testing the silhouette at 16px.
- Keep the file size disciplined: remove unused defs, duplicate points, and
  excessive decimal precision; avoid embedding raster data, fonts, or a full
  design file. Prefer a few paths over a maze of one-point shapes.

Palette variables belong to the consuming stylesheet when possible. A
self-contained asset may include fallback values in `var(--color-brand,
#2563eb)`, but do not silently replace the project's harvested tokens with a
new palette.

## Worked examples

These complete examples are intentionally small. They are valid XML and can be
copied into a repository after the project token values and accessible labels
are reviewed.

### 24px search icon

This icon uses a 24px grid, a consistent 2px stroke, a palette variable, and a
`currentColor` fallback. The path IDs are the only IDs needed.

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" role="img" aria-labelledby="search-title" style="--color-foreground: #1f2937">
  <title id="search-title">Search</title>
  <path id="search" d="m10.75 4.5a6.25 6.25 0 1 0 0 12.5 6.25 6.25 0 0 0 0-12.5Zm4.42 10.67 4.33 4.33" fill="none" stroke="var(--color-foreground, currentColor)" stroke-linecap="round" stroke-linejoin="round" stroke-width="2"/>
</svg>
```

### Wave section divider

This divider is a reusable 1200x160 viewBox. The same harvested surface and
accent tokens can be overridden by the page stylesheet without editing paths.

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1200 160" role="img" aria-labelledby="wave-title" style="--color-surface: #f8fafc; --color-accent: #2563eb">
  <title id="wave-title">Blue wave section divider</title>
  <path id="surface" d="M0 0h1200v160H0z" fill="var(--color-surface)"/>
  <path id="wave" d="M0 92c180-58 330-58 510 0s330 58 510 0c72-23 130-28 180-18v86H0Z" fill="var(--color-accent)"/>
</svg>
```

### `favicon.svg`

Keep the favicon silhouette centered inside a safe area. The fallback colors
make the standalone file useful to renderers that do not apply a page
stylesheet; the variables still map one-to-one when a stylesheet is present.

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" role="img" aria-labelledby="favicon-title" style="--color-background: #111827; --color-foreground: #f8fafc">
  <title id="favicon-title">Cassy</title>
  <rect id="background" width="64" height="64" rx="14" fill="var(--color-background, #111827)"/>
  <path id="mark" d="M19 18h12.5a14 14 0 1 1 0 28H19l10-14-10-14Zm12.5 6h-1.13l5.71 8-5.71 8h1.13a8 8 0 1 0 0-16Z" fill="var(--color-foreground, #f8fafc)" fill-rule="evenodd"/>
</svg>
```

## Raster-to-vector bridge

Use this bridge for an approved flat raster when the organic contour cannot be
written cleanly by hand. Probe first; never assume a tool is installed, and do
not add a package or send an image to a hosted service just to make tracing
work.

```bash
for tool in vtracer potrace inkscape magick convert; do
  if command -v "$tool" >/dev/null 2>&1; then
    printf '%s: ' "$tool"
    "$tool" --version 2>&1 | head -n 1
  else
    printf '%s: unavailable\n' "$tool"
  fi
done
```

Choose the first available route that matches the source. These commands are
examples, not an assertion that any tool exists on the current machine.

### VTracer

VTracer handles color rasters and can constrain a simple mark to a small
palette. Its CLI is commonly exposed as `vtracer`:

```bash
vtracer --input approved-mark.png --output traced-mark.svg \
  --preset poster --filter-speckle 4 --optimize 1
```

For a two-color mark, add `--max-colors 2` or a reviewed `--palette` value.
Inspect the generated paths and simplify them after tracing; a successful
command is not proof of a production-ready logo.

### Potrace

Potrace expects a bitmap format such as PBM, PGM, PPM, or BMP, not every PNG
variant. Convert a high-contrast raster first, then request SVG output:

```bash
magick approved-mark.png -colorspace gray -threshold 55% approved-mark.pbm
potrace approved-mark.pbm --svg --output traced-mark.svg
```

If the `magick` probe fails but the legacy `convert` probe succeeds, substitute
`convert` for `magick`. Use Potrace for clean monochrome silhouettes; color or
soft-edge art usually needs preprocessing and more manual cleanup.

### Inkscape

Check that the installed Inkscape exposes headless actions before relying on
this version-sensitive trace recipe. Inkscape embeds Potrace for bitmap
tracing:

```bash
inkscape --help | grep -q -- '--actions' && \
  inkscape approved-mark.png \
    --actions="select-all;selection-trace:256,false,true,true,4,1.0,0.20;export-filename:traced-mark.svg;export-do;" \
    --batch-process
```

If `--actions` or `selection-trace` is unavailable, open the bitmap in the
installed UI and use Trace Bitmap, or choose another probed tool. Merely
running `inkscape --export-filename=traced-mark.svg approved-mark.png` exports
the raster into an SVG wrapper; it does not vectorize it.

### Decide whether the trace is acceptable

Auto-trace is a useful starting point for a flat, high-contrast, limited-color
mark with generous clear space. It is not a final review for a ribbon-C,
letterform, or mark whose optical balance matters. Reject or manually clean
traces with speckles, doubled edges, self-intersections, hundreds of tiny
paths, embedded images, noisy gradients, or a silhouette that fails at the
target size. Normalize the result to the SVG standards above: a tight
`viewBox`, minimal IDs, palette CSS variables, consistent stroke rules, and no
editor cruft. If every vectorizer is unavailable, retain the approved raster
and follow the existing manual-vectorization advice in `SKILL.md`; do not claim
that a raster-only deliverable is editable SVG.

## Web-asset pipeline

Keep one approved `favicon.svg` as the source of truth. Put it where the
consuming web project expects it—commonly `public/favicon.svg` for a framework
served verbatim, or `assets/brand/favicon.svg` when source assets are bundled.
Derive every bitmap favicon from that one master instead of regenerating each
size independently.

Probe the render and ICO tools, then run only the branch that is available:

```bash
command -v inkscape >/dev/null && command -v magick >/dev/null \
  || printf '%s\n' 'Need Inkscape and ImageMagick for this derivative recipe'

inkscape public/favicon.svg --export-filename=public/apple-touch-icon.png --export-width=180 --export-height=180
inkscape public/favicon.svg --export-filename=public/icon-192.png --export-width=192 --export-height=192
inkscape public/favicon.svg --export-filename=public/icon-512.png --export-width=512 --export-height=512
magick public/favicon.svg -background none -define icon:auto-resize=16,32,48 public/favicon.ico
```

If Inkscape is unavailable, ImageMagick can render the PNG derivatives when
its SVG delegate is present:

```bash
magick -background none public/favicon.svg -resize 180x180 public/apple-touch-icon.png
magick -background none public/favicon.svg -resize 192x192 public/icon-192.png
magick -background none public/favicon.svg -resize 512x512 public/icon-512.png
```

Verify actual alpha and rendered silhouettes at 16px, 24px, 32px, 48px,
180px, 192px, and 512px. A checkerboard painted into pixels is not
transparency. Preserve the SVG master even when a framework also asks for
`favicon.ico`.

For OG/social cards, use the `OG / social card` row in
[asset-playbook.md](asset-playbook.md) and the shared
[output-checklist.md](output-checklist.md): the expected canvas is 1200x630,
with copy inside a tested safe area and the project's contrast tokens. This
reference does not repeat that review checklist.

For generated raster sets, keep a reviewed PNG master and create WebP only
when the local encoder is available:

```bash
if command -v cwebp >/dev/null 2>&1; then
  cwebp -q 82 public/assets/generated/hero.png \
    -o public/assets/generated/hero.webp
elif command -v magick >/dev/null 2>&1; then
  magick public/assets/generated/hero.png -quality 82 \
    public/assets/generated/hero.webp
else
  printf '%s\n' 'No cwebp or ImageMagick; retain the reviewed PNG'
fi
```

Expose responsive variants with `srcset`, preserving the intrinsic fallback:

```html
<img src="/assets/generated/hero-1280.webp"
     srcset="/assets/generated/hero-640.webp 640w, /assets/generated/hero-1280.webp 1280w, /assets/generated/hero-1920.webp 1920w"
     sizes="(max-width: 800px) 100vw, 1280px"
     alt="Descriptive subject">
```

Keep web-delivered files under `public/` when the framework serves that tree
as-is. Keep source brand masters and token-driven SVG under `assets/brand/`
when the project bundles imports. Follow the consuming project's established
convention if it differs, use lowercase kebab-case names, and record the
master/derivative relationship in the asset note. Finish with the shared
output checklist rather than adding a second checklist here.
