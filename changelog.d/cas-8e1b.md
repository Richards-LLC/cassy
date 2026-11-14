### Fixed — Releases

- **A release that adds a column no longer breaks its own cut** — Was: the
  candidate's `release preview` read the live store on the installed CLI's
  older schema and failed with "no such column" whenever the release added a
  column to an existing table; `release track` then reported "no open release
  train" for the same reason. Now: preview reads a private, migrated backup of
  the store and leaves the live store byte-identical; track, the cut's first
  write, applies the candidate's migrations before reading the train and
  stops the cut if one fails. `scripts/release.sh` names the live store with
  `CAS_ROOT` for both candidate calls, so a cut from a Cassy worktree works.
