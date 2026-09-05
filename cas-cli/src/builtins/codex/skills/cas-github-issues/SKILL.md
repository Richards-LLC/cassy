---
name: cas-github-issues
description: Use when sweeping, triaging, deduplicating, verifying, closing, or filing GitHub issues, or reconciling issues with Cassy tasks.
managed_by: cas
---

# GitHub Issues sweep

GitHub Issues is the canonical intake when a project has more than one machine
filing bugs. This skill is the reconciliation loop between that intake and the
Cassy task graph: **every open issue ends the sweep either closed, deduped, or
pointing at a live Cassy task.**

Run all six steps in order. **If no step changed anything, end the turn without
a report** — a silent sweep is the success case, and a "nothing to do" summary
every hour is noise.

## Before you start

```bash
gh repo view --json nameWithOwner -q .nameWithOwner   # which repo am I sweeping?
gh issue list --state open --limit 100 --json number,title,body,labels,createdAt,comments
```

If `gh` is not authenticated (`gh auth status` fails), stop and say so — do not
fall back to guessing from local files.

Then load the Cassy side once, so every later step reads from the same picture:

```
mcp__cs__task action=list limit=100
mcp__cs__search action=search query="<the issue's subject in your own words>"
```

**Do not filter this list by `status=open`.** The status filter is a single
substring match, so `status=open` returns *only* untouched tasks — every task
someone is actually working on (`in_progress`, `blocked`, `awaiting_merge`)
drops out, and the sweep concludes an in-flight issue was never tasked. List
everything and ignore the closed ones yourself.

## Issue routing

The sweep must preserve component ownership. Resolve the four destinations
with `cas config get issues.repo`, `cas config get issues.components.cassy`,
`cas config get issues.components.mecha_cassy`, and
`cas config get issues.components.cloud`. Use `issues.repo` for the current
project, the Cassy component key for runtime/hooks/MCP/factory/skill defects,
the MechaCassy key for Slack hub defects, and the Cloud key for sync,
relay, or pairing defects. If you hit a bug during operation, file a ticket in the matching repo before moving on; do not infer a destination from git remotes.

## 1. List open issues

Fetch the open issues with their bodies **and comment counts**. An issue whose
last comment already carries a `cas-XXXX` task ID is already tasked — it is not
new, and step 4 must not task it again. Build the working set:

| Bucket | Meaning | Handled in |
|---|---|---|
| Duplicate | Same defect as another open issue | Step 2 |
| Claims fixed | Body or comments assert the fix shipped | Step 3 |
| New | Real, unduplicated, untasked | Step 4 |
| Already tasked | Comment names a live Cassy task | Step 5 |

## 2. Dedupe double-filings

Multi-machine intake means the *same* defect gets filed twice within minutes,
with different wording. Compare by **symptom and failing surface, not by title
text** — "worker never closes task" and "close hangs at AwaitingMerge" are one
issue.

Keep the issue with the better reproduction (or the earlier number if they are
equal). On the loser:

```bash
gh issue comment <dup> --body "Duplicate of #<keeper> — same defect, tracking there."
gh issue close <dup> --reason "not planned"
```

If the loser carries detail the keeper lacks, copy that detail into the keeper
**before** closing. Never close a duplicate that has evidence nowhere else.

## 3. Verify-and-close fixed claims

An issue claiming to be fixed is a claim, not a fact. **Verify against the code
or a run before closing** — find the commit, read the changed lines, or run the
reproduction. Do not close on "the task that mentions this issue is closed".

- **Verified fixed** — comment with the evidence (commit SHA, test name, or the
  command you ran and its output), then
  `gh issue close <n> --reason completed`.
- **Cannot verify** — leave it open, comment what you checked and what is still
  missing. A stale open issue is cheaper than a wrongly closed one.

## 4. Task new issues into the active epic

Find the active epic for this intake lane. List **every** epic and judge the
status yourself — an epic is auto-promoted to `in_progress` the moment a
supervisor starts any of its subtasks, so `status=open` hides exactly the epics
that are most alive:

```
mcp__cs__task action=list task_type=epic limit=50
```

- **An epic for this lane is not closed** (`open`, `in_progress`, `blocked`) →
  task into it.
- **Every epic for this lane is closed** (the last one closed with a release) →
  **create a successor epic first.** Never task into a closed epic — a child of
  a closed epic is invisible to the ready queue and will never be picked up.

```
mcp__cs__task action=create task_type=epic title="<intake> burn-down v<N>: <theme> (GH #<lo>–#<hi>)" priority=1
```

Then, for each new issue, one task:

```
mcp__cs__task action=create title="<what will be true when this is done> (GH #<n>)" \
  task_type=bug priority=<0-3> epic=<epic id> \
  external_ref="https://github.com/<owner>/<repo>/issues/<n>" \
  description="<the reporter's symptom, the surface it fails on, and the repro>" \
  acceptance_criteria="<the observable that proves it fixed>"
```

