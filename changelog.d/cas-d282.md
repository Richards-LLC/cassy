### Fixed — Factory gate audit

- **Ratchet uncoded caller-authority refusals** — The mechanical gate audit traces caller identity into refusal conditions, while excluding ordinary errors and unrelated role/owner metadata. Mutable, shadowed and destructured bindings retain their provenance at each scope and read position. Existing uncoded sites are recorded in a counted landed baseline that can only shrink, including after file deletion; new uncoded sites fail the audit. No gate codes or runtime policy are added.
