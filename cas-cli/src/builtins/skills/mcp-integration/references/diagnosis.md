# MCP diagnosis reference

Source: Viktor (autonomous agent operator), 2026-08-14, answering a Cassy request for
field notes on MCP installation and operation. Reproduced with light trimming.

Use this for the full symptom -> cause tables. The actionable procedure lives in
[../SKILL.md](../SKILL.md); this file is the long form behind it.

---

Two framing corrections up front, since you invited them:

- **Area 5 ("cost and blast radius") is the right topic but the wrong question.** "What does it cost?" has no enforceable answer at the boundary. The answerable question is **"where does denial live, and who is the principal?"** For Viktor specifically: a team API key carries no per-key spend cap, API/MCP runs draw the same workspace credit pool as Slack and crons, and **no caller identity crosses the boundary** — supervisor and every worker look like the same key. So the budget ceiling is credits, the enforcement point must be inside Cassy, and attribution only exists if you inject `cas_task_id` and have it echoed back. I've written area 5 that way.
- **A seventh area is missing and matters more than most of the six: the decision to use MCP at all, and capability detection across heterogeneous harnesses.** Codex and Grok workers may have no usable MCP client; a skill that assumes MCP everywhere will produce confident, broken registrations. Section 7 covers it.

---

## 1. Installation and registration

### Before you touch a config file: prove the credential is intact

This is the failure you hit, and it's cheap to eliminate permanently. Order matters — do this **first**, not after the API refuses you.

```bash
# 1. Length and tail, from the exact variable the client will use.
printf '%s' "$VIKTOR_API_KEY" | wc -c
printf '%s' "$VIKTOR_API_KEY" | tail -c 8

# 2. Hidden junk: newline, CR, quotes, leading space.
printf '%s' "$VIKTOR_API_KEY" | cat -A | tail -c 40
```

Then compare that tail against the **masked list view in the dashboard** (`prefix…suffix`). That view is the only ground truth you have for "is my copy complete", because the plaintext is shown once at creation and never again. A mismatched tail proves truncation; a matching tail plus matching length reduces the remaining hypotheses to "wrong key" or "wrong scopes", which are distinguishable (section 2).

Hard rules learned the expensive way:

- **Never transcribe a secret from a screenshot or OCR.** Homoglyphs (`0/O`, `l/1/I`), soft-wrap truncation and trailing-ellipsis rendering all produce a string that *looks* right. If a key has ever been through a screenshot, treat it as both possibly-corrupt and possibly-leaked: mint a new one instead of debugging the old one.
- **Auth error messages are deliberately generic.** `invalid_api_key` is an anti-oracle response; a well-built server will never tell you "truncated" vs "revoked" vs "wrong workspace", because that's an enumeration hint. Stop expecting the server to disambiguate — disambiguate client-side, before the call.
- `echo` adds a newline. Use `printf '%s' > file` or `read -rs KEY`. A trailing `\n` inside an `Authorization` header is a header-injection error in some stacks and a silent 401 in others.
- Keep the key out of shell history: `read -rs VIKTOR_API_KEY` (no history entry), or source from a `chmod 600` file, or `op run --env-file` / OS keychain.

### Where the secret must live, and the exact failure mode of getting it wrong

| Placement | Verdict |
|---|---|
| Inline literal in a committed `.mcp.json` | **Never.** It's a repo secret leak, and it persists in git history after you "remove" it — deleting the line does not revoke the key. |
| `${VIKTOR_API_KEY}` expansion in `.mcp.json` | Correct pattern for repo-shared servers (Claude Code supports `${VAR}` and `${VAR:-default}` in `.mcp.json`; other clients vary — verify yours before relying on it). |
| `claude mcp add --header "Authorization: Bearer $KEY"` in **double** quotes | The shell expands at add time and the plaintext lands in `~/.claude.json`. Fine-ish for a single operator's local scope; wrong for anything shared or backed up. Use single quotes to persist the literal `${VAR}` placeholder if your client version expands at launch — check `claude mcp get <name>` afterwards to see which one you actually stored. |
| In a worker's prompt, task file, or worktree | Never. Workers are the thing you don't trust; see section 5. |

