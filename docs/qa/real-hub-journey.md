# Real disposable hub recovery

## Native recording chapters

Use `showRecordingChapter` from
`hub-web/e2e/journeys/receipt-directory.mjs` for chapter labels in Commander
recordings. The shared journey fixture and real-hub recording use this helper.
It draws a temporary caption with a constructed stylesheet, uses the page's
theme tokens, preserves focus and pointer input, and removes the caption before
the next stage. Titles are plain text and long titles wrap within the viewport.

Playwright 1.64.1's direct `page.screencast.showChapter` injects an inline
`style` element. Native Commander's `style-src 'self'` refuses it; serving static
dist files without that header hides the problem. Keep the production policy
and vendor unchanged. Do not enable `bypassCSP` for a recording.

The native HUB-J24 part records a visible helper caption, checks its removal and
zero style-policy events, then continues the actual image upload. Its binary
comes from `cas qa candidate path --task <task-id>`, and the recording is
`native-chapter.webm` in that part's Playwright output directory.

`HUB-J11` has a separate real-hub part. It serves only the Commander assets
embedded in `CAS_BIN`, pairs a real browser, and runs a real `cas factory daemon` and
`cas hub serve` in a new private HOME and Git project for every repetition.
The supervisor provider is an inert local executable: the real daemon owns
the operator queue, conversation history and WebSocket protocol. No hosted
model, production hub or existing browser pairing is used.

Both the fixture's `/commander` route and HTTPS listener forward bytes to the
actual local hub, including missing-asset responses and security headers. Neither
serves files from `hub-web/dist`. `REAL_HUB_DIST` is refused; external-dist
previews cannot be used as embedded-candidate evidence.
`journey-hub.ts.net` resolves to loopback in Chromium; a disposable certificate
and public address-space override make Chromium enforce actual Local Network
Access. The journey uses CDP to deny and then grant both `local-network` and
`loopback-network`, and verifies the actual permission state. It does not
mock fetch, WebSocket, permissions, hub responses or browser time.

## Run locally

Use a supervisor-built binary from the commit being evaluated. Copy it to
scratch before use, since the assembly's target directory can be rebuilt.
Install hub-web dependencies with `npm ci` and Chromium with
`npx playwright install chromium` if needed. Do not build Rust in a worker.

On macOS the fixture allocates its private HOME under `/tmp` (canonicalized to
`/private/tmp`) with a short `cas-rh-` prefix. The default `/var/folders/...`
`TMPDIR` otherwise makes the daemon's longer GUI socket path 115 bytes, above
macOS's 103-byte pathname limit. No `TMPDIR` override is needed; other platforms
keep using their normal temporary directory. Cleanup still removes only the
fixture's marked private HOME. The native journey preflight and `npm test` run
`node --test e2e/journeys/real-hub-home.test.mjs e2e/journeys/real-hub-server.test.mjs`.
The HOME check binds a real Unix socket even with a deliberately long macOS temp
base. The asset check runs the actual helper against a disposable HTTP upstream:
a missing embedded stylesheet remains 404 even when external dist contains it,
and the document retains the upstream security policy. This boundary regression
does not substitute for a journey against the exact candidate binary.

Start the fixture through the Cassy server registry (replace paths):

```json
{
  "action": "server_start",
  "id": "real-hub-journey",
  "command": "CAS_BIN=/absolute/scratch/cas node e2e/journeys/real-hub-server.mjs",
  "cwd": "/absolute/checkout/hub-web",
  "port": 29925,
  "task_id": "cas-9edf"
}
```

Then, from `hub-web`:

```bash
npm run journeys:real-hub -- --repeat-each=5
```

One repetition takes approximately 80 seconds, including a real 60-second
outage. Workers background the test with a log or arrange a reminder, so
supervisor instructions remain deliverable. Stop the registry server after
the run; workers ask their supervisor to stop it. Each test closes the browser,
stops only its own hub and daemon, and removes its marked disposable HOME,
even when an assertion fails. The server's signal handler performs the same
cleanup when the worker is torn down.

For concurrent evaluations, set `REAL_HUB_CONTROL_PORT`, `REAL_HUB_PORT` and
`REAL_HUB_TLS_PORT` to three distinct free loopback ports in 20000–32767 when
starting the fixture. Give the test the same `REAL_HUB_CONTROL_PORT`. Never
point this harness at an existing hub. `CAS_BIN` is required; there is no
implicit installed-binary fallback.

