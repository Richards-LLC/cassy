# Jev client

`cas jev`, the `jev` MCP tool and `cas::jev::JevClient` share a calibrated
TypeSafe decision client, pinned by default to `jev-1.13.0`. Success returns
`{model, answers, usage}`. Answers are noul (yes probability), choice (label,
probabilities, confidence) or score (weighted score, legend, probabilities,
confidence).

```bash
cas jev ask --state 'Payouts failed for three days' --questions @questions.json
cas jev ask --state @state.txt --questions @questions.json
cat state.txt | cas jev ask --state - --questions @questions.json
cas jev batch --input states.jsonl --questions @questions.json --out answers.json
```

Example question map:

```json
{"urgent":{"type":"noul","instructions":"Does this convey urgency?"}}
```

Ask reads literal text, UTF-8 `@file` or stdin as a string. Batch accepts 1–50
nonblank JSONL lines; each line is a state (string/object/array) or an object
containing `state`. Results preserve input order as one JSON array. `--out`
writes the array to a file; otherwise it goes to stdout. Questions accept
inline JSON or `@file`. Invalid inputs fail before batch evaluation.

All harnesses expose the same `jev` tool in their Cassy namespace:
`mcp__cas__jev` (Claude), `mcp__cs__jev` (Codex), `cas__jev` (Grok).
Ask accepts `{action:"ask", state, questions, advisory?}`; batch accepts
`{action:"batch", records:[state,...], questions, advisory?}`. MCP states can
be strings, objects or arrays. Blocking work runs off the async executor.

Library callers construct `JevClient::from_project(cas_root)` and call
`ask(state, questions, caller, advisory)` or `batch(states, questions, caller,
advisory)`. State and questions are `serde_json::Value`. `Outcome::Available`
holds a typed response. Advisory mode (CLI `--advisory`) returns
`{"status":"unavailable","reason":"..."}` for disabled calls, transport
failure or decision-log failure, allowing callers to keep their normal policy.
Strict callers receive `JevError`. Invalid input remains an error.

## Configuration and transport

- `jev.model`: default `jev-1.13.0`, required nonempty.
- `jev.enabled`: default true; false prevents evaluation and key-file reads.
- `jev.key_file`: optional plain development API-key file. Relative paths
  resolve under the project's `.cas`; absolute paths and `~/` are supported.
  Protect the file with mode 0600.

The default is POST `<CloudConfig.endpoint>/api/jev` with the existing cloud
token (inheriting machine login credentials). It carries state, questions,
model and active team_id/canonical project_id when known. The cloud strips
scope fields before forwarding upstream.

An explicit nonempty `TYPESAFE_API_KEY`, or `jev.key_file` if the environment
key is absent, selects direct development transport to
`https://api.typesafe.ai/v1/systemone`, with no cloud scope fields. A proxy
failure never automatically switches credentials. Debug output redacts keys;
server error bodies are not printed/logged. Redirects are refused. Cloud URLs
require HTTPS (HTTP loopback is allowed for tests).

Each evaluation has a 15-second deadline including retries; a batch shares a
45-second deadline (below the MCP server's 55-second limit). Remaining records receive unavailable/log rows after the
batch deadline. HTTP 429/529 get at most three attempts, with exponential
backoff or Retry-After (seconds or HTTP date). A Retry-After exceeding the
remaining deadline returns unavailable without retrying early. Other HTTP or
network failures return immediately.

## Decision log

Every evaluation appends a locked JSONL row to `.cas/jev-decisions.jsonl`,
created with mode 0600 on Unix. Retries produce one logical row. Batches log
every evaluated record, even when strict mode ultimately reports an error.
Rows contain timestamp, caller, SHA-256 of the JSON-encoded state, question ids,
model, typed answers/probabilities/confidence, input_tokens, latency_ms, status
and X-Request-Id. Unavailable rows have null answers/tokens. Neither state nor
credential is stored. A log write failure yields unavailable for advisory
callers or an error for strict callers.

Release impact: new `cas jev ask|batch`, `jev` MCP tool and shared advisory
library; integration into existing policy callers belongs to follow-up tasks.

## Advisory failure classifier

Sweeps, capped worker checks and release-gate failed rows ask the shared client
for `real_regression`, `load_flake_or_timeout`, `host_or_toolchain_env`,
`known_issue` or `unknown`, plus the probability that the failure explicitly
mentions a code-derived touched path. Notices show, for example,
`Jev: host_or_toolchain_env (0.93); touched-change mention=0.17 (advisory)`.
Sweep task notes and their durable supervisor attention payload share the same
annotation. A label never changes status, attribution, cache authorization,
PASS receipts, merging, or release-gate exit codes.

The internal shell entrypoint is plain text:

```bash
cas jev classify-failure --log failed-step.log --source gate:ci-script-tests \
  --base previous-tip --head merged-tip
```

It prints nothing when configuration, transport, response or decision logging
is unavailable. Older binaries also leave the gate's original failure intact.
The classifier uses the configured shared client and its decision log, with a
2-second evaluation/retry budget and a 1-second read-only Git path probe.
Sweep evaluation runs off the daemon's async thread and once after mechanical
attribution, rather than on every diagnostic replay. Worker progress remains
streamed while a private regular-file capture supplies the failure block.

A log preview reads at most 128 KiB from the head and 6 KiB from the tail. It
preserves failing names, the first panic/assertion in those windows, and the
step tail in at most 6 KiB of UTF-8 text. Touched paths are capped at 2 KiB.
ANSI codes and common credential assignments are removed before evaluation;
raw captures remain private. Setup failures use their current error only,
not an earlier integration receipt. Local macOS realpath/flock hints are
context, never authorization to skip a gate.

The frozen cohort is `cas-cli/src/jev/fixtures/failures.json`, with exact
provenance and limits. On 2026-10-02 the pinned `jev-1.13.0` client agreed on
5/5: Mac 97/7 host (.83), PyYAML host (.97), flock host (.91), HUB-J9 load
(1.00), unnamed sweep targets unknown (.78). The final case accepts unknown or
host because the summary lacks the actual failing step. Expected diagnoses are
excluded from inference state. This small operator/developer-labelled cohort
measures these cases, not general calibration or correctness.

Reproduce with a built client and an isolated output directory:

```bash
python3 docs/jev/scripts/evaluate_failures.py --cas-binary /path/to/cas \
  --artifacts /path/to/evidence --key-file /path/to/dev-key
```

Omit `--key-file` to use the normal cloud login or an existing environment key.
Answers, agreement, hash-only decision rows and inference states are recorded
under that directory; credentials are never printed or persisted there.
