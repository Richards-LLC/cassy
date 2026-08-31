# Close-gate regression coverage — Slack draft

Target: `#cas-internal` (`C0B44GUKDK2`)

Deploy target: `Live on production`

Post in the numbered order below. Send only each body, excluding the separator headers.

=== MESSAGE 1 — USER TOP-LEVEL ===

Live on production — User: When a task is finished, Cassy now proves it can tell a file a reviewer asked to remove apart from work that quietly went missing.

=== MESSAGE 2 — USER REPLY TO MESSAGE 1 ===

Was → That safeguard already existed, but the exact sequence a reviewer actually follows — park the work, ask for a file to be removed, merge, finish — had no dedicated proof. Now → That sequence is proven end to end, and the safeguard still refuses to finish a task whose work has actually disappeared.

=== MESSAGE 3 — DEV TOP-LEVEL ===

Live on production — Dev: Close-gate regression tests now pin integrated descendant-tip proof without weakening fail-closed delivery checks.

=== MESSAGE 4 — DEV REPLY TO MESSAGE 3 ===

Was → The integrated-descendant selector and measured-fact rejection wording were already in the runtime, but the post-park deletion path lacked exact regression coverage. Now → PR #629 adds a real-Git fixture for park → delete-on-branch → merge → close, alongside the existing dropped-content rejection test.

## POSTED

Channel: `#cas-internal` (`C0B44GUKDK2`) · Posted 2026-08-31 via the approved Claude profile route (embargo lifted by the operator on 2026-08-31).

| Message | Slack ts | Permalink |
| --- | --- | --- |
| User top-level | 1788180490.896659 | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788180490896659 |
| User reply (Was → Now) | 1788180495.150219 | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788180495150219?thread_ts=1788180490.896659&cid=C0B44GUKDK2 |
| Dev top-level | 1788180491.382409 | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788180491382409 |
| Dev reply (Was → Now) | 1788180496.234599 | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788180496234599?thread_ts=1788180491.382409&cid=C0B44GUKDK2 |
