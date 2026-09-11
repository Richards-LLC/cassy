# Coordination contracts — main-merge announcement draft

Draft only. Publish after the reviewed PR lands on main. These four message bodies describe source on main; they make no installed-runtime or deployment claim.

Channel: #cas-internal (`C0B44GUKDK2`). Order: User top-level → User reply → Dev top-level → Dev reply. Each reply belongs to the immediately preceding top-level message.

Review basis: assembled branch at `8f7181e4`, compared with its incorporated main baseline. Recheck against the final merged diff, including the pending prompt correction, before publication. Scoped test receipts support the changes; assembled integration approval remains pending.

## 1. User top-level

```text
*Source on main — User — Cassy*
Was: recovery instructions could point to commands your coding tool couldn't run. → Now: they match the tool receiving them.
```

## 2. User reply

```text
• *Usable recovery steps* — Was: suggested fixes could use the wrong command names. → Now: recovery instructions match Claude, Codex, Grok or OpenCode.

• *Clearer acceptance* — Was: instructions requested another acknowledgement after work had already started. → Now: a successful task start confirms acceptance.

• *Attention when needed* — Was: some blocked-completion and failed-check alerts stayed quiet. → Now: those recovery alerts can call for attention, while routine updates remain available on the next turn.

• *Clearer status boundaries* — Was: an empty report could look like there was no work anywhere. → Now: it names the current session and distinguishes entries in other sessions.

Availability: these changes are in the source on main. Installed copies require a later runtime release containing them.
```

## 3. Dev top-level

```text
*Source on main — Dev — Cassy*
Was: recovery templates assumed a tool namespace. → Now: caller and registered-recipient evidence select executable MCP hints.
```

## 4. Dev reply

```text
• *Recipient-aware commands* — Was: fixed prefixes leaked into task and MCP recovery hints. → Now: request-local caller context and role-specific recipient metadata select `mcp__cas__`, `mcp__cs__`, `cas__`, or `cas_`. Unknown recipients receive neutral hints.

• *Durable recovery* — Was: stored event descriptions embedded a harness-specific close command. → Now: persisted facts stay neutral and relay actions render for the registered recipient, including retries after a partial outbox write.

• *Trusted wake classification* — Was: emitted `merged_close_blocked` and `pr_lane_failed` alerts were missing from the wake classifier. → Now: both are recognized on the existing Daemon-authorized lifecycle path, with origin checks and free-text restrictions preserved.

• *Safe close arguments* — Was: quotes or newlines in a close rejection could break the suggested audit argument. → Now: the command uses a stable reason and retains the original rejection as context.

• *Consistent instructions* — Was: prompts asked for redundant ACKs, blurred acceptance with delivery receipts, and could give Codex the wrong startup checklist. → Now: `task action=start` confirms acceptance, receipt and wake guidance matches existing policy, and Codex loads its own checklist.

• *Explicit status scope* — Was: full, summary and empty status responses left their session boundary implicit. → Now: they report the session and counts inside and outside it, partitioning by session before name deduplication. These are registered-entry counts, not process-liveness claims.

Availability: source on main only. No runtime version or deployment is announced here.
```
