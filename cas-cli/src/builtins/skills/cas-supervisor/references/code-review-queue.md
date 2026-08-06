# Supervisor-Owned Code Review Queue (cas-b51a / cas-865b)

When the project config has `[code_review] owner = "supervisor"` (the default as of cas-865b), workers skip the full multi-persona review at close and instead transition their tasks to `pending_supervisor_review`. This eliminates the ~14-minute per-close blocking cost on the worker side.

## The queue is a visibility tool — not the full-review trigger

Use this queue page to see what is awaiting cherry-pick. It does not trigger
the full multi-persona review. Phase 3 uses a lightweight per-merge gate; the
single required full `/cas-code-review` run happens in Phase 4 after the epic
is code-complete.

```
mcp__cas__task action=list status=pending_supervisor_review
```

This shows you which tasks have been closed by workers and are waiting for you to cherry-pick and inspect.

## Per-merge gate

See **workflow.md Phase 3, step 5** for the lightweight gate:
- Read the direct diff against the task spec and acceptance criteria.
- Check ownership boundaries, obvious defects, missing files/tests, and proof.
- Run targeted mechanical verification only when the diff warrants it.
- Add a `mcp__cas__verification action=add` row for the audit trail.

Reserve `/cas-code-review mode=interactive` for Phase 4's assembled epic diff,
unless a single merge is exceptionally risky and you explicitly choose to spend
the full review there.

## Review modes (cas-b667)

`cas-code-review` is Workflow-backed: the skill is a thin wrapper that pre-fetches
the diff, hands Steps 1–4 (intent extraction, persona selection, size-gated parallel
dispatch, deterministic merge) to the Workflow, then routes the merged result by mode.
`mode=interactive` is not the only option:

| Mode | Use it for | Side effects |
|---|---|---|
| `interactive` | The standard supervisor-driven path — Phase 4's assembled epic diff, or an exceptionally risky single merge. | Edits, task creation, `task.close` routing |
| `report-only` | Read-only scans: a survey, a second opinion, a diff you are not gating on. Safe to run in parallel. | None — writes a merged envelope to `docs/reviews/<YYYY-MM-DD>-<short-ref>.md`, makes no edits and no task changes |
| `headless` | Skill-to-skill calls, where another skill or workflow consumes the envelope instead of a human. | Returns the envelope; no interactive prompting |

Pick `report-only` when you want the finding list without the review touching task
state — it is the cheap way to look, and it will not surprise you by closing or
reopening anything.

## The `execution` block is mandatory (cas-acf83)

The `code_review_findings` envelope passed to `task action=close` must carry an
`execution` block, copied **verbatim** from the review result — never hand-written.
Without it, an empty `residual[]` is indistinguishable from a review that never ran,
and `task.close` rejects the envelope. Three distinct rejections:

- **REVIEW EXECUTION UNREPORTED** — no `execution` block at all. Re-run the review and pass the envelope it returns.
- **REVIEW DID NOT EXECUTE** — `personas_run: 0`. Something stopped the personas from launching; a down or out-of-credit review transport is the usual cause, and the reported reason names it when the producer knew.
- **REVIEW INCOMPLETE** — personas ran, but a mandatory lane produced no verdict (`required_personas_missing` non-empty). Every always-on persona shares a transport, so one outage silently takes several out at once while a survivor reports a "successful" run. An empty `residual[]` from a partial review is silence, not a clean bill of health.

**This gate also applies to your escape-hatch closes.** Closing a task on a worker's
behalf does not exempt you from it. The only sanctioned bypass is
`mcp__cas__task action=close id=<id> bypass_code_review=true` — supervisor-only, and
logged as a decision note on the task. Use it when the review transport is genuinely
unavailable and you have reviewed the diff by another means; a recorded decision is
defensible, a silently-empty review is not.

## After review

1. **If clean, record the approval** — Tell the worker the review passed and, optionally, add `mcp__cas__verification action=add task_id=<id> status=approved summary="..."` for the audit trail.
2. **If changes are required, create the task first** — File an epic-child task with the finding, expected fix, acceptance criteria, and proof command in the task description. Then send a short coordination message that points at the task ID and tells the worker to run `mcp__cas__task action=show id=<id>`.
3. **If the work itself is rejected, use `request_changes`** — For a task sitting in `awaiting_merge` (declined merge, amendment needed after the merge landed, rejected work), `mcp__cas__task action=request_changes id=<id>` is the sanctioned exit. It reopens the task with the assignee preserved, so the same worker owns the rework. Do not use `reset` for this — `reset` clears the assignee and exists for tasks orphaned by a dead session.

Do this for both per-merge gate findings and epic-level review fix rounds. Do not deliver actionable findings only as a coordination message: messages are not durable task state, and a one-shot Codex worker recovering through `task mine` will otherwise see nothing to do.

## Config

Default as of cas-865b is `owner = "supervisor"` — no config entry is needed for new projects. To opt out to the legacy inline worker dispatch, add:
```toml
[code_review]
owner = "worker"
```
