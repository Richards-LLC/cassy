### Fixed — Factory

- **Public registration cannot inherit supervisor authority** — A public `register` or `session_start` call now rebinds only to an identity the server has authenticated, either its own server-internal identity or the harness's `CAS_SESSION_ID`. A row created by an earlier public registration can no longer approve verifications.
- **Single-task merge conflict check is advisory** — Outside a release train, the pre-merge conflict check no longer fetches origin. If the target is missing locally, the source cannot be resolved, or origin has diverged, the check defers to the normal merge path.

### Fixed — Commander

- **Header actions menu on desktop** — The inline actions menu no longer acts as a scroll box, so the Show work label is fully drawn.
- **Commands on a phone** — The commands palette opens under the More button you chose it from, not over the top of the header.
