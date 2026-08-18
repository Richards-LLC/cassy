# Decision: enforce supervisor-declared external-resource ownership at the tool boundary

**Status:** Recommendation ready
**Date:** 2026-08-14
**Decision owner:** Factory supervisor
**Scope:** Cassy factory workers using a shared authenticated external MCP identity

## Recommendation

Add **supervisor-declared external-resource leases enforced by a policy-aware MCP gateway**. A lease must be scoped to the factory session, authenticated account, upstream server, resource type, and canonical resource ID; a protected write may proceed only when the calling worker holds that live lease. Start with a Gmail-thread adapter that protects draft create/update/send and treats an unknown `draftId` on send as a deny, not an authorization bypass.

This is deliberately not a general claim that workers may coordinate by prose. The failure class is: **supervisor-declared exclusivity has no enforcement point at the tool layer.** A supervisor can assign a customer exclusively, but today that assignment never reaches the connector that creates or sends the draft, so it binds nothing.

## Decision context

The reported incident involved multiple Claude workers spawned with the same `config_dir`, therefore the same authenticated Gmail mailbox. Jess Webber's thread held three drafts, including two written by different workers three minutes apart. This is dangerous, not merely untidy:

- A bare `draftId` can be used by the connector's send operation, placing an unreviewed live-customer draft one call from dispatch. A queued draft was reportedly sent from this mailbox while an agent was still editing it on 2026-08-12.
- The connector returns a draft body only to the session that wrote it. One worker cannot inspect or safely clean up another worker's draft, so the supervisor had to relay draft bodies manually.
- Direct worker-to-worker collision notice is separately owned by `cas-5068`; even a lower-latency peer channel would be advisory and cannot prevent a protected write.

The decision is needed before another multi-worker sweep uses a shared customer-facing identity. It does not change provider authorization: it adds Cassy-local ownership policy in front of provider calls.

## Cassy evidence gathered first-hand

| Observation | Cassy source | What it proves |
| --- | --- | --- |
| `spawn_workers` writes the explicit `config_dir` into the resolved worker spec and captures the requesting supervisor's `CLAUDE_CONFIG_DIR` when it enqueues the spawn. | `cas-cli/src/mcp/tools/service/factory_ops.rs:1283-1330` | Multiple workers can be deliberately launched under the same authenticated Claude profile. |
| The spawn queue persists `requester_config_dir`; `WorkerSpec` defines explicit `config_dir` and requester-derived fallback. | `crates/cas-store/src/spawn_queue_store.rs:180-220`; `crates/cas-mux/src/spec.rs:75-104` | The selected account context survives the supervisor-to-daemon handoff. |
| The PTY maps the resolved profile to `CLAUDE_CONFIG_DIR` and the related secure-storage path. | `crates/cas-pty/src/pty.rs:303-340`, `1449-1503` | This is credential/identity propagation, not resource partitioning. Worker identity is logged, but not conveyed as resource ownership to an upstream tool. |
| Cassy has exclusive leases only for `task_id` and `worktree_id`. | `crates/cas-store/src/agent_store/mod.rs:91-165` | There is no `external_resource_key`, holder, expiry, history, or authorization query today. |
| Cassy's optional proxy dispatches all configured upstream calls through `mcp_execute` to `ProxyEngine::call_upstream`; the latter resolves server/tool and forwards arguments directly. | `cas-cli/src/mcp/tools/service/mod.rs:1138-1180`; `crates/cas-mcp-proxy/src/lib.rs:578-645` | The proxy is a viable enforcement seam, but currently contains no caller identity, ownership lookup, adapter, or deny policy. Directly exposed connector tools bypass it altogether. |

## Options

| Option | Cost / effort | Blast radius | Outcome and risk | Reversibility |
| --- | --- | --- | --- | --- |
| **Recommended: supervisor declares a resource lease; gateway enforces it** | Medium-to-high: durable lease store, supervisor claim/release/transfer UX, caller identity propagation, protected-tool policy, Gmail thread/draft adapter, audit and tests. | Limited initially to opted-in servers/tools and factory sessions; expands only as adapters are added. | Stops conflicting create/update/send at the actual write boundary. Gives the supervisor an auditable holder and a safe recovery path. Must fail closed for protected unknown resource keys and must prevent bypass through a native connector. | High: per-server/tool opt-in can be disabled; records remain useful audit history. |
| Worker-scoped resource leases (workers self-claim) | Medium: same durable primitive and adapter work, plus contention UX. | Any worker with a shared identity can claim arbitrary customer resources. | Provides atomic ownership, but does not express supervisor intent and creates races or gaming around discovery/claim order. The supervisor still has to arbitrate customer assignment. | High technically, but harder operational rollback because automated workers may acquire claims before scope is reviewed. |
| Per-worker authenticated identities | High and recurring: accounts, OAuth/app access, credential lifecycle, provider quotas, mailbox routing, human review and incident recovery. | Every integration and every customer-facing sender/permission model. | Strong credential separation, but it changes who sends mail and may be unavailable or unacceptable for a shared support mailbox. It does not supply a shared-thread work allocation model. | Low-to-medium: identities and provider grants are durable operational commitments. |

## Why this option

The deciding criterion is whether the mechanism can reject the unsafe operation at the moment it becomes side-effecting while preserving the existing shared mailbox. Only an enforcement point adjacent to the tool call meets that criterion without forcing a new identity and sender model.

