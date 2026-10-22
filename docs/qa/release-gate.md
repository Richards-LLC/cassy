# Release gate

The release gate (epic cas-7789, design art-e3dfbeb7) turns a staging SHA into
one evidence pack that promotion reads. This page covers the project seam that
the rest of the gate builds on: `[qa.release]`, the project QA adapter protocol
and the served-component equivalence preflight. Later slices add their own keys
and verbs here.

## Configuration

Configure the gate in the operator-owned `.cas/config.toml`. Deliveries cannot
change which command runs or which environment it sees.

```toml
[qa.release]
integration_branch = "staging"
branches = ["main"]
deploy_workflows = ["Deploy Backend to Staging", "Deploy Frontend to Staging"]
adapter = "./scripts/qa/cassy-adapter"
env = ["STAGING_HEALTH_TOKEN"]
timeout_secs = 120
grant_envs = ["staging"]
target_env = "prod"

[qa.release.components]
backend = ["apps/backend", "packages/shared"]
frontend = ["apps/web", "packages/shared"]

[qa.release.migrations]
paths = ["apps/backend/prisma/migrations/*/migration.sql"]
env = "staging"
```

| Key | Meaning |
|---|---|
| `integration_branch` | The branch the gate walks. |
| `branches` | Promotion targets whose PRs need a clean gate. |
| `deploy_workflows` | Exact GitHub Actions workflow names watched on `integration_branch` (default empty). |
| `adapter` | Project-relative adapter executable. Empty leaves the gate off. |
| `env` | Environment variable names allowed into the adapter. |
| `timeout_secs` | Per-call timeout (default 120). |
| `retention_days` | Days evidence-pack media stay on disk (default 30); `pack.json` is kept. |
| `components` | Served component name to the repository paths it is built from. |
| `grant_envs` | Environments where preflight may `grant` a balance shortfall (default `["staging"]`). |
| `target_env` | Environment promotion serves; promotion reads its `flags` once (default `prod`). |
| `migrations.paths` | Repository-relative patterns with exactly one `*`, which is the migration id. |
| `migrations.env` | Environment whose applied migrations a merge into `integration_branch` needs. |

`cas config set qa.release.components '{"backend":["apps/backend"]}'` takes
the components as one JSON object. Paths must stay inside the repository: an
absolute path, a `..` segment or git pathspec magic (`:(glob)…`, `:!…`) is
refused when the config is set.

## Sandbox

The adapter runs in the same sandbox as `qa.post_release.command`
(`qa_command`). The executable must stay inside the project. It runs from the
project root with no shell. Its environment is a minimal PATH and LANG plus
only the `env` allow-list. It has a timeout, and stdout is capped at 1 MiB.
Raw stdout and stderr are never stored, even on error.

## Adapter protocol v1

Cassy calls `<adapter> <verb>`, writes one request object to stdin and reads
one response object from stdout. The adapter must exit zero; a nonzero exit is
an adapter bug.

```json
{"v":1,"verb":"served","env":"staging","sha":"<candidate>","journey":"optional","args":{}}
```

```json
{"v":1,"ok":true,"data":[{"component":"backend","sha":"f6563bf","deploy_ref":"deploy-123"}]}
{"v":1,"ok":false,"error":{"kind":"rate_limited","provider":"health","status":429,"retry_after_s":30,"message":"slow down"}}
```

`error.kind` is the classification input. It takes one of these values:

- `external`: a provider failed (5xx, provider down).
- `rate_limited`: throttled (429).
- `insufficient`: a consumable ran out (402).
- `invalid`: the adapter rejected the request.
- `internal`: an adapter bug.

A response with a different `v`, an `ok: true` that carries an error, or an
`ok: false` without one is a protocol failure. Error messages are cut to 200
printable characters.

The verbs are `served`, `flags`, `balance`, `grant`, `fixture-setup`,
`fixture-teardown`, `run`, `readback` and `migrations`. All but `run` are
implemented; the run state machine adds it.

### `served`

`served` returns one entry per deployed component:
`{component, sha?, build_id?, deploy_ref}`. Map build ids to commits in the
adapter, because equivalence needs `sha`.

