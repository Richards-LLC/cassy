### Fixed — Factory

- **Vitest table rows that share a name prove their test** — An `it.each` test whose rows expand to the same name used to fail capped stability, even when every row passed. This covers `%s` rows with a repeated first column and static titles. Worker-check and park now credit the source test when every row passes. A failing row, a test with no rows, or a row that goes missing from any of the five runs (even when a same-named sibling still passes) is still refused.
