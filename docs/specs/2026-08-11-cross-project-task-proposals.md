# Cross-project task proposals

**Recommendation:** add a dedicated cloud proposal API and proposal store; do not route cross-project creation through generic task sync. The CLI must require an explicit `project=<canonical_id>`, gate the call to a registered supervisor/director, and let the cloud validate the shared-team grant, stamp authenticated provenance, hold the item in `proposed`, and materialize a normal `open` task only after a receiving supervisor accepts it.

Status: design decided; implementation split at the cloud contract boundary

Date: 2026-08-11

Audience: CAS CLI and Petra Stella Cloud maintainers

Source task: `cas-a0ba` / GitHub issue `pippenz/cas#171`

Cloud contract: [Richards-LLC/petra-stella-cloud#44](https://github.com/Richards-LLC/petra-stella-cloud/issues/44)

## Decision context

Supervisors sometimes discover work owned by another CAS project. The current handoff is a markdown request that loses task priority, dependencies, lifecycle state, deduplication, and machine-readable completion. The desired experience is a tracked proposal that the target project can accept or reject, with an optional `blocked_by` edge that resolves when the accepted target task closes.

This is not a generic cross-project mutation capability. Only creation is in scope. Cross-project close, update, transfer, gate bypass, implicit target selection, and non-supervisor creation remain out of scope.

The decision was required before implementation because the current generic sync path cannot preserve all four invariants together: explicit targeting, granted authority, trustworthy provenance, and observable cross-project completion.

## Options

| Option | Cost | Risk | Effort | Reversibility | Outcome |
| --- | --- | --- | --- | --- | --- |
| Reuse generic team task push with a different envelope project | Low initially | **Unacceptable:** provenance is opaque client JSON; the origin cannot observe a foreign project's close; dependencies are not synced | Low | Poor after data ships | Creates a target row, but does not safely deliver the feature |
| Store foreign task replicas in the origin project's local database | Medium | **Unacceptable:** defeats fail-closed pull scoping and reopens cross-project contamination | Medium | Expensive | Makes dependency joins easy by weakening a shipped safety invariant |
| **Dedicated cloud proposal API + external dependency signals (selected)** | Medium | Controlled: new endpoints and two purpose-built tables | Medium/high, split across cloud and CLI | Good; isolated from normal task sync | Preserves target review, authoritative provenance, and automatic close observation |
| Keep markdown requests | None | Continued lossy handoff and human polling | None | Immediate | No product improvement |

## Why this option

The deciding criterion is whether the origin can learn that a target task closed without weakening project-scoped pull. Only a cloud-owned dependency signal can do that. A dedicated API also lets the cloud stamp authenticated user/team/time fields instead of trusting opaque task JSON and keeps proposals out of the receiving project's executable backlog until acceptance.

## What we give up

Generic team push already transports task-shaped JSON and would be faster to modify. Choosing a dedicated API adds a server migration and delays the CLI implementation until that contract is live. That is the strongest argument for the runner-up. It is still the wrong trade because a fast client-only path would silently omit the feature's main payoff—automatic cross-project unblocking—and would make provenance forgeable.

## Reversal cost

The new API is additive. Reversal means disabling proposal creation in the CLI and retaining or archiving the two cloud tables. Normal task sync rows and lifecycle behavior remain unchanged. Accepted tasks are ordinary tasks and need no rollback migration.

## Decided authorization predicate

Authorization has two gates; both must pass.

1. **CAS runtime gate:** only a registered `supervisor` or `director` session may call `task create project=...`. A worker or an unregistered/unknown role is refused before network I/O. The refusal must say: `Cross-project task creation requires a registered supervisor or director session.`
2. **Cloud grant gate:** the bearer-token user must be a member of the selected team, and both the origin and target canonical projects must already resolve inside that same team. The target is never auto-registered by this endpoint. A missing grant is refused with: `Cross-project task creation requires membership in a team shared by the origin and target projects.`

Team membership is the v1 grant. A future per-project allowlist may narrow it, but there is no ambient account-wide or cwd-derived grant. Cloud validation is authoritative for user/project access; the CAS role gate is authoritative for which local agent may exercise the user's grant.

The origin project is also explicit in the request body and verified against a registered project in the same team. The cloud does not derive either project from a push envelope, local path, current working directory, or session default.

## Decided proposed-state flow

1. Origin supervisor calls `task create project=<target-canonical-id> ...` with an idempotency key. If `project` is absent, today's local create path is unchanged.
2. CLI verifies the supervisor/director role, resolves its active team grant, and sends the proposal request. It does not insert a foreign task into the local `tasks` table.
3. Cloud validates membership plus the existing origin/target project registrations, reserves a target task ID, stamps provenance, stores `state=proposed`, and returns `proposal_id` plus `target_task_id`.
4. If the request named `blocks_origin_task_id`, cloud validates that task under the explicit origin project and creates a pending external `blocks` edge in the same transaction.
5. A supervisor in the target project lists its proposal inbox separately from normal tasks. Pending proposals are never `ready`, claimable, assignable, or visible as ordinary `open` tasks.
6. **Accept:** cloud atomically changes the proposal to `accepted` and materializes one normal `open` task in the target project's `sync_entities`. The accepting authenticated user owns the sync row; proposal provenance is copied into task JSON for visibility and remains authoritative in the proposal row.
7. **Reject:** cloud records `rejected`, `decided_by_user_id`, `decided_at`, and an optional reason. No task is materialized. A rejected external blocker remains a visible failed handoff and does not silently unblock the origin task; the origin supervisor must remove or replace it.
8. When an accepted target task becomes `closed`, the cloud dependency projection reports the edge as resolved. The origin CLI reconciles that signal and returns the origin task to `open` only when every local and external `blocks` edge is resolved.

Proposal states are `proposed`, `accepted`, and `rejected`. They are not added to `TaskStatus`; a proposal is not a task until accepted.

## Decided provenance schema

The cloud stores two provenance classes so the UI never presents an asserted field as server-attested.

### Server-attested

| Field | Meaning |
| --- | --- |
| `proposal_id` | Cloud-generated immutable proposal identity |
| `target_task_id` | Cloud-reserved CAS task ID, returned at create time |
| `creator_user_id` | Identity from the validated bearer token |
| `team_id` | Team whose membership grant authorized the request |
| `origin_project_canonical_id` | Existing origin project resolved by the cloud |
| `target_project_canonical_id` | Existing target project resolved by the cloud |
| `received_at` | Cloud receipt time |
| `client_request_id` | Caller-provided idempotency key, unique per creator |
| `decided_by_user_id`, `decided_at` | Authenticated receiving-side decision provenance |

### Client-asserted, visibly labeled

| Field | Meaning |
| --- | --- |
| `origin_session_id` | Registered local CAS session making the request |
| `origin_agent_id` | Registered local agent identity |
| `origin_agent_name` | Human-readable local agent name, if present |
| `origin_agent_role` | `supervisor` or `director`, checked by the CLI |
| `client_version`, `client_build` | CAS binary identity |

The cloud stamps server-attested fields from authentication, project lookup, and server time. It never accepts client overrides for them. Session and agent identity remain client assertions because the cloud does not own the local factory registry; the UI must label them accordingly.

## Wire contract

### Create proposal

`POST /api/teams/{team_id}/task-proposals`

```json
{
  "client_request_id": "018f...",
  "origin_project_canonical_id": "petra-stella-cloud",
  "target_project_canonical_id": "cas-src",
  "origin_session_id": "session-uuid",
  "origin_agent_id": "agent-uuid",
  "origin_agent_name": "supervisor",
  "origin_agent_role": "supervisor",
  "client_version": "2.x",
  "client_build": "git-sha",
  "task": {
    "title": "...",
    "description": "...",
    "priority": 2,
    "task_type": "task",
    "labels": [],
    "design": "...",
    "acceptance_criteria": "...",
    "external_ref": "..."
  },
  "blocks_origin_task_id": "cas-abcd"
}
```

Success is `201` with `{proposal_id, target_task_id, state:"proposed", provenance}`. Repeating the same `client_request_id` returns the same result. The endpoint returns `403` for a missing shared-team grant, `404` for an unregistered explicit origin or target project, and `409` when an optional origin blocker task is not yet present in cloud state.

### Triage

- `GET /api/teams/{team_id}/task-proposals?target_project_id=<canonical>&state=proposed`
- `POST /api/teams/{team_id}/task-proposals/{proposal_id}/accept`
- `POST /api/teams/{team_id}/task-proposals/{proposal_id}/reject` with optional `{reason}`

Accept/reject must compare the proposal's target project with the explicit target-project query/body value supplied by the client and validate membership again. Both are idempotent. An opposite second decision returns `409` with the original decision.

### Dependency reconciliation

`GET /api/teams/{team_id}/cross-project-task-dependencies?origin_project_id=<canonical>&since=<cursor>`

Each row returns the origin task, proposal, target task, proposal state, target task status, and resolution state. A target `closed` status resolves the edge. Rejection returns `handoff_rejected`, not `resolved`.

## Storage contract

Cloud adds:

- `task_proposals`: immutable task payload plus state, explicit resolved origin/target project IDs, server/client provenance, decision audit, and unique `(creator_user_id, client_request_id)`.
- `cross_project_task_dependencies`: origin team/project/task, proposal, target project/task, dependency type (`blocks` only in v1), created/resolved timestamps, and a uniqueness constraint preventing duplicate edges.

The CLI successor adds a local external-dependency projection rather than a foreign task replica. Ready/blocked queries consider unresolved external `blocks` rows alongside local dependencies. This keeps the existing fail-closed task pull boundary intact.

## Scope truth for `task list scope=project`

The local `tasks` table has no row-level `project_id`; its project boundary is the `.cas/cas.db` that owns it. Because pending proposals remain in a separate cloud inbox and foreign tasks are never inserted locally, `scope=project` can truthfully mean **the current CAS database, identified in output by its canonical project ID**. The CLI successor must:

- honor `scope=project` by naming that database/canonical ID in the response;
- reject `scope=global` for tasks because global tasks are unsupported;
- document `scope=all` as equivalent to the current project database until a multi-project aggregator exists.

It must not pretend to row-filter on a column that does not exist.

## Current versus planned boundary

Current code already provides team membership validation, explicit project-scoped pulls, normal task sync, and receiving-side task mutations. The proposal API, proposal/dependency tables, CLI `project` parameter, triage actions, and external-dependency reconciliation are planned and must ship in the cloud-first order.

```text
CURRENT: local task create -> local tasks DB -> project-scoped cloud sync

PLANNED: origin supervisor
  -- explicit origin + target + task payload --> cloud proposal API
  -- validated proposal ---------------------> target proposal inbox
  -- receiving accept -----------------------> normal target open task
  <-- external dependency status ------------ target task close
```

## Acceptance tests for implementation successors

1. Local create without `project` is byte-for-byte compatible at the MCP boundary.
2. Worker and unknown-role sessions are refused before network I/O with the supervisor/director requirement.
3. Supervisor with no shared-team grant receives `403` naming that requirement.
4. Neither origin nor target is derived from cwd; omission is an error.
5. Retrying one idempotency key returns one proposal and one reserved task ID.
6. Target proposal is absent from `task ready/list` until accepted.
7. Accept materializes one `open` task with visible server-attested and client-asserted provenance.
8. Reject materializes no task and persists the decision reason/audit.
9. Origin external blocker remains blocked while proposed/accepted-open, reports a rejected handoff without unblocking, and auto-unblocks after the accepted target task closes.
10. `task list scope=project` states the canonical current-project boundary; unsupported global scope is rejected.

## Evidence

| Observation | Source | What it proves |
| --- | --- | --- |
| MCP create always opens the current task store and creates an `open`, project-scoped row | `cas-cli/src/mcp/tools/core/task/lifecycle.rs:43-56,75,163-208` | There is no cross-project create path today |
| Team push obtains one project ID from the current process and applies it to the batch | `cas-cli/src/cloud/syncer/team_push.rs:80-96` | Generic queue transport has no per-proposal target contract |
| Pulled rows missing or mismatching project identity are refused | `cas-cli/src/cloud/syncer/pull.rs:85-135` | Importing foreign task replicas would weaken a deliberate safety boundary |
| Task dependencies are explicitly not synced | `cas-cli/src/store/syncing_task.rs:161-175` | Generic task sync cannot carry cross-project `blocked_by` |
| Ready queries join local dependency rows to local blocker tasks | `crates/cas-store/src/task_store.rs:795-820` | Origin readiness cannot observe a target close without a new projection |
| Cloud team push authenticates membership but treats non-knowledge task data as opaque JSON | `petra-stella-cloud/app/api/teams/[teamId]/sync/push/route.ts:28-42,91-141` | Reusing push would trust client-authored provenance |
| Cloud team pull requires one project and filters rows by it | `petra-stella-cloud/app/api/teams/[teamId]/sync/pull/route.ts:39-80` | Origin and target cannot observe the same task through normal pull |
| Cloud entity identity is keyed by user, entity type, and ID—not project | `petra-stella-cloud/drizzle/schema.ts:15-54` | Cross-project replicas risk collisions/duplicates and are the wrong primitive |

## Open implementation questions

- The cloud team should choose the server-side CAS task-ID generator, preserving the `cas-` prefix while making collision probability safe at team scale.
- Cursor shape for dependency reconciliation should reuse the cloud's server revision/cursor conventions rather than client wall clocks.
- The receiving UI may expose triage in the web explorer as well as MCP; the API contract does not depend on that choice.

## Provenance

Examined on 2026-08-11 against CAS commit `f96b5831` and the local Petra Stella Cloud `main` checkout. Commands used: `rg` over task lifecycle, syncing task store, team push/pull, cloud task mutation helpers, and both repositories' schemas; `nl -ba` for the cited line ranges. The cloud API implementation request is tracked as [Richards-LLC/petra-stella-cloud#44](https://github.com/Richards-LLC/petra-stella-cloud/issues/44).
