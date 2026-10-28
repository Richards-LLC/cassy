# Codex supervisor startup

| Field | Contract |
| --- | --- |
| First two lines | Name the startup and the native context source, then give the supervisor its first identity action. |
| Scannable | The queued launch trigger names current workers; succession groups task names, parked deliveries, worker evidence and pending decisions with counts. |
| Readable | A short instruction runs the supervisor checklist using resident role guidance; handoff details are pulled from the receipt when a decision requires them. |
| Machine output | SessionStart stdout is one JSON document: `hookSpecificOutput` contains `hookEventName` and `additionalContext` for Codex; Claude keeps its own session metadata. The full handoff JSON schema stays unchanged. |
| Omitted | Context and ambient recall stay in the native hook's developer context; historical task notes, worker identifiers, PIDs and raw queue JSON stay in `.cas/supervisor-handoffs/`. Native tool cards remain controlled by Codex. |

## Evidence boundary

Codex 0.165.0 embeds a strict SessionStart output schema that rejects
`sessionTitle` and `reloadSkills`. Cassy's launch preflight requires trusted
native hooks, so a second queued context fallback duplicates the hook delivery.
The queued trigger still starts the checklist after hooks orient the model.

The official configuration reference documents `hide_agent_reasoning` for
reasoning events and `show_raw_agent_reasoning` for reasoning content. Neither
is a tool-call visibility control. No native MCP-card hiding control was found
in the installed CLI or the official reference. Keep tool-call evidence and
reduce the Cassy payloads that feed those cards.

Sources: [hooks](https://learn.chatgpt.com/docs/hooks),
[configuration](https://learn.chatgpt.com/docs/config-file/config-reference).
