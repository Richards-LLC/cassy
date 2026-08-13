# Slack draft — Claude routing account correction on main, 2026-08-13

Channel: `#cas-internal` (`C0B44GUKDK2`). Merged to `main` → **Live on production**.

=== MESSAGE 1 (user top-level) ===
Live on production — *User* — The Claude fallback policy on main now recognizes the approved `pippenz@gmail.com` profile instead of rejecting it as the wrong account.

=== MESSAGE 2 (user reply, thread of 1) ===
Was: the routing guidance allowed only `daniel@petrastella.io`, so it blocked the authenticated profile that is actually approved for this environment. Now: the policy on main checks for the exact `pippenz@gmail.com` first-party profile and still refuses every other account, missing profile, or unclear login result. This main landing updates the routing policy source; it does not announce a new tagged CAS runtime or a runtime restart.

=== MESSAGE 3 (dev top-level, NEW top-level, not a reply) ===
Live on production — *Dev* — The source-managed CLI router on main now gates Claude on a one-entry `pippenz@gmail.com` first-party allowlist across all three harness flavors.

=== MESSAGE 4 (dev reply, thread of 3) ===
Was: the embedded Claude, Codex, and Grok `cli-routing` contracts pinned `loggedIn: true` plus `authMethod: "claude.ai"` to `daniel@petrastella.io`, so the approved current first-party profile failed the gate. Now: all three copies require `loggedIn: true`, `authMethod: "claude.ai"`, `apiProvider: "firstParty"`, and the exact `pippenz@gmail.com` email; every other email, missing field, malformed response, or failed probe remains fail-closed, with catalog and flavor-drift tests preventing the copies from diverging. This is a main-branch policy correction, not a tagged runtime release or restart.

## POSTED

Channel: `#cas-internal` (`C0B44GUKDK2`)
Landing: `main` at `25a31a923ca88d1682a2b285901a34d01bb8d12b` — source-policy correction only; no tagged runtime release or restart announced.

| Post | UTC | Permalink |
| --- | --- | --- |
| User top-level | 2026-08-13T15:41:54.028419Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786635714028419 |
| User reply (Was → Now) | 2026-08-13T15:41:58.935309Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786635718935309?thread_ts=1786635714.028419&cid=C0B44GUKDK2 |
| Dev top-level | 2026-08-13T15:42:03.146029Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786635723146029 |
| Dev reply (Was → Now) | 2026-08-13T15:42:09.856329Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786635729856329?thread_ts=1786635723.146029&cid=C0B44GUKDK2 |
