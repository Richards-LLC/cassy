# Release Notes Rubric

> Canonical CAS template. Copy to `docs/release-notes/RUBRIC.md` and fill the
> `<...>` placeholders. The rules below are the framework contract — a project
> may add to them, never relax them.

## Where to post

- **Channel:** `<#channel-name>` (`<CHANNEL_ID>`)
- **Deploy targets:**
  - merged to `<staging-branch>` → label **`Staging`**
  - merged to `<production-branch>` → label **`Live on production`**

## When to post

**Every PR merged to `<staging-branch>` or `<production-branch>`.** No exceptions:
a revert, a hotfix, and a one-line copy change all get an announcement.

## What to post

Two threads. Each thread = one punchy top-level message + **exactly one** threaded reply.

### 1. User thread

- **Top-level:** `<deploy target>` + **User** + one plain-language sentence on what
  is now possible or better. One punch — no lists.
- **Reply:** the detail as **Was → Now**, in language anyone can follow. One
  Was → Now line per change that a user would notice.

### 2. Dev thread

- **Top-level:** the same `<deploy target>` label + **Dev** + one technical sentence.
- **Reply:** **Was → Now**, technical. GitHub PR numbers are allowed here (Dev thread only).

Post order: user top-level → capture `ts` → user reply → dev top-level → capture `ts` → dev reply.

## Hard rules

- **Was → Now for every item.** Never a bare feature list.
- **No internal ticket labels** (`cas-XXXX`, Jira keys) in any message.
- **No agent / factory / coordination / process talk and no drama.** Describe the
  product, not how it got built. Never mention agents, worktrees, retries, failed
  deploys, or blame.
- **One punch per top-level message.**
- **Plain language** in the User thread — no internal jargon, no module names.
- **Honest reverts:** if something shipped and came back out, say so plainly.

## Artifact

Save the postable draft as `docs/release-notes/<date>-<topic>-slack.md`
(date `YYYY-MM-DD`, topic kebab-case) before posting.

## Example shape

- **User (top-level):** `Live on production` · **User** — Saved filters now survive a reload.
- **User (reply):** Was → you re-applied filters every time you came back to the list.
  Now → the list reopens exactly as you left it.
- **Dev (top-level):** `Live on production` · **Dev** — Filter state persisted per user instead of per session.
- **Dev (reply):** Was → filter state lived in in-memory session store, dropped on reload (#123).
  Now → persisted server-side keyed by user id, restored on list mount.
