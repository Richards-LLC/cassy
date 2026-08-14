---
name: mcp-integration
description: Use when installing, registering, verifying, debugging, or exposing an MCP server — including scope choice, credential handling, zero-scope keys, worktree visibility, and paid/third-party servers. Not for local dev servers (see cas-servers).
managed_by: cas
---

# Wiring an MCP server

**"Connected" is not "working."** The green dot proves a handshake. Capability is a
`tools/list` count. Correctness is one real call. Every rule below exists because
those three were confused.

Field-tested source: Viktor (autonomous agent operator), 2026-08-14, plus the CAS
session that registered him. Full symptom tables in
[references/diagnosis.md](references/diagnosis.md).

## Before you touch a config file: prove the credential is intact

Do this **first**, not after the API refuses you.

```bash
printf '%s' "$THE_KEY" | wc -c          # length
printf '%s' "$THE_KEY" | tail -c 8      # tail
printf '%s' "$THE_KEY" | cat -A | tail -c 40   # hidden CR/newline/quotes
```

Compare that tail against the provider dashboard's **masked list view**
(`prefix…suffix`). That view is the only ground truth for "is my copy complete",
because the plaintext is shown once at creation and never again. A mismatched tail
proves truncation. Matching tail + matching length narrows the remaining causes to
"wrong key" or "wrong scopes", which the ladder below tells apart.

- **Never transcribe a secret from a screenshot.** Homoglyphs (`0/O`, `l/1/I`) and
  soft-wrap truncation produce a string that looks right. A key that has been
  through a screenshot is both possibly-corrupt and possibly-leaked — mint a new
  one rather than debugging the old one.
- **Auth errors are deliberately generic.** `invalid_api_key` will never tell you
  truncated vs revoked vs wrong-workspace; that would be an enumeration hint.
  Disambiguate client-side, before the call.
- `echo` appends a newline. Use `printf '%s'` or `read -rs KEY`. A trailing `\n`
  in an `Authorization` header is a silent 401 in some stacks.

## Where the secret lives

| Placement | Verdict |
|---|---|
| Inline literal in committed `.mcp.json` | **Never.** Deleting the line does not revoke the key; git history keeps it. |
| `${VAR}` expansion in `.mcp.json` | Correct for repo-shared servers. Confirm your client version expands it. |
| `claude mcp add --header "... $KEY"` (double quotes) | Shell expands at add time; plaintext lands in `~/.claude.json`. Acceptable for one operator's `local` scope, wrong for anything shared. Run `claude mcp get <name>` to see what you actually stored. |
| A worker's prompt, task file, or worktree | **Never.** Workers are the thing you do not trust. |

**The env-var trap.** If the variable is unset when the client process starts you do
not get a clean error — the header goes out empty or as the literal `${VAR}`, and
both return a generic 401 **indistinguishable from a bad key**. Environment is
captured at process start, so exporting it afterwards changes nothing, and a
GUI/daemon/supervisor-spawned process never reads your shell rc at all. When you
see 401, dump what was actually sent before suspecting the key.

## Scope choice — and the CAS worktree consequence

`claude mcp add -s <scope>`: `local` (this user, **this directory path**),
`project` (committed `.mcp.json` — everyone who clones, plus CI), `user` (all
projects for this OS user).

**A git worktree is a new directory path, so `local`-scope registrations do not
follow it.** This cuts both ways and you must choose deliberately:

- Want a server available to workers? `local` will not reach them. Use `user`
  scope or an explicit registration step in worktree setup.
- Want a server **restricted to the supervisor**? `local` scope in the main
  checkout is exactly that boundary — workers in `.cas/worktrees/*` cannot see it.
  (This is how the Viktor key is confined to the supervisor.)

Precedence is **local > project > user**, which produces the most demoralising
symptom in MCP setup: you fix `.mcp.json`, nothing changes, because a stale `local`
entry from an earlier attempt shadows it.

```bash
claude mcp list                    # run from the worktree the agent will use
claude mcp get <name>              # resolved transport, URL, headers, scope
claude mcp remove <name> -s local  # remove per scope; a bare name is ambiguous
```

Delete failed attempts before retrying. Half of "MCP is flaky" is three overlapping
registrations.

## The verification ladder

Run top to bottom. **Each rung eliminates one class of failure, and rung N only
means something if rung N−1 passed.** Never skip to the end because the client shows
a green dot.

| Rung | Check | What it proves / distinguishes |
|---|---|---|
| 0 | `initialize` **without** credentials | `404`/`405`/HTML = wrong URL or transport. `406` = you omitted `Accept: application/json, text/event-stream` (streamable HTTP needs both). `401` = **good** — endpoint and transport are right, only auth remains. |
| 1 | Same call **with** the credential | `401` = key wrong/truncated/revoked or header empty. `403 insufficient_scope` = key is **valid**, different problem, different fix. |
| 2 | `whoami` or equivalent no-scope introspection | Catches the failure invisible everywhere else: **authenticated as the wrong account, workspace, or key type.** Assert on the returned values; do not eyeball. |
| 3 | `tools/list`, **counted and diffed** against an expected set recorded in advance | A zero-scope key authenticates perfectly and exposes one tool. Connectivity perfect, capability nil, visually identical to success. Missing tools = scopes; renamed/extra = the server changed under you. |
| 4 | The client's own view (`claude mcp list`, `/mcp`) | If curl works and the client does not, the fault is client config/env/lifecycle. Most clients resolve the tool list **once at startup** — restart before debugging further. |
| 5 | One cheap **read-only** tool call, from the agent process | `tools/list` proves advertisement, not execution. Failures here that passed rung 3 are schema/protocol mismatches. |
| 6 | One real call — with an idempotency key, a deadline, and a plan for timeout | See below. |
| 7 | Re-run rungs 4–5 **as the actual principal** | Same OS user, cwd/worktree, environment and launch method. "Works in my terminal" says nothing about a supervisor-spawned worker with a scrubbed env or different `PATH`. |

