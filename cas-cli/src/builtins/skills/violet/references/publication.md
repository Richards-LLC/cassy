# Publication: no share before verification

A deliverable (a report, a file, any client-bound output) is not shared before
its verification passes. While the epic's verification tasks are open, a Cassy
pre-tool check refuses a `violet_post` that shares a file (`kind: "file"`,
`kind: "file_external"`, or a `kind: "thread"` reply with `files`), and any
post marked `deliverable: true`, with `verification_pending`, and lists those
tasks.

A "final check still running" caveat is not enough: a retraction costs more
than the wait. Wait for verification, or for the operator's own
`PUBLICATION OVERRIDE:` decision note on the epic. The override holds for six
hours, and every post it lets through is logged on the epic.

The epic is the post's `epic_id` or `task_id`, else the session's focused epic.
With neither, or with no open verification, the post is allowed.

For `cas violet post --file`, an explicit `--epic` wins. Otherwise, with an epic
in focus and every attachment a report PDF for the same version, the CLI selects
that version's merged release PR and matches its head branch to one stored epic. PDFs named
`v<version>.pdf` under `release-reports/` are recognized; custom output directories
also work when the generated Markdown companion remains beside the PDF.
The released epic's own open verification still blocks its report.

Missing or ambiguous release/epic evidence and mixed attachments keep the normal
focused-epic gate. Pass `--epic <released-epic-id>` when automatic inference is
unavailable, or name `epic_id` on a direct Violet call. Thread uploads use their
explicit epic or session focus. Keep the session focused on its current work.
