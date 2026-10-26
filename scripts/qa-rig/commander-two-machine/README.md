# Two-machine Commander QA rig

A saved, restartable setup for QA that needs one real Commander browser
paired to two real machines (soundwave, Linux, and prowl, macOS). Each
machine runs a candidate `cas hub serve` and a factory daemon with an inert
supervisor, beside the operator's live hub, and never instead of it.

| Part | soundwave | prowl |
| --- | --- | --- |
| Rig state (`RIG`) | `/mnt/rewind/cas/scratch/qa-rig/commander-two-machine` | `~/.cas/qa-rig/commander-two-machine` |
| Hub listener | `127.0.0.1:4273` | `127.0.0.1:4273` |
| Hub URL (Tailscale Serve, tailnet only) | `https://soundwave-linux.tailf5a734.ts.net:8443` | `https://prowl.tailf5a734.ts.net:8443` |
| Daemon session | `commander-rig` | `commander-rig` |
| Usage ledger | `$RIG/home/.cas/hub/commander-usage.json` | same |

The live hubs keep `127.0.0.1:4173`-style listeners and Tailscale Serve
`:443`. The rig only adds and removes its own `:8443` mapping.

## Layout of `$RIG`

- `bin/cas`: the candidate binary (copy it in; `rig.sh` refuses to start
  without it). `bin/claude` is the inert supervisor the daemon launches.
- `home/`: the rig's private `HOME`. Its hub state, pairing credentials and
  `commander-usage.json` are in `home/.cas/hub`, never the live `~/.cas/hub`.
- `project/`: the Git project the daemon serves; its store is `project/.cas`.
- `logs/`, `run/`: process logs and pid files.
- `browser-profile/` (soundwave only): the persistent Chromium profile that is
  paired to both hubs.

Keep `RIG` short: the daemon's socket is `$RIG/home/.cas/factory-commander-rig.gui.sock`
and must leave room for the terminating NUL in the OS address (104 bytes on
macOS, 108 on Linux). Startup now refuses oversized paths before logging,
spawning a supervisor or registering agents. `rig.sh` clears every inherited
`CAS_*` variable, so a factory shell's `CAS_ROOT` can never point the rig
daemon at a live store.

## Use

From soundwave, with `RIG` set to the soundwave path above:

```bash
scripts/qa-rig/commander-two-machine/rig.sh start    # both machines, from saved state
scripts/qa-rig/commander-two-machine/rig.sh status
node scripts/qa-rig/commander-two-machine/browser.mjs open https://soundwave-linux.tailf5a734.ts.net:8443/
scripts/qa-rig/commander-two-machine/rig.sh stop     # frees ports and :8443 mappings, keeps state
```

`start` runs `local-start` here and over `ssh prowl` (the same script, read
from stdin). No re-pairing is needed: the browser profile keeps both machines.
`browser.mjs walk <url>` drives several Commander journeys and reloads, so
both machines receive the usage counts; `OUT=<dir>` takes its screenshots.

## Pairing (first time only, or after deleting state)

Commander is served by the soundwave rig hub, so the controller origin for
both invitations is `https://soundwave-linux.tailf5a734.ts.net:8443`.

```bash
rig.sh pair https://soundwave-linux.tailf5a734.ts.net:8443          # on soundwave
ssh prowl 'RIG=$HOME/.cas/qa-rig/commander-two-machine bash -s -- pair https://soundwave-linux.tailf5a734.ts.net:8443' < rig.sh
node browser.mjs pair <soundwave invitation url> <prowl invitation url>
```

Each invitation is one-time and expires after 10 minutes, with the default
read-only scopes `machine:read`, `session:read`, `pane:read`. Never commit an
invitation URL or the profile.

## Binaries

Build the candidate for the revision under test: on soundwave the supervisor
runs `cas qa candidate build --task <id> --rev <sha>`; on prowl build it in a
scratch clone (`~/scratch/<task>`, `ZIG` pointing at Zig 0.15.2), never in the
main checkout. Copy each into its machine's `$RIG/bin/cas` and restart.
