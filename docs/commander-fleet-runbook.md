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

## Choose the macOS Tailscale variant

Inbound **Tailscale SSH** uses tailnet identity and policy rather than SSH keys.
On macOS it requires the open-source `tailscaled` daemon. Installing a CLI
wrapper for `Tailscale.app` does not add this server capability.
See [Tailscale SSH's platform limitations](https://tailscale.com/kb/1193/tailscale-ssh).

| Variant | Connect to the tailnet / Serve HTTPS | Tailscale SSH client | Tailscale SSH server | Management |
| --- | --- | --- | --- | --- |
| Standalone GUI app | Yes | Yes, including `tailscale ssh` | No | Menu bar; in-app Sparkle updater |
| Mac App Store app | Yes | Yes, using ordinary `ssh` to a Tailscale SSH server | No | Menu bar; App Store updates |
| Open-source Homebrew **formula** (`tailscale` + `tailscaled`) | Yes | Yes | Yes, after `tailscale up --ssh` and tailnet SSH policy authorization | CLI; root Homebrew service; `brew upgrade tailscale` |

**Fleet requirement:** prowl needs the Homebrew formula because soundwave must
open an identity-based shell with `tailscale ssh prowl`. Any other macOS fleet
machine accepting Tailscale SSH needs the same variant. A Mac used only as a
client can keep the standalone GUI app; an App Store client uses ordinary
`ssh user@machine` for Tailscale SSH destinations. This restriction concerns
Tailscale's SSH server; the macOS Remote Login service is a separate facility.

The daemon has no menu-bar UI and cannot be managed through macOS VPN settings.
Start it with `sudo brew services start tailscale`, which installs a system
LaunchDaemon that runs before user login. `brew services start tailscale`
without sudo is not the fleet's boot-persistence configuration. Check the
installed service definition and `sudo brew services list`; a non-root
`brew services list` may show `none` for this root-owned service.

Inspect the daemon explicitly while migrating:

```sh
tailscale --socket=/var/run/tailscaled.socket status --json
```

An unqualified Homebrew CLI can still select the GUI's local API when the GUI
variant is installed, even if the open-source daemon is the one you intend to
use. Verify the reported daemon version as well as the CLI's `tailscale
version`; do not infer the active variant from the executable path. Keep the
GUI app stopped when the Homebrew daemon owns the tailnet connection.

### Launchd socket selection

Launchd starts services with a minimal environment and does not inherit shell
exports. On a Mac that has both the GUI app and the Homebrew formula installed,
Cassy can select the brew CLI executable but still reach the stopped GUI
daemon's local API. That API reports no usable tailnet state, so hub status can
say Tailscale is unavailable and Serve publication can fail even while
`tailscaled` is running.

For a direct read-only check against the Homebrew daemon, use:

```sh
tailscale --socket=/var/run/tailscaled.socket status --json
```

Check the output's `BackendState`, node name, and Tailscale IPs. Do not rely on
an interactive shell's `TAILSCALE_SOCKET` setting to reach a launchd child.
The Cassy Tailscale helper reads `TAILSCALE_SOCKET`, but released 3.64 service
install and refresh do not preserve that value in launchd's environment. A
manual `EnvironmentVariables` plist edit is temporary: the next service
install, refresh, or `cas update` overwrites it. This persistence fix lands
with cas-f149 in 3.65.

Until that fix is released, the visible symptom is hub status saying
`not publishable … not logged in`. Repeating
`cas hub restart --tailscale-serve` loops without fixing the launchd socket
selection. After 3.65 preserves `TAILSCALE_SOCKET`, have the hub operator
verify status, Serve, and HTTPS health.

### Prowl migration record (2026-10-19)

The 2026-10-17 before-state showed the standalone GUI app connected as
`prowl.tailf5a734.ts.net` at `100.106.196.93`; Homebrew `tailscaled` 1.102.5
was already installed but logged out. The operator completed the switch on
Oct 19: the root Homebrew LaunchDaemon was brought online and signed in to the
same tailnet, Tailscale SSH was enabled against its socket, and the GUI app was
stopped (it remains installed). To repeat the final step, run
`tailscale --socket=/var/run/tailscaled.socket up --ssh`. The read-only
after-state shows
`tailscaled` 1.102.5 Running at `/var/run/tailscaled.socket`, MagicDNS enabled,
and `prowl.tailf5a734.ts.net` at `100.76.232.42`. From soundwave,
`tailscale ssh pippenz@prowl` exited 0 and opened a shell without SSH keys.

The after-state capture also found `tailscale serve status --json` returned
`{}`. This does not prove that the hub's HTTPS route is published; the hub
operator must verify or republish its Serve mapping before relying on that
endpoint. The before/after command outputs are retained in the cas-4f8d task
artifacts.

MagicDNS is supported by the daemon. Current macOS builds manage split-DNS
files under `/etc/resolver` and may configure a default resolver through
SystemConfiguration when the tailnet overrides local DNS. Verify `scutil --dns`
and `dscacheutil -q host -a name MACHINE.TAILNET.ts.net` after migration and
updates. Test short names separately; `dig` alone does not exercise macOS's
system resolver. Do not blindly overwrite LAN DNS settings or add persistent
resolver files before checking what the installed daemon manages.

See Tailscale's [macOS variant comparison](https://tailscale.com/docs/concepts/macos-variants),
the [Homebrew formula](https://formulae.brew.sh/formula/tailscale), and the
[1.102.5 macOS DNS implementation](https://github.com/tailscale/tailscale/blob/v1.102.5/net/dns/manager_darwin.go).

### Keep Tailscale updated

Check for stable updates weekly and before a planned fleet maintenance window.
Schedule service restarts when no acceptance run or hub publication depends on
the machine; use LAN access when changing prowl's networking.

- Standalone GUI: use the menu-bar update action / Sparkle automatic updates.
- App Store: update through the App Store.
- Homebrew daemon: run `brew update` and `brew upgrade tailscale` as the
  Homebrew owner, then `sudo brew services restart tailscale`. Root service
  installation can leave formula paths owned by root; if upgrade reports an
  ownership conflict, inspect the exact paths Homebrew names rather than
  running `sudo brew upgrade` or changing ownership across the prefix.
- Linux: run `sudo tailscale update` on supported package installations, then
  verify the daemon's version and service state.

After an update, check `tailscale status`, MagicDNS, `tailscale ssh prowl` from
soundwave, `cas hub status`, `tailscale serve status`, and HTTPS `/v1/health`.
If the hub's Serve mapping no longer targets the live shim, have its operator
re-publish it with `cas hub restart --tailscale-serve` after confirming the
launchd `TAILSCALE_SOCKET` value.

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
2. **Verify.** Run `verify-dist.sh`. It recomputes the dist digest. Commander
   ships no WebAssembly since cas-f285: a dist that still holds
   `ghostty-vt.wasm`, `ghostty-write-pty.wasm` or `symbols.woff2` is stale, and
   the script's WASM hash check must be retired with it. Stop on any mismatch.
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

`cas hub start`, `cas hub restart`, service install and `cas update` read the
persistent host transport mode from `~/.cas/config.toml`. Set it once with:

```sh
cas config set hub.transport_mode loopback
cas hub restart
```

The modes are `auto` (default: try Tailscale Serve, retain loopback with a warning
if interactive publication fails), `required` (refuse startup without Serve), and `loopback`
(never publish). On macOS, Auto service startup still exits after bounded
publication retries so launchd KeepAlive retries at boot. Interactive Auto fallback
prints the transport warning and `cas hub start --loopback` remedy on stderr,
including with JSON output. For example:

```toml
[hub]
transport_mode = "required"
```

Restart, service launches and updates honor the saved mode and preserve existing
HTTPS port choices. `--tailscale-serve` and `--no-tailscale-serve` remain single
launch overrides; service install encodes that launch choice. The old
`hub.tailscale_serve` boolean is a compatibility fallback only when no mode is set
(`false` maps to `loopback`, otherwise `auto`). Project configuration does not
change the machine hub policy. `cas config get|set|reset hub.*` and
`cas config list --section hub` use the host file, from any working directory,
including outside a project. `HOME` selects that host; `CAS_ROOT` and `--store`
select project stores and do not redirect hub settings.

After a binary update, Cassy starts a stopped hub if its service is installed
or `~/.cas/hub/machine-id` exists. A machine with neither stays stopped and
prints `hub not running; start with cas hub start`. Start/restart and transport
checks have bounded timeouts. Starting a previously absent hub is best effort:
one attempt records `action=start_failed` and its cause if the service cannot
start, while update continues. Recovery of an existing runtime still fails
if loopback cannot be verified. Missing Tailscale, logout, Serve permission or publication
failure in `auto` mode leaves a healthy loopback hub and does not fail the update.
In `required` mode, publication failure refuses startup and update verification. Its receipt
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
