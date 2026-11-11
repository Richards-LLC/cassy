### Fixed — Factory merges

- **Pre-merge verifier response** — Was: merging a parked delivery could wait for verifier account probes and startup work until the tool timed out with an unknown outcome. Now: the merge returns its held dispatch after claiming one verifier, while startup continues in the background. Startup failures retain the existing claim recovery and owner notice.