The resource lease must be **supervisor-declared**, not inferred from task prose or a worker's first write. The declaration makes the intended customer boundary explicit before the worker acts; atomic Cassy storage makes the declaration exclusive; the gateway makes it effective. A task lease is not a substitute: one task can cover many customers, and more than one task can legitimately touch the same external resource.

For Gmail, the canonical protected key should be the authenticated-account namespace plus a normalized thread/conversation ID. The adapter must associate successful draft creation with that canonical key. On update or send, it must authorize the caller against the mapped thread; a bare `draftId` that cannot be mapped to a leased thread must be refused. That closes the precise dispatch path described in the incident instead of only protecting draft creation.

## Required design constraints

1. **Identity-aware enforcement.** The gateway needs the registered Cassy agent ID and factory session for each request; it must not trust a caller-supplied worker name.
2. **Canonical scope.** Lease uniqueness is `(factory_session, account_scope, upstream_server, resource_type, canonical_resource_id)`, with task ID only as attribution metadata. Never store OAuth secrets in the lease record.
3. **Atomic lifecycle.** Claim, renew, release, transfer, expiry, worker death, and supervisor override require the same transactional/audited semantics as task leases. A worker cannot release another worker's live lease.
4. **Tool-specific resource extraction.** Protected operations must have an adapter that derives the canonical resource key from authoritative arguments/result state. Generic argument-name guessing is not safe enough for writes.
5. **Deny rather than guess.** For configured protected writes, unreadable lease state, unknown resource identity, and unmapped `draftId` deny with actionable owner/expiry guidance. Read-only calls remain available unless a provider requires broader protection.
6. **No bypass route.** A protected upstream must be reachable to workers only through the enforcement gateway, or the connector itself must implement the equivalent check. Leaving a native Gmail MCP tool exposed would make Cassy policy advisory again.
7. **Auditability.** Record who declared the lease, holder, task attribution, operation/tool, allow/deny result, and expiry/transfer events. Do not retain message bodies or credentials.

## What we give up

Per-worker identities are the strongest isolation boundary when the provider and workflow support them. Choosing gateway-enforced resource ownership preserves a shared sender and does not protect a worker that has separate credentials to bypass the gateway. That is an intentional trade: Cassy should first make the existing shared mailbox safe without multiplying accounts; per-worker identities remain a later option for workloads that need tenant or sender separation.

## Reversal cost

The rollout is reversible because protection is opt-in per upstream server/tool. Disabling the Gmail policy restores current connector behavior without deleting leases; the audit ledger should be retained. The costly reverse path is operational, not code: once a workflow relies on the protection, removing it reintroduces the known double-draft/double-send hazard.

## Follow-up tasks implied

1. **Resource-lease core:** add a durable external-resource lease/history model, migration, expiry/recovery, and supervisor-only declaration/transfer/release APIs; model the account/server/resource scope above.
2. **Gateway caller context and policy:** propagate registered agent/session identity into proxy execution, add an opt-in protected-tool policy and auditable allow/deny decision before upstream dispatch.
3. **Gmail ownership adapter:** canonicalize thread IDs, bind resulting draft IDs to thread leases, and deny create/update/send—including bare `draftId` send—unless ownership is live; cover shared-profile conflicts and supervisor recovery.
4. **No-bypass integration:** inventory how Gmail is exposed to factory workers; route protected use through the gateway or add an equivalent connector-side guard, and fail factory startup/configuration when protected direct exposure remains.
5. **Operator workflow and regression suite:** document declare-before-work, visibility, expiry/transfer and incident recovery; test competing workers, stale/dead holders, unknown draft IDs, direct-route refusal, and supervisor audit visibility.
6. **Coordinate with `cas-5068`:** allow a scoped peer warning to reduce collision latency, but document it as a complement to—not a substitute for—tool-layer enforcement.

## Open questions

- Which Gmail MCP tool schemas expose thread IDs on draft create/update/send, and can a live draft-to-thread lookup be performed without revealing another worker's draft body?
- Can the factory consistently hide or route native connector definitions when an upstream is protected, across Claude, Codex, and Grok harnesses?
- What lease TTL and renewal UX best fit long-running customer review without leaving abandoned claims after a worker dies?

## Provenance

Analysis was performed against Cassy commit `850679a5de08ad8c7dc6315792dcc9a15b3954c3` on 2026-08-14 UTC. Commands used:

```text
rg -n -C 3 'config_dir|requester_config_dir|CLAUDE_CONFIG_DIR|spawn_workers' cas-cli crates --glob '*.rs' --glob '*.sql'
rg -n -i -C 3 'mcp.?proxy|upstream|tool.*dispatch|dispatch.*tool|proxy.*tool|tool call|tool_call' crates/cas-mcp-proxy cas-cli/src/mcp cas-cli/docs --glob '*.rs' --glob '*.md'
rg -n -i -C 2 'resource (ownership|lease|claim)|external resource|resource_id|resource_key|resource.*owner' . --glob '!target/**' --glob '!vendor/**'
```

Incident facts about the Gmail connector and prior dispatch are supplied by the supervisor/task record; the Cassy implementation facts above were inspected directly. The companion review surface is [`2026-08-14-shared-authenticated-identity-resource-ownership.html`](2026-08-14-shared-authenticated-identity-resource-ownership.html).
