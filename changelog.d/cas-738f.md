### Fixed — Release train

- **Delayed queue failures still trigger repair** — A failed queue run listed
  after its exit notice now returns the parked task for its repair cycle
  without duplicating the delivered relay.
- **Latency receipts query recent queue history** — Queue attempt lookups start
  at the first child commit's date, with a one-week fallback when that context
  is missing, instead of paging through the repository's entire run history.
