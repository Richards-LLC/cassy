# BUG: workers cannot message `target=supervisor` — merge handoffs are lost and tasks strand in awaiting_merge

**Observed:** 2026-07-31, Woodworking factory session `Woodworking-strong-octopus-57`, cas 2.38.1 (3c0e189). Supervisor agent `young-wolf-6`, worker `quiet-shark-6`. Hit twice in one session on tasks cas-c255 and cas-f914.

## Symptom

The close-rejection guidance tells the worker to hand off to the supervisor by name:

```
mcp__cs__coordination action=message target=supervisor summary="ready to merge" message="..."
```

That literal `target=supervisor` does not resolve to any registered agent, so the handoff never arrives. The worker records the failure and stops:

> "A direct Cassy merge request to target `supervisor` was attempted but Cassy could not resolve an active supervisor alias; the AwaitingMerge queue and this note provide the durable handoff."

> "Literal supervisor ACK failed because no supervisor alias resolves."

Registered agents carry generated names (`young-wolf-6`, `quiet-shark-6`), and the supervisor's own name is not discoverable by the worker. The remediation text baked into the MERGE REQUIRED error names an alias that does not exist.

## Why it is worse than a lost message

The failed handoff is not the end state. It wedges the worker:

1. Worker's `task close` → MERGE REQUIRED → task parked `awaiting_merge`, lease released.
2. Worker tries `target=supervisor` → unresolvable → no delivery, no error surfaced to the supervisor.
3. Supervisor merges the branch (it watches the `awaiting_merge` queue independently), then messages the worker to re-close.
4. If the worker is reassigned before it acts on that message, the task stays `awaiting_merge` **forever** — nobody re-closes it.

In this session, cas-c255 sat in `awaiting_merge` long after its commit had merged, silently blocking its dependent cas-d60b. cas-f914 stranded the same way, and the worker then sat with a newly-assigned task **unstarted for ~12 minutes** with no activity, surfacing only as `⚠ ASSIGNED BUT UNSTARTED` in `worker_status`. Both required supervisor escape-hatch closes with `commit_receipt`, plus an urgent interrupt to unwedge the worker.

The worker behaved correctly at every step. This is purely a routing defect.

## Workaround used

1. Supervisor polls `task list status=awaiting_merge` / reacts to the director's MERGE REQUIRED signal rather than relying on worker handoff.
2. Supervisor closes the stranded task itself via `task close` with `commit_receipt` + `bypass_code_review`.
3. Supervisor sends an `urgent=true` interrupt telling the worker to stop retrying the handoff.

## Suggested fix

Any one of these removes the failure:

- **Resolve `supervisor` as a reserved alias** to the session's supervisor agent. The remediation text already promises this contract; make it true.
- **Inject the supervisor's actual agent name into worker context** at spawn (e.g. a `SUPERVISOR_AGENT` value the worker can use in `target=`), and reference that in the MERGE REQUIRED remediation instead of a literal.
- **Make the `awaiting_merge` parking itself notify the supervisor** — the task already knows its parked branch and its supervisor; the worker should not have to hand off manually at all.

Additionally: `message` returned `enqueued`/`delivered` semantics without surfacing an unresolved-target error to the sender's supervisor. An unresolvable `target=` should fail loudly at call time, not resolve into silence.

## Secondary observation (same session, lower severity)

`epic_status` and the close guard evaluate merge state against the **epic branch** even when the project's convention is to land all work directly on `main`. With work correctly merged to `main`, epic-child closes were still rejected, and the guard is explicitly bypass-immune. Workaround was to fast-forward every `epic/*` branch to `main` after each merge so the commits became reachable. Worth considering a project-level setting for main-only repos, or letting the guard accept reachability from the task's configured `target_branch`.


---

## Resolution (2026-07-31, cas-ae2f)

Fixed and merged in epic cas-8c9c at commit `e3959af6`.

Trusted factory spawn now stamps the supervisor pane's harness session id as AgentRole::Supervisor, and CAS_SUPERVISOR_NAME is mirrored into Codex's restricted cs MCP env. The mod.rs:667 privilege boundary is unchanged; public/env registration still cannot self-assign a privileged role.
