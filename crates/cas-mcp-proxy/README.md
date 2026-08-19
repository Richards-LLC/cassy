# cas-mcp-proxy

MCP proxy engine for CAS. Connects to upstream MCP servers and exposes their tools through a unified search and execute interface.

## Configuration

Upstream servers are configured in `.cas/proxy.toml` (project-scoped) and `~/.config/code-mode-mcp/config.toml` (user-scoped). Project config takes precedence.

### Managed Viktor default

On `cas serve` startup, Cassy refreshes a user-scoped `viktor` upstream using
streamable HTTP at `https://api.viktor.com/mcp` and the credential reference
`env:VIKTOR_API_KEY`. The API key is read only when the connection is made; it
is never written to the config file. On this managed host, source the canonical
credential file `~/.cas/viktor.env` into the serving process; never copy its
value into a project file, artifact, or command output. The managed direct-call allowlist is
exactly `ask_viktor`, `create_thread`, `send_message`, `wait_for_run`,
`get_run`, `get_run_result`, `list_threads`, `list_messages`, and `whoami`.
Every forwarded call is attributed by the proxy policy audit to the registered
CAS caller, active task leases, and dispatch timestamp; request arguments and
upstream output are not retained in that receipt.

An existing `.cas/proxy.toml` intentionally replaces the user allowlist and
delegation policy rather than widening it. Projects that have one must add the
same exact routes there to opt into direct Viktor conversations. This preserves
the fail-closed project boundary. The separate
`delegation.external_production_verification` configuration remains the
supervisor-only external-verification gateway; it is not installed by the
managed Viktor default.

### Supported transports

**Stdio** — spawns a child process:
```toml
[servers.my-server]
transport = "stdio"
command = "npx"
args = ["mcp-server-git"]
env = { HOME = "/tmp" }
```

**HTTP** — streamable HTTP connection:
```toml
[servers.sentry]
transport = "http"
url = "https://mcp.sentry.dev/mcp"
auth = "your-token"
```

**SSE** — server-sent events:
```toml
[servers.my-sse]
transport = "sse"
url = "https://example.com/sse"
```

### External dispatch policy

`cas serve` installs an exact `(server, tool)` allowlist at startup and on hot
reload. The default is fail-closed: configuring a server makes its catalog
searchable, but an empty or omitted `allowlist` forwards no external calls.

```toml
[[allowlist]]
server = "github"
tool = "list_issues"
```

When `.cas/proxy.toml` exists its allowlist and delegation sections replace the
user-scoped values rather than unioning them. This prevents a broader personal
configuration from silently widening a project's external dispatch policy.

### Supervisor external verification gateway

The first receipted delegation flow is `verification action=external_verify`.
It is available only to a registered factory supervisor, requires a task in an
epic plus a non-secret local proof reference, and writes a budget reservation
to the project's `cas.db` before starting the upstream run. A timed-out run is
resumed by its recorded run ID; the gateway never starts a replacement run.

Both gateway tools must also appear in the exact allowlist:

```toml
[[allowlist]]
server = "viktor"
tool = "ask_viktor"

[[allowlist]]
server = "viktor"
tool = "wait_for_run"

[delegation.external_production_verification]
server = "viktor"
start_tool = "ask_viktor"
wait_tool = "wait_for_run"
reserved_amount = 1
max_per_run = 1
max_active_per_factory_session = 4
max_active_per_epic = 2
timeout_seconds = 120
```

The run-starting and wait routes cannot be called through generic
`mcp_execute`; the proxy admits them only when CAS marks the request as the
registered-supervisor gateway path. The provider credential remains in proxy
configuration and is never copied into tool arguments or model context.

## Search

`ProxyEngine::search(query, max_length)` filters the tool catalog:

- **Keywords**: case-insensitive substring match on tool name and description
- **Server filter**: `server:github issue` filters to the `github` server first
- **Empty query**: returns all tools

An explicit `server:<name>` query for a configured upstream that is disconnected returns an
`upstream ... is absent` error instead of an empty catalog. Inspect `proxy_health` and repair the
credential or connection before retrying a run-starting call.

## Execute

`ProxyEngine::execute(caller, code, max_length)` dispatches tool calls. The
CAS MCP service derives `caller` from its registered agent row and active task
leases; it is never accepted from the dispatch payload. Before every upstream
request, the engine evaluates its `ProxyPolicy` hook and records a
request-free allow/deny audit entry. A denial is returned without forwarding
the request.

The proxy crate's standalone engine retains an allow-all compatibility default,
but `cas serve` always replaces it with the configured fail-closed policy.
Policies can
inspect the registered caller and arguments to enforce server/tool or
resource-specific rules without retaining arguments in the audit trail. Policy
deny reasons must be safe for the operator audit and MCP error; they must not
copy request arguments or upstream output.

The dispatch formats remain:

**JSON dispatch** (preferred):
```json
{ "server": "github", "tool": "list_issues", "args": { "repo": "myorg/app" } }
```

**Batch** (parallel execution):
```json
[
  { "server": "github", "tool": "list_issues", "args": { "repo": "app" } },
  { "server": "sentry", "tool": "list_errors", "args": { "project": "be" } }
]
```

**Dot-call syntax** (fallback):
```
github.list_issues({"repo": "myorg/app"})
```

## Hot-reload

The daemon watches `.cas/proxy.toml` for changes. On config change,
`ProxyEngine::reload()` compares stored configs against new ones, disconnects
removed servers, reconnects changed ones, and leaves unchanged servers
connected. It then replaces the route/delegation policy from the same parsed
snapshot, so removing an allowlist entry takes effect without a restart.

## Feature flag

Enable with `cargo build --features mcp-proxy`. Without the feature, proxy commands return a helpful error message.