**Verification expires.** Scopes get edited, keys rotated, servers redeployed with
different tool lists. Re-run rungs 2–3 at the start of any session that will spend
money through a server you do not control.

## Operating a server you do not control

**Classify every tool before first use** — this determines retry policy and nothing
else does:

1. Read-only → retry freely on transport errors.
2. Idempotent given a key → retry **only** with the same key.
3. Starts work / costs money / offers no key → **never auto-retry.** Reconcile
   first: query for the object you might have created, then decide.

**Timeout is not failure.** A client-side deadline elapsing tells you nothing about
server state — the work is probably still running, and a naive retry doubles the
spend and can produce two divergent results. Where the server returns a run handle
on timeout, store it and resume; never re-issue the starting call. Where no
idempotency key exists, put a unique correlator in the payload and search for it
before retrying.

**Prefer async over longer timeouts.** For browser verification, research or builds,
use create-then-poll rather than raising the tool timeout. Long inline waits burn a
worker slot, tie the outcome to a fragile connection, and hit proxy idle-timeouts
you do not control.

**Slow vs hung:** zero bytes for well beyond the documented keepalive interval is
hung, not slow (`curl -v --no-buffer` shows this). Fire a trivial call in parallel —
instant response means the server is healthy and your call is genuinely long. Poll
server-side run state out of band if exposed.

**Rate limits:** honour `Retry-After`; otherwise exponential backoff **with jitter**
(unjittered backoff across a fleet produces synchronised retry storms). Cap
concurrency **at the fleet level, not per worker** — twenty workers each politely
limiting to 2 is 40 concurrent calls. Do not mint extra keys to buy throughput; with
a shared credit pool that converts a clean `429` into a silent budget drain.

## Before exposing a paid or credentialed server to the fleet

The useful question is not "what does it cost" — that has no enforceable answer at
the boundary. It is **where does denial live, and who is the principal?** Answer
these in writing first:

1. **Who is the principal at the far end?** One key usually means one identity:
   supervisor and every worker look identical upstream, so there can be no
   server-side per-worker policy. If you need attribution you must manufacture it —
   pass `cas_task_id` and worker id in the request, require them echoed back in the
   response schema, and log both sides.
2. **What is the ceiling and who enforces it?** If the provider has no per-key spend
   cap, **denial must live in CAS**: a budget ledger, per-task and fleet-wide daily
   caps, and a refusal that is a hard error rather than a warning.
3. **What does one bad loop cost?** Not one mistaken call — an autonomous worker
   retrying overnight, times N workers. Compute that number before handing out the
   credential. If it is unacceptable, **the credential does not leave the
   supervisor.**
4. **Minimum scope, one key per purpose**, so one can be revoked without breaking
   everything. Where the key is broader than a worker should hold, filter at
   `cas serve` / the MCP proxy — a tool-name allowlist is a real boundary that
   survives a worker being creative.
5. **What is the kill switch, and have you rehearsed it?** Revoke, then re-run
   rung 1 and confirm 401 within seconds. Do the drill before you need it. Rotate on
   any exposure: screenshot, log, chat paste, CI artifact, committed config.

Two blast-radius items specific to fleets:

- **Context cost.** Every registered server's tool list is injected into every
  agent's prompt, every turn. Five servers × twenty tools is real token spend *and*
  measurably worse tool selection. Register per role; prune aggressively.
- **Untrusted tool metadata.** Tool names and descriptions come from the server and
  land in the model's prompt. Treat them as untrusted input, allowlist names, and
  diff descriptions on change — which is why the rung-3 baseline should record
  descriptions, not just names.

## Heterogeneous harnesses

**Do not assume MCP support.** Claude Code has first-class MCP with local/project/
user scopes. Codex config lives in `~/.codex/config.toml` under `[mcp_servers.*]`
and is historically **stdio-oriented**, so a remote streamable-HTTP server may need
a bridge or proxy — verify against the installed CLI version, not docs for another.
Grok workers may have no MCP client at all.

Detect capability per harness and **degrade explicitly**: a worker without an MCP
client must report "no MCP client, routing via supervisor", never silently skip the
step.

**Centralise transport translation.** CAS ships `cas serve` and an optional proxy —
make that the single place holding credentials, speaking streamable HTTP upstream,
exposing stdio downstream, and enforcing the tool allowlist, fleet concurrency cap
and budget ledger. One credentialed hop is far easier to verify, rotate and revoke
than N heterogeneous client configs.

**Log every MCP call**: server, tool, task/worker id, request id, idempotency key,
duration, outcome, and cost where available. Without it you cannot tell a retry
storm from load, cannot attribute spend, and cannot answer "did that timed-out call
actually run?"

## Don't reach for MCP reflexively

If a worker needs exactly one HTTP call, a script with `curl` is cheaper, more
debuggable, trivially loggable, costs zero context, and has none of the transport,
session and lifecycle surface above. **MCP earns its complexity when the model needs
to choose among several tools dynamically.** For fixed workflows, call the API.

## Finishing an install

An install step that ends without an assertion will eventually hand a worker a
zero-scope key and let it report "MCP configured". Worktree setup must: check for an
existing registration → remove stale ones **per scope** → register → run rungs 0–5 →
assert the expected tool-name set → write the result to the task log.

**Restart semantics are part of the install.** If the procedure does not end with
"restart the client and re-verify", it is not finished.
