# Hub publication during update

| Field | Contract |
| --- | --- |
| First two lines | A verified loopback hub keeps the update successful; if Serve is inactive, name it and give a command to retry. |
| Scannable | One hub outcome line gives prior state, start/restart and endpoint; a warning and remedy follow only when publication is unavailable. |
| Readable | Preserve the Tailscale diagnostic so missing CLI, login, permissions and route conflicts remain distinguishable. |
| Machine output | One update JSON receipt retains `hub_restart.action`, `verified`, `loopback_verified`, `transport_verified`, `transport_warning`, `public_url`, `failure`, `remedy`; optional transport failure means `verified=true`, `transport_verified=false`, `failure=null`. |
| Omitted | Service commands and startup detail remain in the hub log and existing recovery evidence. |

## Verification boundary

Worker source inspection and syntax checks cover the receipt branches. The
supervisor owns Rust execution and terminal captures from the new build under
the explicit no-cargo instruction. Existing strict `cas hub status` diagnostics
continue to distinguish public transport health from loopback readiness.
