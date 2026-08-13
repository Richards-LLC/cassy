# Commander fleet setup runbook

This is the per-machine operating procedure for the Commander hub. Repeat it on every machine that will appear in the browser catalog. Commander traffic stays direct over the tailnet; CAS Cloud discovery is optional and supplies untrusted endpoint hints only.

## Preconditions

- Install the same published `cas` version on every machine and put the binary at a stable absolute path.
- Install Tailscale, join every machine and browser device to the same tailnet, enable MagicDNS and HTTPS certificates for the tailnet, and confirm `tailscale status` reports `Running`.
- On Linux, authorize the account that runs the hub to operate Tailscale without sudo: `sudo tailscale set --operator="$USER"`. Verify it with `sudo tailscale debug prefs`; `OperatorUser` must equal that account. Without this one-time host setting, CAS reports `permission-denied` and keeps the hub loopback-only.
- Choose one machine URL as the browser profile's controller origin. Pair every other machine to that exact origin; changing it requires re-pairing.
- The default controller origin is a paired hub. The hosted static origin is `https://hub.petrastella.io`, an optional explicit trust grant: before using it, verify the pinned `hub-web/dist` commit/digest and WASM hashes, then create new invitations with `cas hub pair --origin https://hub.petrastella.io` on every target. Revoke old-origin devices and re-pair; never copy browser storage or credentials between origins.
- Do not expose port 4173 on a LAN interface. The CAS hub remains on `127.0.0.1`; Tailscale Serve is the TLS terminator.

## Start and verify one machine

Run these commands in order:

```sh
cas --version
tailscale status --json
tailscale serve status --json
cas hub --tailscale-serve
cas hub status
tailscale serve status --json
curl --fail --silent --show-error https://MACHINE.TAILNET.ts.net/v1/health
```

`cas hub --tailscale-serve` prints the stable HTTPS URL. The health response is intentionally minimal: `schema_version` and `ready`. The private files `~/.cas/hub/tailscale-serve.json` and `~/.cas/hub/tailscale-serve-teardown.json` preserve exact before/after Serve status receipts with mode 0600.

If Tailscale is absent, logged out, lacks Serve permission, or the requested HTTPS port already has another handler, startup prints a refusal and the local hub remains available at `http://127.0.0.1:4173`. CAS never runs `tailscale serve reset` and never replaces an unrelated handler.

Use a non-default port only when 443 is deliberately assigned elsewhere:

```sh
cas hub --tailscale-serve --tailscale-serve-port 8443
```

The corresponding stable URL includes `:8443`.

## Make startup survive logout and reboot

Install a user-level service from the stable, published `cas` binary:

```sh
cas hub service install --tailscale-serve
cas hub service status
```

On macOS this writes and bootstraps the launchd LaunchAgent at
`~/Library/LaunchAgents/dev.cas.commander-hub.plist` with `RunAtLoad` and
`KeepAlive`. On systemd Linux it writes `~/.config/systemd/user/cas-hub.service`,
enables it, starts it, and enables user lingering so it survives logout and
reboot. Both definitions invoke `cas hub serve --bind 127.0.0.1 --port 4173`;
they never contain hub identity, auth state, tokens, or credential paths.

Use the port flag when the existing Tailscale HTTPS port is deliberately not
443:

```sh
cas hub service install --tailscale-serve --tailscale-serve-port 8443
```

Do not install from `.cas/worktrees/`; worktrees are disposable and CAS refuses
that path. The service supervises the exact installed binary used for
`install`. After upgrading that binary, `cas hub restart` or restarting the
service manager picks up the new version; `cas hub service status` reports the
supervision state while `cas hub status` reports the running hub and endpoint.

On non-systemd Linux, `cas hub service install` does not pretend it can
supervise the host. It prints the exact rc-script fallback: run
`cas hub start --tailscale-serve` after networking and Tailscale, then check
`cas hub status`. `cas hub service status` remains explicit that reboot
supervision is manual on these hosts.

After a reboot, repeat `cas hub service status`, `cas hub status`, `tailscale
serve status --json`, and the HTTPS health request. The machine ID and URL must
match the pre-reboot values.

## Pair a target machine from the controller browser

On target machine B, bind the invitation to machine A's exact controller origin:

```sh
cas hub pair --origin https://MACHINE-A.TAILNET.ts.net
```

Open the printed fragment URL in the browser on device A and complete the pairing before its ten-minute expiry. The fragment is removed before networking. In browser developer tools, verify:

