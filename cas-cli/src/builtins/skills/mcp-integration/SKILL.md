---
name: mcp-integration
description: Use when installing, registering, verifying, debugging, or exposing an MCP server — including scope choice, credential handling, zero-scope keys, worktree visibility, and paid/third-party servers. Not for local dev servers (see cas-servers).
metadata:
  managed_by: cas
---

# Manage MCP servers through Cassy

Use Cassy's project-scoped MCP proxy as the source of truth. The procedure is:

1. **Inspect the resolved configuration.** Run `cas mcp list --json` from the
   project. It connects to configured servers, reports the tool count, and
   redacts credentials by default. Use `--show-secrets` only for a deliberate
   local diagnostic.
2. **Add or update a server.** `cas mcp add` accepts the familiar
   `--scope`, `--transport`, `--env`, and `--header` flags and writes the
   selected scope. For example:

   ```bash
   cas mcp add --transport http docs https://docs.example.test/mcp
   cas mcp add --scope user -e API_TOKEN='${API_TOKEN}' search -- npx search-mcp
   ```

   For `cas mcp add`, `local` (the default) and `project` write the same file,
   the project's `.cas/proxy.toml`, which every Cassy worktree of the project
   already shares. Use `--scope user` only for a server every project on the
   machine should see; it writes the machine-wide proxy configuration.
3. **Allowlist the routes you will call.** Neither `cas mcp add` nor
   `proxy_add` writes the proxy `allowlist`, and an empty allowlist is
   fail-closed: the server connects and advertises tools, but every call is
   denied by policy. Add each `"<server>.<tool>"` route (a glob such as
   `"<server>.*"` also works) to the `allowlist` of the configuration that
   wins. When the project has a `.cas/proxy.toml`, its allowlist **replaces**
   the machine allowlist rather than merging with it, so creating a project
   file silently drops the machine routes: list every route the project needs
   there.
4. **Import an existing registration when appropriate.** `cas mcp import`
   reads Claude and/or Codex configuration without requiring a hand-copy:

   ```bash
   cas mcp import --from claude --dry-run
   cas mcp import --from claude --force
   ```

   Review the dry-run before using `--force`; never copy a plaintext secret
   into source control, a task, a prompt, or a worktree.
5. **Use the MCP system actions for proxy administration.** These mutate
   `.cas/proxy.toml` or inspect the daemon's health cache. The four actions
   have explicit, auditable calls:

   ```text
   system action=proxy_add name=docs transport=http url=https://docs.example.test/mcp
   system action=proxy_remove name=docs
   system action=proxy_list
   system action=proxy_health
   ```

   Restart `cas serve` after adding or removing a server. `proxy_list` shows
   the configured server count; `proxy_health` is credential-free and reports
   upstream connection/backoff state.
6. **Verify capability, not just connectivity.** Discover the proxy surface
   with `mcp_search` using `server:<name>`, count the advertised
   tools, compare them with the expected set, then make one cheap read-only
   call through `mcp_execute`. A `denied by policy` answer means
   step 3's allowlist is missing the route. A green connection with zero or one
   tool is a narrow-scope configuration, not a successful integration.

## Credential and scope rules

- Keep secrets in environment references or the Cassy proxy credential
  boundary. Never commit inline credentials to `.mcp.json` or `.cas/proxy.toml`.
- Remove stale registrations per scope before retrying. A higher-precedence
  local entry can shadow a corrected project or user entry.
- Verify from the same user, working directory, environment, and launch method
  as the agent that will call the server. Client tool inventories often load
  only at startup, so restart before diagnosing a stale list.
- Classify tools before use: retry read-only calls; retry keyed idempotent calls
  only with the same key; reconcile any timed-out side-effecting call before
  attempting another one.

## When MCP is unavailable

Detect the actual harness and degrade explicitly. If it has no MCP client, say
“no MCP client, routing via supervisor”; never silently skip verification. For a
single fixed HTTP request, a script is usually cheaper and easier to audit than
an MCP server. Use MCP when the agent needs to choose among several tools.

For symptom-to-cause tables and protocol details, read
[references/diagnosis.md](references/diagnosis.md). This skill owns the Cassy
installation procedure; the reference owns diagnostic evidence.