## Served-component equivalence

At gate preflight Cassy asks the adapter what the environment serves. For each
component declared in `components`, it runs
`git diff --quiet <served sha> <candidate> -- <component paths>`. Equal trees
pass. Each of the following refuses with `SERVED MISMATCH`:

- A component's paths differ.
- A declared component is not reported, or reports no `sha`.
- A reported component has no declared paths.
- A served commit is missing locally (fetch it first).

A failed `served` call refuses with `SERVED UNKNOWN` and keeps its error kind.
A served backend can therefore lag the staging tip, as long as no commit since
then touched its paths.

## Config drift: `depends_on_flags`

Evidence proves only the configuration it ran under. A journey names the
flags whose values its result depends on:

```yaml
journeys:
  - id: audit
    depends_on_flags: [post_audit.audio_evidence.enabled]
```

`cas qa journeys --check` rejects an entry that is not 1–128 letters, digits
or `._-:/`. Name only non-secret flags: values are stored verbatim in the
ledger and the pack, must be scalars (a list or object is refused), and the
pack's secret scan refuses a token-shaped value. Cassy never reads project config itself. The adapter's `flags`
verb receives the union of the walked journeys' flags and answers every
requested key, reporting an unset flag as `null`:

```json
{"v":1,"verb":"flags","env":"staging","sha":"<candidate>","args":{"keys":["post_audit.audio_evidence.enabled"]}}
{"v":1,"ok":true,"data":{"post_audit.audio_evidence.enabled":false}}
```

If no journey declares a flag, there is no call. If a requested key is
missing from the answer, the read is refused, because "not reported" is not
"unset". Unrequested keys are dropped and never stored.

Each check runs once. The snapshot is taken at gate preflight on the tested
environment, and the ledger stores it as one header line:
`Flags: staging@<sha> {"post_audit.audio_evidence.enabled":false}`. `|` is
escaped, so a value cannot form a ledger row. Promotion reads the target
environment once and compares the two. Every declared flag whose values
differ, or that either side lacks, is
`CONFIG_MISMATCH <flag>: test=<value> target=<value> (<journeys>)`, and the
named journeys' evidence does not prove the target.

## Evidence pack

Each gate run writes one pack, which is the only input promotion reads:
`<artifacts_root>/<project>/release/<sha>/pack.json`. The pack holds:

- the served components at start and end;
- the flag snapshot;
- the rows, PR coverage, fixture receipts and the resource plan;
- the suite timings;
- media `{path, sha256, bytes}`.

Media are local files under the pack's `media/` directory. They are never
committed to git or uploaded to Cloud by default.

Rows use one vocabulary in the CLI, MCP, pack, PR comment and status:

| Row verdict | Meaning |
|---|---|
| `PASS` | A runner PASS, at least one frame, and every declared invariant read back. |
| `FAIL` | A product defect, including a failed read-back. |
| `NOT_A_ROUTE` | The journey does not exist on this build. |
| `NOT_EXERCISED{fixture_setup\|automation\|timed_out\|operator_skip}` | Not walked, or no frame or read-back. |
| `BLOCKED_EXTERNAL{provider, error}` | A provider failed, throttled (429) or ran out (402). |
| `CONFIG_MISMATCH{flag, test, target}` | The evidence ran under another flag value. |
| `SERVED_DRIFT` | The served components changed during the row. |

The gate is `FAILED` on any FAIL. It is `CLEAN` only when every row is PASS or
NOT_A_ROUTE and every PR is exercised. Anything else, including no rows at
all, is `OWED`, and the gate names each owed row or PR and its cause.

**Per-PR coverage.** At preflight Cassy lists the PRs on the candidate's
first-parent history: `Merge pull request #N` merges and squash subjects
ending in `(#N)`. It maps each PR's diff to journeys through `cas qa impact`.
A PR that no row exercises keeps the gate OWED until a manual row records
`covers_prs: [N]`. Every other first-parent commit, including a root
commit, which is diffed against the empty tree, is a direct push and its own
coverage unit, `push <sha7>`. It is owed the same way until a row
exercises it or a manual row records `covers_commits: [<sha7>]`.

