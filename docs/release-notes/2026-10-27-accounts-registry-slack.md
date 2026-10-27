# Accounts registry — release-note draft

Status: draft for the next runtime release; unposted. Destination: #cas-internal.
The release owner supplies the published version, deploy target and receipts.
No screenshots are attached: these are terminal and JSON changes.

## User top-level

```text
*Live on production — User — Cassy*
Was: shared logins looked identical. → Now: account choices show folder aliases beside their emails.
```

## User reply

```text
• *Account choices* — Was: folders sharing one login showed the same email.
→ Now: the picker shows each folder's alias beside the email.

• *Account launch* — Was: choosing a displayed email or a mistyped name could create an empty folder.
→ Now: unique emails select existing accounts; ambiguous emails and unknown names stop with an alias or login command.

• *Usage* — Was: expired or duplicate snapshots could show misleading usage.
→ Now: expired windows say usage is unknown, and folders sharing a subscription use the newest snapshot.

• *Linked folders* — Was: a linked account could disappear or have its target processed more than once.
→ Now: every account alias remains usable, unreadable entries are skipped, and updates process each target once.

• *Readiness* — Was: a clean installation could omit the Claude and Codex login commands.
→ Now: hub readiness shows each missing account's login command without creating a folder.
```

## Dev top-level

```text
*Live on production — Dev — Cassy*
Was: account identity and reset times were text. → Now: JSON exposes typed identity and timestamp values.
```

## Dev reply

```text
• *Reset timestamps* — Was: cas limits --json returned local display text in each window's reset_at.
→ Now: reset_at is an RFC3339 datetime with an explicit zero offset; parse it as a datetime. Human output still uses local time.

• *Account identity* — Was: identity was JSON text, and stale detection matched error prose.
→ Now: identity is an object with account, workspace and email-fallback fields; stale detection uses the structured authentication state.
```
