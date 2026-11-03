# Brief: Source color in code frames

## Single idea

Explicitly tagged source becomes easier to scan after it is already readable, with its literal bytes and current selection intact.

## Hero form

An annotated specimen in the existing filename / Wrap / Copy frame: violet keywords, teal strings, quiet comments and warm literals on the reading well.

## Emotional register

Quiet and dependable: no spinner, movement, run action or warning severity for unavailable color.

## Distinctive move

A small independently themed syntax vocabulary colors safe text spans; an honest Plain text note explains unavailable or oversized coloring while Copy always holds the source.

## Deliberately omitted

No language detection, external grammars, markup injection, status-color aliases or per-language theme packs. Explicit lexical families cover JS/TS, JSON, Python, Rust and shell. Diagrams and diffs keep their renderer.

## Implementation

The code frame owns source, controls and a loading/fallback note. A dynamic rich-syntax module owns lexical spans, bounded cache and selection restoration. No server, persistent state or motion. Wrap and scheme changes reuse the spans. Only text nodes and fixed token class names enter the DOM. Budgets: 32 Ki UTF-16 source units, 2048 spans per block, 256 KiB conservative retained-cache estimate, 32 entries; oversized blocks remain plain with an explanation.

Design: hub-web/DESIGN.md; component syntax roles proposed below. Existing labels, source and controls retain their hierarchy.

## Syntax palette proposal

Per supervisor direction DESIGN.md is unchanged. The existing code-frame well and type use its tokens. Component-scoped syntax roles in code-frame.css are independent of application severity: keyword #5B3FE0 / #C4B5FD, string #14635F / #81D8CB, literal #854321 / #DCB590; comment reuses ink-mid. Forced colors use CanvasText. Colors are tested against the real rendered well in both schemes; this proposal is explicit rather than a new undocumented global palette.

## Measured budgets and constraints

Both revisions used `npm run build -- --outDir preview-dist`. Entry JavaScript gzip: 198,890 → 199,127 bytes (+237); CSS: 37,825 → 37,983 (+158); the independent lazy syntax chunk is 1,840 bytes gzip. A plain conversation made no syntax request; a tagged fence requested the chunk only after its literal source existed.

The production module on Chromium 156 at 390×844 and 4× CPU throttling painted a 4,689-unit / 360-span specimen in 43 ms cold (13.2 ms warm median) and 2,000 spans in 62.7 ms cold (64.6 ms warm median). A 2,200-span candidate refused color in 3.5 ms; 32,769 source units refused before tokenization. After 200 unique blocks and GC, retained heap rose 166,756 bytes from a module-only baseline. These are synthetic Chromium measurements, not physical-phone timings. Cache accounting includes UTF-16 keys, 96 bytes per run and a Map/object allowance; it does not claim an exact engine heap ceiling.

This is lexical color rather than a complete language parser: template interpolation, Rust lifetime syntax and embedded JSX stay literal source and may receive coarse string/keyword color. No grammar guesses, nesting or execution are promised. The source and its exact Copy remain authoritative.

## Critique

Implementation review from the real production preview walk, strict source-frame captures and direct screenshot inspection. The source-only delivery records the snapshot hash separately; the integration owner regenerates committed dist.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | The existing filename/Wrap/Copy reading tool gains restrained violet keywords and teal source strings. |
| Fit | 5 | Plain source leads; color arrives as an optional aid with exact Copy and clear fallback. |
| Hierarchy | 4 | Source retains mono type and its original geometry; no new badge or competing action. |
| Craft | 4 | Strict 1280×800 and 390×844 light/dark captures PASS with zero findings. |
| Accessibility | 4 | Production preview proves backwards selection through paint and reply repaint, keyboard controls, system-ink forced colors, reduced motion and increased contrast. |
