### Fixed — Releases

- **Preserve prebuilt release archives at queue landing** — Release admission requires singleton merges with every entry passing its checks. Queue configuration and repair commands retain at least five parallel builds while landing one PR at a time, preventing a batched landing from forcing a full archive rebuild.
