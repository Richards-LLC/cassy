# 2026-08-17 — Cloud-sync integrity, cloud-push parking, Commander pairing, CLI help — Slack draft

Channel: `#cas-internal` (`C0B44GUKDK2`)
Deploy target: **Live on production** (merged to `main`)

## User thread

**Top-level:**

> **Live on production — User:** Cloud sync and the Commander both stopped lying to you — closed work stays closed, and a reachable machine that just needs re-pairing now says so instead of looking dead.

**Reply:**

> **Was →** A cloud sync could quietly resurrect tasks you closed weeks ago — closed incidents popping back open at the top of your ready queue with no explanation. **Now →** Closed stays closed unless the other side recorded an explicit reopen; every status change a sync applies carries a note saying exactly which sync did it and what the prior state was, and the sync prints a short summary of what it changed instead of staying silent.
>
> **Was →** A cloud push that the server permanently rejected retried forever and buried you in repeated error spam. **Now →** Permanently rejected items are parked on the first refusal with one concise reason line; `cas cloud queue --verbose` shows the detail, and retrying stays a deliberate command rather than an endless loop.
>
> **Was →** The web Commander showed a machine as offline when its pairing had merely expired, with the browser console filling up with errors. **Now →** A reachable machine that needs pairing shows "Machine needs pairing" with a Re-pair button; genuinely offline machines still show as offline.
>
> **Was →** `cas --help` had stale descriptions and hid the cloud commands entirely. **Now →** Help matches what the commands actually do, `cas cloud sync` is discoverable, and internal maintenance tools no longer clutter the list.
>
> **Was →** Install and update links pointed at the repo's old home. **Now →** Everything points at its real home, Richards-LLC/cassy.

## Dev thread

**Top-level:**

> **Live on production — Dev:** Pull-side cloud sync now enforces a terminal-status guard with provenance-tagged transitions, and push-side permanently parks itemized project/scope mismatches instead of re-queueing them (PR #457).

**Reply:**

> **Was →** Pull apply was last-writer-wins in both the personal and team paths, so a stale remote row could regress Closed/Cancelled to an active status invisibly. **Now →** Terminal states only reopen when the replicated history carries an explicit reopen event newer than the close; every sync-applied transition appends a machine-tagged provenance note (sync id, source, prior status), and the CLI/JSON receipts summarize per-project from→to transitions — silent when nothing changed. (PR #457)
>
> **Was →** Push treated permanent server rejections (project mismatch, scope mismatch) like transient failures — unbounded retry. **Now →** Itemized permanent rejections park at first refusal with reason/count/sample rendering; canonical project-ID case-folding is confined to the cloud boundary, while the shared repo-context selector stays case-preserving. (PR #457)
>
> **Was →** The hub's `/v1/health` withheld CORS from any unpaired origin, so the hosted Commander couldn't distinguish "up, unpaired" from "down", and its connection flow silently retried auth forever. **Now →** Health (GET + preflight) serves CORS to the reviewed hosted Commander origin pre-pairing — all other origins unchanged — and the frontend maps an opaque auth-stage failure after a successful health probe to a terminal needs-pairing state with a Re-pair action.
>
> Also landed on main today: repo references repointed to Richards-LLC/cassy across install/release scripts, Homebrew, and all three builtin harness mirrors (parity gate green); a clap help-text audit (cloud unhidden, internal tools hidden, descriptions corrected); the PTY Esc/Ctrl-C tests moved from 2s polling deadlines to event-driven waits with a 20s ceiling (was flaking on loaded runners); declared MSRV corrected from 1.85 to 1.88; README knowledge-system section expanded with source-verified claims.

## POSTED

- **When (UTC):** 2026-08-17 ~18:42
- **Channel:** `#cas-internal` (`C0B44GUKDK2`)
- **User top-level:** https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786992111280189 (`ts 1786992111.280189`)
- **User reply:** https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786992118522749
- **Dev top-level:** https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786992122225119 (`ts 1786992122.225119`)
- **Dev reply:** https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786992130460889
