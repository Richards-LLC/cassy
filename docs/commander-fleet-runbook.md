# Commander fleet setup runbook

This is the per-machine operating procedure for the Commander hub. Repeat it on every machine that will appear in the browser catalog. Commander traffic stays direct over the tailnet; Cassy Cloud discovery is optional and supplies untrusted endpoint hints only.

## Preconditions

- Install the same published `cas` version on every machine and put the binary at a stable absolute path.
- Install Tailscale, join every machine and browser device to the same tailnet, enable MagicDNS and HTTPS certificates for the tailnet, and confirm `tailscale status` reports `Running`.
- On Linux, authorize the account that runs the hub to operate Tailscale without sudo: `sudo tailscale set --operator="$USER"`. Verify it with `sudo tailscale debug prefs`; `OperatorUser` must equal that account. Without this one-time host setting, Cassy reports `permission-denied` and keeps the hub loopback-only.
- Choose one machine URL as the browser profile's controller origin. Pair every other machine to that exact origin; changing it requires re-pairing.
- The default controller origin is a paired hub. The hosted static origin is `https://hub.petrastella.io`, an optional explicit trust grant: before using it, verify the pinned `hub-web/dist` commit/digest and WASM hashes (see [Hub promotion](#hub-promotion-hosted-commander-at-hubpetrastellaio)), then create new invitations with `cas hub pair --origin https://hub.petrastella.io` on every target. Revoke old-origin devices and re-pair; never copy browser storage or credentials between origins.
- Do not expose port 4173 on a LAN interface. The Cassy hub remains on `127.0.0.1`; Tailscale Serve is the TLS terminator.
- In Chrome, allow **Local network access** for `https://hub.petrastella.io` in the page's site settings when connecting to a tailnet hub; Tailscale's `100.64.0.0/10` addresses are [classified as local by Chromium](https://chromium.googlesource.com/chromium/src/+/d1e9879b75be1e3ef0f9b9991f6831dca5a618f8), so a denied permission blocks requests before they reach the hub and does not mean the browser needs pairing again.

## Hub promotion (hosted Commander at hub.petrastella.io)

Standing policy: after any `main` merge that changes `hub-web/dist`, or hub-web
source that implies a dist rebuild, promote that dist to
`https://hub.petrastella.io`. A supervisor treats such a merge as carrying the
promotion duty. The hosted origin is never left pinned behind `main`.

The pin and its record live in `Richards-LLC/petra-stella-cloud`, not in this
repository:

- Current pin and deployment record: [`hub-static/PROVENANCE.md`](https://github.com/Richards-LLC/petra-stella-cloud/blob/main/hub-static/PROVENANCE.md).
  It names the source commit, dist tree, verify-dist digest, `app.js` and
  `app.css` hashes, Vercel deployment ID and rollback anchor. It is the only
  authority for what the hosted origin serves; this runbook keeps no copy.
- Verifier: [`hub-static/scripts/verify-dist.sh`](https://github.com/Richards-LLC/petra-stella-cloud/blob/main/hub-static/scripts/verify-dist.sh), run as `CAS_SRC_DIR=<pinned cas-src checkout> scripts/verify-dist.sh` from `hub-static/`.
- One-time origin setup, domain attachment, deployment protection and rollback:
  [`hub-static/GO-LIVE.md`](https://github.com/Richards-LLC/petra-stella-cloud/blob/main/hub-static/GO-LIVE.md).
  A routine promotion does not repeat those steps.

Vercel does not deploy `main` by itself. A routine promotion is:

1. **Pin.** On a petra-stella-cloud branch, copy `hub-web/dist/` from a clean
   cas-src checkout of the exact `main` commit (normally the release tag) into
   `hub-static/public/commander/`, and copy its `index.html` to
   `hub-static/public/index.html`.
2. **Verify.** Run `verify-dist.sh`. It recomputes the dist digest and the two
   WASM integrity hashes. The hashes in [`hub-web/README.md`](../hub-web/README.md)
   are the authority: `ghostty-vt.wasm` `6b1df1a9…3f7e` and
   `ghostty-write-pty.wasm` `75cb147e…d3d903`. Stop on any mismatch.
3. **Record and merge.** Update `PROVENANCE.md` with the commit, dist tree and
   digest, then open a petra-stella-cloud pull request and let it auto-merge.
4. **Deploy.** From a clean checkout of petra-stella-cloud `origin/main`, run
   `vercel deploy hub-static --prod --scope richards-llc --project cas-hub-static --yes`.
   Use only the `cas-hub-static` project in the Richards-LLC team, never the
   `petra-stella-cloud` Vercel project. Never create a git-sourced deployment:
   the project has no root directory set, so it would build the repository
   root and serve NOT_FOUND. If the project is in a rolled-back state, promote
   the new deployment.
5. **Check the live bytes.** On both `https://hub.petrastella.io` and the
   deployment's immutable URL, the md5 of `/commander/app.js` and
   `/commander/app.css` must match `hub-web/dist` at the pin, and both WASM
   files must match the pinned hashes. For example,
   `curl -fsS https://hub.petrastella.io/commander/app.js | md5sum`. Every
   route must return HTTP 200 with no redirect to Vercel SSO.
6. **Record the deployment.** Add the deployment ID, URL and rollback anchor to
   `PROVENANCE.md` in a follow-up petra-stella-cloud pull request.

Changing the relay metadata or the origin itself is a security-domain move that
requires every hub to re-pair. A dist-only promotion does not.

Work from a fresh clone or `git fetch` of petra-stella-cloud. A long-lived local
checkout can trail `origin/main`, and then its `PROVENANCE.md` names an old pin.

## Start and verify one machine

Run these commands in order:

```sh
cas --version
tailscale status --json
tailscale serve status --json
cas hub start
cas hub status
tailscale serve status --json
curl --fail --silent --show-error https://MACHINE.TAILNET.ts.net/v1/health
```

`cas hub start` prints the stable HTTPS URL. The health response is intentionally minimal: `schema_version` and `ready`. The private files `~/.cas/hub/tailscale-serve.json` and `~/.cas/hub/tailscale-serve-teardown.json` preserve exact before/after Serve status receipts with mode 0600.

If Tailscale is absent, logged out, lacks Serve permission, or the requested HTTPS port already has another handler, startup prints a refusal and the local hub remains available at `http://127.0.0.1:4173`. Cassy never runs `tailscale serve reset` and never replaces an unrelated handler.

Use a non-default port only when 443 is deliberately assigned elsewhere:

```sh
cas hub start --tailscale-serve-port 8443
```

The corresponding stable URL includes `:8443`.

## Make startup survive logout and reboot

Install a user-level service from the stable, published `cas` binary:

```sh
cas hub service install
cas hub service status
```

Preview the definition and manager actions without writing a unit, changing
launchd/systemd state, or starting the hub:

```sh
cas hub service install --dry-run
```

On macOS this writes and bootstraps the launchd LaunchAgent at
`~/Library/LaunchAgents/dev.cas.commander-hub.plist` with `RunAtLoad` and
`KeepAlive`. On systemd Linux it writes `~/.config/systemd/user/cas-hub.service`,
enables it, starts it, and enables user lingering so it survives logout and
reboot. Both definitions invoke `cas hub serve --bind 127.0.0.1 --port 4173`
and request tailnet-only Tailscale Serve HTTPS by default. They never contain
hub identity, auth state, tokens, or credential paths.

`cas hub start`, `cas hub restart`, service install and `cas update` request
Serve even when the previous hub was loopback-only. Existing HTTPS port choices
are preserved during restart/update. Use `--no-tailscale-serve` for an explicit
loopback-only launch or service install. For a persistent host opt-out, add to
`~/.cas/config.toml` (the hub reads host configuration, not project configuration):

```toml
[hub]
tailscale_serve = false
```

An explicit `--tailscale-serve` overrides this setting for that launch; an
explicit `--no-tailscale-serve` overrides the default. Service definitions encode
the resolved choice so the service child cannot re-enable an opted-out launch.
The next update reapplies the host configuration. Configure this host file with
`CAS_ROOT="$HOME/.cas" cas config set hub.tailscale_serve false --store cas-root`.

After a binary update, Cassy starts a stopped hub if its service is installed
or `~/.cas/hub/machine-id` exists. A machine with neither stays stopped and
prints `hub not running; start with cas hub start`. Start/restart and transport
checks have bounded timeouts. Starting a previously absent hub is best effort:
one attempt records `action=start_failed` and its cause if the service cannot
start, while update continues. Recovery of an existing runtime still fails
if loopback cannot be verified. Missing Tailscale, logout, Serve permission or publication
failure leaves a healthy loopback hub and does not fail the update. Its receipt
records `verified=true`, `loopback_verified=true`, `transport_verified=false`,
a reason in `transport_warning`, and a remedy; run `tailscale status`, then
`cas hub restart` after fixing the cause.

On macOS, the CLI selected through `TAILSCALE`, Homebrew or the app bundle must
be usable by the LaunchAgent. The GUI app and a separately installed tailscaled
can have different login/session state; inspect the actual CLI's `tailscale
status` when publication is unavailable. Installation now attempts Serve and
falls back to loopback instead of refusing launchd publication up front. Check
`cas hub status` for `Tailscale Serve: OK` before relying on phone access.

Service output is written to `~/.cas/hub/hub.log`; `cas hub service status`
reports the manager state, hub health, and this log path.

On systemd Linux, use the port flag when the existing Tailscale HTTPS port is deliberately not
443:

```sh
cas hub service install --tailscale-serve --tailscale-serve-port 8443
```

Do not install from `.cas/worktrees/`; worktrees are disposable and Cassy refuses
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

That default grants read-only scopes: `machine:read,session:read,pane:read`. A
read-only device can watch panes but cannot type into them, send a supervisor
message, or interrupt an agent. To pair a device that controls, name the scopes:

```sh
cas hub pair --origin https://MACHINE-A.TAILNET.ts.net \
  --scopes machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt
```

The invitation is a ceiling, not a request: the exchange refuses any scope the
invitation did not grant, and the link carries its granted scopes so Commander
ticks exactly those. Scopes cannot be widened after the fact — mint a new
invitation with the scopes you want and open that link.

Open the printed fragment URL in the browser on device A. Commander opens the
pairing dialog with the granted scopes ticked and the rest disabled; confirm the
hub URL and labels and complete the pairing before its ten-minute expiry. On
Android, open the link in a new tab: a VIEW intent whose URL differs only by its
`#fragment` lands on the existing tab, and the invitation is never consumed. The
fragment is removed before networking. In browser developer tools, verify:

1. pairing exchange goes directly to machine B's `https://...ts.net` origin;
2. the session list succeeds only after DPoP authentication;
3. WebSocket ticket issuance succeeds and the attach request upgrades at `wss://MACHINE-B.TAILNET.ts.net/v1/sessions/SESSION/attach?...`;
4. reconnecting consumes a new ticket and replaying the prior ticket fails; and
5. the conversation's supervisor messages (and its Raw output) arrive from machine B while machine A remains the controller origin.

Repeat the command on every target. Discovery suggestions from Cassy Cloud never pair, trust, proxy, or add a machine automatically.

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

The foreground hub tears down only its recorded Cassy-owned Serve mapping during
the manager stop. If the live Serve status no longer exactly matches the
recorded Cassy target, Cassy refuses teardown and leaves it untouched for manual
review.

For an upgrade, replace the binary atomically, compare `cas --version`, then
restart the installed service or run `cas hub restart` and repeat the full
start/health/status sequence. Preserve `~/.cas/hub/`; it contains the stable
machine identity and paired-device state. Never delete it as an upgrade step.

### Search index schema upgrades

After installing a release that changes the memory search schema, restart every
running `cas serve` process before checking search health. Cassy stores the
active Tantivy index under a schema-versioned path (for example,
`.cas/index/tantivy-v15`), so an older process may continue using the retired
`.cas/index/tantivy` path without deleting the new index. If `cas doctor` reports
a pre-versioned index, run the reindex maintenance action (`mcp__cas__system action=reindex bm25=true`) from an agent session once; it migrates a compatible
index or quarantines an incompatible one and rebuilds the versioned path.

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