Priority from user impact, not from filing order: data loss / agent-stuck / the
factory cannot make progress → P0–P1; degraded-but-workable → P2; polish → P3.

Close the loop on GitHub so the reporter (and the next sweep) can see it:

```bash
gh issue comment <n> --body "Tracked as \`cas-XXXX\`. <one line on the plan.>"
```

**Issue-comment specificity.** Bad: `Tracked as \`cas-2a13\`.`
Good: `Tracked as \`cas-2a13\`. I’ll add real bad/good pairs to the guidance writers use.`
Keep the tracker link, then state the concrete outcome; a bare ID leaves the reporter without an answer.

The commit that fixes the issue should carry `Fixes #<n>` so GitHub closes it
on merge.

## 5. Unblock chained tasks whose lane merged

Issues get tasked into lanes that block each other. When a lane merges, its
dependents stay blocked until someone says so — that someone is this sweep.

```
mcp__cs__task action=blocked
```

For each blocked task, check whether its blocker actually landed:

```bash
git log --oneline origin/main -20        # or the project's integration branch
gh pr list --state merged --limit 20 --json number,title,mergedAt
```

If the blocker is closed **and merged**, drop the edge:

```
mcp__cs__task action=dep_remove id=<blocked task> to_id=<merged blocker>
```

Merged is the bar, not closed. A closed-but-unmerged blocker still blocks —
removing that edge sends a worker at a base that does not contain the fix.

## 6. File issues for defects you observed

Anything you hit since the last sweep that is a real defect belongs in the
tracker, even if you already worked around it. Search first — the duplicate you
create is the duplicate you have to dedupe next hour:

```bash
gh issue list --state all --search "<distinctive symptom>" --limit 20
```

If it is genuinely new, file it with the standard body — six headings, in this
order, every one filled in:

| Heading | What goes in it |
|---|---|
| **Environment** | Version (`cas --version` or equivalent), OS, harness/CLI, and anything about the machine that could matter. |
| **Repro** | The exact commands, in order, that someone else can paste. Not a description of them. |
| **Actual** | What happened, quoted from the real output — not paraphrased. |
| **Expected** | What should have happened, and why you believe that. |
| **Impact** | Who is blocked and how badly. This is what sets the priority in step 4. |
| **Suggested fix** | Where you think it lives (file:line if you found it) — or "unknown", which is a legitimate answer. |

```bash
gh issue create --title "<surface>: <what goes wrong>" --body-file /tmp/issue.md
```

Write the body to a file rather than splicing a multi-line string through the
shell. Then task it in step 4's format if it is actionable now.

## Why you may be here: the unfiled-reports banner

`docs/requests/` is **deprecated for new outbound actionable requests**. Do not create a new file there: file directly on the receiving Richards-LLC team's issue board and save a Cassy memory receipt (issue URL, one-line ask, date). This skill still sweeps pre-existing staged legacy files so they are not lost; history and inbound `RESPONSE-*.md` files remain readable. Prose-heavy specifications and design documents may remain there until cross-project task proposals ship.

Cassy emits a SessionStart banner when `BUG-*.md` / `FEATURE-*.md` files are
staged at the `docs/requests/` root — reports the write-first flow wrote but
never pushed, so nobody outside that checkout can see them. Sweeping those
staged files into GitHub is part of this skill's job: read each one, search for
an existing issue (step 6's dedupe check applies), file it with `gh issue
create --body-file docs/requests/<file>`, and delete the staged file only after
the issue URL is known. The banner clears when the directory root is empty.

A companion banner fires when `[issues] repo` is unset while `docs/requests/`
exists. It names `cas config set issues.repo owner/repo` and deliberately
suggests no value — use the tracker the receiving team specified, never one
derived from the `origin` remote.

## The recurring sweep

This sweep normally runs from a cron entry in `.claude/scheduled_tasks.json`,
hourly. That file is **untracked runtime state**, and the entry carries a
**7-day auto-expiry** — when it lapses, the sweep silently stops happening and
nothing announces it.

So: if you are reading this skill because someone asked "is the sweep still
running?", check the job is there and has fired recently (`lastFiredAt`), and
**recreate it if it expired**. A sweep nobody re-armed is the failure mode this
whole loop has — it degrades to silence, which is also what success looks like.

## Ending the sweep

- **Something changed** — report only the deltas: issues closed, tasks created
  (with IDs), edges dropped, issues filed. No restating the whole backlog.
- **Nothing changed** — end the turn silently. No message.

## Rules

1. **Never task into a closed epic.** Create the successor first.
2. **Never close on a claim.** Step 3 closes on evidence you gathered, or not
   at all.
3. **Dedupe before tasking.** Two tasks for one defect means two workers in the
   same files.
4. **Every tasked issue gets a comment with its task ID.** That comment is how
   the next sweep knows the issue is not new.
5. **Merged, not closed**, is the bar for unblocking.
6. **Silence is a valid sweep result.**
