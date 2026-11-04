# Epic design-review lift

| Field | Contract |
| --- | --- |
| First two lines | Identify the epic whose pause was lifted, then say staffing can resume. |
| Scannable | One verdict line and one consequence line; the refusal names the trigger epic and reports. |
| Readable | The supervisor prompt asks for a review against the plan before the operator records a reason. |
| Machine output | `--json` returns one object with `epic` and `status: resumed`, without a human banner; refusal uses the existing CLI JSON error envelope. |
| Omitted | The review outcome remains in the epic's decision note and the operator lift record, rather than being repeated in the success receipt. |

The command has no color, cursor control, table or locale-specific glyphs.
Operator authority and a non-empty reason are required. Unit and MCP integration
coverage exercise the mutation and refusal paths; an interactive operator's
successful terminal invocation is reserved for release QA.