The ordinary `npm run journeys` suite excludes this part because it needs the
registered real fixture. `journeys:real-hub` uses the same verified native
Playwright runner and clock/receipt checks, with one worker and no retries.
It keeps real wall time for DPoP and outage duration, and checks only the
conversation and connection surfaces.

## Evidence and acceptance

Each repetition writes its own `HUB-J11/parts/real-hub-run-N` directory:
`receipt.webm`, stage screenshots, final ARIA YAML/JSON, a scrubbed `trace.zip`,
`result.json` and `hub-evidence.json`. Set `JOURNEY_RECEIPTS` and
`JOURNEY_OUTPUT` to fresh absolute paths to retain a release run. The evidence
records the binary's reported revision, real hub audit, actual hub lifecycle
and history logs, daemon log, and finite-request timings. Pairing credentials,
WebSocket tickets and signed auth headers are scrubbed before publishing.
`scripts/journey-bundles.py` folds these parts into the normal journey bundle,
preserving their real transport label and scrubbed traces.

The journey proves:

- Restarting the actual hub preserves identity, reconnects without a reload or
  re-pair, and replays the real supervisor reply queued while it was stopped.
- A real SIGSTOP pause proves an unanswered finite catalog probe aborts within
  five seconds. A 60-second outage bounds retry traffic and finite HTTP request durations;
  the long-lived event stream is recorded separately from finite probes.
- Browser offline/online and actual Chromium permission denial/grant both
  recover and replay the corresponding reply exactly once.
- Exactly one pairing exchange occurred; authentication and conversation
  history were exercised again on recovery. No observed connection state
  said Needs pairing.

For a negative control, ask the supervisor for a baseline candidate binary
that embeds the pre-fix Commander bundle, then copy it to scratch and start a
second registered fixture with that `CAS_BIN` and a different port triple.
Run the same journey against that control port. Record both candidate revisions,
any backend differences, the failing stage, and the visible Needs pairing or
failure to recover. A loose dist directory cannot prove the baseline candidate
embeds those assets. Do not weaken assertions to force a baseline failure.

## Real New session launch (HUB-J13)

The same fixture accepts `newSession: true`: it starts only the real hub, then
Commander browses a disposable repository and calls `POST /v1/sessions`. The
hub starts its real reaper, factory daemon and supervisor PTY. An inert provider
answers the actual injected startup prompt through `cas factory message`, then
answers one Commander instruction through the same durable queue. No reply is
seeded by the browser or fixture control endpoint.

The hub runs without `CAS_ROOT`, matching the production user service. The
separate Rust launch-environment test covers foreign-store hardening; that
condition was absent from the operator's blank launch. Run this part with the
registered fixture:

```bash
npm run journeys:real-hub -- real-hub.journey.ts --grep 'HUB-J13 real hub'
```

Only OS service placement and the provider executable are test boundaries.
The fixture sets `SHELL` to a private adapter that executes `-lc` probes without
loading host login profiles. This keeps its `HOME/bin/claude` first even when
macOS `path_helper` or a profile would prepend Homebrew; no host provider or
account credentials are used. The preflight regression plants a fake host
provider in a login profile and requires lookup and the login probe to use the
disposable provider instead.

The fixture's local `systemd-run` adapter executes the real reaper/daemon under
its private HOME and registered containment, instead of the operator's user
manager on Linux. Its recorded process group and start fingerprint bound cleanup
to the owned launch. On macOS the hub uses its native direct process-session
launch, so that Linux adapter and its `/proc` fingerprint are not invoked.
Cleanup captures that owned hub's descendant PIDs and `ps` start fingerprints
before stopping it, retaining those identities across reparenting. It never
scans host command arguments or environments to establish ownership.
The inert provider consumes actual bracketed-paste frames and CR submissions
without canonical terminal buffering; only a submitted `/exit` command ends it,
so startup prose mentioning that command cannot terminate the provider.
This does not prove host systemd/cgroup placement or model action.
The installed official provider and login remain a separate bounded live check;
never use an existing hub or read/copy credentials into this fixture.

The deferred startup regression is
`supervisor_single_turn_tests::claude_startup_retries_the_deferred_wake_when_the_pane_becomes_ready`.
It queues the real named-profile startup with project context, observes the
Teams write while the PTY is unready, then makes the pane ready and requires
one submitted wake naming that message, with no second Teams body. Its PTY
only captures input and never generates replies. This proves the daemon's
retry after a declined wake, independently of the inert provider's answer;
neither proof claims an official model took a turn.
