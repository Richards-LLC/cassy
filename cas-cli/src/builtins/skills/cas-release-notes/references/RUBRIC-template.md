# Release Notes Rubric

> Canonical Cassy template. Copy to `docs/release-notes/RUBRIC.md` and fill the
> `<...>` placeholders. The rules below are the framework contract — a project
> may add to them, never relax them.

## Where to post

- **Channel:** `<#channel-name>` (`<CHANNEL_ID>`). The Violet hub posts only to a
  channel matching `*-internal` or one it allowlists, and the bot must be a member.
- **Deploy targets:**
  - merged to `<staging-branch>` → label **`Staging`**
  - merged to `<production-branch>` → label **`Live on production`**

## Transport

Use only the Violet hub/bot via the `violet` skill. Never use Claude.ai
Slack or a personal connector. Require authenticated `tools/list` and the
skill's bounded `violet_read` dedupe procedure before `violet_post`; preserve
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
- **Attached files only:** never name a file, path or artifact id unless that
  file is attached to the same message. Attach it, or describe it in words.
- **Before/after screenshots:** when a change in the User reply has a real
  Before and a real After capture (a `pairs[]` entry in that delivery's
  `qa/pairs/pairs.json`), attach both JPGs to that reply, Before first; their
  file names carry the labels. Say so in words in the change's line. A change
  with one side, an `unpaired[]` shot or no capture gets no image, and a
  mocked or edited capture is never attached. One reply carries at most ten
  files, so five pairs: keep the changes a user would notice most.
- **Times in CT:** every time in a post is US Central (`9:42 AM CT`); ET is fine
  when labelled. Never UTC, GMT or a timestamp ending in `Z`.

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
draft with a `## POSTED` block containing the posting time in Eastern 12-hour
form (for example `9:45 AM ET`), channel, and the
`message_id` and permalink of every top-level post and reply. This is the searchable receipt
that the announcement happened.

## Example shape

Slack renders mrkdwn, not Markdown: single `*bold*`, `_italic_`, backtick code
and `•` bullets. The Violet transport refuses a body containing `**`, a
`#` heading or a dash bullet, so write the draft in mrkdwn from the start.

User top-level:

```text
*Live on production — User*
Saved filters now survive a reload.
```

User reply:

```text
• *Saved filters* — Was: you re-applied your filters every time you came back to the list. Now: the list reopens exactly as you left it. (Before and After screenshots attached.)
```

Attach the pair with `--reply-file` flags placed after the reply they belong to, Before first:
`cas violet thread --channel <name> --text "<User top-level>" --reply "<User reply>" --reply-file <pairs-dir>/saved-filters.before.jpg --reply-file <pairs-dir>/saved-filters.after.jpg --idempotency-key <key>`.

Dev top-level:

```text
*Live on production — Dev*
Filter state is persisted per user instead of per session.
```

Dev reply:

```text
• *Filter persistence* — Was: filter state lived in the in-memory session store and was dropped on reload (#123). Now: it is persisted server-side, keyed by user id, and restored when the list mounts.
```
