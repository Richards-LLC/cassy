# Commander reverse pairing and resilient machine control — Slack release note

## User top-level

Live on production — **User** — Commander can now start pairing a machine from the page, so adding a machine no longer begins with moving a link between devices.

## User reply

**Was → Now:** Pairing required running a command on a target and opening its resulting link on the controller. Now Commander creates a short-lived code from the page for the target to authorize, and cancellation is handled safely if you back out. Replacing a machine credential rotates the prior one, invitations are accepted only by their original Commander page, and clear warnings explain when browser storage cannot finish cleanup. This change is on `main`; it does not claim a release tag or runtime restart.

## Dev top-level

Live on production — **Dev** — Commander reverse pairing now makes the relay boundary and credential lifecycle explicit, with strict origin validation at every handoff.

## Dev reply

**Was → Now:** Reverse pairing had only the command-led entry path and less explicit handling around cancellation, replacement, and unsafe origins. Now the short-lived create/poll/ack exchange is isolated from the direct invitation and machine-control path; cancellation and failed installation roll back persisted state, replacement rotates installed authentication, browser storage fails closed with an actionable warning, and controller, relay, and loopback origins are parsed and normalized strictly. This change is on `main`; it does not claim a release tag or runtime restart.

## POSTED

- Channel: `#cas-internal` (`C0B44GUKDK2`)
- User top-level — 2026-08-13T18:54:16.633089Z: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786647256633089
- User reply — 2026-08-13T18:54:21.894309Z: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786647261894309?thread_ts=1786647256.633089&cid=C0B44GUKDK2
- Dev top-level — 2026-08-13T18:54:25.454769Z: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786647265454769
- Dev reply — 2026-08-13T18:54:30.601269Z: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786647270.601269?thread_ts=1786647265.454769&cid=C0B44GUKDK2
