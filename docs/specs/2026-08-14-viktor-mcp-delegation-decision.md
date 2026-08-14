# Decision: make Viktor a supervisor-owned, fail-closed evidence gate—not a worker capability

**Status:** Recommendation ready  
**Date:** 2026-08-14  
**Decision owner:** Factory supervisor  
**Scope:** GH #316 / optional Viktor delegation; no credentials or implementation

## Recommendation

Ship a **supervisor-only Viktor delegation gateway** as the first CAS integration. The first end-to-end gate is an external, read-only verification gate: a supervisor asks Viktor to verify one named public production behavior and receives a structured `pass`, `fail`, or `inconclusive` result. CAS accepts only `pass` with the required evidence; `inconclusive` is a durable non-passing verdict, never a soft success.

Do not expose a Viktor bearer key or `mcp__viktor__*` tools to workers in phase one. Scope-derived tool availability is an upstream permission boundary, but it is not a sufficient CAS worker boundary: a bearer key is transferable, Viktor charges for run-starting calls, and CAS currently has neither caller identity nor budget enforcement at an external direct-MCP call. The failure class is: **an unmetered delegated authority can create paid, external work without a CAS-owned authorization and receipt boundary.**

This preserves Viktor's human-confirmed write model and leaves all repository, CAS-store, release, task-state, credential, and provider write operations with CAS/humans.

## Evidence gathered first-hand

| Observation | Source | Consequence |
| --- | --- | --- |
| CAS's optional proxy resolves an upstream server/tool then forwards arguments to the upstream call; it carries no registered caller identity or policy decision. | `cas-cli/src/mcp/tools/service/mod.rs:1138-1180`; `crates/cas-mcp-proxy/src/lib.rs:578-645` | The proxy is the future enforcement seam, but direct exposure is a bypass until caller context and policy exist. |
| Worker auto-permission generation is an explicit CAS-only allowlist. | `cas-cli/src/cli/hook/config_gen.rs:620-633` | A Viktor worker permission must be an intentional new capability, not an accidental prefix match. |
| Codemap gating builds exact names from the active harness's own prefix. | `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs:1661-1675` | `mcp__viktor__ask_viktor` cannot be mistaken for CAS task/coordination today. |
| CAS remaps only its three known harness prefixes, and PTY parity normalizes only those same three. | `crates/cas-core/src/hooks/context/mod.rs:224-245`; `crates/cas-pty/src/pty.rs:160-180` | Viktor's namespace does not collide with runtime CAS dispatch, but any future prompt/parity treatment of external tools needs an explicit server-qualified rule. |
| Viktor's MCP is stateless streamable HTTP; `ask_viktor` starts and waits, timeout returns `wait_timed_out` plus `run_id`, and run-starting calls accept idempotency keys. | Viktor MCP documentation, https://viktor.com/docs/mcp-server/md (read 2026-08-14) | CAS must persist a run receipt before calling and resume by `run_id`, not ask again. |
| Viktor exposes the stated scoped tools; `ask_viktor` needs `threads:create`, `runs:create`, `runs:read`; files need `files:read`; runs consume credits and rate limits are per key owner. | Viktor MCP/public API documentation, https://viktor.com/docs/mcp-server/md and https://viktor.com/docs/public-api (read 2026-08-14) | Adding keys does not solve concurrency/spend; CAS needs a local budget reservation. |

## Roles and scopes

| Actor | Phase-one access | Scope policy | Why |
| --- | --- | --- | --- |
| Factory supervisor | Gateway-only delegate loop | `threads:create`, `threads:read`, `messages:create`, `messages:read`, `runs:create`, `runs:read`, `files:read` | It can create/resume/cancel delegated research, retrieve evidence, and make the CAS gate decision. The key remains gateway configuration, not model-visible context. |
| Claude worker | None | No key, no direct server, no gateway action | Workers submit an evidence request to their supervisor through existing CAS coordination; they cannot initiate paid work. |
| Codex/Grok worker | None | Same | Their lack of an MCP client is not a reason to create a less-governed REST/chat bypass. |
| Viktor gateway/service identity | Only the supervisor's configured scopes | No `usage:read` or `audit:read` initially | Fixed CAS reservations bound spend without broad workspace accounting/audit visibility. Add those scopes only when an operator dashboard has a concrete consumer. |

The upstream scope-derived tool list is necessary defense in depth but insufficient authorization for workers. It limits what a stolen/misconfigured key can call; it does not prove which CAS role requested a call, enforce task eligibility, prevent a worker from spending the allowed credit, or stop a direct connector bypass.

## First gate and contract

The first gate is **`external_production_verification`**, available only to a supervisor after a task has local proof. Its request declares a single target URL/environment, expected observable behavior, and bounded check list. It is read-only: no login requiring write capability, no submit/mutate/approve action, no repository action.

