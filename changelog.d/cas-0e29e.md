### Fixed — Releases

- **Release preview and task closes walk shared history once** — Was: each
  release member's task commit replayed every delivered path through the same
  train history, and each owner re-judged the same rewritten lines on every
  commit. A 30-member preview took about 17 minutes. Now: each path is walked
  once for every member and commit, and each changed line is judged once per
  commit. The same preview took about 11 minutes under heavy host load, with
  identical results.
