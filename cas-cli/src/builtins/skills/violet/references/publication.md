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
