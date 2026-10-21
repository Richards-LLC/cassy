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

[qa.release.components]
backend = ["apps/backend", "packages/shared"]
frontend = ["apps/web", "packages/shared"]
```

| Key | Meaning |
|---|---|
| `integration_branch` | The branch the gate walks. |
| `branches` | Promotion targets whose PRs need a clean gate. |
| `deploy_workflows` | Exact GitHub Actions workflow names watched on `integration_branch` (default empty). |
| `adapter` | Project-relative adapter executable. Empty leaves the gate off. |
| `env` | Environment variable names allowed into the adapter. |
| `timeout_secs` | Per-call timeout (default 120). |
| `components` | Served component name to the repository paths it is built from. |

`cas config set qa.release.components '{"backend":["apps/backend"]}'` takes
the components as one JSON object.

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
`fixture-teardown`, `run`, `readback` and `migrations`. This slice implements
`served`; the others are reserved for later slices.

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

## Deploy failure alerts

The factory daemon watches configured deploy workflows on the integration branch
at its existing CI polling cadence. Completed `push` and `workflow_dispatch`
runs count; CI and each deploy workflow have independent verdicts. A failed or
timed-out deploy with a failed job becomes a Critical supervisor `BLOCKER` naming
the job and failed step when GitHub reports one. The relay key includes the
branch, workflow and SHA, so repeated polls send one blocker for that failure.
This watcher works even when no release adapter is configured.

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
