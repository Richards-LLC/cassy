### Fixed — Jest and Vitest rows that share a runtime title

- **Table rows with the same expanded name are proved and counted** — Distinct `it.each` rows, or repeated static titles, that produce one runtime name used to fail capped stability, because their passing results were counted together against a single expected pass. Capped Jest and Vitest now accept them when every row passes. Their receipts count the rows that ran, not the distinct names, so the totals match the runner's own output.
