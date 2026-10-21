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
`fixture-teardown`, `run`, `readback` and `migrations`. `served` and
`readback` are implemented; the others are reserved for later slices.

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

## Read-back

A journey declares what the gate may read back after it runs:

```yaml
journeys:
  - id: signup
    readback:
      subjects: [account_uid, workspace_id]
      invariants: [INV-ACCOUNT, INV-WORKSPACE]
```

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
