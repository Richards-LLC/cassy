---
title: Release Notes Rubric — #cas-internal two-thread announcement
managed_by: cas
audience: anyone authoring a Cassy release announcement
---

# Release Notes Rubric

Standard for the Cassy release announcement posted to the **#cas-internal** Slack
channel after a version bump. The announcement is **two independent top-level
threads**, each with exactly one detail reply (4 messages total).

## Structure (4 messages)

**Thread 1 — USER**

1. **USER summary** (top-level): plain-language before / after / impact
   paragraph, ~3 sentences. What the experience was before, what it is now, why
   it matters. Include the version and a "shipped" confirmation. No jargon, no
   feature names, no CLI commands.
2. **USER details** (threaded reply): ~5 emoji-led, bold-header bullets, each
   1–3 sentences. Concrete user-facing wins. Plain prose only.

**Thread 2 — DEV**

3. **DEV summary** (top-level): plain-language paragraph on what changed
   architecturally and the engineering impact. Orient first, specifics second.
4. **DEV details** (threaded reply): ~5 bold-header bullets. File paths, struct
   names, and migration numbers are fine here. Final bullet is **Release
   mechanics**: version bump, tag, and short commit SHAs.

## Required framing

- Every thread answers **"how it was → how it is now."**
- USER leads with the before/after paragraph; bullets are supporting detail.
- DEV may go deeper and more technical, but still orients before drilling in.
- "Info dump" means **comprehensive** — cover every change since the previous
  release tag — while staying under the length cap per reply.

## Hard constraints — do NOT

- No internal task/EPIC IDs (`cas-xxxx`) anywhere.
- No internal-only file paths or struct names in the **USER** thread.
- **Do not describe internal agent-to-agent coordination chatter or the
  factory's internal messaging plumbing** — i.e. how supervisor / worker /
  director sessions talk to each other. When the release enables mixing worker
  harnesses, frame it as a *user capability* ("run Codex and Claude workers in
  one project, both with full project context"), not as message-delivery
  internals.
- No secrets, tokens, or credentials.
- Each reply stays under ~500 words.

## Quality bar

- A reader who does not use the tool can understand the USER thread.
- Concrete over abstract — examples beat descriptions.
- Both threads are independently scannable.

## Process

- Draft all 4 messages first and get approval **before** posting.
- Post order: USER summary → capture `message_ts` → USER details
  (`thread_ts`). Then DEV summary → capture `message_ts` → DEV details
  (`thread_ts`).
- Channel: **#cas-internal**.
