### Fixed — Issue intake

- **Match repeated refusals across file paths** — Find-or-file now collapses path tokens in refusal fingerprints, so the same refusal on different files matches the existing issue. Different refusal words still produce distinct keys, and path-heavy headlines cannot create a key on their own.
