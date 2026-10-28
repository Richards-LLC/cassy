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

## Critique

terminal-qa: PASS codex-handoff · 11 runs · 0 fail · 0 warn · 16 allowed.
Captures: task artifact `terminal-qa/report.json` (80/120 columns, four palettes,
C locale, NO_COLOR and pipe). Two explicit exceptions repeat across the eight
TTY runs: preserve the exact copyable receipt path and inherited safety prose;
Codex's native cards own their paragraph wrapping. Original findings are saved
in `terminal-qa/original-report.json`.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | First two handoff lines name the predecessor/end reason and task/worker/pending counts. |
| Fit | 4 | Real 11/14/110 snapshot projects to 20 human lines and 1036 bytes, from 2205 bytes of raw JSON; at most two rows per group. |
| Craft | 4 | Task names, assignees and parked branches lead; retired worker identifiers and queue payloads remain in the receipt. |
| Theme safety | 5 | ASCII text has no custom color; all four palettes and non-TTY captures agree. |
| Machine contract | 5 | Actual native Codex 0.165 PTY rejects the installed hook and accepts the candidate's two-field SessionStart shape; full receipt arrays are identical. |

Scored on 2026-10-28. Evidence: task `LEDGER.md`, `before-real-pane.ansi`,
`after-final-pane.ansi`, `metadata-and-identity.json`, and candidate receipt at
revision `62d05b091db5844b89829a6ef59b7f583ac53cbf`. The native proof uses an
offline provider; authenticated supervisor adoption remains a live preview
observation. Native tool cards remain visible.
