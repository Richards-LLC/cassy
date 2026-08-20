# Good and Bad Tests

Imported and adapted from mattpocock/skills `tdd/tests.md`, MIT © 2026 Matt Pocock.

Good tests describe observable behavior through a public interface and survive internal refactors. Name what a caller can do, not which helper happened to run.

Avoid tests that inspect private methods, call counts, ordering, or persistence side channels when a public interface can observe the result. Avoid expected values derived by repeating production logic; use a known literal, a worked example, or a specification instead.

One logical capability per test makes failures legible. A small table is useful when independent cases share a public contract; it is not a substitute for behavior-focused test names.