1. pairing exchange goes directly to machine B's `https://...ts.net` origin;
2. the session list succeeds only after DPoP authentication;
3. WebSocket ticket issuance succeeds and the attach request upgrades at `wss://MACHINE-B.TAILNET.ts.net/v1/sessions/SESSION/attach?...`;
4. reconnecting consumes a new ticket and replaying the prior ticket fails; and
5. terminal output arrives from machine B while machine A remains the controller origin.

Repeat the command on every target. Discovery suggestions from CAS Cloud never pair, trust, proxy, or add a machine automatically.

Alternatively, choose **Create pairing code** in Commander, then run the shown
`cas hub authorize CODE` command on the target. In both the embedded-controller
and `https://hub.petrastella.io` static modes, only the short-lived create,
poll, and acknowledge exchange goes to the reviewed PSC relay origin
`https://petra-stella-cloud.vercel.app`. The final invitation exchange and all
hub control traffic still go directly from the browser to the target hub. If
Commander reports that page-initiated pairing is unavailable, do not add a
same-origin rewrite or proxy; use `cas hub pair` or deploy a reviewed bundle
whose relay metadata is present.

## Detect mixed versions and capabilities

For each paired hub, inspect authenticated `GET /v1/machine`. Compare `version`, `schema_version`, and `capabilities` across machines. The client must report a mismatch instead of assuming that `tailscale_serve`, `cloud_device_suggestions`, or a daemon protocol capability exists. Upgrade the older machine before enabling controls that depend on a missing capability.

## Safe stop, upgrade, and teardown

Remove service supervision first so it cannot immediately restart the hub. This
does not remove `~/.cas/hub/`, machine identity, paired-device auth state, or
any unrelated Tailscale mapping:

```sh
cas hub service uninstall
tailscale serve status --json
```

The foreground hub tears down only its recorded CAS-owned Serve mapping during
the manager stop. If the live Serve status no longer exactly matches the
recorded CAS target, CAS refuses teardown and leaves it untouched for manual
review.

For an upgrade, replace the binary atomically, compare `cas --version`, then
restart the installed service or run `cas hub restart` and repeat the full
start/health/status sequence. Preserve `~/.cas/hub/`; it contains the stable
machine identity and paired-device state. Never delete it as an upgrade step.

## H5 proof record (2026-08-09)

Executed in the H5 development environment:

- `tailscale version` reported 1.102.2 and `tailscale status --json` reported a running node with MagicDNS.
- The first 8443 attempt, before the Linux operator prerequisite was configured, reported `permission-denied`; the hub remained healthy at `http://127.0.0.1:4173`, `cas hub stop` removed it, and Serve status remained `{}`.
- After `sudo -n tailscale set --operator=pippenz`, the exact 8443 sequence above was run with the port override. `tailscale serve status --json` returned `{}` before startup. `cas hub --tailscale-serve --tailscale-serve-port 8443` printed `https://soundwave-linux.tailf5a734.ts.net:8443/`; hub status reported version 2.54.1 at the same URL.
- Live Serve status contained only the HTTPS 8443 root handler proxying `http://127.0.0.1:4173`. `curl --noproxy '*' --fail --silent --show-error https://soundwave-linux.tailf5a734.ts.net:8443/v1/health` returned `{"schema_version":1,"ready":true}` with HTTP 200 and TLS verification result 0.
- The active ownership receipt was mode 0600 with `created_by_cas: true`, empty `status_before`, and the exact 8443 handler in `status_after`. `cas --json hub stop` returned `tailscale_serve_removed: true`; final Serve status was `{}`, port 4173 had no listener, the active ownership receipt was absent, and the mode-0600 teardown receipt recorded the handler as `status_before` and `{}` as `status_after`.
- Mocked-binary tests executed exact status-before, port-scoped on, status-after, idempotent reuse, conflict refusal, and owned off flows.
- The local hub/auth test suite exercised pairing exchange, five-minute single-use WS tickets, and attach over the same route that Tailscale proxies as WSS.

Deferred H7 operator acceptance (requires machine B plus a second browser/phone and therefore is not claimed by this single-machine development run): follow the pairing section verbatim, capture the remote HTTPS health response and WSS 101 upgrade, verify output from machine B, then reboot B and repeat the stable URL/version/capability checks. Paste those receipts into the H7 two-machine acceptance record. The single-machine HTTPS receipt above is not a substitute for that two-device proof.

The binding security model and complete assembled acceptance invariants are in `docs/specs/2026-08-08-commander-security-architecture.md` (H1-TLS-02, H2-PAIR-02, H2-WS-04, H7-FLEET-02).
