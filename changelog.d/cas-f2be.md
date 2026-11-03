### Fixed — Merge queue

- **Concurrent deliveries no longer conflict on the changelog** — Was: every
  delivery inserted its section at the top of `CHANGELOG.md`'s
  `## [Unreleased]`, so GitHub's merge queue, which ignores the local
  changelog merge driver, dequeued the second of any two queued pull requests
  as conflicting. Now: each delivery writes its own `changelog.d/<task-id>.md`
  fragment, the close gate credits that fragment (and points a direct
  `CHANGELOG.md` entry at it), the release gate checks fragment format, and
  `scripts/release.sh` folds the fragments into the version section and deletes
  them.
