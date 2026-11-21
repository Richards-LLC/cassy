### Fixed — Release trains

- **Shipped trains stop asking for another cut** — Was: a delayed green CI
  observation could send `release_cut_ready` after a train had shipped. Now:
  the emitter reloads the train and suppresses cut-ready notices for closed or
  cut trains and for green commits contained in a version tag. Unreleased
  trains retain their existing green-after-red notification and deduplication.
