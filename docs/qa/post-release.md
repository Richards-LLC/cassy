# Post-release checks

Configure a project-owned read-only executable in `.cas/config.toml`:

```toml
[qa.post_release]
command = "./scripts/check-release --read-only"
window_mins = 30
interval_mins = 15
timeout_secs = 120
release_branches = ["main"]
env = ["QA_READ_TOKEN"]
```

The executable runs from the project root, with whitespace-separated arguments
and no shell expansion. Its path must stay inside the project. The environment
contains a minimal PATH and LANG plus only the named allow-list variables.
Scripts must implement read-only access; Cassy cannot enforce database permissions.
Use read-only credentials. Raw error stdout/stderr is never stored.

The operator can start a check after the merge lands from their own terminal:

```bash
cas qa post-release start --epic cas-1234 --sha <full-release-sha>
cas qa post-release status --epic cas-1234 --json
```

Start `cas serve` after configuring the check and keep it running. One server
owns a project-wide OS lease and the background job; other servers silently skip
it. Unconfigured projects spawn no runner. The owner polls once per minute; the
CLI returns immediately. A new server can acquire the lease after its owner exits.
The default schedule observes at approximately +0, +15 and +30 minutes, with a
120-second timeout per observation. Concurrent serve processes share a per-epic
file lock. Receipts survive restarts; a missed observation interval fails the
check instead of inventing historical observations. Repeating start for the same
SHA preserves its window. A fix-forward SHA starts a new window and archives
the previous receipt.

Each successful invocation exits zero and prints one JSON object:

```json
{"ok":true,"alarms":[],"checked_at":"2026-10-21T00:00:00Z","readback":{"journey":"signup","subject":"opaque-reference","invariants":["workspace-exists"],"ok":true}}
```

Alarms are objects with `id`, `summary` and `count`. A read-back is optional on
individual runs. The completed window needs at least one successful read-back
with a journey, opaque subject and nonempty invariant list. Any alarm, nonzero
exit, invalid JSON, timeout or failed read-back makes the verdict **failed**.
Otherwise it stays **pending** until the window ends and all observations exist,
then becomes **clean**. A completed window missing a read-back or observation
fails. Treat failure as an incident: file the bug and start a new check on the
fix-forward release SHA.

Each run appends `POST-RELEASE k/n <sha>: ok | ALARM … | ERROR …` to the epic.
The durable receipt lives under the configured project artifacts directory at
`<epic>/post-release.json`; `.cas/operator/post-release/` holds the protected
runner state and recorded SHA. Agents cannot run `start` or write this state.
Legacy state under `.cas/post-release/` is not trusted or imported; restart its
observation with an operator start or let an armed release start automatically.
An epic merge request also arms a durable `branch_contained_in` reminder. When
origin contains a first-parent merge that introduced the epic tip on a configured
release branch, Cassy starts the check against that release merge SHA. A
fast-forward, squash, missing origin ref or merge confined to another branch
cannot establish that identity; use the operator start for observation. Empty
`release_branches` uses the default branch.

During an operator-enabled rapid-response window, pending and failed checks can
be deferred only for a protected recorded release merge that introduced the epic
on a configured origin release branch. Cassy scans `<sha>^1..<sha>`, including
merge-resolution edits: a migration, secret-shaped added line or unreadable diff
keeps the gate. An unverified, fast-forward, foreign or squash SHA keeps the
gate too. A deferred check is recorded in the window's back-fill ledger; closing
the window restores the gate. The background observations continue.

Pending and failed checks refuse release-note posts and epic close. Violet
posts marked `release_notes: true`, or whose first nonempty line has a rubric
release heading (`Live on production` / `Staging`, followed by `User` / `Dev`),
use this gate, including `cas violet post`. Ordinary messages in the same
channel remain exempt.
Specify `epic_id` or `task_id`, or use the session's focused epic.
The CLI accepts `cas violet post --release-notes --epic <id> …` (and the
same flags on `cas violet thread`). Merge, deploy,
tag and publication remain available during the observation window.

An operator can authorize proceeding by recording a decision note on the epic:
`POST-RELEASE OVERRIDE: <operator's words>`. The reason must be nonempty, expires
after six hours, and each use appends an audit note. This override applies only
to the post-release check; deliverable verification has its own publication gate.
Without `qa.post_release.command`, these gates stay off.
