## Core Concepts

### Artifact inventory, retention and candidate fallback

Artifact list reads the named task's synced publication references as well as its local artifact ledger. CLI JSON and MCP structured results carry `task_id`, `state: "partial"` and `artifacts`; the local catalog cannot prove an exhaustive Cloud inventory. Listing does not fetch bytes or mutate task/publication records. Explicit artifact fetch remains the recovery route for a known ID missing from synced notes.

The factory performs bounded host retention across registered projects on idle maintenance and above 80% disk usage. Expired artifacts retain a 14-day window and live/release citation protection. Dead-owner targets and resolved review builds are regenerable; citations into build folders do not preserve them. Source ownership, unpushed commits, active processes, ambiguous references and unsafe paths continue to preserve data. Sweep logs include per-project/category bytes and every considered path; preflight reports disk pressure with the last sweep outcome. `cas factory gc cleanup --dry-run` previews retention.

Workers request candidates through `factory action=candidate_request`. If the queue is unavailable or the host lacks its sandbox, the supervisor runs `cas qa candidate build --task <task-id> --rev <sha>` directly and supplies its receipt.
