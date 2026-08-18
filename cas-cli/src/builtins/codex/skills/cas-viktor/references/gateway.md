# Viktor gateway contract

## Provisioning

`cas init` and `cas update --sync` install the `cas-viktor` skill for Claude, Codex, and Grok. Run
`cas viktor` to see the credential-safe local status. It reports only whether
`VIKTOR_API_KEY` is present, never its value. On the managed host, load the canonical
credential file into the `cas serve` process; do not copy it to project configuration.

At `cas serve` startup, Cassy refreshes the user-scoped `viktor` upstream at
`https://api.viktor.com/mcp` with the credential reference `env:VIKTOR_API_KEY`. The managed
policy admits exactly these conversation tools:

`ask_viktor`, `create_thread`, `send_message`, `wait_for_run`, `get_run`,
`get_run_result`, `list_threads`, `list_messages`, and `whoami`.

If `.cas/proxy.toml` exists, its policy replaces the user policy. It must explicitly configure
the required Viktor server and routes; the managed user allowlist never widens a project.

## Conversation flow

Use `mcp_search` with `server:viktor` before relying on a tool, then call an advertised,
allowlisted route with `mcp_execute`. Pass task context and a bounded objective. A successful
`ask_viktor`, `create_thread`, or `send_message` is registered for daemon-owned follow-up.
CAS polls the provider at its own cadence and queues the completed result as an inbound
notification with `origin=viktor`. The requesting agent must not poll; it receives the reply
normally on a later turn. If the requester is gone, CAS routes the notification to the live
session supervisor.

## Cost and security

Viktor starts can cost money or outlive a client timeout. Do not automatically retry a start;
reconcile the returned thread/run first. Keep questions bounded and use the existing thread for
follow-ups. The CAS proxy records caller/task attribution and applies the exact allowlist.
Credentials remain environment references at the proxy boundary: never pass a key in tool
arguments, source control, artifacts, or agent context.
