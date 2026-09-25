# Principles for a stuck design or fix

Use these when a proposed change grows faster than the behavior it must prove.

- **Attack the premise.** After a second failed gate or test round, write the
  assumption driving the fix as a task note. Compare the gate's actual checks
  with the files and behavior changed. Revise the assumption before another
  patch.
- **Laziness.** Do the least work that proves the point. Find the smallest
  observable slice and stop adding structure once that slice works.
- **Subtract.** Remove obsolete branches, wrappers, and configuration before
  adding another path. If deletion makes the behavior clearer, keep the
  smaller design.
- **Redesign at the third patch.** When a third local patch is needed for one
  behavior, restate the invariant and choose a seam that handles the cases
  together. Record why the former seam could not.
- **Test behavior.** Assert what a caller can observe, using an expected value
  independent of the implementation. Ask whether the test would still pass if
  every import returned `undefined`; if so, it has not proved the behavior.
- **Use types to enforce invariants.** Represent valid states explicitly and
  make illegal combinations unrepresentable where the language permits it.
  Keep parsing and validation at the boundary so internal code handles a
  narrower type.
