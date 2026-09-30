# Hook wire captures

`claude-2.1.265-user-prompt-submit.json` is the retained live capture from
`captured_265_prompt_payload_reaches_handler_and_records_turn` in
cas-cli's factory_inbox_surfacing tests. Only the transcript/cwd paths were
already sanitized to `/fixture/...`; ids, keys, prompt and session title remain.

`cas-f3e3-message-display.json` preserves the raw JSON retained by
`messagedisplay_payload_carries_text_under_delta` from the Claude Code 2.1.224
capture described in docs/analysis/2026-08-07-hook-wire-shape-audit.md.
The cas-5e46 files are the later interactive captures described by that audit.

For a changed HookInput alias or rename, retain an independently captured raw
payload here and add its wire-key/consumed-field pair to `wire_contract.rs`.
The contract parses the same bytes through serde and checks the decoded value
against the wire value, including that it differs from the field's default.
Do not add a new spelling to the grandfathered legacy inventory: those entries
identify historical compatibility names with no retained capture evidence.
Handler-logic tests may keep struct literals; they are not wire-contract proof.
