### Fixed — Catalogue-driven Jest specs are no longer partly proved

- **A changed JSON catalogue widens its spec to a whole-file run** — When a delivery changes a JSON file that a spec imports by relative path, the capped runner no longer selects only the spec's changed literal tests and reports a full pass. The tests generated from the catalogue have no changed source line, so the run widens to the whole spec, and park asks for that same whole-spec proof.
- **A catalogue-only change still owes proof** — A delivery that changes only the JSON catalogue, and not the spec, now makes park ask for a whole-spec run of every test file that imports it.
- **Switching a spec to another catalogue widens too** — A spec whose import moves from one existing JSON catalogue to another generates different tests without any JSON in the diff; it now widens to a whole-spec run instead of proving only its changed literal tests.
