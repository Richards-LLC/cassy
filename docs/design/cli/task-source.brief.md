# Task source readback

| Field | Contract |
| --- | --- |
| First two lines | Keep the existing task/start status and target context; immediately follow with the recorded delivery source branch and full start-base SHA. |
| Scannable | `Recorded source branch:` and `Start base:` use identical labels in compact/full show and brief/full start; a parked branch remains visible separately. |
| Readable | When start observes a different checkout branch, an ASCII `WARNING` names both branches and points to the supervisor's audited `task action=rebind_branch` recovery. |
| Machine output | These are MCP text-content additions only; no request/response schema or stored binding changes, and no inferred source is reported for legacy tasks without a record. |
| Omitted | No automatic rebinding or new refusal; task source binding, assignments, leases, rebind authorization and close behavior keep their existing contracts. |

The recorded branch/base describe the immutable start boundary, not the checkout's advancing HEAD. Commit progress on the same branch is not a mismatch. The source readback and mismatch warning stay in the start response prefix so optional context clipping cannot hide them.

## Critique

This surface is MCP text content, not an ANSI terminal renderer; palette/PTY gates do not apply. The real-tool regression exercises compact/full show, brief/full repeated start, a matching checkout and an owned replacement branch, and confirms the persisted boundary and checkout remain unchanged.
