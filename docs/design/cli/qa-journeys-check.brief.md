# Journey check history recovery

| Field | Contract |
| --- | --- |
| First two lines | Keep the journey verdict and problem/debt counts first; a damaged comparison receipt must not obscure the current check. |
| Scannable | Retain debt-kind counts and change summary, then one history-reset notice when the prior receipt is replaced. |
| Readable | The notice states that history restarted at the current check; verbose mode already shows individual findings and needs no hint to itself. |
| Machine output | `--json` remains one CheckReport document; debt_summary adds optional history_notice, retains history_status=recorded, and uses null previous_total/delta/grew for the fresh baseline. |
| Omitted | Do not dump corrupt metadata or parse details; genuine database/write failures retain history_error and unavailable status. |

The current metadata key alone is replaced under the existing transaction; other
project/registry histories stay intact. No manual deletion is required.

## Critique

terminal-qa: PASS cas-c58e · 12 runs · 0 fail · 0 warn · 0 allowed

Supervisor proof #543044 uses candidate c3dba8a2e; captures and report live in
task cas-c58e's `terminal/`. The direct verbose capture omits the Details hint.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Verdict/counts remain first, then kind counts and baseline state. |
| Fit | 5 | The 80-column capture's maximum width is 68; reset notice is one line. |
| Craft | 4 | Recovery states what was replaced and saves a usable baseline. |
| Theme safety | 5 | Dark/light/Solarized, pipe, NO_COLOR and C-locale checks pass. |
| Machine contract | 5 | JSON is one document with the reset notice and null comparison. |

Scored by the implementing worker from the supervisor's real candidate captures,
2026-10-22. This critique changes documentation only; candidate runtime inputs
remain those of c3dba8a2e.
