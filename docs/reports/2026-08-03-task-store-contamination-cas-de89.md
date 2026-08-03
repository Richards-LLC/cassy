# cas-de89: task-store contamination and safe recovery

Date: 2026-08-03

Status: the historical write path is fixed, but contaminated databases and an
unclassified sync backlog remain. This report is a recovery plan, not a data
migration. Do not run it as a deletion script.

## Finding

CAS task scope is determined by the database opened by the caller. It is not
stored as task provenance: `crates/cas-store/src/task_store.rs:221-224` assigns
`Scope::Project` while reading any task row, and the `tasks` insert at
`crates/cas-store/src/task_store.rs:291-325` has no project identifier.

The contamination was caused by cloud pull, not by a task create writing to two
databases. Immediately before commit `d69a94cf` (2026-05-12),
`cas-cli/src/cli/cloud.rs::execute_pull` did all of the following:

1. Built `/api/sync/pull` without `project_id` at historical lines 1148-1158.
2. Iterated every task returned by that unscoped request at lines 1189-1194.
3. Upserted each task into the caller's current `task_store` at lines 1196-1198.

Commit `d69a94cf` removed that parallel URL builder and routed the command
through `CloudSyncer::pull`. A second acceptance hole remained: the initial
client filter in `1109b01d` deliberately accepted missing, null, and unexpected
project fields as legacy data. Commit `b490f652` (2026-05-18) made the filter
fail closed. The current guards are at
`cas-cli/src/cloud/syncer/pull.rs:21-75`, the request includes
`project_id` at lines 181-190, and task rows are filtered before upsert at
lines 241-262.

This explains the on-disk shape: unrelated projects and scratch directories
received the same user-wide response into whichever `.cas/cas.db` was current.
The older `cas.db.pre-purge-20260512` / `cas.db.post-purge-broken` files are
evidence of an attempted cleanup on the same day, not a separate scope-aware
restore mechanism. The shipped `purge-foreign` command cannot identify local
ownership because local tasks have no project identifier; its wipe-and-re-pull
strategy is not safe for irreplaceable, local-only data in this incident.

## Measured blast radius

The supervisor collected these counts with read-only SQLite URIs. No live
database was modified.

| Store | Tasks | IDs also in global | Overlap |
| --- | ---: | ---: | ---: |
| `.cache/cas-4fb9-demo.GkMKx1` | 2,672 | 1,525 | 57% |
| `Petrastella/cas-src` | 2,672 | 1,525 | 57% |
| `tmp-bugrepro` | 2,278 | 1,525 | 66% |
| `project-a` | 1,419 | 1,255 | 88% |
| `project-b` | 445 | 384 | 86% |
| `project-c` | 2,876 | 1,692 | 58% |
| `project-d` | 2,668 | 1,624 | 60% |
| `project-e` | 184 | 37 | 20% |
| `project-f` | 32 | 19 | 59% |
| `project-g` | 1,552 | 1,460 | 94% |
| `project-h` | 127 | 3 | 2% |
| `project-i` | 81 | 2 | 2% |
| `project-j` | 33 | 3 | 9% |
| `project-k` | 2 | 1 | 50% |

Approximately 18 additional stores contained no tasks. The identical 1,525-ID
overlap in cas-src, tmp-bugrepro, and a disposable demo store rules out an
isolated restore error. Every heavily used project is affected.
The internal store-name mapping remains available in the CAS task record for
`cas-de89`; it is intentionally omitted from this public report.

The duplicated cas-src tasks are sharply bounded by creation date: 665 were
created in March 2026, 859 in April, and one in July. In cutoff terms, 1,524 of
1,525 were created before push stopped on 2026-05-12; one was created on or
after that date. This is a systemic **historical event**, not evidence of an
ongoing leak. Its residue is widespread, but the unscoped pull window closed in
May and the later local-only pull apply fix closed the feedback loop in July.

## Why the pending queue is unsafe

Until commit `0f2f7fc4` (2026-07-09), pull apply opened the normal syncing task
store. `SyncingTaskStore::add` and `update` enqueue the entire serialized task
(`cas-cli/src/store/syncing_task.rs:42-64,96-112`), so remote rows pulled into
the wrong project were queued to be pushed again. The local-only opener now at
`cas-cli/src/store/detect.rs:326-328` prevents that feedback loop.

Queue timestamps cannot separate good rows from contaminated rows. An ordinary
later edit of an already-contaminated task re-enqueues it, and
`cas-cli/src/cloud/sync_queue/queue_ops.rs:34-62` replaces the existing row and
resets `created_at`, retry state, and payload. A post-2026-07-09 timestamp is
therefore only the latest enqueue time, not proof of clean origin.

For task upserts, `sync_queue.payload` contains the full task body and can be
used for content comparison without rereading the task table. It does **not**
contain trustworthy ownership: `project_canonical_id` is added to the push
envelope only at send time, while a task reloaded from SQLite always serializes
with `scope=project`. Delete rows have no payload. Classification still needs
an external ownership manifest or authoritative per-project export.

### No local ownership manifest exists

Read-only checks against the affected store ruled out all plausible local
provenance sources:

- `Task.scope` is assigned at read time by
  `crates/cas-store/src/task_store.rs:224`; it is not stored provenance.
- `events -> sessions.cwd` cannot classify tasks. Events were replicated with
  the contaminated tasks, and `events.session_id` is almost always null. Only
  3 of 406 open tasks had any event that joined to a local session. A confirmed
  foreign task had 13 local events while a native task had one.
- `known_repo_bindings` and `commit_links` contained zero rows in the project
  store. `known_repos` is a directory-touch index, not a task-to-project map.

