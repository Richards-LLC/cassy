# Exemplar and renderer contract

The approved starting point is Cassy v3.19.0 in the Cassy source repository:

- [Markdown](https://github.com/Richards-LLC/cassy/blob/d9edb6f910344e9030085197234cd98ab6933215/docs/release-reports/2026-09-08-v3.19.0.md)
- [Concept brief](https://github.com/Richards-LLC/cassy/blob/d9edb6f910344e9030085197234cd98ab6933215/docs/release-reports/v3.19.0.brief.md)
- [HTML](https://github.com/Richards-LLC/cassy/blob/d9edb6f910344e9030085197234cd98ab6933215/docs/release-reports/v3.19.0.html)
- [PDF](https://github.com/Richards-LLC/cassy/blob/d9edb6f910344e9030085197234cd98ab6933215/docs/release-reports/v3.19.0.pdf)

In a Cassy checkout these files live under `docs/release-reports/`. The exemplar's
`render-v3.19.0.py` is historical; use this skill's generic `scripts/render.py`.
Keep the approved section order and dot-map identity. v3.19.0 is a v1 source
(one dot per closed issue); new reports use the v2 change map below. Its original
PDF forced fresh pages per section; this template deliberately uses continuous
flow.

## Markdown input v2

Begin with three blank-line-separated blocks: `# <product> <version>`, a
publication date/time line, and a verdict paragraph. Then provide these exact
`##` headings once each, in this order:

1. `Change map`: claim paragraph, reading-guide paragraph, a three-column table
   (`Area | Changes | What changed`), then source paragraph(s). Each area row has
   a positive integer count and that many plain change titles separated by semicolons;
   end with a `Total` row. Omit areas without changes: page one never shows a
   zero row. A v1 table (`Surface | Issues closed | Issue numbers`) still renders
   for older sources; its zero rows are not drawn.
2. `Release at a glance`: a three-column table (`Measure | Value | Definition`),
   followed by the counting note. Lead with `Changes shipped` and then
   `GitHub issues closed`. List only measured, nonzero values, with short
   definitions; name zero and unmeasured values in the counting note instead.
3. `What you can do now`: `###` themes and `####` change titles; each change has
   `Was: ...`, a blank line, `Now: ...`, and optional `Source: ...` paragraphs.
   A change with a captured pair adds `Pair: <task-id>/<shot-id>` between
   `Now:` and `Source:`. The renderer invokes `cas --json deliverable pairs
   --task <task-id> --root <project-root>` to resolve the durable
   `qa/pairs/pairs.json`, using only verified `pairs[]`, never `unpaired[]`.
   Missing, incomplete or invalid evidence shows no figure and prints a warning;
   no pair means nothing shown. `cas release report` supplies its own executable;
   a standalone render uses `cas` on PATH (override with `--cas-bin`).
   Alternatively add explicit `Before: <path>` and `After: <path>` paragraphs.
   Choose one form per change. Before is the earlier
   capture of the same surface (a previous visual-QA or QA baseline, a base
   build's run or the prior release's shot); After is the delivery's own
   capture. Paths are absolute or relative to the project root; PNG, JPEG or
   WebP up to 400 kB each, at most six pairs per report. The pair is inlined as
   a labelled Before | After figure inside the change's card. A lone side, a
   missing or oversized file, or a seventh pair renders no figure and prints a
   warning. Never mock or reconstruct a before.
4. `Under the hood`: the same grouped Was/Now structure for developers.
5. `Fixes ledger`: a Markdown table (`Issue | Title | Closed | Area`) whose first
   column holds one linked issue number per row, without duplicates. These are
   the GitHub issues closed with the release; a release without any keeps a
   header-only table and says so in a sentence.
6. `Install`: commands in fenced code, archive URLs and complete digests.
7. `Evidence and scope`: source URLs, query/window, retrieval timestamps and
   limitations. State latency endpoints; never substitute tag latency for
   green-to-published time.

GitHub issues closed: the generator counts the issues that the merged release PR
(its title names the version) or the CHANGELOG section references and that closed
between the PR opening and 15 minutes after its merge. Issues are closed by hand
a few minutes after the merge, so a run that starts inside that window re-reads
issue states until the referenced issues close or the window ends. Issues still
open, or closed outside the window, are listed under the ledger and not counted.
Do not correct the count by hand; rerun with `--refresh-sources` if a source
was wrong.

Supported Markdown: paragraphs, inline code, links, emphasis, strong text,
unnested dash lists, fenced code, tables and the headings above. Raw HTML is
escaped. Avoid pipes inside table cells and multiline link destinations. Normalize
arbitrary changelog Markdown into this source contract before invoking the script.
Malformed sections, inconsistent counts and unsafe URL schemes fail before writing.

## Rendering and tokens

`cas release report` renders the Markdown with `scripts/render.py`, then fills
the template's brand markers from `cas deliverable brand`. By hand:

```bash
python3 <skill-dir>/scripts/render.py docs/release-reports/<version>.md \
  --output docs/release-reports/<version>.html --project-root . \
  --emphasis 'exact verdict phrase'
```

Only source and `--output` are required. `--pdf-href` overrides the sibling PDF
link; `--footer` overrides the short footer. For a CLI embedding the bundle,
import `render(source, project_root=..., source_href=..., pdf_href=...,
emphasis=..., footer=...)`; it returns HTML without writing files. Keep the
`references/` directory next to `scripts/` when extracting builtin resources.

The renderer holds no brand code. Its HTML keeps a `/*cas:brand*/` style marker
and a `<!--cas:brand-mark-->` lockup marker; `cas release report` fills both.
`cas deliverable brand --json` names the source it chose: explicit `--tokens`,
the DESIGN.md named by `docs/deliverables/PROFILE.md`, the DESIGN.md nearest
`--surface`, the root `DESIGN.md`, a single nested one such as
`apps/frontend/DESIGN.md`, then `design-tokens.json` and the bundled defaults.
It follows `inherits:` when that names another DESIGN.md. A DESIGN.md with CSS
`source` files and `maps:` roles is inlined (relative `.woff2` faces become data
URIs; any other `url(`, `@import` or `</` refuses it, with a warning); otherwise
its design-spec `colors`, `typography` and `rounded` values are used, and the
receipt names the fallback font stack when no local `.woff2` exists.
Dark-scheme blocks are narrowed to `screen`, so print is always the light set.
An optional `brand-mark:` is inlined beside the product name. The template reads
the Petrastella roles plus optional `hero-band`, `glass-panel`,
`glass-panel-strong`, `glass-edge`, `glass-blur`, `glass-shadow`, `lift`,
`card`, `emphasis`, `on-emphasis`, `radius-panel`, `radius-tile` and
`radius-chip`; each falls back to a Petrastella role or to none, so a flat
palette still renders. No network font loads.

## Template and change-map data contract v2

`template.html` owns CSS and fixed section markup; `{{NAME}}` slots are filled
once with escaped content or renderer-owned HTML. Stable `data-section` values:
`hero`, `glance`, `users`, `dev`, `ledger`, `install`, `provenance`.

The inline `svg[data-change-map="1"]` carries `data-total`. Each directly labelled
`g[data-area]` carries the area name, integer `data-count` (at least 1), JSON
string array `data-changes` (the plain titles) and Boolean-string
`data-emphasis`. Every circle has `data-change` and a `<title>` naming the change.
Largest groups get equal emphasis, including ties. Each dot means one change;
areas without changes are not drawn. v1 sources draw `g[data-surface]` with
`data-issues` and `data-issue` circles instead. Rows wrap at eight dots while
preserving equal area. The SVG description and the open `#map-data` table, which
opens Release at a glance ahead of the stats, repeat all data. The hero note
states the GitHub issues closed as a secondary figure. Long labels or large maps
still need the brief's first-screen QA; regroup with an explicit editorial
rationale if they cannot fit legibly.

## Exemplar dry run

From the Cassy repository root, substitute the installed skill path below:

```bash
python3 <skill-dir>/scripts/render.py docs/release-reports/2026-09-08-v3.19.0.md \
  --output .report-build/v3.19.0.html --project-root . \
  --emphasis 'clearer states' --footer 'Clearer states. Safer next steps.'
```

Expect the same source content, 21 dots across six surfaces (a v1 source), 36
Was/Now articles, 21 ledger rows, full hashes and all source links. The legend and metadata are
now input-derived; print pagination intentionally differs from the historical PDF.
