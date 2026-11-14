### Fixed — Factory

- **Capacity waits show the load measured now** — Was: `worker_status` and
  the 20-minute supervisor notice repeated the figure from the last refusal,
  so a wait could still say "1-minute load 52.36" after the load had fallen
  well below the limit. Now: both show the refusal's figure as "cause at last
  refusal" next to a fresh "now" reading of load, builders and disk.
