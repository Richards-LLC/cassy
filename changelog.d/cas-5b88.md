### Fixed — Releases

- **Reuse green train proof after release documentation changes** — CODEMAP, changelog, release notes and reports can follow a green train run without another full train CI. Pending CODEMAP edits join the release commit. Missing release review stays in the recorded close preview and no longer blocks queue admission; the publication verdict still controls delivery closure.
