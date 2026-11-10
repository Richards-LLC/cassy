### Fixed — Factory

- **Public registration cannot inherit supervisor authority** — A public `register` or `session_start` call now rebinds only to an identity the server has authenticated, either its own server-internal identity or the harness's `CAS_SESSION_ID`. A row created by an earlier public registration can no longer approve verifications.

### Fixed — Commander

- **Header actions menu on desktop** — The inline actions menu no longer acts as a scroll box, so the Show work label is fully drawn.
