# Slack draft — release gate timing and reuse (main merge, PR #771)

Channel: #cas-internal. Deploy target: Live on production (main).

## User thread

Top-level:
Live on production · User · Cassy's release checks now run the test suite once instead of twice and can re-check an unchanged release in under a minute.

Reply:
Was → every release check compiled and ran the whole test suite twice and started from scratch after any one-line fix, so a release spent most of an hour re-checking work it had already verified. Now → the suite runs once, every check records how long it took, and an unchanged release can be re-checked in about forty seconds while the safety rules that decide what may ship stay exactly as strict.

## Dev thread

Top-level:
Live on production · Dev · The release gate records per-row timings, runs the workspace suite once across the nextest and archive rows, and offers an opt-in content-keyed `--reuse` re-gate that never authorizes the pipeline.

Reply:
Was → `scripts/release-gate.sh` ran the in-tree nextest workspace and the archive run as two full suite executions, kept no per-row timings, and the doctest row bypassed the CI verified-test wrapper; a fix forced a full ~20-minute re-gate. Now → rows log UTC/wall/user/system timings, the nextest row covers only the snapshot complement while the archive run executes the workspace once, `--reuse` re-gates an unchanged tree from content-keyed PASS receipts and can never authorize `--pipeline`, and doctests use the same wrapper and identity scrub as ci.yml. Measured: baseline 1207 s, optimized 1170 s, unchanged reuse 37.5 s. PR #771.

## POSTED
Posted 2026-09-08 23:34Z via the claude.ai Slack MCP to #cas-internal (C0B44GUKDK2):
- User top-level ts 1788910476.381799 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788910476381799 (reply in thread)
- Dev top-level ts 1788910477.288529 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788910477288529 (reply in thread)
