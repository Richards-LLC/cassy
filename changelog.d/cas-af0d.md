### Fixed — Cloud sync

- **A parked delivery rejected by a newer cloud revision now reaches the cloud after one pull** — Was: when a team task push was rejected with a revision conflict and the cloud row held an older Open state than the local park or close, the pull kept the local row but parked the queued push for good, so the cloud revision was never adopted and every retry was rejected again. Now: that same pull adopts the cloud revision and rebuilds the queued push from the local row, so the next push is accepted.
