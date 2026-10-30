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

Use only the Violet hub/bot via the builtin `violet` skill and the
runbook's authenticated preflight and bounded dedupe read. Never use Claude.ai
Slack or a personal connector. A worker with no hub connection saves the exact
draft and hands its path, channel, deploy target and receipt request to the
supervisor, who must use the same hub and return message IDs and permalinks.

If the hub cannot complete publication, preserve the draft and partial receipts
and report the measured failure. Never claim `POSTED` without returned message
IDs and permalinks.

## When to post

**Every PR merged to `staging` or `main`.** No exceptions: a revert, a hotfix, and
a one-line copy change all get an announcement. A runtime release additionally
follows the two-top-level-post workflow in `RELEASE_SLACK_RUBRIC.md`; a
harness-diary merge additionally follows the parent + three-reply workflow there.

## What to post

Two threads. Each thread has one punchy top-level message and **exactly one
logical threaded reply**: one reviewed reply body submitted once. Violet may
split that body into several Slack messages. Keep all parts in returned order
under the same parent; do not shorten the release notes to avoid a split.
Report uploads and report links are separate deliveries in those threads.

### 1. User thread

- **Top-level:** the deploy-target label + **User** + one plain-language sentence on
  what is now possible or better. One punch — no lists.
- **Reply:** the detail as **Was → Now**, in language anyone can follow.

### 2. Dev thread

- **Top-level:** the same deploy-target label + **Dev** + one technical sentence.
- **Reply:** **Was → Now**, technical. GitHub PR numbers are allowed here (Dev thread only).

Post order: user top-level → capture `message.message_id` → user reply → dev top-level → capture `message.message_id` → dev reply. Set each reply’s `reply_to` to its parent’s `message_id`.

## Hard rules

- Put documentation-only and test-only CHANGELOG entries under headings starting
  `Doc` or `Test` so release reports exclude them from shipped change totals.
- **Was → Now for every item.** Never a bare feature list.
- **No internal ticket labels** (`cas-XXXX`, epic IDs) in any message.
- **No agent / factory / supervisor / worker / coordination narration and no drama.**
  Describe the product, not how it got built.
- **One punch per top-level message.**
- **No line over 300 characters** (a `<url|label>` link counts as its label).
  The Violet hub refuses the whole post. A Was → Now item too long for one line
  starts `→ Now:` on a second line.
- **Plain language** in the User thread.
- **Honest reverts:** if something shipped and came back out, say so plainly.
- **Before/after screenshots:** when a change in the User reply has a real
  Before and a real After capture (a `pairs[]` entry in that delivery's
  `qa/pairs/pairs.json`), attach both JPGs to that reply, Before first; their
  file names carry the labels. Say so in words in the change's line. A change
  with one side, an `unpaired[]` shot or no capture gets no image, and a
  mocked or edited capture is never attached. One reply carries at most ten
  files, so five pairs: keep the changes a user would notice most.

## Artifact

Save the postable draft as `docs/release-notes/<date>-<topic>-slack.md`
(date `YYYY-MM-DD`, topic kebab-case) before posting.

Immediately after posting, before ending the task or turn, annotate that saved
draft with a `## POSTED` block containing the Eastern 12-hour posting time (for example `9:45 AM ET`), channel, and a
returned message ID and permalink for every top-level post and logical reply.
For a split reply, record **every** returned `message.part_ids` entry, in order,
alongside the returned `message_id` and opaque permalink; the singular ID does
not represent all parts. If Violet returns no `part_ids`, record the singular
ID as the one-element list. Do not invent individual part permalinks.

The release train saves these lists as JSON arrays in
`USER_TOP_LEVEL_PART_IDS`, `USER_REPLY_PART_IDS`, `DEV_TOP_LEVEL_PART_IDS` and
`DEV_REPLY_PART_IDS` in `announce.receipt`, retaining the existing `*_ID` and
`*_PERMALINK` fields. Its `## POSTED` rows contain `message_id=…`,
`part_ids=[…]` and the returned permalink. Record file uploads with their
**message ID and message permalink as well as file ID**. A missing part or
upload message ID is an incomplete receipt: preserve it and reconcile through
a bounded Violet read before completion; never repost to repair a receipt.
This is the searchable evidence that the announcement happened.
