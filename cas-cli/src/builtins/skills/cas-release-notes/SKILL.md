---
name: cas-release-notes
description: Use when a merge reaches staging or main, or when asked to draft or post release notes for the team or for customers, Slack updates or a release-notes rubric.
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
2. **Start the post-release check.** If `.cas/config.toml` sets
   `[qa.post_release] command`, run
   `cas qa post-release start --epic <id> --sha <released-sha>` now (a repeat
   for the same SHA keeps its window), and draft while it runs. Post only once
   `cas qa post-release status --epic <id>` reads `clean`, about 30 minutes
   after deploy; Cassy refuses release-notes posts while it is `pending` or
   `failed`. A failed check is an incident: report it to the supervisor and
   post nothing until a fix-forward release's check is clean or the operator
   records a `POST-RELEASE OVERRIDE:` note on the epic. Without that config,
   a production release still waits for the supervisor's manual
   post-release check.
3. **Gather the merge.** Read the merged change set, for example
   `gh pr view <n> --json title,body,commits,files`. Describe what changed for
   the person on the other side of the screen, not an inventory of touched
   files. For each user-facing delivery, read its
   `~/.cas/artifacts/<project-key>/<task-id>/qa/pairs/pairs.json` when it
   exists. The template's before/after screenshots rule decides which images
   go out, including for a project rubric copied before that rule existed.
4. **Draft the messages from the rubric.** Follow its deploy-target label,
   audience labels, Was → Now format, thread count, reply count and Slack
   mrkdwn shape. Put one punch in each top-level message and the supporting
   detail in its reply. Do not include internal ticket labels or
   implementation process. Lay each message out with the
   [slack-formatting](../slack-formatting/SKILL.md) skill and pass its
   self-check: name no file unless it is attached to that message, and show
   every time in CT (or labelled ET), never UTC.
5. **Save the draft.** Write the exact postable text to
   `docs/release-notes/<date>-<topic>-slack.md` before posting.
6. **Post in rubric order** through [violet](../violet/SKILL.md),
   steps 3–6, marking each post `release_notes: true` with its `epic_id`
   so the post-release gate reads the right check. It owns preflight, posting order, pacing, upload integrity and
   failure handling. Publish only through the Violet hub; never use
   Claude.ai Slack or a personal Slack connector. If the
   hub cannot complete publication, report the measured failure with the
   partial receipts; never claim that it was posted.
7. **Record the receipt.** violet step 6 appends the `## POSTED` block to
   the saved draft.

Done when the saved draft carries a `## POSTED` block, or a blocked report
names the operation that stopped and keeps the partial receipts.
