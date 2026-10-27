# Accounts registry — release-note draft

Status: draft for the next runtime release; unposted. Destination: #cas-internal.
The release owner supplies the published version, deploy target and receipts.
No screenshots are attached: these are terminal and JSON changes.

## User top-level

Live on production · User · Account choices now distinguish folders sharing one login.

## User reply

Was: accounts sharing a login appeared with the same email and no folder alias.
→ Now: the picker shows the alias beside the email so you can tell them apart.

Was: choosing a displayed email or a mistyped name could start an empty account folder.
→ Now: unique emails resolve to existing accounts; ambiguous emails and unknown names stop with an alias or login command.

Was: expired or duplicate account snapshots could show misleading usage.
→ Now: expired windows say usage is unknown, and folders sharing a subscription use the newest snapshot.

Was: an account stored in a linked folder could disappear from the picker.
→ Now: linked account folders appear, and unreadable entries are skipped.

## Dev top-level

Live on production · Dev · Account identity and reset times have structured JSON values.

## Dev reply

Was: `cas limits --json` returned local display text in each window's `reset_at`.
→ Now: `reset_at` is an RFC3339 datetime with an explicit zero offset; parse it as a datetime. Human output still uses local time.

Was: the registry encoded identity as JSON text, and stale detection matched error prose.
→ Now: identity is an object with account, workspace and email-fallback fields; stale detection uses the structured authentication state.
