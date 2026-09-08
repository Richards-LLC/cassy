# Exploration matrix builder

The matrix tests the demo and the nearby risks, rather than replaying a report.
Write the expected result in the user's words before running any cell.
The project-agnostic matrix and quota guidance are adapted from
https://github.com/Richards-LLC/cassy/issues/759; PostHog and mobile-device
details from that issue are intentionally outside this skill.

1. Name the build (binary version or commit SHA), the one-line scope, and the
   surface named by the task. Make row 1 the demo statement's happy path.
2. Enumerate state × action × condition from the feature's code and visible
   contract. Add at least three conditions the statement did not mention: an
   empty state, a failure or timeout, a second visit, phone/resize, or
   keyboard-only use. Choose at least one adjacent surface (for example, the
   list beside a form or the status line after a command).
3. Score consequence, change proximity, and whether the condition was never
   exercised. Keep the highest-risk cells, cap the matrix at eight, and leave
   zero replay cells after row 1. Do not stretch a cell into a disguised happy
   path just to meet a quota.
4. Give each cell a stable id (`M01`, `M02`, …), a user action, an expected
   result, a needed evidence label, and a capture filename before the run.
   Record unchosen risks in the ledger honesty section when they matter.
5. Run cells in order against the real build. Capture after the state settles;
   time-box the complete pass at 30 minutes and mark every remaining cell
   `NOT EXERCISED` when the box expires.
