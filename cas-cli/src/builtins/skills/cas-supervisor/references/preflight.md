# Unified factory preflight

Before spawning workers, call `system action=preflight` through the connected
CAS MCP session. Without an active MCP connection, run
`cas --json factory preflight` (or `cas factory preflight` for the human view).

Both routes are read-only, finish within seven seconds, and connect no optional
upstream, model or worker. One schema-versioned report covers:

- the running Cassy build SHA versus identifiable Cassy source or configured
  deployment evidence;
- the portable repository selector and active target branch;
- CAS MCP registration and compiled `coordination`/`task` availability;
- credential-free optional-upstream health and backoff state;
- typed Claude, Codex and Grok conformance receipts, live default-version
  observations and receipt-time observations, kept separate.

The `required` harnesses are the effective supervisor and worker harnesses from
`.cas/config.toml`; a required harness without a passing receipt or matching
live default warns. Others are informational.

Exit status is nonzero only for critical blockers: wrong, ambiguous or
unresolved repository identity; missing CAS MCP; or a compiled registry missing
required Cassy tools. Binary identity, optional upstream failures and harness
version drift are warnings that never block launch. An active
`system action=preflight` call is itself live MCP evidence, so a missing
`.mcp.json` can show `configured=false` with `observed_via_mcp=true`.

Read `factory_blocked` and `findings[].code`/`severity`, not just `overall` or
`binary.state`. With `factory_blocked=true` (or a nonzero shell exit), do not
spawn; fix the critical finding and rerun. With `factory_blocked=false`, a
`cas_mcp.state=ready` connection and empty `missing_required_tools` establish
MCP readiness even when binary comparison is unavailable. For binary findings:

- `binary.source_evidence_missing`, `binary.source_probe_timed_out`, or
  `binary.deployment_unverified`: comparison is unknown, not proof of an
  outdated runtime. Continue startup while gathering evidence: set
  `CAS_SOURCE_DIR` to the Cassy checkout, retry with responsive local Git, or
  obtain `CAS_EXPECTED_DEPLOYMENT_SHA`. These alone need no rebuild or reconnect.
- `binary.checkout_behind` or `binary.checkout_diverged`: continue. Git proved
  the runtime contains the checkout, or the diverged checkout adds only
  documentation. Use a matching checkout for source work; do not rebuild or
  downgrade the runtime.
- `binary.deployment_stale`: the runtime differs from the explicit expected
  deployment, or Git proved it behind the checkout. Resolve the intended
  deployment with the operator before replacement; an explicit
  `CAS_EXPECTED_DEPLOYMENT_SHA` stays authoritative.
- `binary.deployment_diverged`: ancestry does not establish compatibility.
  Resolve the intended deployment with the operator; a different SHA does not
  prove the runtime older or authorize a downgrade.
- `binary.expected_sha_invalid`: correct the pin and rerun.
  `binary.identity_untrusted`: get a clean, identifiable deployment with the
  operator before deciding on replacement.

If replacement is needed, ask the operator to install the intended clean build
and use the harness's MCP reconnect/restart control, which keeps every worker
running; then confirm the Cassy tools are back and rerun preflight. Exiting
for a new session stops every worker; `factory action=restart_supervisor` does
not ([supervisor restart](worker-recovery.md#supervisor-restart)).
Never restart `cas serve` from the active MCP session.

Repository registry findings: for `repository.candidate_limit`, preview with
`cas known-repos prune-missing --dry-run`, then apply it without the flag; it
removes only rows for missing paths. Remove a registered non-project path, such
as an artifact copy or scratch root, with `cas known-repos forget <path>`. When
two live clones share a selector, inspect `cas known-repos status` and pick the
canonical root with `cas known-repos bind --repo <path>`
(`cas known-repos unbind <exact-selector>` first to replace a choice). None of
these touch repository files, and bindings stay in the host registry. A
downstream project HEAD is never compared to the embedded Cassy SHA;
`CAS_EXPECTED_DEPLOYMENT_SHA` takes a 7–40 character hexadecimal commit.
