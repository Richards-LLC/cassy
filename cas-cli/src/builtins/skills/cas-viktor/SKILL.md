---
name: cas-viktor
description: Use when a task needs long-horizon or parallel research, independent external verification, or a durable answer from Viktor through the managed CAS gateway.
managed_by: cas
---

# Viktor delegation

Use Viktor for long-running or parallel research and independent external
verification. Keep short, local questions in the current agent. Read [the
gateway contract](references/gateway.md) before a paid or run-starting call.

## Gateway procedure

1. Discover the currently connected Viktor surface with
   `mcp__cas__mcp_search` using `server:viktor`.
2. Call only an advertised, allowlisted route through
   `mcp__cas__mcp_execute`.
3. Include the active task id and a bounded objective in the request.
4. Let CAS watch the result and deliver it as an inbound notification; do not
   poll provider run endpoints yourself.

The dispatch shape is a JSON string passed through the `code` parameter:

```text
mcp__cas__mcp_search(code="server:viktor", max_length=4000)
mcp__cas__mcp_execute(code="{\"server\":\"viktor\",\"tool\":\"whoami\",\"args\":{}}", max_length=4000)
mcp__cas__mcp_execute(code="{\"server\":\"viktor\",\"tool\":\"ask_viktor\",\"args\":{\"question\":\"Review this bounded question\",\"cas_task_id\":\"<task-id>\"}}", max_length=8000)
```

The JSON dispatch form is equivalent to the proxy's dot-call form when that
route advertises it. Never invent a tool name, send a key in `code`, or bypass
the allowlist.

## Replies, cost, and timeout

Run-starting calls can spend credits and outlive the client connection. Use a
bounded question, never automatically retry an uncertain start, and reconcile
the returned thread or run before attempting another call. CAS watches
successful `ask_viktor`, `create_thread`, and `send_message` calls and queues
completed results with `origin=viktor`. If a Viktor-originated question arrives,
reply on its supplied thread rather than creating a replacement.

The daemon owns follow-up polling. If a requester is gone, CAS routes the
notification to the live session supervisor. A timeout is not proof that the
provider failed; resume the existing watch or run handle.

## Boundary

The proxy owns the credential reference and applies a fail-closed tool allowlist.
Never handle, paste, request, log, or add `VIKTOR_API_KEY` to a project,
artifact, prompt, or tool argument. If no credential is configured, follow the
one-time `cas viktor key` setup procedure in the gateway contract, then start a
new CAS session.
