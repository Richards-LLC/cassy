# Slack draft — hook-silence recovery and ambient ranking (main merges, PR #772 and PR #773)

Channel: #cas-internal. Deploy target: Live on production (main). Reaches installed hosts with the next runtime release.

## User thread

Top-level:
Live on production · User · Cassy's mid-session memory and factory mail no longer depend on a Claude Code hook that stopped firing, and recall now surfaces the lesson you saved today.

Reply:
Was → since 2026-09-06, long-running Claude sessions never received Cassy's automatic recall, supervisor reminders, or factory mail mid-session, because Claude Code stopped invoking the prompt hook, and nothing reported the silence. Recall also ignored memories saved the same day and its semantic search always gave up. Now → the same context arrives through the tool-result path or any Cassy tool response, exactly once per prompt, and `cas doctor` reports when the prompt hook goes quiet. Recall looks at the best-matching memories first instead of the eight most recently touched, and the semantic step gets a budget it can actually meet.

## Dev thread

Top-level:
Live on production · Dev · Turn context (recall, supervisor reminder, inbox) is delivered once per prompt through PostToolUse or any MCP response when UserPromptSubmit is silent, and ambient discovery ranks by term overlap before recency with a measured 1500 ms semantic deadline.

Reply:
Was → `cas hook UserPromptSubmit` was the only channel for ambient recall and factory inbox surfacing; Claude Code ≥ 2.1.263 does not invoke it in long-lived `--team-name` sessions (anthropics/claude-code#90784, #49989), so no live session had a recall decision or captured prompt after 2026-09-06 17:57Z. Ambient discovery read the 8 most recently updated rows (`read_surface`), so a same-day memory matching nine prompt terms sat 29th and never reached the scorer; `HOOK_SEMANTIC_TIMEOUT` was 400 ms against an endpoint p50 of 354 ms and p95 of 956 ms; unbound history commits outranked guidance. Now → `hooks/turn_context` keeps a per-prompt receipt; PostToolUse (synchronous, matcher `*`, no-op 10.6 ms mean) and the MCP response wrapper deliver inbox + local-only recall once per prompt; a doctor row counts observed misses. Discovery windows rank by overlapping snippet terms before recency, prompt terms cap at 16, the semantic deadline is 1500 ms, unbound commits sort behind guidance with one injection at most, and ≥3-term lexical matches survive beside semantic candidates. Live proofs: two-prompt team session with UPS omitted receives the card on prompt 2; rebuilt probe injects memory 2026-09-08-10 with a populated vector. PR #772 (closes #763), PR #773 (closes #764).

## POSTED
Posted 2026-09-09 00:04Z via the claude.ai Slack MCP to #cas-internal (C0B44GUKDK2):
- User top-level ts 1788912264.843669 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788912264843669 (reply ts 1788912303.301229)
- Dev top-level ts 1788912266.596539 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788912266596539 (reply ts 1788912308.187159)
