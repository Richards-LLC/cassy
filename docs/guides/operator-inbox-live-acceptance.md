# Deployed Cloud operator inbox acceptance

`hub-web/scripts/operator-inbox-live.mjs` runs the cas-2077 acceptance with two
independent Chromium profiles on **one disposable account's feed**. Team
membership does not make separate accounts share operator history.

The test uses the real CLI to enroll a disposable hub, bind a project, approve
each browser, record supervisor messages and drain the durable outbox. It then
stops the hub and takes the older browser offline before enrolling the phone.
The phone replays retained history and queues a reply; the returning hub admits
one command and writes one receipt. A hub restart preserves that admission.
Revoking the older browser must produce `401 grant_revoked` on its next request
and clear its local inbox. The phone must decrypt a new message sealed under a
different epoch. Cleanup revokes both browsers and the machine.

Cloud APIs reach `https://petra-stella-cloud.vercel.app` directly. Browser routing
serves only this checkout's committed `hub-web/dist` at the allowed
`https://hub.petrastella.io/commander/` origin. No installed browser profile,
running hub or production user HOME is used. The disposable supervisor is an
idle stand-in; the CLI, outbox, encryption, Cloud replay and command admission
are real. The test creates fresh retained messages; it does not simulate weeks
of storage aging.

## Run

Use the ordinary sandbox credentials supplied by Cloud issue #160 in
`~/.config/cas/cloud-test-profiles.env`, a regular mode-0600 file. The default
account is B. Do not use an admin secret or reset the shared sandbox. Coordinate
the sandboxB window with the supervisor before running.

Choose an unused short absolute scratch root named `cas-2077-live-*`, an absolute
task artifact directory, and the supervisor-built `cas` binary. Short paths
matter because the native factory uses Unix sockets. In a factory session,
start the native control through the server registry with `task_id=cas-2077`:

```bash
CAS_2077_LIVE=1 \
CAS_2077_PROFILE=B \
CAS_2077_LIVE_ROOT=/mnt/rewind/cas/scratch/cas-2077-live-example \
CAS_2077_LIVE_ARTIFACTS=/absolute/task/artifacts/live/example \
CAS_BIN=/absolute/path/to/cas \
node hub-web/scripts/operator-inbox-live-native.mjs
```

The control listens on loopback port 29935 and the disposable hub uses 29936.
Set `CAS_2077_CONTROL_PORT` and `CAS_2077_HUB_PORT` on the native control to
change them; pass the same control port to the test. With the same live, profile,
root and artifact variables, run this one named Playwright test:

```bash
node hub-web/scripts/operator-inbox-live.mjs
```

The wrapper invokes only `playwright.operator-inbox-live.config.ts`, with one
worker and no retries. It scrubs the runner trace before copying it into
artifacts. Native credentials, browser grants and raw traces stay under the
private scratch root. On success, cleanup removes that root; on failure,
inspect the cleanup receipt before removing private files or retrying cleanup.
Stop the registered native server after the test, including on failure.

## Receipts

`live-receipt.json` records the tested commit and binary version, six user-path
cells, command/prompt/receipt IDs, the revoked request, and the old/new epochs.
`cleanup.json` lists only this run's principals and reports cleanup failures and
remaining active principals. `qa/` contains cell captures, desktop/phone renders
in both schemes, asserted ARIA snapshots, a rendered static DOM/CSS snapshot,
and the scrubbed test-runner trace. Preserve failed runs separately.

Offline preflight coverage needs no credentials, browser or Cloud connection:

```bash
node --test hub-web/scripts/operator-inbox-live-safety.test.mjs
```
