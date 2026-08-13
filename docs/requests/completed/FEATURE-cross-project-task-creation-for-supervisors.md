---
from: CAS Cloud team (petra-stella-cloud factory supervisor, on behalf of Daniel)
to: CAS CLI team
date: 2026-08-07
priority: P2
---

> **Disposition (2026-08-13, cas-e0c9):** IMPLEMENTED — the authorization predicate,
> proposal/triage flow, provenance schema, and cross-project dependency projection are defined in
> [`docs/specs/2026-08-11-cross-project-task-proposals.md`](../../specs/2026-08-11-cross-project-task-proposals.md).
> The shipped CLI mechanism is `task action=create project=<target-canonical-id>` plus the dedicated
> `proposal_inbox`, `proposal_accept`, `proposal_reject`, and `proposal_reconcile` actions. Pending
> proposals never enter the local task table; optional origin blockers live in the
> `external_task_dependencies` projection and unblock only after reconciliation reports the accepted
> target task closed. The cloud contract is tracked at
> [Richards-LLC/petra-stella-cloud#44](https://github.com/Richards-LLC/petra-stella-cloud/issues/44);
> create/triage/dependency endpoints are live, while authoritative push-side preservation of the
> materialized task's provenance copy remains a cloud hardening gate. Original CAS issue:
> [#171](https://github.com/pippenz/cas/issues/171).

# Feature Request: let supervisors create tasks in other projects, when appropriate and authorized

## The ask (Daniel's words)

> it would be nice to allow supervisors to make tasks in other projects when appropriate and authorized

Today `mcp__cas__task action=create` scopes the new task to the calling session's project. A
supervisor coordinating work that *touches* another project has no way to file a tracked task
in that project's space — the only cross-team channel is dropping a `docs/requests/*.md` file
in the other repo and hoping their supervisor triages it into real tasks.

## Concrete pain, from today's session (petra-stella-cloud, 2026-08-07)

While implementing your `2026-08-06-cloud-knowledge-sync-and-embeddings.md` request, the cloud
factory surfaced work that conceptually belongs to *your* project:

1. **Cross-team knowledge bleed** — a multi-team user sharing one `project_canonical_id` pulls
   the union of both teams' pages. The clean fix is client-side (send the active `team_id` on
   the knowledge pull). That is a cas-cli task; the cloud supervisor could only describe it in
   a RESPONSE doc and file a *local* placeholder task (cloud cas-4b16) that waits for a human
   to relay it.
2. The same pattern applies to every finding in our RESPONSE docs' "open questions" sections —
   each one dies in a markdown file unless someone on your side re-types it as a task.

The file-inbox convention works, but it is lossy (no priority, no dependencies, no status, no
dedupe against your existing backlog) and adds a full human round-trip per item.

## Sketch of what "appropriate and authorized" could mean

We deliberately leave the design to you; some anchors observed from the supervisor seat:

- **Explicit target, never inferred**: e.g. `task create project=<canonical_id> ...` — the
  cross-project write must be a stated intent, not a side effect of cwd or session state.
- **Authorization is a grant, not a default**: only supervisors, and only into projects where
  the grant exists (team membership on a shared team seems like the natural predicate; a
  per-project allowlist would also work). Everyone else keeps today's behaviour.
- **Provenance stamped on the task**: created-by session/agent + origin project, so the
  receiving team can see it arrived from outside and triage accordingly — perhaps landing in a
  distinct `inbox`/`proposed` state rather than straight into `open`, so the receiving
  supervisor keeps final say over their backlog.
- **Cross-project dependencies are the payoff**: the origin project's follow-up task (like our
  cas-4b16) could then be `blocked_by` the target project's task, and unblock automatically
  when your side closes it — replacing the current "watch their docs/requests/ for a RESPONSE
  file" polling.

Related friction worth fixing en route if you touch this area: `task list scope=project` does
not actually filter by project (the local tasks table has no project_id), which is part of why
cross-project task hygiene is hard today.

## Not asked for

- No cross-project *close/update/transfer* — creation (or proposal) only.
- No bypass of the receiving project's verification/close gates.
- No change for non-supervisor roles.
