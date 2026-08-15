# Commander field-fix wave — 2026-08-15 (hosted page + hub daemon)

Two top-level #cas-internal posts per docs/RELEASE_SLACK_RUBRIC.md. Deployed
via the hub.petrastella.io promotion pipeline and hub daemon restarts; no new
runtime tag (all changes are on `main`, superseding pins recorded in
petra-stella-cloud `hub-static/PROVENANCE.md`).

=== MESSAGE 1 (top-level, user perspective) ===
Commander on hub.petrastella.io actually works everywhere now. This morning it could greet you with a stale look, black terminal panes, machines stuck "retrying dialing," and pairing that silently went nowhere — now the page connects, terminals stream live for observers, and when something is wrong it tells you what and how to fix it.

• Was: terminals rendered black with "Terminal transport problem — forbidden" unless you took control of a session. Now: watching a terminal just works in observer mode; typing still requires taking control.
• Was: machines could sit on "retrying dialing" forever from the hosted page. Now: they connect within seconds.
• Was: pairing a new device could hang at "Creating this browser credential…" with no explanation. Now: after approval the page checks it can actually reach your machine, and if it can't, it says so in plain language — check Tailscale, check Private DNS, grab a fresh code.
• Was: on phones the pair control was a tiny "+" hiding behind Android's gesture bar. Now: a proper "+ Pair" button, big enough to tap, clear of the gesture bar.
• Was: machine names truncated to one letter when a status message got long. Now: the name always wins; status text truncates instead.
• Plus: the drawer explains itself when empty ("No machines paired yet — press + to pair this machine"), panels reflow cleanly from desktop to phone, and the tab finally has an icon.

=== MESSAGE 2 (top-level, dev perspective) ===
The hosted Commander origin exposed two cross-origin regressions the same-origin test setups could never catch — both fixed at the hub, plus a wave of client polish, all live on hub.petrastella.io and the soundwave hub daemon.

• Was: the client's new dial gate does a real CORS fetch of `/v1/health`, but the hub sent no CORS headers on that route (the old client used an opaque `no-cors` ping). From the hosted origin every dial failed. Now: `/v1/health` echoes `Access-Control-Allow-Origin` + `Vary: Origin` for paired origins only; unpaired origins and CLI requests are byte-identical to before.
• Was: every non-resize websocket message required an active control lease — including the `RequestPaneKeyframe`/`ScrollbackRequest` reads the renderer needs to paint at all, so observers got black panes. Now: read-class messages authorize on `pane-read` alone; mutations keep the lease requirement; read audits are labeled `websocket_read`.
• Pairing UX: after cloud approval the dialog probes the invitation hub's `/v1/health` (no-cors, 3s) and renders three distinguishable states — waiting, approved-but-unreachable (with a Tailscale/Private-DNS recovery hint), and exchange-failed with the hub's real error text instead of a generic failure.
• Mobile: the rail reserves `env(safe-area-inset-bottom)` (viewport-fit was already cover), and the pair control is a bordered 44px one-line "+ Pair" button.
• Factory tooling hardened along the way: the delivery-content close gate now falls back to zero-context added-hunk survival when sibling lanes edit adjacent lines of the same file (reverse-apply context drift no longer fabricates "dropped content"); worker context gauges read the harness-reported `model_context_window` instead of assuming 200K (a 1M-window worker at 40k tokens no longer reads "near-limit"); and worker guidance now distinguishes live prompt occupancy from cumulative token totals, ending false "context exhausted" checkpoints at 85% real headroom.
• Deployment provenance: each hosted promotion is pinned (source commit + dist digest posted before promotion) and recorded in `hub-static/PROVENANCE.md`; the hub binary serves the same bundle via `include_bytes!`, and the tailnet origin now serves the favicon too.
