# Unified factory preflight

Run this before spawning workers:

```bash
cas factory preflight
cas --json factory preflight
```

The command is read-only and hard-bounded below seven seconds, including every
known-repository lookup and Git subprocess. It does not connect optional
upstreams, start a model, or spawn a worker. The human and JSON views come from
the same schema-versioned report covering:

- the running Cassy build SHA versus identifiable Cassy source or configured
  deployment evidence;
- the portable repository selector and active target branch;
- CAS MCP registration and compiled `coordination`/`task` availability;
- credential-free optional-upstream health and backoff state;
- typed Claude, Codex, and Grok conformance receipts, live default-version
  observations, and receipt-time observations kept as separate evidence.

The `required` harness set is the effective supervisor and worker harnesses from
`.cas/config.toml` (including stock defaults). Catalogued harnesses that are not
used by either role remain visible but informational. Required harnesses without
a passing receipt or observable matching live default produce a warning. This
policy never invents a receipt: readiness is based only on real conformance
evidence for the factory configuration that will actually launch.

Exit status is nonzero only for critical factory blockers: unresolved,
ambiguous, or wrong repository identity; uninitialized/missing CAS MCP; or a
compiled registry missing required Cassy tools. Dirty, unknown, or stale binary
identity is a warning. Optional upstream failures and harness version drift are
warnings and never block factory launch.

For automation from an active CAS MCP session, call `system action=preflight`.
That invocation is itself live CAS-MCP evidence, so a missing project
`.mcp.json` is reported as `configured=false`, `observed_via_mcp=true`, and is
not critical by itself.

The report never emits absolute paths, proxy endpoints, headers, credentials,
raw upstream content/errors, environment values, or MCP session IDs. Each
non-ready state includes a stable finding code, evidence time when available,
and remediation. Deadline failures expose only typed component identifiers such
as `repository` or `harness.codex`; probed paths and commands are never emitted.
If `repository.candidate_limit` is reported, preview safe registry-only cleanup
with `cas known-repos prune-missing --dry-run`, then apply it without the flag.
This removes only rows for paths that no longer exist; it never deletes repo files.
A row whose path still exists but is not a project — a factory artifact copy or a
scratch root that got registered — is removed with `cas known-repos forget <path>`,
which also drops any selector binding aimed at it and never deletes repo files.
If two live clones share one selector, inspect host-local state with
`cas known-repos status`, then explicitly select the intended canonical root
with `cas known-repos bind --repo <path>`. Remove a stale choice with
`cas known-repos unbind <exact-selector>` before rebinding. Bindings stay in the
host registry only; task, delivery, MCP, and preflight JSON remain path-free.
`CAS_SOURCE_DIR` can identify a Cassy source checkout when the
project being checked is downstream; `CAS_EXPECTED_DEPLOYMENT_SHA` can provide
an explicit expected 7–40 character hexadecimal deployment commit. A
downstream project HEAD is never compared to the embedded Cassy SHA.
