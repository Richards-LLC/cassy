---
name: cas-release-notes
description: Use when a merge reaches staging or main, or when asked to draft or post team release notes, Slack updates or a release-notes rubric; notes for customers or a client go to cas-customer-release-notes.
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
   files. Run `cas --json deliverable pairs --range <prev>..<new>` for the
   released commit range. Use only its verified `pairs[]`: join `directory`
   with `before.jpg` and `after.jpg`, and match `task_id` to the change being
   described. Exit 1 can include good pairs alongside findings for other shots;
   retain those findings and omit `unpaired[]`, unavailable and unlinked changes.
   Never fabricate a missing side. The template's before/after screenshots rule
   also applies to a project rubric copied before that rule existed.
4. **Draft the messages from the rubric.** Follow its deploy-target label,
   audience labels, Was → Now format, thread count, reply count and Slack
   mrkdwn shape. Put one punch in each top-level message and the supporting
   detail in its reply. Write in the target project's voice
   ([DESIGN.md first](../design-spec/references/design-md-first.md)). Do not include internal ticket labels or
   implementation process. Lay each message out with the
   [slack-formatting](../slack-formatting/SKILL.md) skill and pass its
   self-check: name no file unless it is attached to that message, and show
   every time in CT (or labelled ET), never UTC.
5. **Save the draft.** Write the exact postable text to
   `docs/release-notes/<date>-<topic>-slack.md` before posting.
6. **Post in rubric order** through [violet](../violet/SKILL.md),
   steps 3–6, marking each post `release_notes: true` with its `epic_id`
   so the post-release gate reads the right check. Attach each selected pair's
   JPGs to the threaded reply describing that change, Before first, using
   `--reply-file <directory>/<before.jpg> --reply-file <directory>/<after.jpg>`
   after that reply's `--reply` argument (`cas violet thread`). Respect the ten
   files per reply limit (five pairs); with no verified pair, attach nothing.
   Name a capture file only in the reply that attaches it (Violet's attach rule).
   Violet owns preflight, posting order, pacing, upload integrity and failure
   handling. Publish only through the Violet hub; never use
   Claude.ai Slack or a personal Slack connector. If the
   hub cannot complete publication, report the measured failure with the
   partial receipts; never claim that it was posted.
7. **Record the receipt.** violet step 6 appends the `## POSTED` block to
   the saved draft.

Done when the saved draft carries a `## POSTED` block, or a blocked report
names the operation that stopped and keeps the partial receipts.
