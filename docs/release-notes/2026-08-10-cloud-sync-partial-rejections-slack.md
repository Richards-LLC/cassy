# Release notes — cloud sync accepts partial rejection itemization (2026-08-10)

Channel: `#cas-internal` (`C0B44GUKDK2`). Merged to `main` → **Live on production**.

=== MESSAGE 1 (user top-level) ===
Live on production — User: cloud sync no longer gets stuck retrying the same batch forever when the server turns away a few items — the rest of your data now syncs through.

=== MESSAGE 2 (user reply, thread of 1) ===
Was: if the sync server declined even a few items in a push batch (for example items that already exist under a different scope in the cloud), the whole batch was marked failed and re-queued, so every `cas cloud sync` run repeated the identical failure and nothing ever drained. Now: only the specifically declined items are marked failed, each with its reason visible in `cas cloud queue --verbose`, and everything else in the batch syncs and settles. Genuinely malformed server responses are still treated as a full-batch retry rather than guessed at.

=== MESSAGE 3 (dev top-level, NEW top-level, not a reply) ===
Live on production — Dev: the push client's itemized-rejection validator now accepts partial itemization (rejected ⊆ skipped) instead of demanding an exact count match.

=== MESSAGE 4 (dev reply, thread of 3) ===
Was: `itemized_rejections_for` required `rejected.len() == skipped`, but the server computes skipped as sent − inserted − updated and itemizes only genuine identity collisions — benign same-scope stale/no-op writes are counted but never itemized — so any batch containing both kinds made the response "invalid" and the caller fail-closed the entire sub-batch back onto the queue. Now: the validator accepts `rejected.len() <= skipped`; named rows terminal-fail with reason and existing canonical id, unnamed skips settle. Over-count, unknown-id, and duplicate-id itemizations still fail closed. Regression tests pin the exact production shapes (6 rejected / 20 skipped personal; 14 / 17 team). A server-side follow-up is tracked for scope-aware uniqueness: the cloud primary key omits scope, so a personal-scope row collides with the same task pushed team-scope, surfacing as scope_mismatch with an identical canonical id.
