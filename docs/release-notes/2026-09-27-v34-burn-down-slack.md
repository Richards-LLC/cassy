# Slack draft — Cassy v34 burn-down runtime release

Channel: #cas-internal (`C0B44GUKDK2`). Transport: MechaCassy hub/bot only.

**Status:** Draft. The version is set at cut time: add `vX.Y.Z` to both
top-level labels then. Do not post until the tagged GitHub workflow publishes
both assets, the release report is verified, and `release-published-receipt.sh
--write-draft` replaces both checksum placeholders below.

## User thread

**Top-level:**

```text
*Live on production — User — Cassy*
Was: the hub could die and a network switch could strand Commander. → Now: both recover by themselves, and every message arrives once.
```

**Only reply:**

```text
*Staying connected*

• *Hub stays up* — Was: a browser closing mid-reply could stop the hub, and it stayed down. → Now: the hub carries on, and the system service restarts it if it ever stops.

• *Network switches* — Was: moving between Wi-Fi, cellular or a VPN could leave Commander stuck or lose a message. → Now: it reconnects without a reload and sends each waiting message exactly once.

• *Phone after sleep* — Was: a phone waking from a long sleep could read as signed out and go dark. → Now: it signs in again by itself and stays paired.

• *Session blips* — Was: a message sent while a session briefly lost its link read "Not sent". → Now: it waits in the conversation and goes out once the session is back.

• *Messages that wait too long* — Was: after two minutes a waiting message told you to re-pair this device, while the line below still promised it would go out. → Now: it says the session didn't come back, with a Retry.

• *No double messages* — Was: a message sent again after a reconnect could arrive twice. → Now: it arrives once.

*Conversations*

• *Failed sends* — Was: failed messages piled up and crowded out the conversation. → Now: swipe one away or tap its ×, and a small chip brings them back.

• *Old questions* — Was: a pinned question stayed open and took up the screen. → Now: it folds to a one-line bar you can dismiss, and a question still waiting on you keeps its choices.

• *Delivery status* — Was: "Not confirmed" lingered and blamed the hub after the reply had come. → Now: it settles once the reply arrives, shows Delivered when the receipt lands, and offers a quiet Send again.

• *Blockers* — Was: a blocker you had answered stayed alarm red. → Now: it quiets and says "Acknowledged — you replied".

• *Take control* — Was: a refused Take control repeated its reason in several places. → Now: it says why once, plainly, and waits for the other device.

• *Files* — Was: a file that failed to open left an empty tab. → Now: its card says it couldn't open, and no tab is left behind.

• *Earlier messages* — Was: loading earlier messages lost your place. → Now: you stay where you were reading.

*Finding your way*

• *Names* — Was: headers and lists led with machine names and could cut a session's name to a sliver. → Now: they lead with the project, and names stay readable at any width.

• *Fleet overview* — Was: it read like a debug page, and projects that repeated looked identical. → Now: it reads as a product page, and repeated projects carry a short name tag.

• *Same session on two machines* — Was: in the Fleet overview, one session name running on two machines showed as two identical rows. → Now: each row carries a short machine tag that stays whole at any width.

• *Connection status* — Was: the header could say "live" next to Degraded, or "Status unavailable". → Now: it says Checking… and then the machine's real state, and Degraded clears once the machine is back.

• *Keyboard* — Was: focus fell to the page after pairing or opening a conversation. → Now: it lands where you were going.

• *Pairing from a phone* — Was: pairing a new machine from a phone left you on the list. → Now: it opens that machine's conversation.

• *Shortcuts and narrow screens* — Was: the palette shortcut always read ⌘K, and a narrow header could hide Back. → Now: it reads Ctrl K or ⌘K for your platform, and the title gives way first.

*Reviews*

• *No review bypass* — Was: a change could be merged on GitHub without its independent review. → Now: the review's verdict is required first.

• *Fair reviews* — Was: a review could fail a change for problems that were already there. → Now: it judges only what changed, and older problems become follow-ups.

Update with `cas update`. Published Linux x86_64 archive SHA-256: `{{LINUX_SHA256}}`. Published Apple Silicon macOS archive SHA-256: `{{MACOS_SHA256}}`.
```

## Dev thread

**Top-level:**

```text
*Live on production — Dev — Cassy*
Was: SIGPIPE killed long-lived servers and GitHub merges could skip independent QA. → Now: servers survive gone peers, held sends deliver once, QA can't be bypassed.
```

**Only reply:**

