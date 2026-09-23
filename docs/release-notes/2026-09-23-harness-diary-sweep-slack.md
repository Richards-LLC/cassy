# 2026-09-23 — Harness diary sweep — #cas-internal thread

Channel: `#cas-internal` (`C0B44GUKDK2`). Diary-only thread: one parent plus
three replies ordered Grok, Claude, Codex. Bodies below are exactly as posted.

## Parent post

```text
*Dev — Harness diary sweep — 2026-09-23*
Was: Cassy validated against Grok 1.0.5 and Codex 0.149.1 → Now: validated against Grok 1.0.40 and Codex 0.156.0.
```

## Grok reply

```text
• *Range reviewed* — Was: diary stopped at Grok Build 1.0.5 → Now: 1.0.6–1.0.40 reviewed, and 1.0.40 is the validated pin.

• *Worker rules* — Was: rules were visible in Grok's saved system prompt → Now: 1.0.40 no longer saves them there, and a live check proved workers still follow them.

• *Urgent redirect* — Was: 1.0.24 stopped Esc from cancelling a turn → Now: Cassy already cancels Grok turns with Ctrl+C, and a live redirect on 1.0.40 passed.

• *Not yet checked* — Was: untracked → Now: mid-call MCP input, MCP consent popups, Grok memory, and long-session compaction are recorded as not covered.

Source gaps: no per-version notes for 1.0.14–1.0.16, 1.0.26–1.0.29, 1.0.35–1.0.40. Grok 1.0.41 has since installed and is not yet validated.
```

## Claude reply

```text
• *Range reviewed* — Was: diary stopped at Claude Code 2.1.245 → Now: 2.1.246–2.1.280 reviewed.

• *Verdict* — Was: open question → Now: no Cassy change needed; MCP, hook, subagent, and messaging fixes are host-side reliability gains.

• *Model defaults* — Was: Opus 5 and earlier Fable defaults → Now: Claude Code defaults to Opus 5.5 (2.1.280) and Fable 5.1 (2.1.257); Cassy's lane choices are unchanged pending review.

Source gaps: no official changelog section for 2.1.249, 2.1.253–2.1.256, 2.1.262, 2.1.264, 2.1.279.
```

## Codex reply

```text
• *Range reviewed* — Was: diary stopped at Codex 0.149.1 → Now: stables 0.150.0–0.156.0 reviewed, and 0.156.0 is the validated pin.

• *Launch contract* — Was: seven releases touching MCP, sandbox, AGENTS.md trust, and resume were unvalidated → Now: a live 0.156.0 run passed all of them.

• *Verdict* — Was: open question → Now: no Cassy code change needed.

Source gaps: none.
```

## POSTED

- **Posted at (UTC):** `2026-09-23T12:53Z`
- **Channel:** `#cas-internal` (`C0B44GUKDK2`)
- **Parent:** `message_id=1790168012.319609` · <https://petra-stella.slack.com/archives/C0B44GUKDK2/p1790168012319609>
- **Grok reply:** `message_id=1790168016.254469` · <https://petra-stella.slack.com/archives/C0B44GUKDK2/p1790168016254469?thread_ts=1790168012.319609&cid=C0B44GUKDK2>
- **Claude reply:** `message_id=1790168019.574359` · <https://petra-stella.slack.com/archives/C0B44GUKDK2/p1790168019574359?thread_ts=1790168012.319609&cid=C0B44GUKDK2>
- **Codex reply:** `message_id=1790168022.637549` · <https://petra-stella.slack.com/archives/C0B44GUKDK2/p1790168022637549?thread_ts=1790168012.319609&cid=C0B44GUKDK2>
