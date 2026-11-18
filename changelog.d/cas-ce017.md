### Fixed — Factory

- **Recycled workers start their kept task** — Was: a recycled worker's task start was refused for up to 30 minutes by its retired predecessor's lease. Now: the new session of the same worker name reclaims it at once.
- **Train commands see the live supervisor** — Was: `cas factory train drop` and `train merge` run from the supervisor's shell refused with "requires a live registered supervisor", and a dead supervisor still marked active blocked train takeover. Now: both resolve the supervisor from the harness, and takeover checks the recorded process is still running.
