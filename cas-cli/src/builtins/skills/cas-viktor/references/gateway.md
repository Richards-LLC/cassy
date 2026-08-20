# Viktor gateway contract

## Provisioning

`cas init` and `cas update --sync` install the `cas-viktor` skill for Claude, Codex, and Grok. Run
`cas viktor` to see the credential-safe local status. When it reports no credential, get an
operator-issued key and enter it once with `cas viktor key`, then paste it when prompted. The command
performs one non-spending MCP handshake and tool-list validation before saving the key only in
the machine-scoped Cassy configuration. An invalid or expired key is not saved and the command
does not retry it. Start a new CAS session after a successful entry. `cas viktor` reports only
whether a credential is present, never its value; do not copy a key to project configuration.

At `cas serve` startup without `.cas/proxy.toml`, Cassy refreshes the user-scoped `viktor`
upstream at `https://api.viktor.com/mcp` with the credential reference `env:VIKTOR_API_KEY`.
The managed policy admits exactly these conversation tools:

`ask_viktor`, `create_thread`, `send_message`, `wait_for_run`, `get_run`,
`get_run_result`, `list_threads`, `list_messages`, and `whoami`.

If `.cas/proxy.toml` exists, it opts out of the managed default. It must explicitly configure
the required Viktor server and routes; the managed user configuration never widens a project.

If `server:viktor` discovery reports that the upstream is absent, do not treat that as an empty
tool catalog or retry a run-starting call. Run `cas viktor` for the credential-safe connection
state and durable pending run IDs. If no credential is configured, use the one-time
`cas viktor key` setup command; otherwise investigate the reported upstream
state. On daemon restart, Cassy alerts a live session supervisor when such watches cannot be
polled; let the existing watch resume rather than starting a replacement run.

## Conversation flow

Use `mcp_search` with `server:viktor` before relying on a tool, then call an advertised,
allowlisted route with `mcp_execute`. Pass task context and a bounded objective. A successful
`ask_viktor`, `create_thread`, or `send_message` is registered for daemon-owned follow-up.
CAS polls the provider at its own cadence and queues the completed result as an inbound
notification with `origin=viktor`. The requesting agent must not poll; it receives the reply
normally on a later turn. If the requester is gone, CAS routes the notification to the live
session supervisor.

Viktor-originated threads require no prior CAS watch. The daemon checks Viktor's newest threads
on the same 30-second cadence, skips every thread with any local watch row, and spends at most one
32-thread `list_threads` scan plus four `list_messages` calls and four seconds per tick. Each provider message ID
is durable and idempotent. It is routed with `origin=viktor` to one live, factory-session-filtered
supervisor; when none is live, the question is retained and surfaced exactly once at the next
supervisor SessionStart. Reply with `send_message` on the supplied thread ID.

## Cost and security

Viktor starts can cost money or outlive a client timeout. Do not automatically retry a start;
reconcile the returned thread/run first. Keep questions bounded and use the existing thread for
follow-ups. The CAS proxy records caller/task attribution and applies the exact allowlist.
Credentials remain environment references at the proxy boundary: never pass a key in tool
arguments, source control, artifacts, or agent context.
