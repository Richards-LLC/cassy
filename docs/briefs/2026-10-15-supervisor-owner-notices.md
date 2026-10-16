# Supervisor ownership and integration notices

Task: cas-5c2c. Contract: communication v2 P3.2, P3.5 and P3.6.

A supervisor ending its session leaves its unfinished epics explicitly orphaned,
with one grouped notice to each live local peer and the operator. It does not
assign new work to a peer without acceptance. Closed and cancelled epics keep
their audit ownership. A superseded SessionEnd cannot tear down a rebound agent.

Owner-addressed integration and attention notices resolve a live same-name
successor, then a live supervisor in this project's local registry, then the
operator. The receiving supervisor's actual name and session replace the old
routing. Operator prompts use Commander schema 2. Orphan and integration
fallbacks remain in the project's durable notice outbox until a connected daemon
adopts them into its own session; the ending session cannot capture its orphan
notice. Without another daemon, ownership is cleared and the notice remains
pending. Ordinary task lifecycle transitions stay with their owning session or
the same supervisor's successor; they never fall back to the operator or an
unrelated peer. A heartbeat older than five minutes is not live even if the row
still says Active. Worker commands retain their session isolation.

Focusing overlapping integration lanes tells both owners and deduplicates a
repeated focus, task edits and claim renewals. Lane or owner changes create a new
conflict identity. A remote epic-focus claim conflict sends the other supervisor a
Cloud mailbox notice. These are separate producers: SessionEnd and daemon
notices perform no remote discovery; only the existing focus-claim conflict
event messages the supplied remote holder. Its bounded HTTP send runs in the
background without delaying the focus response. Cassy awaits that actual result
before choosing peer delivery or operator fallback, so a late send cannot race a
timeout fallback. Orphan notice identity includes the epic ids and end time;
legacy name ownership is preserved when a same-name successor has registered.

Explicit cross-machine sends and replies keep the complete body in the Cloud
mailbox, including bodies beyond the local inbox cap. Local artifact paths are
not a transport for remote readers.

## Live receipt

The two-machine receipt is **deferred: needs a live remote peer**. The supervisor
approved that deferral on 2026-10-15. Unit tests, fake-Cloud integration tests and
the probe's offline self-tests do not supply this receipt. No release-gate row
is added.

Once both machines have the candidate binary and real device credentials, run:

```bash
python3 scripts/live-check-peer-collab.py \
  --binary /path/on/this/machine/cas \
  --prowl-host remote-host \
  --prowl-binary /path/on/remote/cas \
  --cloud-url https://your-cassy-cloud.example \
  --token-file-a /private/local-device-token \
  --token-file-b /private/remote-device-token \
  --steps peers_discovery,message_reply_status,overcap_message_reply_status \
  --receipts-dir /path/to/task-artifacts/live
```

The existing probe starts isolated scratch factories on both machines and tears
them down. It records the binaries, Cloud ids, local notification ids, send and
receive timestamps, delivery status and receiver-side exact body digests. Both
the over-cap send and its over-cap reply must match. Exact probe token lines
delimit the measured body independently of routing preamble blank lines.
Omit `--cloud-url` only for
a diagnostic stand-in run; its evidence is labelled `stand-in-only`.

## Release impact

Operators can see and adopt orphaned epics, and integration conflicts reach both
supervisors. A dead session no longer captures new owner notices. There are no
new configuration keys, migrations, MCP actions or browser journeys. Validation
runs on Linux; live two-machine validation remains explicitly deferred.