**Sealing.** Sealing rehashes every media file and refuses a link or any path
outside `media/`. It derives the gate, runs the evidence secret scan over the
whole pack (flag values included) and writes `pack.json` atomically. Reading
a pack refuses missing or changed media, and a stored gate that its rows no
longer derive.

**Ledger.** The pack ledger is the batch QA ledger. When a sealed pack exists
for `staging_sha`, `verification action=batch_receipt` binds it by its bytes
and records its gate. If that pack exists but fails verification, the receipt
is refused. A changed `pack.json` stops the receipt satisfying close, exactly
as a changed `LEDGER.md` does.

**Slack.** The Violet bundle posts `pack.json`, with the summary as its
comment, and then each image or video in that thread. An image or video
must also have matching magic bytes, so a zip named `x.png` is not uploaded.
Other media (traces, logs, archives) can carry credentials. They are never uploaded; the summary
names them by local path. Every file is rechecked
against the pack digest and is at most 25 MiB. Video over the limit is
re-encoded with `ffmpeg`: H.264, at most 720p, no audio, at a bitrate that
fits. If re-encoding fails or the video is still too large, it stays
local-only and the row's frames carry the evidence.

**PR comment.** Cassy keeps one comment per PR, marked
`<!-- cassy:release-gate -->`, and edits it in place. A body over 65,000
characters (GitHub's limit is 65,536) is cut at a line and ends with a
pointer to the pack.

**Retention.** `qa.release.retention_days` (default 30) deletes old media
files only inside each pack's own `media/`. Links are never followed or
deleted.

`pack.json` stays as the record, and its bytes never change, so a batch
receipt bound to them stays valid. Before deleting a pack entry, retention
records its path and digest in `pruned.json` beside the pack, atomically.
Loading accepts a missing entry only when `pruned.json` records it with the
pack's own digest. It refuses a record the pack does not list. An entry whose
file changed is not pruned, so loading keeps refusing it. A reseal writes the
new `pack.json` first and only then drops the records it no longer lists, so
a reseal that fails leaves the previous pack loadable.

## Promotion: `cassy/release-gate`

Promotion reads the pack. A head passes when either of these sealed packs
is CLEAN and the target serves the flags the evidence ran under:

- the pack for that SHA;
- a pack for a commit with the same whole tree. A merge commit whose tree
  equals the walked SHA counts as walked.

Each evaluation reads `target_env` through the `flags` verb once, for the
walked journeys' `depends_on_flags`, and compares the result with the pack's
snapshot.

| Result | When | Description names |
|---|---|---|
| `success` | A CLEAN pack and no flag mismatch. | The walked SHA. |
| `pending` | No sealed pack exists yet. | The head. |
| `failure` | Packs exist, but none walked this tree. | The head and the latest walked SHA. |
| `failure` | The pack is FAILED or OWED. | The FAILED rows, or each OWED row or PR and its cause. |
| `failure` | A flag differs on `target_env`, or the read failed. | Each `CONFIG_MISMATCH`, or `FLAGS UNKNOWN`. |

`cas qa release-gate verify --sha X` prints the verdict and exits nonzero
unless it passes, so projects without GitHub can use it as their check. With
`qa.github_status = true`, it also publishes the verdict as the commit status
`cassy/release-gate` on X. After each seal, the run publishes the status on
the head of every open PR into `branches`. Require `cassy/release-gate` in
branch protection on each promotion branch. Statuses use the same `gh`
publisher as `cassy/independent-qa`.

**Override.** A live registered supervisor may record the operator's
decision as one decision note on any task:

```text
task action=notes note_type=decision notes="RELEASE-GATE OVERRIDE sha=<full 40-char sha>: <reason>"
```

The override passes that exact SHA, or a commit with the same whole tree,
for 6 hours from the note. The status description names the override and
when it expires. Each head it passes is logged once as a decision note on
that task.

The task store reserves the override line. Only the live supervisor's
decision note is appended as trusted. Every other writer gets its copy
indented so it never parses: worker notes, task updates (including the
pre-compact flush), cloud sync merges and plain appends. An override
therefore works only on the machine where the supervisor recorded it.

**Freeze status.** While a run walks `integration_branch`, Cassy publishes a
pending `cassy/release-freeze` status on the head of every open PR into
that branch. Outside a run, the status is success. The run driver publishes
it at start, seal and abort, and again on every tick, so a PR opened or
pushed between runs is released too. Each call posts only heads whose state
changed, recorded in `.cas/release-gate/freeze-status.json`. Require the
context on `integration_branch` so merges from outside Cassy wait too.

PR listing reads every page of open PRs. One failed post does not stop the
others; the error names each failure, and a failed head is retried on the
next call.

## Independent runs: `independence_key`

A product can dedupe a resubmission and answer with the same in-flight
result, so three recorded runs may be two real ones. A journey names the
result identity that makes runs independent:

```yaml
journeys:
  - id: audit
    independence_key: post_audit_v2_runs.id
```

`cas qa journeys --check` rejects a key that is not 1–128 letters, digits or
`._-`. Each PASS or FAIL run row records `post_audit_v2_runs.id=<id>` in any
cell. Only distinct ids count toward the runs a gate requires; a duplicate id
or a row without one is refused by name, for example
`3 audit runs recorded, 2 independent by post_audit_v2_runs.id: duplicate post_audit_v2_runs.id=b2 (2 rows)`.
The approved `qa_record` journey check (G8) applies it today; the golden-set
matrix (C8) passes its `runs` count to the same rule.

## Deploy failure alerts

The factory daemon watches configured deploy workflows on the integration branch
at its existing CI polling cadence. That branch also joins the ordinary CI watch
set. Completed `push` and `workflow_dispatch`
runs count; CI and each deploy workflow have independent verdicts. A failed or
timed-out deploy with a failed job becomes a Critical supervisor `BLOCKER` naming
the job and failed step when GitHub reports one. The relay key includes the
branch, workflow and SHA, so repeated polls send one blocker for that failure.
This watcher works even when no release adapter is configured.

Ordinary CI runs ending in `failure` or `timed_out` count as red when GitHub
reports a failed or timed-out job. They raise the usual high-priority CI alert
at the branch tip; a run with only cancelled jobs stays silent.

Path-filtered deploys may not rerun after the branch moves. An observed red deploy
therefore stays red until a newer successful run of the same workflow on the
integration branch. Cancelled, skipped and neutral runs do not clear it. Latest
deploy observations persist in `.cas/ci-deploy-watch.json`, including successful
run watermarks, so daemon restarts and eviction from the 100 completed-run window
do not lose an observed failure. Removing a workflow from config drops its state.

The watcher reuses its existing GitHub run list and red-run detail lookups. It
adds no GitHub calls; it cannot discover a deploy already outside the first
100-run snapshot before observation. Configure the watcher while deploys are
current. The release gate's served-component check still detects stale deploys
when a gate is attempted.

If the project config cannot be loaded or saved deploy observations cannot be
read, deploy monitoring pauses and the existing ordinary CI branch watches stay
active. The watcher preserves corrupt state for repair and warns once per path
and error class (`read`, `parse`, `write`) until that operation recovers. A
healthy read rearms read/parse warnings; a matching persisted observation or a
successful write rearms write warnings. State-write failures keep current alerts
available, and unchanged observations do not rewrite the state file.

## Resource budgets

The registry header declares the provider budgets that the gate shares, and
each journey declares what one run uses:

```yaml
resources:
  staging-api: {kind: rate, per_min: 60, concurrency: 4}
  visitor-audits: {kind: quota, per_day: 3}
  credits: {kind: balance}
journeys:
  - id: audit
    consumes: {staging-api: 12, visitor-audits: 1, credits: 10}
```

`cas qa journeys --check` rejects an unknown kind, a missing or zero limit,
a limit that belongs to another kind, and a `consumes` entry that names an
undeclared resource or uses zero units.

### Shared ledger

Rate and quota resources are metered per request in `qa_resource_use` in
`.cas/cas.db`, one bucket per environment and resource. Every process on the
host that uses this store shares it. Each take runs in one `BEGIN IMMEDIATE`
transaction, so two processes together never take more than `per_min` units
in any minute or `per_day` in any 24 hours, nor hold more than `concurrency`
requests at once. A request takes its `consumes` in name order before the
adapter call and releases its concurrency slot when the call ends; its use
keeps counting for the rest of the window. A crashed holder's slot frees after
`timeout_secs` plus five seconds. When the window cannot free in time for the
row's wall clock, the row is `NOT_EXERCISED{timed_out}` and the adapter is not
called. Migration 284 installs the ledger in existing stores through
`cas update --schema-only`, and new stores get it at initialization.

### Preflight need

Before the freeze, need = Σ consumes × runs × cells per resource. These checks
refuse locally, with no adapter call:

- One run uses more of a rate resource than `per_min`, so it could never fit.
- A quota's need exceeds what is left of `per_day` in the last 24 hours.

For balance resources Cassy then calls `balance` once, then `grant` for the
shortfall when the environment is in `grant_envs`:

```json
{"v":1,"verb":"balance","env":"staging","sha":"<candidate>","args":{"resources":["credits"]}}
{"v":1,"ok":true,"data":{"credits":5}}
{"v":1,"verb":"grant","env":"staging","sha":"<candidate>","args":{"amounts":{"credits":15}}}
{"v":1,"ok":true,"data":{"credits":20}}
```

`grant` answers each granted resource's new balance. Every refusal names the
resource, the need and the balance:

- `RESOURCE SHORTFALL: credits needs 20, balance 5 on prod; prod is not in qa.release.grant_envs`
- `RESOURCE SHORTFALL: credits needs 20, balance 12 on staging; grant did not cover it`

A `balance` or `grant` failure caused by a provider (`external`,
`rate_limited` or `insufficient`) is `BLOCKED_EXTERNAL`.

### Throttling and exhaustion

A mid-run `rate_limited` (429) answer is retried once, after `retry_after_s`,
when that still fits the row's wall clock; the retry is metered like any other
request. A second 429, or one that cannot wait, is
`BLOCKED_EXTERNAL{provider, error}`. An `insufficient` (402) answer is
`BLOCKED_EXTERNAL` at once and is not retried. Either way the row stays owed.

## Fixtures and personas

Registry fixtures share `description` and optional criterion `labels`. Each
fixture declares exactly one source; existing `provider` entries remain valid:

```yaml
fixtures:
  existing:
    description: Existing account seed
    provider: account-seed
  stored-video:
    description: Immutable golden-set video
    frozen:
      sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
      storage_key: golden/videos/example.mp4
  past-due:
    description: Past-due staging persona
    recipe:
      name: past_due
      envs: [staging]
      provides: [account_id]
      ttl_mins: 30
```

Missing sources and misspelled source or nested fixture fields remain readable
by registry consumers; `--check` reports them as validation problems. Read-back
unknown fields likewise retain YAML values (including tags and map keys) for
validation instead of aborting the registry load.

`frozen.sha256` is 64 hexadecimal characters, and `storage_key` is a nonempty
opaque storage key. `cas qa journeys --check` reports HTTP(S) provider URLs as
`live_url_fixture` debt: freeze their bytes to avoid deleted/private media and
provider outages changing the golden set. Non-URL providers keep their current
behavior. Recipes require a name, nonempty environment allow-list, unique
nonempty output names and a positive TTL. Registry recipes never contain commands.

`fixture-setup` receives `args: {name, provides, ttl_mins, cleanup_id}` and
returns `data: {handle, outputs: {account_id: "opaque-id"}}`. Cassy refuses an
environment outside `envs` before any adapter call. A setup failure or missing
declared output is `NOT_EXERCISED{fixture_setup}`.

The suite guard journals `{handle, env, expiry}` in `qa_fixture_cleanup` before
returning the setup receipt. `finish` and early-return/unwind cleanup call
`fixture-teardown` with `args: {name, handle, cleanup_id}` and the original
environment and SHA. Teardown returns `data: {ok: true}` and must be idempotent:
an already absent handle is successfully cleaned. Cleanup errors retain the
row as orphan debt, separately from the suite verdict.

After a crash, run `cas qa fixtures --sweep` in the project checkout. It tears
down expired handles using the configured adapter, deletes successful rows and
reports retained orphan debt (also available with `--json`). Unexpired handles
stay available to their running suite. The TTL is a recovery deadline; set it
longer than the expected suite duration. Migration 283 installs the journal in
existing stores via `cas update --schema-only` and in newly initialized stores.

## Read-back

A journey declares what the gate may read back after it runs:

```yaml
journeys:
  - id: signup
    readback:
      subjects: [account_uid, workspace_id]
      invariants: [INV-ACCOUNT, INV-WORKSPACE]
```

`cas qa journeys --check` rejects a `readback` block with an unknown key, an
empty list or an entry that is not an identifier.

Cassy forwards only allow-listed values to the adapter's `readback` verb: the
declared subject kinds with their opaque ids, and the declared invariant IDs.
It never forwards a query. The following are refused before any call:

- An undeclared subject kind.
- A subject id that is not an opaque token of at most 128 letters, digits and
  `-_.:@`.
- An invariant ID that is not a plain identifier.

```json
{"v":1,"verb":"readback","env":"staging","sha":"<candidate>","journey":"signup","args":{"subjects":{"account_uid":"uid-7f3a","workspace_id":"ws_42"},"invariants":["INV-ACCOUNT","INV-WORKSPACE"]}}
{"v":1,"ok":true,"data":{"INV-ACCOUNT":{"ok":true,"observed":1},"INV-WORKSPACE":{"ok":true,"observed":"…"}}}
```

The result is post-release's read-back shape: `{journey, subject, invariants,
ok}`, with the subject ids rendered as `kind=id`. Observed values are never
kept. The adapter must run these checks with read-only credentials.

| Answer | Outcome |
|---|---|
| Every declared invariant `ok: true` | PASS |
| Any invariant `ok: false` | FAIL, naming the failed invariants |
| An `external`, `rate_limited` or `insufficient` error | BLOCKED_EXTERNAL |
| An invariant missing from the answer, any other error or a timeout | no PASS (missing) |

## Pending-migration merge refusal

`worktree_merge` checks migrations once per merge into `integration_branch`,
after lane CI admission and before Git runs. Other targets skip the check.
Cassy lists the files the delivery adds, with renames counted as additions:
`git diff --name-only --diff-filter=A <merge-base> <tip> -- <migrations.paths>`.
The text the `*` matches is the migration id. A merge that adds no migration
costs this one local diff and makes no adapter call.

When ids are found, Cassy calls the `migrations` verb:

```json
{"v":1,"verb":"migrations","env":"staging","sha":"<tip>","args":{"ids":["20261021_add"]}}
```

```json
{"v":1,"ok":true,"data":{"20261021_add":"applied"}}
```

The merge is admitted only when every id is `applied`. Each other outcome
refuses with "No merge was attempted":

| Outcome | Refusal |
|---|---|
| An id is `pending` or `unknown`, or missing from `data` | `MIGRATION PENDING: <ids> on <env>` |
| `external`, `rate_limited` or `insufficient` error, or a timeout | `MIGRATION STATUS UNKNOWN` |
| `invalid` or `internal` error, or a malformed response | `MIGRATION ADAPTER BUG` |
| Local git cannot diff the delivery range | `MIGRATION CHECK FAILED` |

Each refusal names its next step: apply the migration, retry when the
adapter can report status, fix the adapter or its configuration, or fetch the
target and tip so local git can diff them.

A registered supervisor may override a refusal with `supervisor_override=true`,
`task_id` and a `reason` that names the refusal class, for example
`MIGRATION PENDING applied by hand`. A reason written for another gate, such
as red CI, does not accept a migration refusal the supervisor has not seen.
The override is logged as a decision note on the task. Cassy never applies a migration to a
shared environment.
