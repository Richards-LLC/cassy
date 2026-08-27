# Release Notes Rubric — cas-src

> cas-src's copy of the Cassy release-notes rubric (canonical template ships in the
> `release-notes` builtin skill at `references/RUBRIC-template.md`). This repo has
> an additional, stricter workflow for runtime releases and harness diaries — see
> [docs/RELEASE_SLACK_RUBRIC.md](../RELEASE_SLACK_RUBRIC.md). Where the two
> overlap, `RELEASE_SLACK_RUBRIC.md` wins for this repo.

## Where to post

- **Channel:** `#cas-internal` (`C0B44GUKDK2`)
- **Deploy targets:**
  - merged to `staging` → label **`Staging`**
  - merged to `main` → label **`Live on production`**

For **how** to post when the session has no Slack connection of its own, see
[docs/SLACK_POSTING_RUNBOOK.md](../SLACK_POSTING_RUNBOOK.md).

## Transport ownership

Run the runbook's transport preflight before posting. The canonical cas-src
route is the supervisor-owned `claude.ai Slack` MCP on the approved
`pippenz@gmail.com` profile. Default Codex workers have no Slack transport, so
their designed path is to save the exact draft and hand its path, target
channel, deploy target, and receipt request to the supervisor. The supervisor
posts through the canonical route and returns timestamps/permalinks for the
worker to record.

If preflight fails, leave the draft saved and report the duty blocked with the
measured error. Never claim `POSTED` without returned timestamps and
permalinks.

## When to post

**Every PR merged to `staging` or `main`.** No exceptions: a revert, a hotfix, and
a one-line copy change all get an announcement. A runtime release additionally
follows the two-top-level-post workflow in `RELEASE_SLACK_RUBRIC.md`; a
harness-diary merge additionally follows the parent + three-reply workflow there.

## What to post

Two threads. Each thread = one punchy top-level message + **exactly one** threaded reply.

### 1. User thread

- **Top-level:** the deploy-target label + **User** + one plain-language sentence on
  what is now possible or better. One punch — no lists.
- **Reply:** the detail as **Was → Now**, in language anyone can follow.

### 2. Dev thread

- **Top-level:** the same deploy-target label + **Dev** + one technical sentence.
- **Reply:** **Was → Now**, technical. GitHub PR numbers are allowed here (Dev thread only).

Post order: user top-level → capture `ts` → user reply → dev top-level → capture `ts` → dev reply.

## Hard rules

- **Was → Now for every item.** Never a bare feature list.
- **No internal ticket labels** (`cas-XXXX`, epic IDs) in any message.
- **No agent / factory / supervisor / worker / coordination narration and no drama.**
  Describe the product, not how it got built.
- **One punch per top-level message.**
- **Plain language** in the User thread.
- **Honest reverts:** if something shipped and came back out, say so plainly.

## Artifact

Save the postable draft as `docs/release-notes/<date>-<topic>-slack.md`
(date `YYYY-MM-DD`, topic kebab-case) before posting.

Immediately after posting, before ending the task or turn, annotate that saved
draft with a `## POSTED` block containing the UTC timestamp, channel, and a
permalink for every top-level post and reply. This is the searchable receipt
that the announcement happened.
