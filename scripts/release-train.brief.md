# Release-train status and receipt brief

| Field | Contract |
| --- | --- |
| **First two lines** | The status output identifies the run and whether publication is verified; the next evidence line exposes green-to-published latency with intervention count. |
| **Scannable** | One stable `KEY=value` receipt row carries interventions, blocker stage names, and both hand-off delays; the human status keeps those metrics beside the latency row. |
| **Readable** | Operators can read the UTC intervention log to identify the subcommand, caller session, source kind, and canonical stage for each detour. |
| **Machine output** | `release-latency-receipt.sh` emits `INTERVENTIONS`, `BLOCKERS`, `GREEN_TO_PIPELINE_SECS`, and `MERGED_TO_PUBLISHER_SECS` as stable `KEY=value` fields alongside the existing latency fields. |
| **Omitted** | Individual intervention log lines stay out of the normal status summary; inspect `<run dir>/interventions.log` when the caller, timestamp, or subcommand is needed. |
