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

## File questions

```bash
cas jev files --glob 'cas-cli/src/**/*.rs' --questions @questions.json
cas jev files --glob 'scripts/*.sh' --rev main --offset 50 --questions @questions.json
cas jev files --path docs --recursive --max-files 10 --max-bytes 8192 --questions @questions.json --advisory
```

The MCP equivalent is `{action:"files", paths:["docs"], recursive:true,
max_files:10, max_bytes:8192, offset:0, rev:"main", questions, advisory:true}`; `globs` is an array of
quoted project-relative patterns. Paths/globs may be combined. Globs support
`**` recursively; directory paths include immediate files unless `recursive`
is true. Paths resolve relative to the project containing `.cas`, rather than
the shell's working directory. Outside-root paths/globs and symlinks resolving
outside that root are reported as skipped. Other symlinks are also skipped,
preventing an alias from exposing an ignored target. Symlink directories are
not followed.

Selection respects `.gitignore`, local Git exclusions and nested ignore files,
including explicit file paths. Hidden files are considered. The following path
components are refused before content is read: `.env*`, `*.pem`, `creds`,
`credentials`, `.credentials`, `secrets`, `.secrets`, `.ssh`, `.aws`, `.gnupg`,
`.git` and `.cas` (case-insensitive). Both supplied and canonical target paths
are checked, preventing a benign symlink name from bypassing secret refusal.
Git/Cassy internals are also excluded from walking.

`max_files` defaults to 50 and accepts 1–50 matching candidates (including
candidates later skipped); overlapping selectors are deduplicated. Selection
is deterministic by path. `limit_reached:true` reports more matches than the
cap or scan deadline exhaustion. Resume with the returned `next_offset` as
`offset` (CLI: `--offset`), keeping selectors and revision unchanged. Offsets
count matching candidates, including those subsequently skipped. A null
`next_offset` means selection is exhausted. Working-tree pages assume files
stay unchanged; use `rev` to pin a sweep. If scanning times out before the
requested offset, narrow the selectors rather than treating an empty page
as complete. Empty matches return an empty list. Explicit
ignored, missing, refused and unreadable paths have skip reasons. No ignored
file is read to classify it. Binaries (NUL-containing/non-UTF-8 prefixes) and
non-regular files are skipped. Classification inspects only the capped prefix.

`max_bytes` defaults to 24 KiB and accepts 1–128 KiB. Reads stop after cap + 1
bytes. An over-cap file returns `status:"incomplete", truncated:true`
and a reason, with no answers and no Jev call. A truncated prefix cannot prove
absence; increase `max_bytes` or supply a smaller complete input. This
abstention applies to every question type, in both strict and advisory modes.
Questions are validated before reading/evaluation. All file calls and scanning
share a 45-second deadline, each request also bounded by 15 seconds.

Optional `rev` (CLI: `--rev`) resolves a Git commit once. Paths, directories
and globs select its tree, including files missing from the checkout; blob
reads stay capped and never follow symlinks or checkout filters. The response
returns the resolved `revision`; use that SHA on subsequent pages. Ignore
rules come from `.gitignore` and `.ignore` at that revision rather than dirty
checkout rules. Secret path refusals still apply. Missing revision paths are
reported as `missing at revision`, distinct from unreadable blobs. Without
`rev`, existing working-tree ignore rules and reads remain in use.

For each accepted complete file, only code reads its content and sends
`{path, content}` as state to Jev. The agent receives compact JSON:

```json
{"files":[{"status":"available","path":"src/example.rs","truncated":false,"model":"jev-1.13.0","answers":{"urgent":{"type":"noul","noul":0.1}},"usage":{"input_tokens":123,"output_tokens":20}},{"status":"skipped","path":".env.local","reason":"secret path"}],"limit_reached":false}
```

Available rows preserve typed probabilities/confidence. Incomplete rows have
no answers; do not treat them as negative evidence. No content/state is
returned. Advisory failures are `{status:"unavailable",path,reason}` rows;
strict failures return an error after logging the attempted files. Every file
call uses the existing hash-only decision log (`cli:jev.files` or
`mcp:jev.files`); skipped candidates make no HTTP call or decision-log row.

## Hook gate shadow (not part of 3.45.0)

`cas config set jev.gate.shadow true` opts into observing PreToolUse Bash,
Write and Edit calls. The default is false. The existing handler completes
first; the observer cannot replace or mutate its result. Actual permissions,
reasons and rewrites remain identical with shadow enabled or disabled.
`jev.enabled=false` records unavailable observations without contacting Jev.

The shipped `docs/research/jev-gate-questions.json` supplies the risk Score and
user-requested/untrusted Nouls. Each tagged `gate_shadow` row in the existing
`.cas/jev-decisions.jsonl` contains the original decision/output hash, answers,
request ID, latency, state hash and both candidate policies:

- `would_decide_eval` follows the evaluated policy: preserve existing denials,
  deny at risk ≥2.5 or from_untrusted ≥0.8, ask at risk ≥1.5 unless
  user_requested ≥0.8, and preserve existing asks/rewrite/allow otherwise.
- `would_decide_literal` omits the user-requested exemption. The actual hook
  decision is unchanged for both policies, including unavailable responses.

The synchronous observer uses an 800ms caller watchdog covering credential
resolution, HTTP/retries and response-body reading. It does not join a late
network worker. Remaining local configuration/redaction/JSON/file-I/O cost is
measured separately by real-hook before/after timings; this is not a hard
real-time filesystem guarantee. Logging uses a nonblocking lock; a busy,
non-regular or unavailable log is best-effort and never blocks or changes the
hook. A late network result cannot write a duplicate row.

Runtime context is deliberately marked incomplete: hooks do not supply
verified recent user requests, tool-output provenance or recovery facts. No
transcript text is promoted to trusted evidence. Bash commands are capped at
16KiB and pass through secret redaction. Write/Edit contents are omitted (only
path and byte counts are sent). Raw state, command/body contents and keys are
absent from the log and report. These reduced observations do not reproduce
the evaluation corpus's rich supplied context and do not justify enforcement.

```bash
cas jev gate-report
cas config reset jev.gate.shadow
```

The report is local JSON, requires no credentials or API call, and ignores
non-shadow evaluations. It reports available/unavailable totals, agreement
among available observations, would-deny/would-ask counts for both policies,
exemption differences, latency median/p95/max and up to ten disagreement
examples identified by timestamp/tool/state hash. Existing denials count in
would-deny totals; unavailable observations preserve the existing decision.
Examples contain no commands or file contents. Malformed JSON lines are
counted and skipped. Release assembly and any future enforcement are separate.
