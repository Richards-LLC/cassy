### Fixed — Factory

- **Honor required release imports** — A supervisor can pin required release or sibling imports with `merge_source=<full commit SHA>` in an existing change-request reason. Close preserves that upstream provenance while keeping later unapproved commits and unrelated work fenced. Quoted markers in QA/verifier findings or queue-ejection details grant no authority and cannot block rejection routing.
