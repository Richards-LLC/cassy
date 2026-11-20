### Fixed — Factory

- **Armed merges re-run a cancelled lane run once** — Was: a lane job that
  hit its time limit reported cancelled, nothing replaced it, and the armed
  automatic merge stopped until someone re-ran CI and asked again. Now: the
  armed retry re-runs that workflow run once and merges when it turns green; a
  second cancellation still stops with one notice.