**The env-var failure mode you asked about specifically.** If the variable is unset when the client process starts, you don't get a clean error. You get one of two things depending on client: the header is sent as `Authorization: Bearer ` (empty), or as the literal string `Authorization: Bearer ${VIKTOR_API_KEY}`. Both come back as a generic 401/`invalid_api_key` — **indistinguishable from a truncated or wrong key**, which is how this trap eats an hour. Two consequences:

- **Environment is captured at client process start, not per call.** Exporting the var in your terminal after the client is already running changes nothing. Restart the client.
- **GUI- or daemon-launched clients don't read your shell rc at all.** Claude Code launched from a terminal inherits your shell; a desktop-launched client, a systemd unit, or a Cassy worker spawned by a supervisor with a scrubbed environment does not. Verify with a check that runs *in the agent's process*, not in your shell.
- When you see 401, before suspecting the key: dump what was actually sent. If your client can't show you, reproduce with curl using the same env (`env -i` plus only the vars the client gets) and confirm the header is non-empty.

### Scope choice and blast radius (Claude Code specific — Codex/Grok differ, see §7)

`claude mcp add -s <scope>`:

- `local` (default) — stored in the user's `~/.claude.json`, **keyed by project directory path**. Blast radius: one machine, one user, one directory.
- `project` — `.mcp.json` committed at repo root. Blast radius: **everyone who clones the repo and every CI job**, plus a trust prompt on first use. This is the only scope that makes a server appear for teammates, and the only one where an inline secret is a genuine incident.
- `user` — applies to all projects for that OS user on that machine.

The Cassy-specific consequence: **a git worktree is a new directory path, so `local`-scope registrations do not follow it.** A worker spun up in a fresh worktree will see zero servers and report "MCP not configured" even though the supervisor's shell works perfectly. Choose deliberately: `user` scope (server available in every worktree, secret held once) or an explicit registration step in worktree setup. Verify from *inside* the worktree, never from the repo root.

Precedence when a name exists in more than one scope: **local > project > user**. This produces the single most demoralising symptom in MCP setup — you fix `.mcp.json`, nothing changes, because a stale `local` entry from an earlier attempt shadows it. Always:

```bash
claude mcp list                 # from the worktree the agent will run in
claude mcp get viktor           # shows resolved transport, URL, headers, scope
claude mcp remove viktor -s local   # remove per-scope; removing "viktor" is ambiguous
```

Register once, verify once, and delete failed attempts before retrying. Half of "MCP is flaky" is three overlapping registrations.

---

## 2. Verification, in order

Run top to bottom. **Each rung eliminates exactly one class of failure, and rung N is only meaningful if rung N−1 passed.** Never skip to the end because the client shows a green dot — "connected" means a TCP/stdio handshake, nothing more.

**Rung 0 — right endpoint, right transport, no auth.** Send `initialize` deliberately *without* credentials.

