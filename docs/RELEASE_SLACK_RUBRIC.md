# Release Slack Rubric

**This is a hard rule. Runtime releases and harness-diary updates each have a
mandatory #cas-internal publication workflow. They are separate duties.**

> This file defines **what** to post. For **how** to post it when the session has
> no working Slack connection of its own — the Codex `codex_apps` Slack plugin,
> and the stdin / approval / buffering traps that make it look broken — see
> [docs/SLACK_POSTING_RUNBOOK.md](SLACK_POSTING_RUNBOOK.md).

## Runtime releases: two top-level posts

After a release is pushed + tagged, post to **#cas-internal** (`C0B44GUKDK2`). Always **two distinct top-level posts** (not threaded replies) — one per audience:

1. **User-perspective post**
2. **Dev-perspective post**

### Shape (identical for both posts)

1. **Open with the punch** — one or two lines of plain, punchy language framed as **how it was → how it is now**. Lead with the change in experience, not the mechanism.
2. **Details below** — bullets fleshing out the punch, written from that post's perspective.

### Runtime voice rules

- **User post: ALWAYS plain language.** Describe what the user feels/sees. No jargon dumps.
- **Dev post: may be more technical** — code/behavior level is fine.
- **BOTH posts:**
  - **No CAS-internal agent actions.** Do not narrate supervisor/worker/factory/director orchestration, task lifecycle bookkeeping, who-closed-what, epics, etc.
  - **No ticket numbers.** No `cas-xxxx`, no epic IDs. Describe the change, not the tracking artifact.
  - Lead with the before→after punch; keep it tight.

## Harness-diary updates: one parent + three replies

After any update to the Claude, Codex, or Grok changelog diary merges to `main`,
publish one thread in **#cas-internal** (`C0B44GUKDK2`):

1. **One top-level parent** — summarize the cross-harness sweep and lead with why
   the changes matter to CAS users and maintainers.
2. **Exactly three threaded replies**, in this order:
   1. **Grok**
   2. **Claude**
   3. **Codex**

The parent and replies must use impact-first prose. Each harness reply names the
version or version range reviewed, the notable CAS touchpoints, the resulting CAS
verdict/action, and any source gaps (write `none` when there are none). Report what
changed and what CAS users should expect; do not narrate how the diary work was
assigned or executed.

This is one shared three-harness thread even when only one diary changed. Use the
other two replies to state their current reviewed ranges and verdicts so the thread
always presents one complete harness snapshot.

### When runtime and diary changes overlap

- A merge containing both a runtime release and a harness-diary update requires
  **both workflows**: the two runtime-release top-level posts and the separate diary
  parent with exactly three replies.
- A diary-only merge uses only the diary thread. Do **not** fabricate a release,
  tag, shipped runtime behavior, or user/dev release announcement.

## Rules for every post and reply

- Lead with impact, not mechanics or bookkeeping.
- Include version ranges, CAS verdict/action, and source gaps in the diary thread.
- Use **zero ticket/task/epic IDs** (including `cas-xxxx`).
- Use **zero internal agent, worker, supervisor, director, or factory narration**.

## Why

These messages are for a product/stakeholder audience. They communicate *impact*,
not internal process. Runtime releases split the plain-language user story from the
technical dev story; diary threads give both audiences one traceable cross-harness
compatibility snapshot. Neither includes factory plumbing or ticket IDs.

## Checklist

### Runtime release

- [ ] Release pushed to `origin/main` + tag pushed
- [ ] Post 1 (user): punch (was→now) + plain-language details
- [ ] Post 2 (dev): punch (was→now) + technical details
- [ ] Both: zero ticket numbers, zero internal-agent narration

### Harness-diary update

- [ ] One top-level parent in `C0B44GUKDK2` explains the cross-harness impact
- [ ] Exactly three replies, ordered Grok → Claude → Codex
- [ ] Each reply includes version range, CAS touchpoints, verdict/action, and source gaps
- [ ] Parent and replies contain zero ticket IDs and zero factory narration
- [ ] If runtime code also shipped, complete the runtime-release checklist too
- [ ] If the merge is diary-only, make no runtime-release claim
