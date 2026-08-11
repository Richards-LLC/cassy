# Feature request: cloud-owned cross-project task proposals and dependency signals

CAS CLI is adding explicit `task create project=<canonical_id>` for supervisors. The client half must not ship on generic sync because generic task JSON cannot provide authoritative provenance and project-scoped pulls cannot tell an origin project when a target task closes.

The full decided contract and evidence will land in `docs/specs/2026-08-11-cross-project-task-proposals.md` in the CAS repository (`pippenz/cas`, task `cas-a0ba`, issue `#171`). This request is tracked as `Richards-LLC/petra-stella-cloud#44`.

## Requested cloud surface

1. `POST /api/teams/{team_id}/task-proposals`
   - Requires bearer-token membership in a team shared by two **existing, explicit** canonical projects.
   - Never infers or auto-registers origin/target projects.
   - Takes an idempotency key, task payload, client-asserted CAS session/agent provenance, and optional `blocks_origin_task_id`.
   - Server stamps authenticated user, team, resolved origin/target projects, and receipt time.
   - Returns a stable `proposal_id` and reserved `target_task_id` in `state=proposed`.
2. Target-project proposal list plus idempotent accept/reject endpoints.
   - Accept atomically materializes a normal `open` task under the target project.
   - Reject records actor/time/reason and materializes no task.
3. A project-scoped external dependency feed for origin projects.
   - Pending while proposal is proposed or accepted target task is open.
   - Resolved only when the accepted target task is closed.
   - Rejection is `handoff_rejected`, not successful resolution.
4. Two purpose-built tables (`task_proposals`, `cross_project_task_dependencies`) rather than foreign task replicas in `sync_entities`.

## Authorization decision

CAS gates the operation to registered supervisor/director sessions before network I/O. Cloud owns the actual grant: bearer user membership plus both projects existing in the same team. A refusal must name the shared-team requirement. A future project allowlist may narrow this grant; v1 has no ambient account-wide grant.

## Provenance decision

Server-attested: proposal/task IDs, authenticated creator user, team, resolved origin/target canonical projects, receipt time, idempotency key, and receiving decision actor/time. Client-asserted and visibly labeled: CAS origin session ID, agent ID/name/role, client version/build. Clients cannot override server-attested fields.

## Why generic sync is insufficient

- Team push validates membership but stores task data as opaque JSON, so claimed provenance is forgeable.
- Team pull requires one project ID and CAS refuses foreign rows at ingest.
- CAS dependency rows are local-only and its ready query joins blockers to local task rows.
- `sync_entities` identity excludes project ID from its primary key, making cross-project replicas unsafe.

## Cloud acceptance tests

- Missing target or origin project is rejected; no cwd/session inference exists server-side.
- User without membership shared by both projects gets `403` naming the grant requirement.
- Repeated idempotency key returns exactly one proposal/task ID.
- Proposal is not returned as a normal task before accept.
- Accept creates exactly one target-project open task with visible provenance; reject creates none.
- Target close changes the dependency feed to resolved; target rejection returns `handoff_rejected`.
- Existing generic sync contracts and normal task push/pull remain unchanged.

CAS will implement the CLI parameter, local role gate, receiving triage, provenance rendering, project-scope truthfulness, and external dependency reconciliation after this cloud contract is live.
