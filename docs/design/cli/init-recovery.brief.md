# Init recovery errors

| Field | Contract |
| --- | --- |
| First two lines | State that initialization is incomplete, then tell the user to fix the reported cause and run `cas init --force`. |
| Scannable | Keep the underlying provisioning error after the recovery instruction so the blocked file remains identifiable. |
| Readable | Add recovery advice only when this run created a store before failing; a pre-existing store or early refusal keeps its actual error and remedy. |
| Machine output | Failed JSON init exits nonzero without an initialized result; the recovery instruction stays on stderr. Successful and already-initialized JSON retain their existing shape. |
| Omitted | Do not reset existing stores or config on force retries, or replace an unreadable or invalid `.mcp.json`; preserve the file and report its cause. |

The shared recovery boundary covers JSON, defaults and wizard entry paths for
Claude and Codex. Claude and Grok share the MCP JSON writer; both must propagate
its refusal. A valid existing file must still retain other servers. Recovery
requires fixing the obstruction before retrying with `--force`.
Force retries preserve existing configuration bytes in JSON, defaults and wizard
modes. The explicit non-force wizard choice to reconfigure remains intentional.
New JSON projects save their full configuration before fallible provisioning,
so a failed run's force recovery keeps the same configuration as a clean init,
including hooks, tasks and the enforce defaults for upkeep and the QA registry.
Existing MCP roots and `mcpServers` fields must be JSON objects; arrays, null and
scalars are refused without mutation or panic.
The MCP phase keeps its leading refusal short enough for 80 columns; root and
server-shape causes remain in the detailed error chain. The generic phase and
top-level diagnostic both report failures; their shared duplication is tracked
separately in cas-d258.

## Evidence plan

Run the existing JSON init test with malformed-file preservation, and the CLI
recovery table with real blocked provisioning, plain retries and forced repairs.
Walk the supervisor-built candidate and inspect the new stderr at 80/120 columns;
record terminal captures and the machine-output/file read-backs in the task ledger.
