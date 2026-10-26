# Two-machine Commander rig

Some Commander QA needs one real browser paired to two real machines: usage
counts reaching both hubs, switching machines, or a machine that reconnects
while the other stays live. The reusable rig for that is in
[`scripts/qa-rig/commander-two-machine/`](../../scripts/qa-rig/commander-two-machine/README.md).

It runs a candidate `cas hub serve` and an inert-supervisor factory daemon on
soundwave and on prowl, each in its own saved `HOME` beside the live hub, on
listener `127.0.0.1:4273` and Tailscale Serve `:8443`. A persistent Chromium
profile on soundwave stays paired to both, so `rig.sh start` brings the whole
rig back without re-pairing, and `rig.sh stop` frees the ports and `:8443`
mappings while keeping the state. The README lists both machines' state paths,
hub URLs, the pairing origin and how to load a new candidate binary.

The rig never touches the live hubs' `~/.cas/hub`, their `:443` mappings or
any project store: `rig.sh` clears inherited `CAS_*` variables before it starts
anything.
