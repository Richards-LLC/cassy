# Fleet twin tags

> **Retired (cas-0546, 2026-10-05).** The Terminal view this document designs (fleet board, session ledger, machine rail, pane grid, fleet-wide Attention feed, take-control, side composer) was removed; Conversations is Commander's only surface (see `hub-web/DESIGN.md`). Kept as a historical design record.

Reader: An operator comparing two sessions with the same codename on different machines.

Decision: Open the intended session without guessing from a prematurely shortened tag.

Hero form: Keep the Fleet work-state plot and its adjacent session ledger; fit the tag to the actual first column.

Distinctive move: Preserve the existing project-first label and short distinct machine mark, spending remaining column width on the codename tail before trimming from the left.

Omitted: New layout, palette, and breakpoint rules; the Pebble mono plot already supplies the comparison, and measured fitting removes the 12-character assumption.

Implementation: Port the structural intent of e43a5d91, preserve the established distinct codename suffix, measure the tag's rendered font including its separator, and refit without replacing rows or keyboard focus. Disconnect observation when the board is removed.

## Critique

Real source-build renders were inspected at1280×800 light and390×800 dark. The project still keeps a letter; phone tags trim from the left with complete, distinct machine marks. Desktop grows back to the longest fitting suffix. Existing intentional project ellipsis and the unchanged session ledger retain the comparison language.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Project-first Pebble monospace plot, compact state dots, and short machine marks retain the product's established identity. |
| Fit | 4 | Actual glyph/column measurement removes the character guess; width, seven font stacks, and resize return are exercised. |
| Hierarchy | 4 | Fleet verdict precedes the plot; marks distinguish the paired rows before the full ledger details. |
| Craft | 4 | Full tag title, complete mark fallback, stable row nodes, and observer cleanup support both space and focus. |
| Accessibility | 4 | Full row-header names remain available; forced colors, reduced motion, increased contrast, focus retention, and destination aria snapshots are captured. |

Strict visual QA compares the same production page and Fleet stimulus against the epic base; known JS-disabled production fallback findings belong to cas-1f04. No new visual finding is accepted.
