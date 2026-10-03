# Fleet twin tags

Reader: An operator comparing two sessions with the same codename on different machines.

Decision: Open the intended session without guessing from a prematurely shortened tag.

Hero form: Keep the Fleet work-state plot and its adjacent session ledger; fit the tag to the actual first column.

Distinctive move: Preserve the existing project-first label and short distinct machine mark, spending remaining column width on the codename tail before trimming from the left.

Omitted: New layout, palette, and breakpoint rules; the Pebble mono plot already supplies the comparison, and measured fitting removes the 12-character assumption.

Implementation: Port the structural intent of e43a5d91, preserve the established distinct codename suffix, measure the tag's rendered font including its separator, and refit without replacing rows or keyboard focus. Disconnect observation when the board is removed.
