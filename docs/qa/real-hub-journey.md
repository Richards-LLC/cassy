# Real disposable hub recovery

`HUB-J11` has a separate real-hub part. It serves the committed Commander
bundle, pairs a real browser, and runs a real `cas factory daemon` and
`cas hub serve` in a new private HOME and Git project for every repetition.
The supervisor provider is an inert local executable: the real daemon owns
the operator queue, conversation history and WebSocket protocol. No hosted
model, production hub or existing browser pairing is used.

The fixture's HTTPS listener only forwards bytes to the actual local hub.
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

For a negative control, extract the committed `hub-web/dist` from pre-fix
`5eaba13da` into scratch and start a second registered fixture with
`REAL_HUB_DIST=/absolute/pre-fix/hub-web/dist` and a different port triple.
Run the same journey against that control port. Keep the same tip-built hub
binary to isolate the browser reconnect fix; record both revisions, the failing
stage, and the visible Needs pairing or failure to recover. Do not weaken the
assertions to turn an unexpected passing baseline into a failure.
