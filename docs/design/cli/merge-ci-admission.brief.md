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

Pending commit-bound candidate captures; no terminal PASS claimed yet.
