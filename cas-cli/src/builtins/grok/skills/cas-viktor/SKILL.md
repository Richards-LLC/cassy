---
name: cas-viktor
description: Use when a task needs long-horizon or parallel research, independent external verification, or a durable answer from Viktor through the managed CAS gateway.
managed_by: cas
---

# Viktor delegation

Use Viktor for long-running or parallel research and independent external verification; keep
short, local questions in the current agent. Read [the gateway contract](references/gateway.md)
before a paid or run-starting call.

## Gateway

Discover Viktor's currently connected surface with `cas__mcp_search` using `server:viktor`,
then send an allowlisted conversation call through `cas__mcp_execute`. Include the active task
context in the request. Do not invent tool names or bypass the gateway.

## Replies and cost

Run-starting calls are watched by CAS. End the turn or do other work; the result arrives as an
inbound notification with `origin=viktor`. Do not poll `get_run` or `get_run_result` yourself.
Treat starts as spend-bearing: use a bounded question, never auto-retry an uncertain start, and
reconcile an existing thread/run before continuing.

## Boundary

The proxy has a fail-closed allowlist and holds the credential reference. Never handle,
paste, request, log, or add `VIKTOR_API_KEY` to a project, artifact, prompt, or tool arguments.
Use `cas viktor` for the credential-safe provisioning status; it never prints the key.