There is therefore **no ownership manifest in the local stores**. Local data
alone cannot produce a safe automated classification. These failed signals
should not be re-investigated during recovery.

The cloud is the only system that recorded `project_canonical_id`, because the
client adds it to the push envelope at send time. Cloud provenance ends when
push stopped on 2026-05-12, but 1,524 of the 1,525 duplicated tasks predate that
cutoff. An authorized recovery query is therefore expected to cover 99.9% of
the affected IDs, leaving one known post-cutoff exception for manual review. A
recovery export must be obtained by an authorized operator after credential
rotation; this report does not query the service.

At minimum, the recovery query must return `task_id` and
`project_canonical_id`. It should also return the owning account/team, cloud
`created_at` and `updated_at`, task body or stable semantic-body hash,
deletion/tombstone state, and the server-side receipt or version identifying
the accepted push. `updated_at` plus a semantic hash lets recovery distinguish
an identical duplicate from a local copy that later diverged. Results must be
grouped by project and must expose IDs that appear under more than one cloud
project rather than silently choosing one.

Tasks not covered by cloud provenance require content-based heuristics followed
by human review. Heuristics may rank likely owners using product names, paths,
labels, external references, and semantic similarity, but they are not
authority and must never directly drive deletion or upload.

Commit `7ff1be85` (cas-8248) repairs the previously dead automatic team drain.
The affected host currently predates that commit, so its 3,245 rows remain
inert. Installing a build containing cas-8248 before classification can publish
contaminated tasks to the current project's cloud scope and then to other
machines.

## Required order of operations

1. **Keep the operational hold.** Do not deploy cas-8248 to the affected host,
   manually sync, drain, retry, delete, or rewrite its queue. Do not enroll a
   second machine against these stores.
2. **Retain the pull protections.** The scoped request, fail-closed entity
   filter, and local-only pull-apply stores are prerequisites. The regression
   in `cas-cli/tests/pull_scoping_regression_test.rs` must remain green.
3. **Recover cloud provenance for the pre-cutoff set.** After credential
   rotation and explicit user authorization, export the
   `task_id -> project_canonical_id` mapping and divergence fields specified
   above for the 1,524 duplicated tasks created before 2026-05-12. This is a
   read/export operation, not a sync or drain.
4. **Hand-adjudicate the one post-cutoff task.** Cloud cannot be assumed to
   know its origin. Use content-based evidence and explicit user review; if
   ownership remains ambiguous, preserve every copy and make no queue decision
   for that ID.
5. **Snapshot before local remediation.** With CAS stopped, create verified,
   read-only copies of every database and its WAL/SHM state using SQLite's
   online backup API or a checkpointed copy procedure. Record checksums. All
   inventory and comparison work runs against copies, never the live files.
6. **Build an ownership and conflict manifest.** Inventory each task ID across
   global, every project store, cloud exports grouped by project, and queue
   payloads. Record locations, semantic-body hashes, timestamps, operations,
   and queue/team IDs. Cloud push receipts are the authoritative source where
   available. Uncovered IDs require heuristic ranking and human approval. Do
   not include sensitive task text in a public report.
7. **Classify the queue before enabling its drain.** Upsert payloads may be
   compared directly with the manifest. Deletes require ID-based ownership.
   Quarantine every row whose ownership is absent, ambiguous, or inconsistent
   with the queue's current project; no timestamp-based exemption is allowed.
   Because 1,524 duplicates were created by April, exact-body comparison
   against cloud and queue payloads should classify most unchanged rows without
   judgment; any mismatch remains a human-reviewed conflict.
8. **Resolve database copies only after explicit user approval.** Produce a
   dry-run plan and new verified backups. Apply changes per store in a
   transaction, then run integrity checks and compare counts/hashes against the
   approved manifest. The implementation of this repair is a separate task.
9. **Deploy the drain fix only after stores and queue are safe.** First deploy
   the already-landed pull/feedback-loop protections and the approved isolation
   remediation. Then deploy cas-8248. Flush only rows explicitly classified for
   that project, in bounded batches with retry/error observation.
10. **Verify propagation before normal operation.** Confirm cloud counts and a
    clean second-machine pull for each project before lifting the hold.

At present, **flush the queue has no safe automated form**. The 3,245 rows must
stay in place and the affected host must remain on its pre-cas-8248 binary until
cloud provenance is recovered or the user explicitly accepts a
heuristic-and-human-reviewed classification. A repaired drain is not itself a
classification mechanism.

There is no new task-write isolation code to deploy from cas-de89: current
create and store writes already target one caller-selected database, and the
historical pull defects are already patched. This task adds durable diagnosis,
recovery ordering, and behavioral regression coverage; it deliberately does
not mutate or deduplicate user data.

## Divergence rule

Never pick a winner solely by `updated_at`, queue `created_at`, `scope`, or
which database was inspected first.

- If copies have the same semantic body, they are duplicate representations,
  but deletion still waits for an explicit authoritative-owner mapping. Keep
  the authoritative copy and remove only manifest-approved non-owner copies.
- If copies differ, preserve both. Mark the ID conflicted, select ownership
  from external project provenance, and perform a reviewed field-level merge.
  Store the losing body in the recovery artifact before any write. If ownership
  cannot be established, make no change.
- If one ID legitimately needs independent records in multiple projects,
  allocate a new ID for the non-authoritative fork. Do not keep divergent rows
  under one globally synchronized ID.
- If a queued payload differs from the chosen authoritative body, quarantine
  it for review. Do not assume the queue is newer or safer.

The repair must be restartable, manifest-driven, dry-run by default, and must
refuse to operate without verified backups and an explicit owner for every row
it would change.
