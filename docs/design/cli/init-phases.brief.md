# Init phase output

| Field | Contract |
| --- | --- |
| First two lines | During initialization, each completed phase identifies its result; the existing final initialized location remains the overall verdict. |
| Scannable | A pipe gets one completed line per phase; failure rows carry the phase label and leave the complete cause to stderr; a real TTY keeps the existing spinner frames and completion redraw. |
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
and check JSON as the adjacent surface. Candidate and critique receipts are recorded below; the complete ledger retains
the adjacent JSON defect.

## Critique and completed proof

Supervisor candidate `b0d496086b3afaae71b1f7e2c92e5299a00f49c6`,
`cas 3.71.0 (b0d4960 2026-10-22)`, SHA-256
`da4025368721893e1816cc1463da5b4e84ed6a941868f9337b3ed59c95aab9a3`.

- `terminal-qa: PASS init-yes · 11 runs · 0 fail · 0 warn · 260 allowed`
- `terminal-qa: PASS init-interactive · 11 runs · 0 fail · 0 warn · 388 allowed`
- `terminal-qa: PASS init-reconfigure · 11 runs · 0 fail · 0 warn · 464 allowed`

All final full-flow gates contain zero piped-control findings, including allowed
findings. Exceptions retain only exact baseline-matched palette, tooling-width
and reconfiguration-ellipsis findings, explicitly approved by the supervisor.
The original path-specific allowlist mismatch and failed report are preserved
in the task ledger; no redraw exception remains.

Actual baseline versus candidate TTY captures are byte-identical for defaults
and fresh interactive init at 80/120 UTF-8, and interactive C/NO_COLOR. Both
human pipe entries, reconfiguration and real filesystem failure append one
completed phase line per operation, with zero carriage returns or ANSI codes.
Read-back confirms config, store/tasks, hooks and MCP for successful human flows.
The adjacent JSON flow returns one valid result but omits `.mcp.json` on both
baseline and candidate; this pre-existing defect remains **FAIL**, tracked as
`cas-7b8d`. The eight-cell ledger is 7 PASS / 1 FAIL, never 8 PASS.

| Dimension | Changed phase scope | Full-command limitation |
| --- | --- | --- |
| Hierarchy | 4: each completed operation leads with its outcome and label | Overall init success remains after its phases. |
| Fit | 4: pipe phases append once, without transient frames | On a terminal, a detail wider than the screen continues on rows under the detail column, breaking only between items; a pipe keeps one line. |
| Craft | 4: aligned completion rows retain one consistent grammar | Existing Next steps box misalignment remains in cas-8bc8. |
| Theme safety | 5 for plain pipe output, which has no color | Existing TTY light-theme contrast remains cas-f15f. |
| Machine contract | 5 for phase controls and error exit behavior | Adjacent JSON MCP provisioning remains cas-7b8d. |

The 80-column dark/light HTML captures and raw pipe transcript were inspected.
This is a scoped phase-output pass under the requirement that TTY bytes stay
unchanged; it is not a full-theme or full-command quality claim. The adjoining
[pipe excerpt](init-phases.pipe.capture.txt) comes from the actual full init;
the ledger retains all complete captures and durable read-backs. Published
portable artifact references are recorded on the task.


## Generic failure diagnostic (cas-d258)

| Field | Contract |
| --- | --- |
| First two lines | A failed phase names the blocked operation; the final stderr diagnostic gives the full cause and, for incomplete new stores, the existing `cas init --force` remedy. |
| Scannable | Completed successes keep their detail column; a failed phase shows its label once without copying the cause onto stdout. |
| Readable | Init-only stderr wraps prose at word boundaries within 80/120 columns, including forced retries of existing stores; every cause and remedy token remains complete. |
| Machine output | `cas init --json` keeps its existing result/error behavior and receives no human phase rows; other commands retain their current diagnostic formatter. |
| Omitted | The repeated phase cause is removed from stdout; no error detail is discarded from stderr, and no new escape flag or file is introduced. |

Supervisor approval #542326 permits an indivisible token wider than the terminal
on its own line, following the existing `wrap_line` contract. Ordinary prose is
wrapped without splitting or truncating tokens. The short MCP refusal remains
unchanged. One existing CLI init regression covers defaults and wizard input,
new and existing stores, real malformed Codex configuration, unchanged obstructing
bytes, and the complete parser cause at 80/120 columns. Terminal QA uses the
supervisor-built candidate; its receipt and critique follow after execution.


## Generic failure critique and proof

Supervisor candidate `9cdc308d33fe23a353cde166f12c4ef11ebbed29`,
`cas 3.72.0 (9cdc308 2026-10-22)`, SHA-256
`61c748722f950b6aab76391dc7981b142b1e816f518a6fee326b86768a810d85`.

- `terminal-qa: PASS init-phase-yes-new · 11 runs · 0 fail · 0 warn · 0 allowed`
- `terminal-qa: PASS init-phase-wizard-new · 11 runs · 0 fail · 0 warn · 0 allowed`
- `terminal-qa: PASS init-phase-yes-existing · 11 runs · 0 fail · 0 warn · 0 allowed`
- `terminal-qa: PASS init-phase-wizard-existing · 11 runs · 0 fail · 0 warn · 0 allowed`
- `terminal-qa: PASS init-phase-short-failure · 11 runs · 0 fail · 0 warn · 0 allowed`

Every gate covers 80/120 columns, dark/light/Solarized palettes, C locale,
NO_COLOR and pipe. All 56 fixture read-backs preserve the obstructing file,
created store and any existing configuration. Adjacent failed JSON init keeps
stdout empty, exits 1 and reports the full cause and recovery remedy on stderr.
The [pipe capture](init-phase-error.pipe.capture.txt) shows the actual candidate.
The task's portable proof contains complete reports, raw captures and `LEDGER.md`.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | The failed phase names the operation; the final diagnostic starts with incomplete-init status and its force-retry remedy when applicable. The existing init banner remains outside this change. |
| Fit | 4 | One cause is on stderr, with phase context on stdout; all prose fits at 80/120 without dropping or splitting tokens. |
| Craft | 4 | The existing phase column and error mark stay consistent; final stderr reserves space for the widest ASCII mark. |
| Theme safety | 5 mechanical | All four palette checks pass with no exception; NO_COLOR/C and pipe checks pass. Rendered HTML inspection was not exercised. |
| Machine contract | 5 | Error exit1, existing empty JSON stdout on failure, no human phase rows in JSON, and unchanged config/obstruction read-backs. |

Scored by warm-tiger-88 on 2026-10-22 from actual captures and mechanical gates.
HTML visual inspection is **NOT EXERCISED**: the browser runtime reported no
browser available after documented troubleshooting. Supervisor #542582 accepted
the computed palette/contrast and layout checks as the substitute for this
plain-text error change; no rendered PNG review is claimed.
