# Slack draft — Viktor delegation gateway v1 (PR #439 → main, 61fcaebf) + attention ticket fix (PR #441 → main, 368c3909)

Channel: #cas-internal (C0B44GUKDK2)
Order: messages 1–2 = Viktor user thread, 3–4 = Viktor dev thread, 5–6 = ticket-fix user thread, 7–8 = ticket-fix dev thread.

=== MESSAGE 1 (user top-level) ===
**Live on production — User:** Agents can now safely ask an outside expert service to verify their work — every answer comes back as hard evidence, an expert who couldn't answer never counts as a pass, and only the supervisor holds the paid account.

=== MESSAGE 2 (user reply, thread of message 1) ===
Was → Now:
• There was no controlled way for the team to use the external expert service — the account key would have granted anyone unlimited paid calls with nothing able to say no. Now every external call carries who is asking and passes a policy check before it goes anywhere, with an audit trail.
• A misspelled or lookalike service name could have slipped through a naive filter. Now only exactly-approved (service, tool) pairs are allowed; everything else is refused before leaving the building.
• Paid calls could have been double-charged on retries or blown past budgets. Now every delegation gets a receipt with duplicate-call protection, budget reservations that deny over-cap requests locally, and the ability to resume a timed-out run instead of paying again.
• An external check that timed out, answered vaguely, or failed to connect could have been mistaken for approval. Now every one of those outcomes is recorded as a durable non-pass — an expert who could not answer never reads as a yes.

=== MESSAGE 3 (dev top-level) ===
**Live on production — Dev:** Viktor delegation gateway v1: registered caller identity + pre-dispatch policy hook in the MCP proxy, exact parsed (server, tool) allowlisting, a delegation receipt store with budget reservations and run resumption, and a fail-closed verdict contract (PR #439).

=== MESSAGE 4 (dev reply, thread of message 3) ===
Was → Now:
• The MCP proxy carried no caller identity and had no policy decision point → every external dispatch is attributed to a registered caller and passes a pre-dispatch policy hook with redacted audit.
• External tool admission would have been a raw mcp__ prefix match → exact parsed (server, tool) routing; lookalike servers and foreign tools deny before upstream; prompt/PTY parity preserves external names across Claude/Codex/Grok harnesses.
• No persistence for delegations → receipt store (migration 236): backend-generated idempotency keys, budget reservations with local cap denial, timeout run-id persistence and resumption, terminal evidence/verdict fields.
• No verdict discipline → gate passes only on a schema-valid pass meeting policy; fail, inconclusive, wait_timed_out, requires_action, thread_busy, insufficient scope, rate limit, cancellation, malformed output, and transport failure each produce a durable non-passing receipt (one test per outcome); requires_action never auto-continues. (PR #439)

=== MESSAGE 5 (user top-level) ===
**Live on production — User:** Alert cards now always show which ticket they belong to, wherever the alert came from.

=== MESSAGE 6 (user reply, thread of message 5) ===
Was → Now: alerts arriving through the live event stream displayed without their ticket number, so you couldn't tell which piece of work an alert was about without digging. Now every alert card names its ticket regardless of how it arrived.

=== MESSAGE 7 (dev top-level) ===
**Live on production — Dev:** hub-web renderCard read card.latest.ticketId, which is unset for cards derived from the hub event stream — it now reads the derived card.content.ticketId, with an invariant test for that path (PR #441).

=== MESSAGE 8 (dev reply, thread of message 7) ===
Was → Now: event-stream-derived attention cards carried their ticket on card.content.ticketId while the renderer read card.latest.ticketId, so the ticket rendered only for locally-built cards → renderer reads the derived content field; test covers an event-stream card with a ticket; dist rebuilt once (+2 bytes). (PR #441)

## POSTED

- UTC: 2026-08-16T13:39:53Z
- Channel: #cas-internal (C0B44GUKDK2)
- Message 1: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786887549245609
- Message 2: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786887557741749?thread_ts=1786887549.245609&cid=C0B44GUKDK2
- Message 3: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786887561921689
- Message 4: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786887568936739?thread_ts=1786887561.921689&cid=C0B44GUKDK2
- Message 5: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786887572299709
- Message 6: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786887579021429?thread_ts=1786887572.299709&cid=C0B44GUKDK2
- Message 7: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786887583481139
- Message 8: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786887587662399?thread_ts=1786887583.481139&cid=C0B44GUKDK2
