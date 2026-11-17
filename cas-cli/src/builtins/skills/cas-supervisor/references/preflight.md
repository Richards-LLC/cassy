# Unified factory preflight

Before spawning workers, call `system action=preflight` through the connected
CAS MCP session. Without an active MCP connection, run
`cas --json factory preflight` (or `cas factory preflight` for the human view).
Both routes are read-only and finish within seven seconds; each finding carries
its own remediation.

Read `factory_blocked` and `findings[].code`/`severity`, not just `overall` or
`binary.state`. With `factory_blocked=true` (or a nonzero shell exit), do not
spawn; fix the critical finding and rerun. Only repository identity, missing
CAS MCP or missing required Cassy tools block. With `factory_blocked=false`, a
`cas_mcp.state=ready` connection and empty `missing_required_tools` establish
MCP readiness even when binary comparison is unavailable; an active preflight
call is itself MCP evidence, so a missing `.mcp.json` can show
`configured=false` with `observed_via_mcp=true`.

Binary findings and harness version drift are warnings:

- `binary.source_evidence_missing`, `binary.source_probe_timed_out` and
  `binary.deployment_unverified` mean the comparison is unknown, not that the
  runtime is outdated. Continue startup; no rebuild or reconnect.
- `binary.checkout_behind` and `binary.checkout_diverged` mean the runtime
  already contains the checkout or differs only in documentation. Continue;
  use a matching checkout for source work.
- `binary.deployment_stale`, `binary.deployment_diverged` and
  `binary.identity_untrusted`: resolve the intended deployment with the
  operator before replacement. A different SHA alone never proves the runtime
  older or authorizes a downgrade, and an explicit
  `CAS_EXPECTED_DEPLOYMENT_SHA` (7–40 hex characters) stays authoritative. A
  downstream project HEAD is never compared to the embedded Cassy SHA.

If replacement is needed, ask the operator to install the intended clean build
and use the harness's MCP reconnect control, which keeps every worker running;
then confirm the Cassy tools are back and rerun preflight. Never restart
`cas serve` from the active MCP session, and do not exit for a new session:
that stops every worker ([supervisor restart](worker-recovery.md#supervisor-restart)).

Repository registry fixes touch no repository files; bindings stay in the host
registry. Remove a registered non-project path, such as an artifact copy or
scratch root, with `cas known-repos forget <path>`. When two live clones share
a selector, inspect `cas known-repos status` and pick the canonical root with
`cas known-repos bind --repo <path>` (`cas known-repos unbind <exact-selector>`
first to replace a choice).
