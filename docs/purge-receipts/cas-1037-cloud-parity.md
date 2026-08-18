# cas-1037 cloud parity repair receipt

## Verdict

**The destructive repair is clean, with high confidence.** All 315
evidence-gated fixture tasks were deleted from the `cas-src` cloud project,
fresh project-scoped pulls contain no cloud-only rows, and the local queue is
empty with no failures. The remaining 55 local-only rows are verified
server-side identity/binding collisions and were intentionally left untouched
for operator review rather than risking data owned by other projects.

## Overview

| Field | Result |
| --- | --- |
| Question | Can this machine hand off a clean `cas-src` cloud snapshot without deleting legitimate data? |
| Verdict | Yes for cloud cleanup; 55 collision-blocked local rows require server repair before exact two-way parity |
| Confidence | High |
| Scope | Entries, tasks, rules, skills, knowledge pages, and the live sync queue |
| Data window | Baseline and repair pulls on 2026-08-09; five-pull close-gate series 13:39:11–13:39:36 EDT |
| Author | Cassy factory worker `loyal-koala-52`, task `cas-1037` |

## Baseline

The initial fresh, project-scoped personal and team pulls were compared by
entity ID against the live local project store. The combined cloud set had
2,438 entries and 2,490 tasks; the local store had 2,449 entries and 2,219
tasks. Exact set differences were 11 local-only entries, 44 local-only tasks,
zero cloud-only entries, and 315 cloud-only tasks.

## Evidence

| Observation | Source | What it proves |
| --- | --- | --- |
| All 315 cloud-only tasks passed every fixture predicate and had zero local overlap | `cas-1037-cloud-task-manifest.tsv`; fresh pre-delete `cas-src` team pull | The deletion set was fully enumerated and no candidate was inferred from title alone |
| The fixed binary drained 315 team task-delete tombstones and the queue reached `total=0`, `pending=0`, `failed=0` | `./target/debug/cas --json cloud sync`; `./target/debug/cas --json cloud queue` | The intended delete operations completed without a stranded or failed queue row |
| Five consecutive post-delete pulls have zero cloud-only tasks and zero cloud-only entries | Personal/team pull series from 13:39:11–13:39:36 EDT; ID-set comparison against SQLite | The purge removed the complete candidate set and did not leave new cloud leakage |
| Three valid team entry copies appeared transiently at 13:34, then disappeared by 13:38; a personal entry visible at 13:26 also disappeared | Scoped pull comparison for IDs `2026-07-31-6`, `2026-08-01-1`, `2026-08-01-2`, and `2026-08-09-18`; queue stayed empty | Entry identity/visibility is eventually or inconsistently scoped; a single snapshot is not a trustworthy backfill receipt |
| Team upserts resolved canonical project ID `cas-src` but each missing task returned a nested skip | Captured team push response: `synced.tasks.skipped=1`, `canonical_id=cas-src` | The task residual is not caused by this client pushing under the wrong current canonical ID |
| Direct legacy ID lookups show 11 still-missing foreign entries, 24 foreign tasks, and 20 same-identity tasks hidden under divergent or unscoped bindings | `cas-1037-operator-review-residual.tsv` | Retrying, deleting, or replacing these global IDs could overwrite another project's data |
| The client now retains nested and malformed skip responses for retry instead of marking them synced | Commit `6754d146`; scoped push/team-sync tests | The live response shape can no longer silently discard a skipped upsert |

## Fixture deletion gate

Every cloud-only task had to pass every predicate below before deletion:

- absent from the local `tasks` table;
- project ID exactly `cas-src`;
- created from 2026-03-20 through 2026-06-07;
- status `open`, task type `task`, priority 2;
- empty description, acceptance criteria, and labels;
- title exactly one of `MCP Protocol Test Task`, `Context test task`,
  `Consolidated task test`, or `Test task for notification test`.

All 315 rows passed. Counts by title were 81, 79, 76, and 79 respectively.
The per-row evidence and final `deleted` result are in
`cas-1037-cloud-task-manifest.tsv`. No fixture candidate failed verification,
so the deletion-gate operator-review residual is empty.

## Repair result

A fresh, project-scoped pull immediately before deletion reproduced the
manifest exactly: 315 of 315 candidate IDs were present in the `cas-src` team
pull, absent locally and from the personal pull, and passed every fixture
predicate. The fixed worktree binary (`cas 2.55.5`, commit `6754d146`) then
drained exactly 315 team task-delete tombstones. A fresh pull after the drain
found zero cloud-only tasks and zero cloud-only entries. The queue finished
with `total=0`, `pending=0`, and `failed=0`.

