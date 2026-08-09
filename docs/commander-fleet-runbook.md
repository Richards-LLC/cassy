# Commander fleet setup runbook

This is the per-machine operating procedure for the Commander hub. Repeat it on every machine that will appear in the browser catalog. Commander traffic stays direct over the tailnet; CAS Cloud discovery is optional and supplies untrusted endpoint hints only.

## Preconditions

- Install the same published `cas` version on every machine and put the binary at a stable absolute path.
- Install Tailscale, join every machine and browser device to the same tailnet, enable MagicDNS and HTTPS certificates for the tailnet, and confirm `tailscale status` reports `Running`.
- Choose one machine URL as the browser profile's controller origin. Pair every other machine to that exact origin; changing it requires re-pairing.
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

### Linux systemd user service

Create `~/.config/systemd/user/cas-hub.service` with the installed binary's absolute path:

```ini
[Unit]
Description=CAS Commander hub
After=network-online.target tailscaled.service
Wants=network-online.target

[Service]
Type=simple
ExecStart=/ABSOLUTE/PATH/cas hub serve --tailscale-serve
Restart=on-failure
RestartSec=3

[Install]
WantedBy=default.target
```

Then run:

```sh
systemctl --user daemon-reload
systemctl --user enable --now cas-hub.service
loginctl enable-linger "$USER"
systemctl --user status cas-hub.service
```

After a reboot, repeat `cas hub status`, `tailscale serve status --json`, and the HTTPS health request. The machine ID and URL must match the pre-reboot values.

### macOS launchd user agent

Create `~/Library/LaunchAgents/dev.cas.commander-hub.plist` with the installed binary's absolute path and arguments `hub`, `serve`, and `--tailscale-serve`. Set `RunAtLoad` and `KeepAlive` to true, then run:

```sh
launchctl bootstrap "gui/$(id -u)" ~/Library/LaunchAgents/dev.cas.commander-hub.plist
launchctl kickstart -k "gui/$(id -u)/dev.cas.commander-hub"
cas hub status
```

Do not point either service definition into `.cas/worktrees/`; worktrees are disposable.

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

## Detect mixed versions and capabilities

For each paired hub, inspect authenticated `GET /v1/machine`. Compare `version`, `schema_version`, and `capabilities` across machines. The client must report a mismatch instead of assuming that `tailscale_serve`, `cloud_device_suggestions`, or a daemon protocol capability exists. Upgrade the older machine before enabling controls that depend on a missing capability.

## Safe stop, upgrade, and teardown

Stop the service manager first so it cannot immediately restart the hub, then let CAS remove only its owned mapping:

```sh
systemctl --user disable --now cas-hub.service  # Linux, when installed
cas hub stop
tailscale serve status --json
```

On macOS, use `launchctl bootout` for the agent before `cas hub stop`. If the live Serve status no longer exactly matches the recorded CAS target, CAS refuses teardown and leaves it untouched for manual review.

For an upgrade, stop as above, replace the binary atomically, compare `cas --version`, re-enable the service, and repeat the full start/health/status sequence. Preserve `~/.cas/hub/`; it contains the stable machine identity and paired-device state. Never delete it as an upgrade step.

## H5 proof record (2026-08-09)

Executed in the H5 development environment:

- `tailscale version` reported 1.102.2 and `tailscale status --json` reported a running node with MagicDNS.
- `tailscale serve status --json` returned `{}` before the proof, establishing no pre-existing mapping.
- Mocked-binary tests executed exact status-before, port-scoped on, status-after, idempotent reuse, conflict refusal, and owned off flows.
- The local hub/auth test suite exercised pairing exchange, five-minute single-use WS tickets, and attach over the same route that Tailscale proxies as WSS.

Deferred operator acceptance (requires machine B plus a second browser/phone and therefore is not claimed by this single-machine development run): follow the pairing section verbatim, capture the remote HTTPS health response and WSS 101 upgrade, verify output from machine B, then reboot B and repeat the stable URL/version/capability checks. Paste those receipts into the H7 two-machine acceptance record.

The binding security model and complete assembled acceptance invariants are in `docs/specs/2026-08-08-commander-security-architecture.md` (H1-TLS-02, H2-PAIR-02, H2-WS-04, H7-FLEET-02).
