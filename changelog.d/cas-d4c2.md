### Fixed — Factory

- **Artifact work preserves shared checkouts** — Starting a no-code task in a shared repository leaves its branch and pending edits untouched. Artifact-only close recognizes published trunk imports used for report tooling while retaining checks for unpublished source work.
- **Repository scan failures identify their scope** — Incomplete target resolution names the requested repository and branch, the roots that could not be checked, and the identities that were read, without assuming a storage outage.
