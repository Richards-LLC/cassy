# Handoff: cas-0632

Task is started on `factory/golden-cobra-72-cas-0632` at `636b0d84eebb85819d2d3b2b00f8542a6bc98312`. No implementation edits yet.

Acceptance scope:

- Treat a consumed `changelog.d/<task>.md` fragment as delivered when its entry appears under a released version in target `CHANGELOG.md`; a missing entry remains a drop.
- Add one idempotent publish/backfill step for the four shipped trains and their children/epics. Move open or unstarted children to the next open train. Report and leave `cas-526e` open because it has two unmerged commits.
- Add the three required behavior tests: consumed fragment accepted, fragment absent from `CHANGELOG.md` still dropped, publish backfill idempotence.

Start notes identify the source as `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/receipt_scope.rs` and the release publish path. `cas-2581` is the repro. Record a batch receipt for shipped children, close eligible children and epics, and preserve the `cas-526e` exception.

Supervisor constraint: `rustc` is not on minimal PATH yet. Do not prefix PATH. If a capped red/green command fails at `rustc -vV`, note the blocker, set task status blocked, message the supervisor, then wait for the operator's links.
