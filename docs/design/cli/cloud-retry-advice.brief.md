# Cloud push and doctor retry advice

| Field | Contract |
| --- | --- |
| First two lines | Push and doctor retain their warning and queue counts; a blocked-only rejection set points to its repair without an immediate unscoped retry. |
| Scannable | Existing push summary segments and doctor queue rows keep their order; rejection reason counts remain visible. |
| Readable | Each rejection keeps its existing explanation, including Cloud issue 184 and the conditional scoped retry for invalid_json_nul. |
| Machine output | Existing JSON fields and counts are preserved; only human remediation wording changes, with no MCP schema or description changes. |
| Omitted | No new queue policy, payload transformation, network retry, layout or styling is introduced. |

## Acceptance and evidence plan

- Blocked-only invalid_json_nul, version_gate and unknown reasons omit the immediate generic queue-retry command.
- Transient rejection advice and ordinary failures retain their retry action.
- Doctor continues to name how other pending work can be pushed while explaining that rejected rows need their repair first.
- Existing push-renderer and real SQLite doctor fixtures supply red/green assertions through lane CI. No local Cargo is allowed.
- Real-build terminal QA and width/theme review remain pending a usable CI-built binary; an installed older binary is not final-build evidence.
