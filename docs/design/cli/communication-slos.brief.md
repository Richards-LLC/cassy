# Communication SLOs in cas doctor

| Field | Contract |
| --- | --- |
| First two lines | The existing doctor verdict includes communication breaches; the communication row starts PASS, BREACH or UNKNOWN and names the last factory session. |
| Scannable | Five indented metric lines show outcome, value and target, including numerator/denominator for relay receipts and nudges. |
| Readable | Unknown historical coverage is explicit; a breach names the metric and retains the session for diagnosis. |
| Machine output | Doctor JSON retains its checks array: communication SLOs has status/message with the same five outcomes; session-end JSON also contains typed counts and coverage. |
| Omitted | Message bodies and worker transcripts never appear; only stored surfacing observations count, so historical duplicate reads before instrumentation remain unknown. |

The body metric counts extra surfaced bodies per (notification, recipient),
following cas-447d dups.py. A pointer or transport claim is not a body. Repeated
bookkeeping of one observed wake is not another body. A poll followed by a hook
that renders the same message is another body, even if recovery was deliberate.
Operator relay receipts count only enrolled cloud deliveries. Unenrolled local
snapshots render `N/A ... not applicable` with the `cas hub operator bind` step,
or `; N unenrolled excluded` beside an enrolled ratio; recorded summaries from
before that split say `legacy denominator`. A receipt is cloud retention, not
proof that anyone handled the turn.
No operator events is vacuously 100% (0/0, labelled no events); nudges with no
measurable worker exposure breach rather than producing a false zero rate.

The release fixture row runs in the existing no-build fast/integration stage.
It shares the production SQL, and negative controls must reject each breach.

## Critique

terminal-qa: PASS communication-slos-pass · 12 runs · 0 fail · 0 warn · 0 allowed · <artifacts>/cas-7e24/terminal-qa/pass/report.json

terminal-qa: PASS communication-slos-breach · 12 runs · 0 fail · 0 warn · 0 allowed · <artifacts>/cas-7e24/terminal-qa/breach/report.json

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Communication starts with PASS/BREACH/UNKNOWN, then the five measurements; the existing TTY safe-fix prompt precedes the overall doctor verdict. |
| Fit | 5 | Both 80-column captures keep all five targets; 24 terminal runs report zero overflow, clipping or word splitting. |
| Craft | 4 | Parallel metric lines retain counts and worker-hours; historical coverage says UNKNOWN rather than hiding a missing measurement. |
| Theme safety | 5 | Four palettes, NO_COLOR, C locale and pipe pass; light/dark PNGs inspected for both PASS and BREACH. |
| Machine contract | 5 | JSON remains one checks array and carries the same five values/targets/outcomes in the communication message. |

Scored by proud-phoenix-82 on 2026-10-16 against the actual binary built from
9e26cb090b9f58abdccc248fea99788511c0aeb8. Input stores are isolated fixtures
initialized by that binary. The existing host/index/issue-repository findings
remain visible; the communication row adds one warning only on BREACH/UNKNOWN.
Captures above use the actual output; checked-in copies redact the sandbox path.

Published evidence: `art-bfb60782` (`cas-7e24`).
