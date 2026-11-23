### Fixed — Train merge

- **A long train no longer stalls at its first delivery** — The daemon used to start the reviews for every later delivery before it took the head's own step. On a train of 40 or more deliveries that took longer than its two-minute advancement lease, so each pass lost the lease before reaching the head, and the train never advanced. The head now takes its step first. Later reviews start afterwards, renewing the lease before each one, and they stop as soon as the lease is lost (GH #2311).
