# Rolling integration

Every daemon-observed `worktree_merge` into an epic schedules a rolling union
of fetched `origin/main` and open epic branches, sorted by task creation time.
The branch is `integration/<checkout-name>` (non-alphanumeric characters other
than hyphens become hyphens). Branchless epics contribute no code. Local-only
and remote-only epics are supported; when both refs exist, the newer descendant
wins, and divergent refs require reconciliation.

The daemon uses the shared repository's delivery-target lock and a detached
`.cas/epic-integration-<checkout-name>-merge` checkout. A newer event supersedes
the daemon's previous job; independent sessions serialize under the lock.
Assembly conflicts preserve source refs and the last integration ref, identify
pairwise Git conflicts and files, and invalidate the previous passing receipt.
Higher-order conflicts name the participating prefix without inventing a pair.

A clean union updates the integration ref with compare-and-swap, then runs the
bounded workspace nextest sweep. The process has a separate session, ignores
SIGHUP, inherits the configured Cargo job cap, and resolves Zig from the same
locations as the release gate. Build pressure leaves a DEFERRED receipt and an
owner alert, with a retry after 30 seconds. Tests run asynchronously; reports are delivered when the daemon
reaps the completed job, without waiting inside the merge MCP request.

Each successful equivalent row is also written to
`.cas/merge-sweeps/row-cache/<row>.<key>` in the release gate's
`row-cache-v2` format. The receipt binds the row to the shared Git checkout
identity, input tree hash, normalized environment and toolchain fingerprints,
gate implementation digest, exact sweep SHA, and a UTC epoch. The gate's
`--reuse` mode accepts a sweep row only when those fields match, the receipt is
younger than 24 hours, and `integration.json` is `PASSED` for the current tip.
The current sweep emits `nextest`; `workspace-tests` and `doctests` use the
same format when a future sweep executes those rows. Archive, scratch,
identity, procedure, ledger, and cleanliness checks remain live.

On failure, reported failing targets are rerun on the prior integration tip and
on preceding union prefixes to identify the introducing merge. Compilation
failures without test names require a workspace rerun. Missing or timed-out
probe evidence is reported as incomplete attribution. Each affected epic gets a
note; each distinct owning supervisor gets one idempotent notification routed
to its own factory session. Closed epics are omitted on the next merge event.

For release assembly, create a clean detached or `release/` worktree from
`origin/main`, then run:

```bash
scripts/release-train.sh <version> <release-worktree> --assemble
```

The action fast-forwards from the tested integration tip. It refuses a busy
lock, nonpassing receipt, changed input refs, changed integration tip, dirty
checkout, or protected destination branch. It neither resolves conflicts nor
resets the destination. Continue with the normal `--gate` and `--pipeline` steps.
The daemon receipt is `.cas/merge-sweeps/integration.json`; sweep logs are in
`.cas/merge-sweeps/`. The integration ref is local to the repository shared by
the factory sessions and the release worktree.