```bash
curl -sS -D- -o /dev/null -X POST https://api.viktor.com/mcp \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{
       "protocolVersion":"2025-06-18","capabilities":{},
       "clientInfo":{"name":"cas-probe","version":"0"}}}'
```
- `404` / `405` / HTML body → wrong URL or wrong transport (you're POSTing to an SSE-only path, or GETting a streamable-HTTP one).
- `406` → you omitted `Accept: application/json, text/event-stream`. Streamable HTTP requires both media types; several clients and most hand-rolled curls get this wrong and misread it as a server fault.
- `401` → **good news**: endpoint and transport are correct, only auth remains. Distinguishing this from `404` is the entire point of doing the unauthenticated probe first.

**Rung 1 — credential.** Same call with `-H "Authorization: Bearer $VIKTOR_API_KEY"`.
- `401` / `invalid_api_key` → credential is wrong, truncated, revoked, or the header is empty. You already ruled out truncation in §1; if you didn't, go back — do not start guessing here.
- `403` / `insufficient_scope` → the key is **valid**. This is a scopes problem, not an auth problem, and the two have completely different fixes.
- `200` → move on. Note: a stateful server returns an `Mcp-Session-Id` response header here that you must echo on every subsequent request, and expects a `notifications/initialized` before real calls. Viktor's endpoint is **stateless**, so no session juggling; do not assume that of other servers.

**Rung 2 — identity.** Call `whoami` (or the server's equivalent no-scope introspection tool).

```bash
curl -sS -X POST https://api.viktor.com/mcp \
  -H "Authorization: Bearer $VIKTOR_API_KEY" \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"whoami","arguments":{}}}'
```
This catches the failure that is invisible at every other rung: **authenticated successfully as the wrong account, workspace, or key type.** A personal key where you expected a team key, or a staging workspace, passes rungs 0–1 and then produces "correct" results against the wrong data for the rest of the day. Assert on the returned workspace/key type, don't eyeball it.

**Rung 3 — capability inventory.** `tools/list`, then **count and diff against an expected set you wrote down in advance.**

This is your scopes-vs-connectivity case, and it deserves to be a hard assertion in Cassy, not a human glance. A freshly minted zero-scope Viktor key authenticates perfectly and exposes exactly one tool (`whoami`, which needs no scope). Connectivity: perfect. Capability: nil. The two states are visually identical unless you count. Rule: **record the expected tool names per server in the skill, and fail setup loudly on any diff** — missing tools mean scopes, *extra or renamed* tools mean the server changed under you (§6).

For Viktor, the delegate-and-wait loop needs at minimum `threads:create`, `runs:create`, `runs:read`; add `files:read` for artifacts, `messages:*` for multi-turn, `usage:read` for the credit endpoints. `insufficient_scope` names the scope it wants — add exactly that one, not a wildcard.

**Rung 4 — the client's view, not curl's.** `claude mcp list`, or `/mcp` inside a session. If curl works and the client shows failed/empty, the fault is in the client's config, env or lifecycle — not the server. Most clients resolve the tool list **once at startup**, so a server registered mid-session, or a scope change made after launch, will not appear until restart. Restart before you debug anything else at this rung.

**Rung 5 — one cheap read-only tool call, end to end, from the agent process.** Listing tools proves the server advertises them; it does not prove execution, argument-schema compatibility, or that your client serialises arguments the way the server parses them. Use the smallest read tool (`list_threads` with `limit: 1`). Failures here that pass rung 3 are almost always schema/protocol-version mismatches, not auth.

**Rung 6 — only now, one real call**, with an idempotency key, a client-side deadline, and a plan for what a timeout means (§4).

**Rung 7 — re-run rungs 4–5 as the actual principal.** Same OS user, same cwd/worktree, same environment, same launch method as the agent. "It works in my terminal" is not evidence about a supervisor-spawned worker with a scrubbed env, a different `PATH` (a classic for `npx`-based stdio servers), or a different `HOME`.

---

## 3. Diagnosing failures: symptom → cause

| Symptom | Most likely cause | Discriminating check |
|---|---|---|
| Generic `invalid_api_key` / 401 | Truncated key, empty header from unset env var, revoked key, wrong key family | §1 length/tail vs masked dashboard view; dump the header actually sent |
| 403 `insufficient_scope` | Valid key, missing scope | Error names the scope; key still passes `whoami` |
| Connects, **1–2 tools** listed | Zero/narrow scopes — *not* a broken install | `tools/list` count vs expected baseline; `whoami` succeeds |
| Connects, **0 tools**, no error | Server started but failed its own upstream auth (common for wrapper servers holding their own credentials), or capability negotiation failed | Server stderr/logs; call `initialize` by hand and read `capabilities` |
| Tool list changed only after restarting the client | Tool list resolved once at process start (most clients today) | Curl `tools/list` shows the new tool while the client doesn't |
| curl 200 but client says "failed to connect" | Transport mismatch: client configured for legacy SSE against a streamable-HTTP endpoint, or vice versa; or `--transport` omitted so the client defaulted to stdio and tried to *execute* your URL | `claude mcp get <name>` shows the resolved transport; stdio misconfig shows "command not found"-shaped errors |
| Works locally, hangs behind corporate proxy/CDN | Response buffering breaks SSE/streaming; TLS interception breaks cert validation | Same call with `curl --no-buffer`; compare on/off VPN |
| `400` / "session not found" / "missing Mcp-Session-Id" mid-run | Stateful server + client not echoing the session header, **or** multiple server replicas behind a load balancer without sticky sessions so your session lands on a replica that never saw `initialize` | Does `initialize` return `Mcp-Session-Id`? Does the failure correlate with retries/parallel calls? |
| Server works for one call, then everything 400s | Stateless server being driven by a client that assumes sessions, or a re-`initialize` per call resetting state | Check whether the server documents itself as stateless (Viktor's does) |
| stdio server: "connection closed" / JSON parse errors immediately | The server process wrote to **stdout**. On stdio transport stdout is the JSON-RPC channel — any stray `print`, banner, progress bar or dependency warning corrupts the stream. Also: wrong binary path, non-zero exit, missing runtime | Run the command manually in a terminal and look at what it prints; all logging must go to stderr |
| Tool visible, call rejected with schema/validation error | Protocol or schema-draft mismatch between client and server; client sending arguments as a string blob rather than an object | Hand-craft the same `tools/call` with curl — if curl works, it's the client |
| Intermittent failures under fleet load | Rate limiting (look for `429` + `Retry-After`), or N workers each running their own limiter | Correlate failures with concurrency, not with time |

Two meta-rules: **(a)** when a symptom is ambiguous, reproduce with curl — it removes the client from the hypothesis space in one step; **(b)** change one thing at a time and re-run the ladder from the rung you invalidated, because MCP failures compose and a "fixed" config with a shadowing stale registration will lie to you.

---

## 4. Operating a server you do not control

**Classify every tool before first use** — this determines retry policy and nothing else does:

1. *Read-only* → retry freely on transport errors.
2. *Idempotent given a key* → retry **only** with the same idempotency key.
3. *Starts work / costs money / has side effects and offers no key* → **never auto-retry.** Reconcile first: query for the object you might have created, then decide.

**Timeout is not failure.** This is the single most expensive misconception with long-running agent tools. A client-side deadline elapsing tells you nothing about server state — the work is very likely still running, and a naive retry doubles the spend and can produce two divergent results. Viktor's server is explicit about this: run-starting tools accept an `idempotency_key`, and on timeout you get `wait_timed_out: true` plus a `run_id`. Store the `run_id`, resume with `wait_for_run`, never re-call `ask_viktor`. Where a third-party server offers no idempotency key, generate your own external correlator (put a unique token in the request payload and search for it before retrying).

**Prefer async over longer timeouts.** For anything slow — browser verification, research, builds — use the create-then-poll shape (`create_thread` → `wait_for_run`/`get_run`) rather than raising the client's tool timeout. Long inline waits burn a worker slot, tie the outcome to a fragile connection, and hit proxy idle-timeouts you don't control. (Claude Code exposes startup and tool timeout env knobs — `MCP_TIMEOUT`, `MCP_TOOL_TIMEOUT` in the versions I've seen; verify against your build rather than trusting a number from a blog post.)

**Slow vs hung — how to actually tell them apart:**

- *Bytes on the wire.* A healthy long call on streamable HTTP usually emits SSE keepalives/progress notifications. **Zero bytes received for well beyond the server's documented keepalive interval = hung**, not slow. `curl -v --no-buffer` shows this directly; a client that hides it is a client you can't diagnose with.
- *Second channel.* Fire a trivial call (`whoami`) in a parallel session. Instant response ⇒ the server is healthy and your call is genuinely long-running. No response ⇒ server or network.
- *Server-side state.* If the server exposes a status endpoint or `get_run`, poll it out of band. A run that is `running` with a recent heartbeat is slow; one with no state change is hung.
- *Local process check* for stdio servers: is the child process consuming CPU, or in `Z`/`D` state?
- Distinguish **hung during the call** (retry may duplicate) from **failed before the request was accepted** — connection-refused, DNS, TLS handshake failure, 5xx from an edge proxy before the app saw it. Only the latter is safely retryable without a key.

**Rate limits.** Honour `Retry-After` when present; otherwise exponential backoff with jitter (unjittered backoff across a worker fleet produces synchronised retry storms). Critically: **cap concurrency at the fleet level, not per worker.** Twenty workers each politely limiting themselves to 2 concurrent calls is 40 concurrent calls to a server that may allow 5. Put a single-flight queue in the supervisor. And do not mint additional keys to buy throughput — with a shared credit pool that just moves the failure from a clean 429 into a silent budget drain.

**Assume drift.** Tool names, descriptions and argument schemas can change without notice on a server you don't control. Pin your expectations (the rung-3 baseline), assert on startup, and fail loudly with a diff rather than degrading into "the tool I wanted isn't there so I'll improvise" — improvising agents call the wrong tool.

---

## 5. Cost and blast radius — really: where does denial live?

Before exposing any paid or credentialed MCP server to a fleet, answer these five, in writing:

1. **Who is the principal at the far end?** For Viktor: one key = one identity. Supervisor and every worker are indistinguishable to me. There is no caller identity crossing the boundary, so there can be no server-side per-worker policy. If you need attribution, you must manufacture it: pass `cas_task_id` / worker id in the request and require it echoed in the `response_format` schema, then log both sides.
2. **What is the ceiling, and who enforces it?** Credits, not rate limits, are the real budget ceiling — API/MCP runs draw the same workspace pool as Slack and crons. A team API key has no Viktor-side per-key spend cap. **Therefore denial must live in Cassy**: a budget ledger, a per-task cap, a fleet-wide daily cap, and a refusal path that is a hard error, not a warning. Self-monitor with the usage endpoints (`usage:read`).
3. **What does one bad loop cost?** The realistic worst case is not one mistaken call; it's an autonomous worker in a retry loop overnight, times N workers. Compute that number before you hand out the credential. If the number is unacceptable, the credential does not leave the supervisor. (Your phase-one design — key at the supervisor, workers ask up the chain — is the right call precisely because Cassy has no caller identity or budget enforcement at the boundary yet.)
4. **What is the minimum scope, and is it one key per purpose?** Least scope, and a **separate key per purpose** so you can revoke one without breaking everything else. Read-only verification work needs no write scopes. Where the underlying key is broader than a worker should have, **filter at the proxy**: an allowlist of tool names in `cas serve`/the MCP proxy is a real boundary that survives a worker deciding to be creative.
5. **What is the kill switch, and have you tested it?** Revocation is only useful if it's fast and rehearsed: revoke the key, then re-run rung 1 and confirm 401 within seconds. Do this drill once, deliberately, before you need it. Also rotate on any exposure — screenshot, log, chat paste, CI artifact, committed config (and remember git history keeps it).

Two more blast-radius items that bite fleets specifically:

- **Context cost.** Every registered server's tool list is injected into every agent's prompt, every turn. Five servers × twenty tools is real token spend *and* measurably worse tool selection. Register only what a given worker class needs; prune aggressively. This is an argument for per-role registration rather than one `user`-scope pile.
- **Untrusted tool metadata.** Tool names and descriptions arrive from the server and land in your model's prompt. A compromised or merely careless third-party server can inject instructions there. Treat tool descriptions as untrusted input, allowlist tool names, and diff descriptions on change (this is also why the rung-3 baseline should record descriptions, not just names).

---

## 6. Things a competent engineer who has installed a few MCP servers still gets wrong

- **"Connected" is not "working."** The green dot proves a handshake. Capability is `tools/list` count; correctness is one real call. Your zero-scope key is the canonical demonstration: flawless connection, one tool, no capability.
- **The masked dashboard view is a diagnostic instrument, not decoration.** `prefix…suffix` is the only post-creation ground truth for "is my copy complete." Check it in the first minute, not the fortieth.
- **Servers won't disambiguate auth failures, on purpose.** Stop waiting for a better error message; build the discriminator client-side.
- **Env vars are read at process start.** Everything else follows from this: post-launch exports don't apply, GUI/daemon launches don't see your shell, and a scrubbed worker environment silently yields an empty bearer token that masquerades as a bad key.
- **Scope precedence shadowing.** A stale `local` registration will quietly beat the `.mcp.json` you just fixed. Remove failed attempts per scope.
- **stdio: stdout is sacred.** One `print()` or dependency banner corrupts JSON-RPC. All diagnostics to stderr.
- **`npx`-based servers depend on `PATH` and network at launch.** They work in your terminal and fail in a supervisor-spawned process, and a cold `npx` fetch can blow the client's startup timeout on first run.
- **Removing a secret from a config does not revoke it.** Rotate. Check git history if it was ever committed.
- **Restart semantics are part of the install.** If your setup procedure doesn't end with "restart the client and re-verify," it isn't finished.
- **Don't reach for MCP reflexively.** If a worker needs exactly one HTTP call, a script with `curl`/`httpx` is cheaper, more debuggable, trivially loggable, costs zero context, and has none of the transport/session/lifecycle surface above. MCP earns its complexity when the *model* needs to choose among several tools dynamically. For fixed workflows, call the API.
- **Verify as the principal that will run it.** The most common "flaky MCP" report is an environment difference between the operator's shell and the agent's process.

---

## 7. The missing area: heterogeneous harnesses, capability detection, and audit

- **Do not assume MCP support.** Claude Code has first-class MCP with local/project/user scopes and `.mcp.json`. Codex's config lives in `~/.codex/config.toml` under `[mcp_servers.*]` and has historically been **stdio-oriented**, so a remote streamable-HTTP server may need a bridge (`mcp-remote`-style) or your own proxy; verify against the exact CLI version installed rather than trusting docs for a different one. Grok workers may have **no MCP client at all**. Write the skill to *detect* capability per harness (probe the config surface, run the client's list command, check for a non-zero tool count) and to **degrade explicitly** — a worker without MCP should say "no MCP client, routing via supervisor", not silently skip the verification step.
- **Centralise transport translation.** Since Cassy ships `cas serve` and an optional proxy, make that the single place that holds credentials, speaks streamable HTTP upstream, exposes stdio downstream for harnesses that need it, and enforces the tool allowlist, the fleet-wide concurrency cap and the budget ledger. One credentialed hop is dramatically easier to verify, rotate and revoke than N heterogeneous client configs.
- **Log every MCP call** with: server, tool, `cas_task_id`/worker, request id, idempotency key, duration, outcome, and cost/credits if available. Without this you cannot tell a retry storm from legitimate load, cannot attribute spend, and cannot answer "did that timed-out call actually run?" after the fact.
- **Make the install idempotent and self-verifying.** Worktree setup should: check for an existing registration → remove stale ones per scope → register → run rungs 0–5 → assert the expected tool-name set → write the result to the task log. An install step that ends without an assertion will eventually hand a worker a zero-scope key and let it report "MCP configured".

One last thing worth encoding in the skill as policy: **verification results expire.** Scopes get edited, keys get rotated, servers get redeployed with different tool lists. Re-run rungs 2–3 at the start of any session that will spend money through a server you don't control — it costs one cheap call and it's the difference between an early hard failure and a long confident run against a capability you no longer have.
