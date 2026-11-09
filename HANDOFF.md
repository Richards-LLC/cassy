# Worker handoff: cas-8391

- Branch: `factory/kind-lark-36`; delivery: `push_branch` to `epic/release-train-vnext-cas-db63`.
- Current objective: suppress supervisor-stall assignment nudges while a live operator ask is open and every available worker is holding a blocked task; keep taskless workers eligible.
- Current implementation is committed at `a77462eb57dd8459cb9f4996bac22c2210c7c7d7`; this checkpoint adds the fixture correction and removes one unused import.
- First capped test run compiled `cas` and failed before the behavior assertion because the test inserted an operator-ask reminder before initializing the reminders table (`assignment_evidence_tests.rs:243`, `no such table: reminders`). Two injected guard tests passed.
- The fixture now initializes `reminder_store` before inserting the reminder. Rerun the focused test and the cas-e239 operator-blocker regression; capture passing capped receipts at the final delivered tip before parking.
- Focused command: `cargo nextest run -p cas --lib -E 'test(operator_ask_does_not_count_workers_holding_blocked_tasks_as_idle)'`.
- Regression command: `cargo nextest run -p cas --lib -E 'test(operator_blocked_epic_does_not_stall_cas_e239)'`.
- Changelog fragment exists at `changelog.d/cas-8391.md`. Task has no live journey; close reason should state the `no case:` decision or defer it to the next train's batch QA.
