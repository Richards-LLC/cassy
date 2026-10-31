# Unified factory preflight

Before spawning workers, call `system action=preflight` through the connected
CAS MCP session. Without an active MCP connection, run
`cas --json factory preflight` (or `cas factory preflight` for the human view).

Both routes are read-only and hard-bounded below seven seconds, including every
known-repository lookup and Git subprocess. Neither route connects optional
upstreams, starts a model, or spawns a worker. The human and JSON views come from
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

An active `system action=preflight` invocation is itself live CAS-MCP
evidence. A missing project `.mcp.json` can therefore report `configured=false`
with `observed_via_mcp=true` without being critical. Prefer this route over a
separate shell process when already connected.

Read `factory_blocked` and `findings[].code`/`severity`, not just `overall` or
`binary.state`. With `factory_blocked=true` (or a nonzero shell exit), do not
spawn; fix the critical finding and rerun. With `factory_blocked=false`, a
`cas_mcp.state=ready` connection and empty `missing_required_tools` establish
MCP readiness even when binary comparison is unavailable. Apply the binary
finding as follows:

- `binary.source_evidence_missing`, `binary.source_probe_timed_out`, or
  `binary.deployment_unverified`: comparison is unknown, not proof of an outdated
  runtime. Continue startup with healthy MCP while gathering source evidence:
  identify the Cassy checkout with `CAS_SOURCE_DIR`, retry complete, responsive
  local Git history, or obtain the intended `CAS_EXPECTED_DEPLOYMENT_SHA`.
  These findings alone do not require a rebuild or MCP reconnect.
- `binary.checkout_behind` or `binary.checkout_diverged`: continue startup.
  Git proved the runtime contains the checkout, or the runtime matches its
  release tag and the diverged checkout adds only documentation. Select a
  matching checkout for source work; do not rebuild or downgrade the runtime.
- `binary.deployment_stale`: the runtime differs from the explicit expected
  deployment, or Git proved it is behind the source checkout. Resolve the
  intended deployment with the operator before replacement. An explicit
  `CAS_EXPECTED_DEPLOYMENT_SHA` remains authoritative even when local ancestry
  or a release tag would otherwise allow the runtime.
- `binary.deployment_diverged`: ancestry does not establish compatibility.
  Resolve the intended deployment with the operator; a different SHA alone
  does not prove the runtime is older or authorize a downgrade.
- `binary.expected_sha_invalid`: correct the configured pin and rerun.
  `binary.identity_untrusted`: obtain a clean, identifiable deployment with the
  operator before deciding whether replacement is needed.

If replacement is needed, ask the operator to install the intended clean build
and use the harness's MCP reconnect/restart control, which keeps every worker
running; then confirm the Cassy tools are available and rerun preflight. Exiting
for a fresh session stops every worker; `factory action=restart_supervisor` does
not ([supervisor restart](worker-recovery.md#supervisor-restart)).
Never restart `cas serve` from the active MCP session.
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
