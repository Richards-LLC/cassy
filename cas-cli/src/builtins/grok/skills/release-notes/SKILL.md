---
name: release-notes
description: Use when a merge reaches staging or main, or when the user asks to draft or post release notes, Slack updates, or a release-notes rubric.
managed_by: cas
---

# Release Notes

**Every PR merged to `staging` or `main` must be announced in the project's Slack release channel, following `docs/release-notes/RUBRIC.md`.** This skill installs that rubric when it is missing and drafts + posts the announcement when a merge lands.

## Step 1 — ensure the rubric exists

Check for `docs/release-notes/RUBRIC.md`.

- **Missing:** copy the canonical template from this skill's `references/RUBRIC-template.md` to `docs/release-notes/RUBRIC.md`, then fill the placeholders: the project's Slack **channel name + channel ID**, and the branch → deploy-target mapping (which branch means `Staging`, which means `Live on production`). Leave the rules verbatim — they are the framework-level contract, not per-project taste.
- **Present:** read it. A project-local rubric may add rules (extra threads, a diary workflow, a different channel); it must never relax the hard rules below.

## Step 1a — preflight and worker ownership

Run the transport preflight in `docs/SLACK_POSTING_RUNBOOK.md` before attempting
to post. For cas-src, the canonical route is the supervisor-owned
`claude.ai Slack` MCP on the approved `pippenz@gmail.com` profile. A connected
server listing is not a posting receipt; the read-only preflight must succeed.

Default Codex workers do not have a Slack transport. When the worker reaches
this duty, it saves the exact draft and hands the supervisor the draft path,
target channel, deploy target, and receipt request. The supervisor posts through
the canonical route and returns timestamps/permalinks for the worker to record.
This is a designed handoff, not a failed fallback. If preflight fails, stop
after saving the draft and report the measured error; do not claim `POSTED`.

## Step 2 — gather the merge

```bash
git log --oneline <last-release>..HEAD
gh pr list --state merged --base <branch> --limit 10
```

Read the actual diff for anything you are unsure about. Describe **what changed for the person on the other side of the screen**, not what the commit touched.

## Step 3 — draft the two threads

Post **two threads**, each a top-level message plus **exactly one threaded reply**:

- **User thread** — top-level: deploy target (`Staging` / `Live on production`) + **User** + one plain-language punch. Reply: the detail as **Was → Now**, in language anyone can follow.
- **Dev thread** — top-level: same deploy-target label + **Dev** + one technical punch. Reply: **Was → Now**, technical. GitHub PR numbers are allowed here (Dev thread only).

### Hard rules

- **Was → Now for every item.** No bare feature lists.
- **No internal ticket labels** (no `cas-XXXX`, no Jira keys) in any message.
- **No agent / factory / coordination / process talk and no drama.** Describe the product, not how it got built. Never mention agents, worktrees, retries, failed deploys, or blame.
- **One punch per top-level message.** The detail belongs in the reply.
- Plain language in the User thread; honest acknowledgment when something is a revert.

## Step 4 — save the draft

Save the postable draft to `docs/release-notes/<date>-<topic>-slack.md` (date `YYYY-MM-DD`, topic kebab-case). This is the artifact reviewers read and the record of what was announced.

## Step 5 — post

Post to the channel named in the rubric, in order: **user top-level → capture its `ts` → user reply → dev top-level → capture its `ts` → dev reply.** A reply must carry the parent's `ts`; a reply posted without one becomes a stray top-level message.

Immediately after posting, before ending the task or turn, annotate the saved
draft with a `## POSTED` block containing the UTC timestamp, channel, and a
permalink for every top-level post and reply. This mandatory, searchable receipt
records that the outward action happened.

If no Slack transport is configured, stop after Step 4 and tell the user the draft path plus the channel to post it in — do not silently skip the announcement.

## Anti-patterns

- Announcing a merge without the deploy-target label — readers cannot tell staging from production.
- One combined thread instead of separate User and Dev threads.
- More than one reply per thread (the detail is one reply, not a running commentary).
- Leaking ticket IDs or internal process into user-facing copy.
- Writing the draft but never posting it, or posting without saving the draft.
- Rewriting the hard rules into the project's own rubric. Project rubrics may add, never relax.
