# `cas doctor` check matrix

Doctor separates findings that Cassy can repair from findings that require an
operator decision. `cas doctor --fix` runs safe repairs and previews consent
repairs. `cas doctor --fix --yes` applies consent repairs after the same dry run
has completed successfully. In a terminal, `--fix` asks once before applying a
consent repair; JSON and non-interactive runs remain dry-run-only unless
`--yes` is supplied.

| Finding | Scope | Classification | Action |
| --- | --- | --- | --- |
| schema migrations, legacy index, stale projections, code index | project | auto-fix | `cas doctor --fix` |
| foreign cloud scopes | project/cloud | consent-fix | `cas doctor --fix --yes` (dry run, purge, sync) |
| unattributed open cloud task rows | project/cloud | consent-fix | `cas doctor --fix --yes` (quarantine) |
| quarantined cloud task rows | project/cloud | consent-fix | `cas doctor --release-cloud-rows --yes` |
| malformed `.cas/config.toml` | project | consent-fix | `cas doctor --fix --yes`; preserves `config.toml.corrupt-*` |
| missing `CHANGELOG.md` | project | info | no action; the repository has no changelog |
| GitHub history authentication | external | human | `gh auth login` |
| unconfigured history repository | project | human | `cas config set issues.repo <owner/repo>` |
| unregistered cloud project | cloud | human | `cas cloud sync` |

Every consent repair prints its plan before applying it. A purge that reports a
safety refusal is never followed by an apply or sync.
