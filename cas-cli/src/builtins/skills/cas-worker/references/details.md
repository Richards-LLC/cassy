# Details — Evidence, Sync and Resume

Read for a report/evidence task, a requested sync or a context-resume handoff.
The task/coordination schemas supply exact fields and valid actions; use
`cas-search` for exploratory retrieval and `rg` for exact code matches.

## Read-only evidence

Use task/search/coordination records, `.cas/logs`, Git and exported artifacts
first. If insufficient, note the missing evidence, then snapshot the store with
`sqlite3 <db> ".backup <path>"` into the configured task artifacts directory.
Avoid copying a database, worktree or build cache into RAM-backed temporary
storage; it competes with every session on the host. For live inspection use a
read-only URI such as `file:/abs/path/to/.cas/cas.db?mode=ro`, never unrestricted
SQLite access. Workers do not edit the shared database directly.

## Requested sync

Checkpoint dirty work as a commit, then `git rebase <branch>` onto the exact
branch the supervisor named. Resolve conflicts or escalate the affected files.
Use a commit instead of `git stash`: all worktrees share the stash stack.
Choose the local or remote ref from current assignment evidence; do not assume
their freshness. A frozen parked delivery stays on its original branch; a new
assignment uses its own per-task branch from the target tip.

## Resume state

Read task state with `action=show` or `action=start brief=true` after a context
clear. Supplement milestone notes with bounded `state_patch` when useful:

```text
task action=update id=<task-id> state_patch='{"phase":"verify","receipts":[{"command":"python3 scripts/check.py","exit_status":0}],"files_touched":["scripts/check.py"],"next_step":"push branch"}'
```

Fields are `phase`, `receipts` (`command`, `exit_status`), `files_touched`,
`decisions`, `next_step`; `null` deletes a field. Use `id` for the task and
`notes` for appended notes. `platform_proof` supplies non-Rust platform evidence.

## Credentials and routing

For a database branch, ask the supervisor with `blocker=true`, task ID and
reason. The supervisor writes `DATABASE_URL` to `.env.cas-db`; source it without
printing or committing it. Workers do not create branches or hold Neon credentials.
Load only `task` and `coordination`; `server_start`/`server_list` are the assigned
server exception, with `cas-servers`. Fleet, merges, cleanup and database control
belong to the supervisor even where the runtime exposes them.

Operational bugs use `cas config get issues.repo` and
`issues.components.{cassy,violet,cloud}`. File in the matching tracker before
moving on; CAS source bugs become in-repo fix tasks. With no tracker, record a
task note. Use the supervisor's filing reference for public-safe reports.
Put long evidence in the configured task artifacts directory and send its path;
message and note schemas enforce the traffic limits.
