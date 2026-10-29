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