CAS sends `ask_viktor` with a JSON-schema `response_format` equivalent to:

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": ["verdict", "checks", "limitations"],
  "properties": {
    "verdict": {"enum": ["pass", "fail", "inconclusive"]},
    "checks": {"type": "array", "minItems": 1, "items": {
      "type": "object", "additionalProperties": false,
      "required": ["name", "expected", "observed", "evidence"],
      "properties": {
        "name": {"type": "string"}, "expected": {"type": "string"},
        "observed": {"type": "string"}, "evidence": {"type": "string"}
      }
    }},
    "limitations": {"type": "array", "items": {"type": "string"}}
  }
}
```

CAS validates the schema *and* its own policy: a gate passes only when `verdict == "pass"`, every requested check is represented, evidence is nonempty, and no limitation contradicts a required check. `fail`, `inconclusive`, `wait_timed_out`, `requires_action`, `thread_busy`, insufficient scope, rate limit, cancellation, malformed output, and transport failure all record a non-passing receipt. `requires_action` is answerable by a supervisor in a later explicit action; it is not permission to continue automatically.

## Idempotency and budget

Before the first upstream request, CAS writes a delegation receipt with a generated opaque ID and a stable idempotency key:

`v1:<factory-session-id>:<task-id>:<gate-kind>:<request-digest>:<attempt>`.

The backend—not a model prompt or worker—generates it, persists the request digest and reservation atomically, and reuses it on all retries. If the call returns `wait_timed_out`, CAS stores `run_id` and uses `wait_for_run(run_id)`; it never creates another run. A changed request deliberately creates the next attempt only after supervisor approval and a new reservation.

Initial budget policy:

1. Supervisor must opt in on a task and declare why local tests/CI cannot answer it.
2. One active run per task and one first-attempt run per task gate; continuation requires an explicit supervisor action.
3. Reserve a configured per-run credit ceiling before dispatch; reserve configured per-epic and per-session totals. Exhaustion denies new calls, never queues debt.
4. Limit each call's timeout and check count; honor Viktor `Retry-After`; no automatic retry that can start a run.
5. Persist request digest, key, run ID, reserve/settle amount when available, terminal verdict, and non-secret evidence reference for audit.

This uses fixed, configured caps rather than the broad `usage:read` scope in phase one. It gives up exact workspace credit reconciliation until a later audited budget dashboard exists.

## Prefix safety result

**No current CAS dispatcher treats `mcp__viktor__` as a CAS tool.** The reviewed prefix-sensitive runtime gate compares exact generated names (`<own-prefix>task` and `<own-prefix>coordination`) at `pre_tool.rs:1669-1675`; the known-prefix remappers at `context/mod.rs:240-245` and `pty.rs:162-165` list only CAS's harness aliases. Thus adding the namespace does not itself break current dispatch.

That is not a rollout approval for direct MCP. A tool named `mcp__viktor__ask_viktor` would bypass CAS's exact CAS-tool gate and current proxy policy alike. Follow-up implementation must register an external-server allowlist by parsed `(server, tool)` components—not a raw `starts_with("mcp__")` check—and add adversarial tests for `mcp__viktor__`, a lookalike server name, and foreign tools in prompt/parity text.

## Alternatives

| Option | Blast radius | Reversibility | Result |
| --- | --- | --- | --- |
| **Recommended: supervisor-only gateway + one read-only evidence gate** | One opt-in CAS path and one provider key | High: disable one policy/configuration; receipts remain audit evidence | Enforces identity, idempotency, budget, and fail-closed verdicts before expansion. |
| Give workers a restricted `ask_viktor` key | Every worker prompt and retry path | Medium: revoking key stops calls but cannot undo spend/runs | Scope limits features but cannot enforce CAS task/budget policy; rejected. |
| Direct MCP for Claude, REST/chat for Codex/Grok | Three divergent clients and bypass routes | Low: secret/config distribution and behavior diverge | Maximizes capability but makes policy advisory; rejected. |

The deciding criterion is **a single CAS-owned enforcement point before a paid external run begins**. Only the recommended option has one. It gives up worker autonomy and immediate Codex/Grok parity; that is intentional until the same gateway provides it safely.

## Never delegate

CAS must never delegate: repository writes, merge/push/tag/release, CAS task/lease/verification state transitions, secret/key minting/storage/rotation, credentialed provider writes or approvals, production mutation, payroll/legal/financial decisions, deletion, incident command authority, or a final gate decision. Viktor may research, read, draft, and report evidence; CAS/humans decide and execute writes.

## Follow-up tasks

1. Gateway core: secret-provider integration outside agent context, registered supervisor/session identity, delegation receipt/idempotency store, budget reservations, and rate-limit handling.
2. First gate: supervisor-only task API, schema validation, `inconclusive`/`requires_action` terminal handling, durable evidence receipt, and tests for retry/run-ID resume.
3. External-tool routing: parsed server/tool allowlist, no direct-worker bypass, prefix/lookalike regression coverage across Claude/Codex/Grok prompt and hook surfaces.
4. Later expansion decision: evaluate a read-only worker request/queue (not a worker key) and only then expose the gateway uniformly to Codex/Grok; separately justify `usage:read` accounting.

## Provenance

CAS source and Viktor documentation were inspected on 2026-08-14. The issue request is GH #316: https://github.com/pippenz/cas/issues/316. This document deliberately makes no network call, credential request, key creation, or implementation change.
