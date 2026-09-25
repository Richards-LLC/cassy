# Release Notes Rubric

> Canonical Cassy template. Copy to `docs/release-notes/RUBRIC.md` and fill the
> `<...>` placeholders. The rules below are the framework contract — a project
> may add to them, never relax them.

## Where to post

- **Channel:** `<#channel-name>` (`<CHANNEL_ID>`). The MechaCassy hub posts only to a
  channel matching `*-internal` or one it allowlists, and the bot must be a member.
- **Deploy targets:**
  - merged to `<staging-branch>` → label **`Staging`**
  - merged to `<production-branch>` → label **`Live on production`**

## Transport

Use only the MechaCassy hub/bot via the `mecha-cassy` skill. Never use Claude.ai
Slack or a personal connector. Require authenticated `tools/list` and the
skill's bounded `mecha_read` dedupe procedure before `mecha_post`; preserve
message and upload-integrity receipts. If the hub cannot complete publication,
save the draft and partial receipts and report blocked; handoff uses the same hub.

## When to post

**Every PR merged to `<staging-branch>` or `<production-branch>`.** No exceptions:
a revert, a hotfix, and a one-line copy change all get an announcement.

## What to post

Two threads. Each thread = one punchy top-level message plus a threaded reply.
**Default: one threaded reply per thread.** A project may explicitly document a
different reply count in its local rubric when its posting route requires it.

### 1. User thread

- **Top-level:** `<deploy target>` + **User** + one plain-language sentence on what
  is now possible or better. One punch — no lists.
- **Reply:** the detail as **Was → Now**, in language anyone can follow. One
  Was → Now line per change that a user would notice.

### 2. Dev thread

- **Top-level:** the same `<deploy target>` label + **Dev** + one technical sentence.
- **Reply:** **Was → Now**, technical. GitHub PR numbers are allowed here (Dev thread only).

Post order: user top-level → capture `message.message_id` → user reply → dev top-level → capture `message.message_id` → dev reply. Set each reply’s `reply_to` to its parent’s `message_id`.

## Hard rules

- **Was → Now for every item.** Never a bare feature list.
- **No internal ticket labels** (`cas-XXXX`, Jira keys) in any message.
- **No agent / factory / coordination / process talk and no drama.** Describe the
  product, not how it got built. Never mention agents, worktrees, retries, failed
  deploys, or blame.
- **One punch per top-level message.**
- **Plain language** in the User thread — no internal jargon, no module names.
- **Honest reverts:** if something shipped and came back out, say so plainly.

**Was → Now punch (observed 2026-08-11):** Bad: `Added hook support, tests, and CI updates.`
Good: `Was → Codex ignored an allowed hook decision. Now → it receives the harness-specific empty allow response.`
Lead with the user-visible before/after; an implementation inventory is not a release note.

## Published version report

After a published version release, run `cas-release-report` and link its HTML
and PDF from the announcement (PDF attached to the User thread, HTML linked
from the Dev thread).

## Artifact

Save the postable draft as `docs/release-notes/<date>-<topic>-slack.md`
(date `YYYY-MM-DD`, topic kebab-case) before posting.

Immediately after posting, before ending the task or turn, annotate that saved
draft with a `## POSTED` block containing the UTC timestamp, channel, and the
`message_id` and permalink of every top-level post and reply. This is the searchable receipt
that the announcement happened.

## Example shape

Slack renders mrkdwn, not Markdown: single `*bold*`, `_italic_`, backtick code
and `•` bullets. The MechaCassy transport refuses a body containing `**`, a
`#` heading or a `- ` bullet, so write the draft in mrkdwn from the start.

User top-level:

```text
*Live on production — User*
Saved filters now survive a reload.
```

User reply:

```text
• *Saved filters* — Was: you re-applied your filters every time you came back to the list. Now: the list reopens exactly as you left it.
```

Dev top-level:

```text
*Live on production — Dev*
Filter state is persisted per user instead of per session.
```

Dev reply:

```text
• *Filter persistence* — Was: filter state lived in the in-memory session store and was dropped on reload (#123). Now: it is persisted server-side, keyed by user id, and restored when the list mounts.
```
