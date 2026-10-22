# Init phase output

| Field | Contract |
| --- | --- |
| First two lines | During initialization, each completed phase identifies its result; the existing final initialized location remains the overall verdict. |
| Scannable | A pipe gets one completed success or failure line per phase, with its label and detail; a real TTY keeps the existing spinner frames and completion redraw. |
| Readable | Configuration and file paths stay beside their phase label so a log shows which operation completed or failed. |
| Machine output | `cas init --json` keeps one result object with status, path, agent and integration outcomes and next_steps; it bypasses these human phase messages. |
| Omitted | Piped output omits transient spinner frames, carriage-return redraws and animation delays; terminal output retains its existing bytes. |

The shared step is reached by defaults (`init --yes`) and by fresh or
reconfiguration wizard confirmation. A non-TTY success or action failure must
append exactly one phase line. Whether animation was requested does not make a
pipe capable of redraws. TTY with animation disabled still retains its existing
first frame and completion redraw for compatibility.

## Evidence plan

Extend the existing CLI init regression through defaults and interactive
confirmation, each with success and a real `.gitignore` filesystem failure.
Read config/store state back after each operation. Full-build QA must complete
both human entry paths under pipe, retain TTY bytes against the source baseline,
and check JSON as the adjacent surface. Candidate and critique receipts will be
recorded after execution; no rendering pass is claimed yet.
