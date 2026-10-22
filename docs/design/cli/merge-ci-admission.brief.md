# Published merge CI admission

**Reader and decision:** A factory worker or supervisor reading a task's merge
receipt needs to distinguish confirmed CI from an advisory admission or an
explicit override before describing the delivery as green.

**First two lines:** The existing merge tip remains the receipt. Its next line
says `CI admission: green`, `CI admission: advisory (unconfirmed)` or
`CI admission: override`. The worker close notice carries the same distinction.

**Scannable:** One admission row accompanies the published merge tip in task
show. Existing merge-response CI policy and gate details remain authoritative;
no new table or TUI status is added.

**Action:** This is evidence of the admission decision, not a replacement for
verification or an instruction to retry a committed merge. Advisory deliveries
retain ordinary close gates; override merges do not automatically close.

**Machine output:** `deliverables.merge_ci_admission` is an optional enum with
`green`, `advisory` and `override`. Legacy records without it read as green.
The legacy `green_merge_commit` remains a published merge tip, independently of
admission. Both fields are recorded atomically and cleared on a new park cycle.

## Critique

`terminal-qa: PASS cas-8439-advisory · 12 runs · 0 fail · 0 warn · 0 allowed`

The green, override and legacy gates each also passed 12 runs with no findings
or allowlists. Candidate `cas 3.72.0 (06c4fc3 2026-10-22)` maps to
`06c4fc35d6b2d4404a3f7dce32f1be1bdfd062a7`; subsequent brief-only edits do
not change the tested runtime, fixtures or tools.

```text
Merge commit: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
CI admission: advisory (unconfirmed)
```

Supervisor-run `qa-admission.py` captures the actual candidate MCP task-show
handler over explicitly seeded private fixtures. It scopes only the native
merge/admission rows; full MCP wire output, stderr and 48 unchanged-store
readbacks are retained in cas-8439 artifacts. This proves the read/render path;
the native merge integration separately proves lifecycle persistence and close
policy. Legacy missing state renders green by the compatibility contract.
`terminal-{advisory,green,override,legacy}/report.json` cover 80/120 columns,
four palettes, pipe, C locale, NO_COLOR and one-document MCP JSON transport.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Merge identity is first, with the CI admission distinction immediately below. |
| Fit | 5 | Two native receipt rows, with advisory explicitly qualified as unconfirmed. |
| Craft | 4 | Longest scoped row is 54 ASCII cells; the full SHA remains visible at 80 columns. |
| Theme safety | 5 | Uncolored text reads identically in every palette and monochrome capture. |
| Machine contract | 5 | MCP transport is one JSON document; native type/store tests prove enum round trips and legacy defaults. |

Scoped text and mechanical captures reviewed by fierce-marten-72 on 2026-10-22;
no GUI screenshot, live factory merge or full task-output grading is claimed.


## cas-4fe8: delivery source resolution

An explicit task owns its source: resolve its worker identity from the task and
agent store, then prefer its recorded exact branch and anchor. A missing recorded
branch permits discovery of canonical, shortened task-ID and retry/fix/resume
branches for that worker and task. Several discovered names refuse before merge,
even at the same SHA. The refusal names every candidate in sorted order and tells
the supervisor to record the intended branch and anchor. Existing authorization,
repository binding, anchor, QA and CI gates still apply. No new TUI surface.


## cas-98f9: escaping follow-up

Route hook-parse/trust error paths and merge-conflict path lists through their
existing visible-label helpers. Align hook-path invisible-character coverage
with the Git label policy. Keep path labels single-line and visibly escaped;
render multi-line Git details by escaping each line while retaining structural
line breaks. This preserves readable porcelain residue without permitting a
newline inside a filename to impersonate another diagnostic row.

**First two lines:** Existing refusal names and causes stay visible; path labels
escape controls and invisible Unicode before the operator reads them.
**Scannable:** Conflict paths stay comma-separated; Git residue retains one
physical line per original porcelain or stderr line.
**Readable:** Each detail line escapes controls, tabs, bidi and Unicode line
separators; real LF separators and the trailing LF are retained.
**Machine output:** The existing MCP error code and message fields remain; JSON
encoding carries the message's real line breaks and literal path escapes.
**Omitted:** No new command or terminal layout; unsafe paths still refuse a
copyable Git remedy, while filesystem paths and trust keys retain their bytes.

Three existing regressions cover hook errors under bidi/U+2028/zero-width paths,
unsafe conflict labels, and multi-line Git detail rendering across merge,
checkout-residue and generic failures. Ordinary quoted remedies retain their
interactive Bash argv check; stale hook bytes remain unchanged. These exercise
production functions with disposable fixtures; no live factory spawn or terminal
palette walkthrough is claimed. Integrated execution remains merge-queue-owned.
