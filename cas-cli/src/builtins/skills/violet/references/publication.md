# Publication: no share before verification

A deliverable attributed to an epic is not shared before its verification
passes. While that epic's verification tasks are open, a Cassy pre-tool check
refuses an attributed `violet_post` that shares a file (`kind: "file"`,
`kind: "file_external"`, or a `kind: "thread"` reply with `files`), and any
post marked `deliverable: true`, with `verification_pending`, and lists those
tasks.

A "final check still running" caveat is not enough: a retraction costs more
than the wait. Wait for verification, or for the operator's own
`PUBLICATION OVERRIDE:` decision note on the epic. The override holds for six
hours, and every post it lets through is logged on the epic.

The epic comes from the post's `epic_id`, the parent of its `task_id`, or its
artifact record's owning task. Session focus alone does not attribute an
operational report to that epic. Unattributed files pass this verification gate;
each attributed file in a batch waits for its own epic's verification.

For `cas violet post --file`, an explicit `--epic` wins. Otherwise, with an epic
in focus and every attachment a report PDF for the same version, the CLI selects
that version's merged release PR and matches its head branch to one stored epic. PDFs named
`v<version>.pdf` under `release-reports/` are recognized; custom output directories
also work when the generated Markdown companion remains beside the PDF.
The released epic's own open verification still blocks its report. If a
recognizable generated report has mixed attachments, malformed source metadata
or no unambiguous released epic, the CLI retains the focused release's
verification hold. Ordinary operational PDFs, including version-looking names
without generated-report evidence, remain unattributed.

Pass `--epic <released-epic-id>` when automatic inference is unavailable or
attachments mix report versions, or name `epic_id` on a direct Violet call.
Thread uploads also use explicit attribution. Release-note announcements retain
the separate post-release check, including its focused-epic context. Keep the
session focused on its current work.
