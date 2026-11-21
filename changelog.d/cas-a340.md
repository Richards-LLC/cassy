### Fixed — Catalogue-driven Jest specs are no longer partly proved

- **A changed JSON catalogue widens its spec to a whole-file run** — When a delivery changes a JSON file that a spec imports by relative path, the capped runner no longer selects only the spec's changed literal tests and reports a full pass. The tests generated from the catalogue have no changed source line, so the run widens to the whole spec, and park asks for that same whole-spec proof.
