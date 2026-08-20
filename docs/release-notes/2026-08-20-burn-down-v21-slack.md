# 2026-08-20 — Burn-down v21 afternoon wave — Slack draft

Channel: `#cas-internal` (`C0B44GUKDK2`)
Covers main merges: PR #551, #552, #554–#565.

## User thread

**Top-level**

> **Live on production — User**
> Cassy's memory now speaks up when it knows something relevant, Viktor works with one pasted key, Macs install with one command — and eleven long-standing annoyances are gone in one afternoon.

**Reply**

> **Was:** Cassy could hold the exact answer you were rediscovering and stay silent about it; getting Viktor working needed hand-provisioning; Macs had to build from source; a helper whose work fell out of the merge line waited forever without telling anyone; the dashboard listed long-dead projects as live; and closing a big project could time out with no way to tell if it actually closed.
> **Now:** stored knowledge surfaces on its own when your work matches it — and every silence is now explainable, not a mystery; `cas viktor key` plus a key from the operator is the whole Viktor setup; Macs on Apple Silicon install with the standard one-liner; dropped work announces itself immediately; the dashboard shows only genuinely live projects; and big closes come back in about a second with a clear receipt.

## Dev thread

**Top-level**

> **Live on production — Dev**
> Eleven issues closed in one wave: ambient recall reads tool traffic with per-turn decision traces, terminal task states are audit-trailed against LWW resurrection, merge-queue ejections push durable relays, and the fork-PR approval gate seals the self-hosted runner.

**Reply**

> **Was:** ambient recall's trigger read only prompt text and Write/Edit paths, silently starving on sessions whose signals lived in tool results (#553); cloud pull's LWW reconcile could flip Closed epics back to Open with no actor trail (#516); a PR ejected from the merge queue left its task in awaiting_merge forever; the docs-only CI short-circuit skipped the Rust tier for markdown compiled into the binary; installs rejected macOS; concurrent cas-updates reset the shared build worktree mid-build; worktree GC stranded 1900+ symlinks producing false-green jest runs; bare-/tmp writes were advisory-only; epic close rendered a 200KB narrative synchronously and timed out ambiguously at 55s.
> **Now:** recall ingests bounded, redacted tool-traffic terms with source-attributed decision traces and a strong-signal injection floor (#560, #562); terminal status changes require an attributed reopen and unattributed remote reopens park in the conflict journal exactly once (#556); queue ejections push episode-keyed durable relays to supervisor and worker (#561); everything under cas-cli/src/ is rust-affecting with a mutation-proof guard (#563); cas-install.sh handles Apple Silicon with Gatekeeper quarantine clearing and the from-zero doc gets a binary fast path plus the documented team-membership flow (#564); cas-update takes an atomic holder-visible lock (#554); GC refuses removal when external symlinks resolve into the worktree (#555); a configured scratch root is PreToolUse-enforced with bare-/tmp denial preserved (#557); epic close is commit-then-respond with a compact receipt and timeout messages state whether the write landed (#565); Viktor is one `cas viktor key` away (#552, #559); org fork-PR approval covers all external contributors (#551); Commander panes derive epic liveness from task status (#558).

## POSTED

- UTC: 2026-08-20T16:55Z (all four messages)
- Channel: `#cas-internal` (`C0B44GUKDK2`)
- User top-level: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787244881332049 (`ts 1787244881.332049`)
- User reply: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787244887911929
- Dev top-level: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787244893101729 (`ts 1787244893.101729`)
- Dev reply: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787244902264639
