# Brief: `cas cloud queue`

| Field | Sentence |
| --- | --- |
| First two lines | Whether queued cloud work is retryable or parked, followed by the one identity repair needed for parked registration conflicts. |
| Scannable | The normal summary keeps queue counts compact; `--verbose` adds one indented `parked-with-reason` row only for structured parked outcomes. |
| Readable | The parked reason preserves the cloud's conflict code and points to the registered identity; the operator can then use the doctor remedy to converge the project. |
| Machine output | `--json` remains one queue document with each item's `last_outcome`, `last_reason`, and `failed_client_version` fields; no human banner or colour is emitted. |
| Omitted | Raw server response envelopes remain in the sync failure; parked queue rows show the structured reason code instead of duplicating that long diagnostic. |

## Rendering decisions

- A parked row is visibly distinct from a transport retry: `parked-with-reason` is an
  explicit status label, while the retry count remains unchanged.
- The reason is rendered only in verbose mode, preserving the compact default queue view.
- The structured fields are carried by the same `QueuedSync` value used for JSON output, so
  human and machine consumers observe the same state.

## Critique

Pending terminal QA receipt after the rebuilt binary is captured.
