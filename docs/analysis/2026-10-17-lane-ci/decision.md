# Operator decision: delivery CI ownership

Decision pending. Approve moving broad Rust proof out of both factory CI routes, keeping focused worker regressions and the final-tree merge queue, and removing arbitrary dependent-package receipt runs at close. Measured on 30 delivered v46 heads: lane median 16.9 minutes; conservative replay 2.5 minutes, with commit→epic 35.1→20.4 minutes. Removing only the factory/PR row misses 13 broad runs in the fast row.

Separately decide whether to move changed-input no-build rows to pre-push and delete the factory-lane copy. Never move build rows or full script/browser suites there. Map package scripts to the existing 0.001-second memory weight assertion; its absence contributed to the 3.61 queue ejection.

The immediate commit→main model is 31.2 minutes, still over 30; actual batching measured 268.7 minutes. Record the operator's approval before implementation, then measure real deliveries and releases. No CI, gate, hook or runtime changes accompany this analysis. The full map, sources and forecast assumptions are in report.md and its offline HTML counterpart.
