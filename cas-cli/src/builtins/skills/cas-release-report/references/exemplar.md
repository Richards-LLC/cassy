# Exemplar and renderer contract

The approved starting point is Cassy v3.19.0 in the Cassy source repository:

- [Markdown](https://github.com/Richards-LLC/cassy/blob/d9edb6f910344e9030085197234cd98ab6933215/docs/release-reports/2026-09-08-v3.19.0.md)
- [Concept brief](https://github.com/Richards-LLC/cassy/blob/d9edb6f910344e9030085197234cd98ab6933215/docs/release-reports/v3.19.0.brief.md)
- [HTML](https://github.com/Richards-LLC/cassy/blob/d9edb6f910344e9030085197234cd98ab6933215/docs/release-reports/v3.19.0.html)
- [PDF](https://github.com/Richards-LLC/cassy/blob/d9edb6f910344e9030085197234cd98ab6933215/docs/release-reports/v3.19.0.pdf)

In a Cassy checkout these files live under `docs/release-reports/`. The exemplar's
`render-v3.19.0.py` is historical; use this skill's generic `scripts/render.py`.
Keep the approved section order and closure-stitch identity. Its original PDF
forced fresh pages per section; this template deliberately uses continuous flow.

## Markdown input v1

Begin with three blank-line-separated blocks: `# <product> <version>`, a
publication date/time line, and a verdict paragraph. Then provide these exact
`##` headings once each, in this order:

1. `Change map`: claim paragraph, reading-guide paragraph, a three-column table
   (`Surface | Issues closed | Issue numbers`), then source paragraph(s).
   Use a nonnegative integer count per surface, `#123` identifiers once each,
   and a final `Total` row. Include a zero row for an improvement without an
   issue; its third cell explains that distinction.
2. `Release at a glance`: a three-column table (`Measure | Value | Definition`),
   followed by counting methodology. Missing measurements say unavailable.
3. `What you can do now`: `###` themes and `####` change titles; each change has
   `Was: ...`, a blank line, `Now: ...`, and optional `Source: ...` paragraphs.
4. `Under the hood`: the same grouped Was/Now structure for developers.
5. `Fixes ledger`: a Markdown table whose first column contains one linked issue
   number per row. Its issues must match the map exactly, without duplicates.
   A zero-issue release uses a header/separator-only ledger and zero map rows.
6. `Install`: commands in fenced code, archive URLs and complete digests.
7. `Evidence and scope`: source URLs, query/window, retrieval timestamps and
   limitations. State latency endpoints; never substitute tag latency for
   green-to-published time.

Supported Markdown: paragraphs, inline code, links, emphasis, strong text,
unnested `- ` lists, fenced code, tables and the headings above. Raw HTML is
escaped. Avoid pipes inside table cells and multiline link destinations. Normalize
arbitrary changelog Markdown into this source contract before invoking the script.
Malformed sections, inconsistent counts and unsafe URL schemes fail before writing.

## Rendering and tokens

```bash
python3 <skill-dir>/scripts/render.py docs/release-reports/<version>.md \
  --output docs/release-reports/<version>.html --project-root . \
  --tokens path/to/design-tokens.json --emphasis 'exact verdict phrase'
```

Only source and `--output` are required. `--pdf-href` overrides the sibling PDF
link; `--footer` overrides the short footer. For a CLI embedding the bundle,
import `render(source, project_root=..., tokens=..., source_href=..., pdf_href=...,
emphasis=..., footer=...)`; it returns HTML without writing files. Keep the
`references/` directory next to `scripts/` when extracting builtin resources.

Tokens use `color.light.<role>.$value`, `color.dark.<role>.$value` and
`typography.family.{display,body,mono}.$value` arrays. Roles are those in
`default-tokens.json`; hex and numeric rgb/rgba values are accepted. Partial
overrides inherit missing defaults. Precedence: explicit `--tokens`, root
`design-tokens.json`, `docs/design/design-tokens.json`, bundled defaults. The
agent reads DESIGN.md and resolves any other token path or maps another schema;
the renderer does not infer visual design rules from prose. No network font loads.

## Template and change-map data contract v1

`template.html` owns CSS and fixed section markup; `{{NAME}}` slots are filled
once with escaped content or renderer-owned HTML. Stable `data-section` values:
`hero`, `glance`, `users`, `dev`, `ledger`, `install`, `provenance`.

The inline `svg[data-change-map="1"]` carries `data-total`. Each directly labelled
`g[data-surface]` carries the surface name, integer `data-count`, JSON string array
`data-issues` (digits without `#`) and Boolean-string `data-emphasis`. Every circle
has `data-issue`. Largest positive groups get equal emphasis, including ties.
Each dot means one closure; zero rows have a label and no mark. Stitches wrap at
seven dots while preserving equal area. The SVG description and open `#map-data`
table repeat all data. Long labels or large maps still need the brief's first-screen
QA; regroup with an explicit editorial rationale if they cannot fit legibly.

## Exemplar dry run

From the Cassy repository root, substitute the installed skill path below:

```bash
python3 <skill-dir>/scripts/render.py docs/release-reports/2026-09-08-v3.19.0.md \
  --output .report-build/v3.19.0.html --project-root . \
  --emphasis 'clearer states' --footer 'Clearer states. Safer next steps.'
```

Expect the same source content, 21 dots across six surfaces, 36 Was/Now articles,
21 ledger rows, full hashes and all source links. The legend and metadata are
now input-derived; print pagination intentionally differs from the historical PDF.