```text
*Hub, daemon and protocol*

• *SIGPIPE* — Was: `main` sets SIGPIPE to SIG_DFL, so a `writev` to a gone peer killed the hub, MCP server, bridge and factory daemon. → Now: long-lived servers ignore it and drop only that peer.

• *Hub service* — Was: the systemd unit left a signal-killed hub down. → Now: it restarts after signal deaths, and doctor flags an inactive service.

• *upstream_unavailable* — Was: a missing daemon upstream was refused as `forbidden`. → Now: an audited, retryable `upstream_unavailable`; the hub closes the session stream and the page re-holds the send and resends it once.

• *Held-send retries* — Was: while the daemon link stayed down the page resent about once a second, and a legacy-socket send after a refusal ended Not confirmed. → Now: retries back off 1, 2, 4, then 8 s, and later legacy sends are re-held.

• *Held-send expiry* — Was: the expired bubble fell through to "re-pair this device" and the composer still said "will go out by itself". → Now: "The session didn't come back while it waited", Retry, and the composer points at the message.

• *SendMessage dedupe* — Was: a resent SendMessage could queue twice. → Now: the daemon answers a repeated `client_ref` with its first receipt and queues it once.

• *DPoP* — Was: every 401 read as a revoked pairing, so proofs signed before a long idle darkened the phone. → Now: a refused proof is retried with a fresh one, and the audit row records the denial reason.

• *Welcome decode* — Was: an older client failed to decode a Welcome that advertised an unknown capability. → Now: unknown capabilities are ignored.

• *Hub logs and audit* — Was: a detached `cas hub serve` traced into the project it started from, and a failing audit writer was silent. → Now: traces go to `~/.cas/hub/logs`, and `cas hub status` and doctor show audit-writer failures.

*hub-web*

• *Network switches* — Was: a half-open socket after a route change stuck the connection, and a send into it vanished. → Now: wake and online events probe the socket and replace it, and sends are held while it is in doubt (HUB-J12).

• *Failed sends and questions* — Was: refused sends stacked up and a pinned question stayed open. → Now: dismiss by swipe or ×, restore from a chip; the question folds, and it retires only on evidence it was answered.

• *Receipts* — Was: Not confirmed outlived the supervisor's reply and a late receipt never showed Delivered. → Now: both settle correctly, and a settled card offers a quiet Send again.

• *Identity and layout* — Was: machine-first rows, codename slivers, and a Terminal header that clipped Back or ⌘K. → Now: project-first everywhere, the machine name yields first, and the title gives way before Back or ⌘K.

• *Status and focus* — Was: the header chip said "live" beside Degraded, and focus dropped to body after pairing, Load earlier or opening. → Now: Checking… then the machine's state, and focus lands on the control or conversation.

• *Fleet overview* — Was: a debug page whose plot rows repeated the same project label. → Now: a product page with the shortest distinct codename tag on repeated projects.

• *Fleet twin tags* — Was: one codename on two machines fell back to the full machine label and collapsed to " · <codename>". → Now: rail initials, a short name prefix ("Atl"/"Att") or an ordinal ("BS1"), capped at 12 characters so the tag stays whole in the 132px column.

*QA and close gates*

• *GitHub merges* — Was: a raw GitHub merge or a taskless worktree merge could land without the independent QA verdict. → Now: both are held to it (GH #1023, #1024).

• *Delivery-scoped QA* — Was: reviewers rejected for defects already on the page. → Now: only delivery regressions or unmet acceptance reject; pre-existing findings become linked follow-ups.

• *QA dispatch* — Was: user-facing parks without a demo skipped QA. → Now: they get it, supervisors can request it, and reviewers get a credential and account-capacity preflight.

• *Evidence gate* — Was: local auth was the only accepted proof. → Now: deployed authenticated staging runs count when local auth is impossible, and visual-qa captures declared interaction, loading and error states.

• *visual-qa parsing* — Was: OKLCH colours, single-source JSON and non-content surfaces tripped the gate. → Now: all three are handled, and final Expect outcomes are counted (GH #1013, #1017, #1025, #1027, #1037).

• *Close checks* — Was: a hunk the task itself rewrote read as DELIVERY CONTENT DROPPED, and per-task delivery branches confused B2 and the pre-close check. → Now: both are right, and the refusal names the missing lines (GH #1040).

• *Receipts at close* — Was: merge-tip proof, epic headlines and target-sync receipts could miss the real delivery. → Now: all three follow the task's commits and targets (GH #895, #1018, #1028, #1038).

*Config and orchestration*

• *Config keys* — Was: `cas config get/list` missed most `factory.*` keys, including `epic_base_branch`. → Now: every settable key round-trips, and an unset `artifacts_root` reads as its default (GH #1011).

• *Workers* — Was: shared-clone supervisors could reap each other's workers, and a worker parked on a decision slept through it. → Now: neither happens, and approved verification commits wake the worker (GH #1036).

• *Liveness and reminders* — Was: Codex workers read as dead after MCP reparenting, and cross-session reminders could reach the wrong recipient. → Now: both are fixed, and the rolling sweep finds its runner (GH #1039).

Validation: the full nextest suite plus doctests, and all twelve hub journeys at the assembled head. Published `cas-x86_64-unknown-linux-gnu.tar.gz` SHA-256: `{{LINUX_SHA256}}`. Published `cas-aarch64-apple-darwin.tar.gz` SHA-256: `{{MACOS_SHA256}}`. INTERVENTIONS={{INTERVENTIONS}} GREEN_TO_PIPELINE_SECS={{GREEN_TO_PIPELINE_SECS}} MERGED_TO_PUBLISHER_SECS={{MERGED_TO_PUBLISHER_SECS}} GREEN_TO_PUBLISHED={{GREEN_TO_PUBLISHED}}
```

## Posting sequence

1. Land the release commit through the protected-main PR, then tag the fetched
   landing and publish using the release train.
2. Verify both published archives, installation, the release report and the
   latency receipt. Use `release-published-receipt.sh --write-draft` for both
   digests, and fill the Dev trailer from the latency receipt.
3. Post the User parent and reply, then the Dev parent and reply through
   MechaCassy. Append returned timestamps and permalinks below.

## POSTED

Channel: `#cas-internal` (`C0B44GUKDK2`).

| Message | UTC timestamp | Permalink |
| --- | --- | --- |
| User top-level |  |  |
| User reply (Was → Now) |  |  |
| Dev top-level |  |  |
| Dev reply (Was → Now) |  |  |
