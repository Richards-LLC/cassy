# Brief: `cas doctor`

| Field | Sentence |
| --- | --- |
| First two lines | Whether the local store is healthy, how many findings need a hand, and which project and version this is about. |
| Scannable | One line per healthy group with its marks; one row per finding under its group, including the `SessionStart budget` guard with its byte headroom. |
| Readable | Each finding's cause (at most three lines) and its remedy under `→`; everything whole under `--verbose`. |
| Machine output | `--json`: one array of `{name, status, group, message, remediation, duration_ms, phase}`; nothing else on stdout. |
| Omitted | Per-check timings, the slow-phase table, and instances beyond the first of a repeated finding — all under `--verbose`; detailed payload sections remain in the SessionStart references. |

## Rendering decisions

- The verdict word is the count of findings (`2 warnings`, `1 error`, `healthy`); the healthy
  count, project, and version are the detail.
- Colour touches marks only. The old renderer painted every line in its status colour, which
  on a light terminal put `#e4e5eb` text on white (1.2:1) and amber paragraphs at 2.8:1.
- A token wider than the column (a cloud scope id) is cut with `…` instead of being split
  across lines.
- Consecutive findings with the same name and status fold into one row with `×N`.
- Cause and remedy each stop at three lines with `…`; the receipt names `--verbose`.
- The receipt keeps the exact `N ok · N warnings · N errors · time` grammar scripts grep for.

Message text itself (the cause and remedy sentences) belongs to the doctor self-heal work;
this brief covers the render only.

The cloud queue warning now separates pending registration conflicts from retryable transport
failures: it reports parked rows and gives `cas cloud project set <registered-canonical-id>`
or the cloud-owner alias path, then directs the operator to sync; it does not prescribe
`cas cloud queue --retry` for parked rows.

The MCP upstream reachability check is one grouped row: it reports the number probed, names
reachable upstreams, and includes the bounded credential-free cause for each unavailable one;
the same message is carried in the existing `--json` check object.

The host `hub service` row now reports `service installed but inactive, detached hub running`
when an installed manager is inactive while a non-service hub owns the runtime lock. The same
message is carried by the existing JSON check object; healthy service state retains the concise
inspection hint.

The `SessionStart budget` row reports supervisor guidance bytes and remaining headroom below
the protected 8,192-byte ceiling. It warns at less than 512 bytes of headroom and directs the
operator to move detail into `cas-supervisor/references/`; its status and message are carried
by the existing `--json` check object.

## Critique

Before (build `eda3dfd1`): `terminal-qa: FAIL cas-doctor · 12 runs · 841 fail · 24 warn` — 788 contrast, 28 word-split, 24 overflow, 1 unicode-without-fallback.

After (with the doctor self-heal epic merged): `terminal-qa: PASS cas-doctor · 12 runs · 0 fail · 4 warn · 0 allowed · docs/design/cli/captures/after/cas-doctor/report.md` — the four warns are one unconfirmed `word-split` heuristic on a remedy line that word-wraps exactly at the column edge.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | verdict is line one; each finding's remedy is its own `→` line |
| Fit | 4 | healthy groups and repeats collapse; the remaining causes are still sentences authored as paragraphs |
| Craft | 4 | no split tokens, no overflow at 80 or 120; the `×N` widens the name column for the whole group |
| Theme safety | 5 | marks only; four palettes pass |
| Machine contract | 5 | `--json` unchanged, one array |

The registration-conflict fixture capture was also run at
`/home/pippenz/.cas/artifacts/cas-998a/terminal-qa/cas-doctor-registration-conflict-final/report.json`;
it reports 36 truncation findings from existing doctor checks plus the new remedy. The focused
`doctor_queue` tests pass 4/4; the shared doctor renderer remains a follow-up surface outside
this task.

