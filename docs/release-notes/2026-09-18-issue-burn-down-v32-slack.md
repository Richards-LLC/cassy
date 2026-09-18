# v32 issue burn-down — source-on-main Slack announcement draft

Channel: `#cas-internal` (`C0B44GUKDK2`)

Deploy target: Source on main. This is a source-merge announcement, not a runtime release.

Publication status: Draft only; no messages have been posted.

Review basis: the actual source diff at `2adae618`. The four fenced blocks below are the complete postable text, in order: User top-level, User reply, Dev top-level, Dev reply.

## User top-level

```text
*Source on main — User — Cassy*
Was: after a reviewer merged your work, closing the task could fail with a confusing error. → Now: the task closes from the merged work.
```

## User reply

```text
• *Closing after review* — Was: after a reviewer merged your work, closing could reject the recorded change. → Now: the task closes from that merged work.

• *Retrying a rejected check* — Was: retrying could replay the same rejection. → Now: retry checks the repaired change while keeping prior feedback.

• *Starting independent work* — Was: an open prerequisite could block unrelated work from starting. → Now: unrelated work can start while the prerequisite remains open.

• *Fresh reminders* — Was: a recent action could still trigger an old reminder. → Now: recent activity resets the reminder quiet period.

• *Protected workspace* — Was: maintenance could pull the workspace out from under active work. → Now: active work keeps its workspace intact.

• *Clear build failures* — Was: a knowledge build could fail without naming the file. → Now: it identifies the file that failed and explains next steps.

• *Workspace-preserving restart* — Was: restarting a near-limit session could lose its workspace. → Now: an idle session restarts in place with its workspace and setup.

• *Shortcut names* — Was: shortcut names could behave differently from their documentation. → Now: documented names resolve consistently.
```

## Dev top-level

```text
*Source on main — Dev — Cassy*
Was: close and lifecycle decisions trusted mutable refs and reusable state. → Now: delivery evidence and verification cycles bind to current ancestry.
```

## Dev reply

```text
• *#873 / #887 / #892 — Anchors* — Was: merge tips, remote-only checks, and missing receipts could obscure delivery. → Now: recorded anchors, local targets, and branch tips preserve close proof.

• *#883 — Fresh verification* — Was: a rejected dispatch could authorize a retry. → Now: rejection stays in history and retry mints a current-head dispatch.

• *#886 — Dependency timing* — Was: dependencies stopped start and serialized work. → Now: start warns; close and merge enforce order.

• *#888 — Relay supersession* — Was: delayed relays could revive old state. → Now: transition and branch-tip identity revalidate each relay.

• *#884 / #885 — Checkout ownership* — Was: CLI sync could rebase a live checkout and detach HEAD. → Now: sync refuses active checkouts and preserves refs.

• *#890 — Shutdown reachability* — Was: shutdown checked only the default branch. → Now: it checks assigned targets and fails safe on unknown reachability.

• *#889 — In-place recycling* — Was: a near-limit idle process needed a replacement that could lose routing. → Now: recycling preserves its checkout and launch specification.

• *#893 — Idle reset* — Was: a tool call could leave the idle timer running. → Now: activity resets it and starts a new quiet interval.

• *#874 — Source diagnostics* — Was: builds hid which source failed. → Now: progress names each source; status exposes failure details.

• *#875 — Session binding* — Was: Stop could consume another session's observations. → Now: observations and summaries stay bound to one session.

• *#891 — MCP aliases* — Was: aliases could drift from dispatch. → Now: `get`→`show` and `inbox`→`inbox_poll` stay canonicalized.

• *#897 — Seed freshness* — Was: seed artifacts could be missing, unrelated, or old. → Now: source and crate changes are checked; invalid seeds fail closed.

Availability: These changes are on the source branch for the main merge; this announcement does not claim that an installed host has been updated.
```

## Source traceability (not posted)

- #873 → `fc54a7ba`; #874 → `11f675a4`; #875 → `b19c4303` and `98a4123e`.
- #883 → `7d647c45`; #884/#885 → `99f4206d`; #886 → `d7ff53ad` and `f37c2d3a`.
- #887 → `53f442f6`; #888 → `d32087f6`; #889 → `b642e3c9`.
- #890 → `16113313`; #891 → `57a0761a`; #892 → `6651c536`; #893 → `41d19588`; #897 → `433fae5e`.
