# Reproduction attempt: "Edit reports REJECTED but the write lands on disk"

- **Task:** cas-40c9 · **Issue:** GH #143 · **Date:** 2026-08-07
- **Outcome: not reproduced in 28 attempts across two denial paths — and the
  incident-matching condition was never covered.** No fix was made to the harness layer.

Read the caveat before the number. The original incident was interrupt-adjacent: the
edit raced a supervisor interrupt / merge window. That exact condition — an interactive
interrupt landing mid-edit — could not be driven from a headless run and **remains
untested**. What was tested is the two *denial* paths.

A negative result that never covered the incident shape is not a negative result about
the incident. This is not grounds to close GH #143.

## The report

During cas-f102 a factory worker's `Edit` tool call returned REJECTED and the write
landed on disk anyway. The worker noticed only because it ran an unprompted
`git status`; it reverted, and the tree was confirmed byte-identical to the merged
tip, so nothing unreviewed shipped. One occurrence, no repro at the time.

The dangerous part is not the stray byte — it is the *silence*. A tree that diverges
from the reviewed tip surfaces later as a diff misattributed to whoever touches that
file next.

## What was tested

Both denial paths a factory worker can actually hit, with disk state measured by
sha256 before and after, independent of anything the agent reported.

| Variant | Denial source | Attempts | Wrote to disk |
|---|---|---|---|
| A | `PreToolUse` hook returning `permissionDecision: deny` | 10 | 0 |
| A′ | Same hook, 3s sleep before the verdict (widens any optimistic-write window) | 10 | 0 |
| B | Permission ruleset (`--disallowedTools Edit,Write`) | 8 | 0 |

Harness: Claude Code 2.1.222, headless `claude -p`, `--permission-mode acceptEdits`
so the *only* source of rejection is the verdict under test, `--model haiku`, one
fresh temp working directory per attempt.

Validity was confirmed on a `--output-format stream-json` run rather than assumed
from the agent's prose: the transcript carries a real `"name":"Edit"` tool_use, its
result is `is_error: true`, `permission_denials` lists `Edit`, and the target file is
byte-identical afterwards. So the Edit was genuinely attempted and genuinely denied —
these are 28 real denials, not 28 turns where the model declined to call the tool.

An earlier batch of 5 was discarded as invalid: `--allowedTools Edit Write Read`
word-split and swallowed the prompt, so `claude` exited on a usage error before any
tool ran. Those runs also showed "disk unchanged", which is exactly why they had to
be thrown out — the same reading for the wrong reason.

## Reproducing

```bash
# PreToolUse deny hook
cat > /tmp/deny-hook.sh <<'SH'
#!/usr/bin/env bash
sleep "${REPRO_HOOK_SLEEP:-0}"
printf '%s' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"repro"}}'
SH
chmod +x /tmp/deny-hook.sh
# settings.json: hooks.PreToolUse[0] = {matcher:"Edit|Write|MultiEdit",
#                                       hooks:[{type:"command",command:"/tmp/deny-hook.sh"}]}

# per attempt, in a fresh dir containing target.txt:
sha256sum target.txt
claude -p "Read target.txt, then use the Edit tool once to replace ORIGINAL LINE B \
with MUTATED LINE B. If it is rejected, stop and reply REJECTED." \
  --settings /tmp/settings.json --model haiku \
  --allowedTools "Edit,Write,Read" --permission-mode acceptEdits < /dev/null
sha256sum target.txt   # must be unchanged
```

## Conclusion

28/28 denied edits left the file byte-identical across both denial paths, including
a deliberately widened race window. But the interrupt-adjacent condition that the
cas-f102 incident actually exhibited was never exercised, so this says the *denial*
paths are clean — not that the incident shape is. The harness layer was **not**
changed: fixing a race on a single unreproduced occurrence would be guessing, and
so would closing GH #143 on a negative that never covered it.

What did change is the detection layer, where the cost is bounded and the benefit
does not depend on the theory being right: `close_ops.rs` now produces a clean-tree
receipt at worker close and NAMES a divergence instead of passing it in silence, and
the `cas-worker` close-gate guidance (all three harness flavors) tells workers to
verify `git status --porcelain` and HEAD themselves and to believe git over a tool
result when the two disagree.

If this recurs, capture immediately: the tool result text, `git status --porcelain`,
`git diff` of the stray write, and whether an interrupt (Esc) landed anywhere near
the edit. That last one is the untested condition — it needs an interactive session
and could not be driven from a headless run.
