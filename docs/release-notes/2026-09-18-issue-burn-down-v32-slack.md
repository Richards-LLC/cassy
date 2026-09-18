# v32 issue burn-down — source-on-main Slack announcement draft

Channel: `#cas-internal` (`C0B44GUKDK2`)

Deploy target: Source on main. This is a source-merge announcement, not a runtime release.

Publication status: Draft only; no messages have been posted.

Review basis: the actual source diff at `2adae618`. The four fenced blocks below are the complete postable text, in order: User top-level, User reply, Dev top-level, Dev reply.

## User top-level

```text
*Source on main — User — Cassy*
Was: finishing a change could depend on how it arrived. → Now: the right change stays attached through review and handoff.
```

## User reply

```text
• *Reliable handoffs* — Was: a finished change could lose its place after review or handoff. → Now: it stays tied to the right result.

• *Fresh retries* — Was: a failed check could repeat old feedback. → Now: each retry checks repaired work and keeps earlier feedback for reference.

• *Independent starts* — Was: one prerequisite could block unrelated work. → Now: independent work starts while final ordering stays protected.

• *Current reminders* — Was: stale updates or activity could trigger an old reminder. → Now: current changes and activity reset the reminder clock.

• *Safer workspaces* — Was: maintenance could disturb a workspace or miss landed work. → Now: active work is protected and cleanup checks relevant locations.

• *Visible build progress* — Was: a knowledge update could fail without naming its source. → Now: progress and next steps identify what needs attention.

• *Fresh build inputs* — Was: a workspace could start from outdated build files. → Now: old files are rejected or called out before misleading a build.

• *In-place recovery* — Was: a nearly full coding session needed a restart. → Now: an idle session refreshes without losing its workspace or setup.

• *Session-local activity* — Was: activity from one coding session could be attributed to another. → Now: each session keeps its own activity and summary.

• *Consistent shortcuts* — Was: shortcuts could be accepted inconsistently. → Now: documented shortcuts resolve to the same actions everywhere.
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
