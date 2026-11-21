### Fixed — Verification recovery

- **Recover expired reviews after verifier death** — Retrying a merge can release
  an expired claim when its registered local verifier is confirmed dead. The retry
  retains the exact candidate and supervisor, revokes the old verifier authority
  with an audit event, and allows one replacement review. Live, remote and
  unverifiable children keep their claims.
