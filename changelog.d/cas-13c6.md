### Fixed — Workspace shell analysis

- **Captured exit-status comparisons** — The workspace shell analyzer recognizes variables assigned from `$?` as numeric `test -eq` inputs while keeping unresolved operands guarded.
