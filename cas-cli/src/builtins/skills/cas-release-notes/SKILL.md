---
name: cas-release-notes
description: Use when a merge reaches staging or main, or when the user asks to draft or post release notes, Slack updates, or a release-notes rubric.
metadata:
  managed_by: cas
---

# Release Notes

Use the project's release rubric as the contract. The procedure is:

1. **Ensure the rubric exists.** Check `docs/release-notes/RUBRIC.md`. If it is
   missing and the project wants release notes, copy this skill's
   `references/RUBRIC-template.md`, fill its project placeholders, and preserve
   every hard rule. If it exists, read it; local additions may tighten the
   contract but may not relax its hard rules.
2. **Gather the merge.** Read the merged change set, for example
   `gh pr view <n> --json title,body,commits,files`. Describe what changed for
   the person on the other side of the screen, not an inventory of touched
   files.
3. **Draft the messages from the rubric.** Follow its deploy-target label,
   audience labels, Was → Now format, thread count, reply count and Slack
   mrkdwn shape. Put one punch in each top-level message and the supporting
   detail in its reply. Do not include internal ticket labels or
   implementation process.
4. **Save the draft.** Write the exact postable text to
   `docs/release-notes/<date>-<topic>-slack.md` before posting.
5. **Post in rubric order** through [mecha-cassy](../mecha-cassy/SKILL.md),
   steps 3–6. It owns preflight, posting order, pacing, upload integrity and
   failure handling. Publish only through the MechaCassy hub; never use
   Claude.ai Slack or a personal Slack connector. If the
   hub cannot complete publication, report the measured failure with the
   partial receipts; never claim that it was posted.
6. **Record the receipt.** mecha-cassy step 6 appends the `## POSTED` block to
   the saved draft.

Done when the saved draft carries a `## POSTED` block, or a blocked report
names the operation that stopped and keeps the partial receipts.
