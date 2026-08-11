# cas-4e3a final measurement — negative result

## Decision

Do not merge PR #240. Its caching machinery makes the dominant **fresh-SHA** required path materially slower, violating the operator CI-speed rule.

## Comparable CI receipts

| Measurement | Required fan-in | Full-suite job | Compile/source path | Test execution | Result |
| --- | ---: | ---: | ---: | ---: | --- |
| Existing warm baseline, PR #227 attempt 2 | 6m06 | 5m52 | 3m16 compile | 93.442s | Baseline |
| PR #240 fresh SHA `61f97492`, run 31528056042 | 8m33 | 8m22 | 5m44 source path | 91.593s | Regressed by 2m27 required-path |

The fresh-SHA run passed all 5,464 tests. The suite setup restored the 701 MB workspace cache with a full key match, and sccache reported 665 hits / 2 misses (99.70%). Cargo nevertheless recompiled from `proc-macro2` onward; the remaining test-binary/link work is non-cacheable. The source path was followed by 1m44 archiving (including a 1m38 Cargo compile), making the candidate worse even though the exact-revision archive was smaller (433.5 MB with debug info disabled).

## Floor and next levers

The practical architecture floor is about six minutes: approximately 5m44 non-cacheable compile/link work plus 1m32 execution in this CI configuration. The five-minute target is not reachable through caching levers alone.

Candidates deliberately not implemented:

1. Prebuilt exact-SHA test binaries — helps only same-SHA reruns; this behavior was measured, but it cannot improve the fresh-SHA headline.
2. Suite partitioning across parallel required jobs — revisit carefully; the prior sharding experiment had a macOS-failure misattribution (see GitHub #234 timeline).
3. Larger paid runners — the remaining lever most likely to improve the compile/link-dominated fresh path.

## Durable evidence

- `baseline-warm-attempt2.json` and `baseline-warm-suite.log`: baseline receipt.
- `pr240-cross-sha-run.log`: final fresh-SHA run 31528056042, including cache restore and sccache statistics.
- `tier-contract-workspace-cache.log`: local CI-tier contract receipt (118 passed, 0 failed).

