# Slack draft — deterministic relay fixture for the invalid-invitation test (main merge, PR #792)

Channel: #cas-internal. Deploy target: Live on production (main). Reaches installed hosts with the next runtime release.

## User thread

Top-level:
Live on production — User — Was: one pairing test could fail a release check by racing its own throwaway server under load, blocking an unrelated change. Now: the test waits for its server to be ready and, if anything still goes wrong, says exactly which address refused.

Reply:
• Deterministic fixture — Was: the invalid-invitation pairing test started its throwaway server in a thread and sent the request straight away; on a busy CI shard the request could reach the port before the server was listening, and the failure only said "request was refused". Now: the test waits for the server to signal readiness, bounds the accept and read with deadlines, and any failure names the exact address and endpoint.
• Scope — test code only; no pairing behavior changed. Fifty concurrent runs pass.

## Dev thread

Top-level:
Live on production — Dev — Was: `hub_reverse_pairing::tests::invalid_invitation_reports_the_field_contract_and_sent_shape` bound a `TcpListener`, spawned `accept()` in a thread, and let the client race it; merge-group run 34369856376 got "request was refused". Now: readiness channel before the request, nonblocking accept with a 3 s deadline, 1 s read timeout, address-bearing panics and endpoint-bearing assertions; 50/50 under `xargs -P8` (PR #792).

Reply:
• Readiness handshake — Was: the client call raced the server thread's first `accept()`. Now: nonblocking listener, `sync_channel(1)` readiness signal, `ready_rx.recv()` before the request.
• Bounded waits — Was: a hung accept or read hung the test. Now: 3 s accept deadline with 5 ms polling; 1 s read timeout.
• Diagnostics — Was: bare `unwrap()` and `{error}` assertions. Now: every panic and assertion names the bound address or endpoint.

Proof: `hub_reverse_pairing` 21/21; fifty concurrent invocations under `xargs -P8` all pass. Test-only change. PR #792.

## POSTED
Posted 2026-09-09 18:14Z via the MechaCassy hub to #cas-internal (C0B44GUKDK2):
- User top-level ts 1788977646.259509 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788977646259509 (reply ts 1788977653.240639)
- Dev top-level ts 1788977655.167029 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788977655167029 (reply ts 1788977668.600549)