| Entity | Local | Cloud | Local-only | Cloud-only |
| --- | ---: | ---: | ---: | ---: |
| entries | 2,449 | 2,438 | 11 | 0 |
| tasks | 2,219 | 2,175 | 44 | 0 |
| rules | 173 | 173 | 0 | 0 |
| skills | 3 | 3 | 0 | 0 |
| knowledge pages | 107 | 107 | 0 | 0 |

Seven of the baseline's 11 local-only entries were safely backfilled. The
remaining set changed as project-scoped visibility exposed additional global
short-ID collisions, leaving eleven entry residuals. All 44 task backfills remain
server-blocked.

## Reasoning chain

1. The pre-delete manifest was regenerated from a fresh scoped pull and
   matched all 315 IDs exactly, ruling out queue pollution or a stale snapshot.
2. Every row passed independent local-presence, project, date, shape, and title
   gates. The destructive set was therefore bounded before any tombstone was
   enqueued.
3. The fixed worktree binary drained exactly that bounded set. A second scoped
   pull contained none of the IDs and showed no new cloud-only rows, while the
   queue was empty.
4. Backfill retries returned structured skips even though the team response
   echoed canonical project `cas-src`. Direct ID lookups then exposed either a
   foreign entity or the same task under a non-`cas-src` binding.
5. That evidence rules out client repinning as a safe complete repair. The
   only safe remaining action is a server-side project-scoped identity
   migration; the 55 rows remain local and are listed individually.

## What would falsify this

The cleanup verdict would be overturned by any fresh `cas-src` pull containing
one of the 315 manifest IDs, any new cloud-only row absent from the final
receipt, or any nonzero pending/failed queue count. The collision verdict would
be overturned if a project-scoped server lookup showed that one of the 55
residual IDs is unoccupied in `cas-src` and can be inserted without touching a
foreign row.

## Next actions

1. The `petra-stella-cloud` owner should implement the project-scoped identity
   and migration request in
   `../requests/2026-08-09-project-scoped-entity-identity-collisions.md`.
2. An operator should rerun the 55-row residual manifest after that migration
   and confirm exact local/cloud parity before the Mac handoff.
3. Keep the skip-parser fix from commit `6754d146` deployed so future unknown
   or nested skip signals remain retryable.

## Provenance

- Markdown source: `docs/purge-receipts/cas-1037-cloud-parity.md`
- Row evidence: `cas-1037-cloud-task-manifest.tsv` and
  `cas-1037-operator-review-residual.tsv`
- Code examined and fixed at commit: `6754d146`
- Repeated close-gate extraction: 2026-08-09 13:39:11–13:39:36 EDT (five pulls)
- Cloud scope: team `2a57bec9-5dfa-4a8f-b711-31f9aeb8d6cb`, canonical project
  `cas-src`
- Commands and queries used:

```sh
sqlite3 /home/pippenz/Petrastella/cas-src/.cas/cas.db
curl -fsS -H "Authorization: Bearer $CAS_CLOUD_TOKEN" \
  "$CAS_CLOUD_ENDPOINT/api/sync/pull?types=entries,tasks,rules,skills&project_id=cas-src"
curl -fsS -H "Authorization: Bearer $CAS_CLOUD_TOKEN" \
  "$CAS_CLOUD_ENDPOINT/api/teams/2a57bec9-5dfa-4a8f-b711-31f9aeb8d6cb/sync/pull?project_id=cas-src"
curl -fsS -H "Authorization: Bearer $CAS_CLOUD_TOKEN" \
  "$CAS_CLOUD_ENDPOINT/api/sync/pull?types=knowledge_pages&team_id=2a57bec9-5dfa-4a8f-b711-31f9aeb8d6cb&project_id=cas-src"
comm -23 final-local-*-ids.txt final-cloud-*-ids.txt
./target/debug/cas --json cloud sync
./target/debug/cas --json cloud queue
```

The installed daemon at `/home/pippenz/.local/bin/cas` (2.55.5, PID 3983675)
was known to perform automatic drains with the pre-fix delete behavior during
the investigation. The destructive purge itself was explicitly executed by
the fixed worktree binary after the live queue was confirmed exclusive. The
fresh cloud pull and empty-queue receipt independently verify the result.
