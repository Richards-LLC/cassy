# Exploration matrix builder

The matrix tests the demo and the nearby risks rather than replaying a report.

1. Name the build (binary version or commit SHA), the one-line scope and the
   surface named by the task. Row 1 is the demo statement's happy path.
2. Enumerate state × action × condition from the feature's code and visible
   contract. Add at least three conditions the statement did not mention (an
   empty state, a failure or timeout, a second visit, phone/resize,
   keyboard-only use) and at least one adjacent surface (the list beside a
   form, the status line after a command).
3. Score consequence, change proximity and whether the condition was ever
   exercised. Keep the highest-risk cells, cap the matrix at eight, and leave
   zero replay cells after row 1. A cell stretched into a disguised happy path
   does not count toward the quota.
4. Before the run, give each cell a stable id (`M01`, `M02`, …), a user action,
   an expected result in the user's words, the evidence label it needs and a
   capture filename. Record unchosen risks that matter in the Honesty section.
5. Run cells in order against the real build and capture after the state
   settles.
