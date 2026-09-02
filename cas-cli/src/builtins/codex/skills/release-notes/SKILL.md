---
name: release-notes
description: Use when a merge reaches staging or main, or when the user asks to draft or post release notes, Slack updates, or a release-notes rubric.
managed_by: cas
---

# Release Notes

Use the project's release rubric as the contract. The procedure is:

1. **Ensure the rubric exists.** Check `docs/release-notes/RUBRIC.md`. If it is
   missing, copy this skill's `references/RUBRIC-template.md`, fill its project
   placeholders, and preserve every hard rule. If it exists, read it; local
   additions may tighten the contract but may not relax its hard rules.
2. **Gather the merge.** Read the commits and merged change set since the last
   release. Describe what changed for the person on the other side of the
   screen, not an inventory of touched files.
3. **Draft the messages from the rubric.** Follow its deploy-target label,
   audience labels, before/after format, thread count, and reply-count default.
   Put one punch in each top-level message and the supporting detail in its
   reply. Do not include internal ticket labels or implementation process.
4. **Save the draft.** Write the exact postable text to
   `docs/release-notes/<date>-<topic>-slack.md` before posting.
5. **Post in rubric order.** Use the configured project channel and preserve
   the parent identifier on every reply. If the configured posting route is
   unavailable, stop after saving the draft and report the measured failure;
   never claim that it was posted.
6. **Record the receipt.** After posting, append a `## POSTED` block with the
   UTC timestamp, channel, and permalink for every top-level message and reply.

### Quality bar

- State every user-visible change as **Was → Now**.
- Use plain language for the user-facing message and technical language only
  where the rubric calls for a developer-facing message.
- Keep exactly one idea in each top-level punch. Replies carry the detail.
- Describe a revert honestly.
