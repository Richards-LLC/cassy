# Coordination contracts — main-merge announcement draft

Posted source-merge announcement. The reviewed changes are on main and included in published Cassy v3.25.4. These four bodies remain a separate source announcement and make no installed-host claim.

Channel: #cas-internal (`C0B44GUKDK2`). Order: User top-level → User reply → Dev top-level → Dev reply. Each reply belongs to the immediately preceding top-level message.

Review basis: PR853 landed on main at `5640e6b4` from reviewed assembly `8f6d0b63`, including the final prompt correction. The assembled gate passed 9,392 workspace tests and 2 doctests. Published v3.25.4 source `875e353b` contains these changes.

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

Availability: these changes are on main and included in published Cassy v3.25.4. Older installations need updating to receive them.
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

Availability: this source-on-main change set is included in published Cassy v3.25.4. This source announcement does not claim any host was updated.
```

## POSTED

- **Posted at (UTC):** `2026-09-11T18:14:57.905212+00:00`
- **Channel:** `#cas-internal` (`C0B44GUKDK2`)
- **User top-level:** `message_id=1789150465.818649` · `thread_id=1789150465.818649` · <https://petra-stella.slack.com/archives/C0B44GUKDK2/p1789150465818649>
- **User reply:** `message_id=1789150483.335439` · `thread_id=1789150465.818649` · <https://petra-stella.slack.com/archives/C0B44GUKDK2/p1789150483335439?thread_ts=1789150465.818649&cid=C0B44GUKDK2>
- **Dev top-level:** `message_id=1789150491.380109` · `thread_id=1789150491.380109` · <https://petra-stella.slack.com/archives/C0B44GUKDK2/p1789150491380109>
- **Dev reply:** `message_id=1789150497.805649` · `thread_id=1789150491.380109` · <https://petra-stella.slack.com/archives/C0B44GUKDK2/p1789150497805649?thread_ts=1789150491.380109&cid=C0B44GUKDK2>

Transport: authenticated MechaCassy hub/bot. Four source-announcement messages only; runtime/report publication has its separate receipts. Approved pre-publication draft SHA-256: `7d4b03de6749144270145db57fa5e253bca41bf488021bfaaf12c64cb631e185`. No host update is claimed.
