# PENDING: unposted release announcements for #cas-internal

Status as of 2026-08-06 ~17:30 UTC. Delete this file once everything below is posted.

Both Slack transports were down on 2026-08-06 (Codex quota exhausted until **Aug 8
2026 10:07 AM**; claude.ai Slack connector unauthorized on the `~/.claude-alt`
profile — a human `/mcp` re-auth fixes that earlier). Several releases are tagged
and merged but unannounced. ANY agent on ANY account can post once a transport works.

**Confirmed unposted** — each file carries a `Status: DRAFT — not yet posted`
header. Post in this order (oldest release first) so the channel reads in
release order:

1. `2026-08-06-team-config-dir-slack.md` — v2.44.0 content
2. `2026-08-06-builtin-refresh-slack.md` — v2.44.0 content
3. `2026-08-06-session-start-size-budget-slack.md` — v2.44.0 content
4. `2026-08-06-v2.45.0-slack.md` — v2.45.0

**Verify before assuming posted** — these two same-day drafts have no Status
header; check `#cas-internal` history (or ask pippenz) before posting or skipping:

- `2026-08-06-v2430-supervisor-signals-status-truth-slack.md` (v2.43.0)
- `2026-08-06-codex-trust-wakeups-sweep-skill-slack.md`

Each draft is postable as written: two top-level messages (User, then Dev), each
with exactly one threaded reply, per `docs/RELEASE_SLACK_RUBRIC.md`, posted via
`docs/SLACK_POSTING_RUNBOOK.md`. No ticket IDs, no process narration.

**Tracking task:** `cas-a872` (CAS task — record permalinks in its notes, flip each
draft's Status header to posted, close the task, delete this file).

**Caveat before announcing:** both tags' Release runs failed on the 2026-08-06
GitHub Actions outage and needed reruns — verify `gh release view v2.44.0` and
`gh release view v2.45.0` succeed before announcing artifacts that may not exist.
