### Fixed — Factory

- **Armed merges re-run a cancelled lane run once** — Was: a lane job that
  hit its time limit reported cancelled, nothing replaced it, and the armed
  automatic merge stopped until someone re-ran CI and asked again. Now: the
  armed retry re-runs that workflow run once, keeps waiting while the original
  cancellation is still the latest result, and merges when the re-run turns
  green; it stops with one notice only when the re-run is cancelled too.
