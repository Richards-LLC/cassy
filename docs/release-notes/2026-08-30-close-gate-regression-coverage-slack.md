# Close-gate regression coverage — Slack draft

Target: `#cas-internal` (`C0B44GUKDK2`)

Deploy target: `Live on production`

Post in the numbered order below. Send only each body, excluding the separator headers.

=== MESSAGE 1 — USER TOP-LEVEL ===

Live on production — User: Regression coverage now protects the difference between an intentional reviewed deletion and genuinely missing delivery.

=== MESSAGE 2 — USER REPLY TO MESSAGE 1 ===

Was → The close-gate behavior already existed, but the exact review flow—parked work, a requested deletion, then merge—was not pinned by a dedicated test. Now → That flow is covered end to end, while a companion check continues to prove that genuinely dropped delivery is rejected.

=== MESSAGE 3 — DEV TOP-LEVEL ===

Live on production — Dev: Close-gate regression tests now pin integrated descendant-tip proof without weakening fail-closed delivery checks.

=== MESSAGE 4 — DEV REPLY TO MESSAGE 3 ===

Was → The integrated-descendant selector and measured-fact rejection wording were already in the runtime, but the post-park deletion path lacked exact regression coverage. Now → PR #629 adds a real-Git fixture for park → delete-on-branch → merge → close, alongside the existing dropped-content rejection test.