Task capture: `terminal-qa: PASS cas-doctor-fcba · 12 runs · 0 fail · 8 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-fcba/terminal-qa/cas-doctor/report.json`.
The eight warnings are the existing 120-column word-split heuristics for temporary registered
roots and the quarantine remedy; the new reachability row did not introduce a failure or overflow.

SessionStart budget capture: `terminal-qa: PASS cas-doctor-budget · 12 runs · 0 fail · 4 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-6a20/terminal-qa/cas-doctor-budget/report.json`.
The four warnings are the existing 120-column word-split heuristic on the quarantine remedy;
the new `SessionStart budget` row is present in the 80-column capture and the JSON document,
with no new failure or overflow.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | The first two lines give the warning count and the first group; the new budget status is a grouped Config row. |
| Fit | 4 | The byte guard is one scan-friendly row and remains a typed JSON check rather than a separate output shape. |
| Craft | 4 | The row names guidance bytes, headroom, and the protected ceiling in one line at 80 columns. |
| Theme safety | 5 | The four palettes, `NO_COLOR`, and C-locale runs have no mechanical color or Unicode failures. |
| Machine contract | 5 | The JSON run is one document and includes `name`, `status`, `message`, `phase`, and timing fields. |

Scored by Codex on 2026-09-09; hierarchy, fit, and craft floors hold.

Task capture: `terminal-qa: PASS cas-doctor-hub-service · 12 runs · 0 fail · 0 warn · 0 allowed`
against the built binary with scrubbed HOME/XDG state and a stub `systemctl`; the inactive
detached-service warning appeared in both human and JSON host reports.

Task capture: `terminal-qa: PASS cas-doctor-866 · 12 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-35be/terminal-qa/cas-doctor/report.json`.
The rebuilt healthy-baseline host report stayed within 80/120 columns across four palettes,
pipe, `NO_COLOR`, C locale, and JSON. Fixture captures at
`/home/pippenz/.cas/artifacts/cas-35be/qa/` separately show managed stale-skill cleanup,
preserved unmarked user skills, intentional Claude/Codex/Grok twins, unexpected drift, and
the attributed scratchpad policy row.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | Verdict first; host skill and scratchpad rows are grouped, with remedies under `→`. |
| Fit | 5 | Intentional namespace twins collapse to a healthy row; only unexpected drift and stale ownership need action. |
| Craft | 4 | Safe remedies are copyable and the real-build captures have no terminal-qa failures or overflow. |
| Theme safety | 5 | Four palettes, piped, `NO_COLOR`, and C-locale runs pass without colour or Unicode defects. |
| Machine contract | 5 | JSON remains one document and carries the normalized duplicate, stale-skill, and scratchpad statuses. |

Scored by Codex on 2026-09-14; hierarchy, fit, and craft floors hold.

Task capture: `terminal-qa: PASS cas-af06 · 12 runs · 0 fail · 4 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-af06/terminal-qa/report.json`.
The new project-alias warning and dry-run/apply messages preserve the existing doctor
renderer contract; the four warnings are the pre-existing 120-column quarantine-remedy
word-split heuristic.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | The doctor verdict remains first; project-alias findings put the missing alias and one copyable remediation sequence in the finding/remedy pair. |
| Fit | 4 | Healthy alias checks collapse; drift and queue reasons expand only when actionable. |
| Craft | 4 | Remote slugs and reason counts remain whole tokens, with existing width truncation and `--verbose` escape. |
| Theme safety | 5 | Terminal QA passed all four palettes, piped, `NO_COLOR`, C-locale, and JSON runs. |
| Machine contract | 5 | `--json` remains one check array with stable `name`, `status`, `message`, and `remediation` fields. |

Scored by Codex on 2026-09-18; hierarchy, fit, and craft floors hold.

Task capture: `cas doctor --fix --json` keeps nested foreign-scope purge previews
inside the top-level checks array; strict decoding now accepts both `--fix` and
`--fix --yes` as exactly one JSON document. The nested purge/sync renderers stay
silent when doctor owns the report.
