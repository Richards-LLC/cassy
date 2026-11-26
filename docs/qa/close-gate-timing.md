# Close gate timing evidence

G-55 could repeat the G-56 delivery-content check with a fresh allowance after
fetching an unchanged target. The measured fix skips that duplicate check.

Both measurements used the same `develop` merge fixture: the delivery commit
is reachable, its changes are missing, and a supervisor supplies the merged
receipt. The only input varied within each run was task notes length. The
fixture opens the copied notes read-only and never closes a live task.

```sh
cargo nextest run -p cas --lib -E 'test(merged_override_notes_ab_cas_c129)'
```

| Notes (bytes) | Baseline median | Delivery median | G-56 checks per close |
| ---: | ---: | ---: | --- |
| 42 | 1,541 ms | 1,301 ms | 2 → 1 |
| 164,055 | 1,518 ms | 1,285 ms | 2 → 1 |

Each median uses five samples. Every close refused the missing content and
left the task awaiting merge. Longer notes did not slow this fixture; these
timings do not estimate production latency or exercise a deadline expiry.

The `cas-c129` task artifacts `MERGED-OVERRIDE-BOUND.log` and
`MERGED-OVERRIDE-HARNESS.patch` contain both checked tree IDs, capped receipts,
raw measurements, and the temporary test. The treatment uses the delivered
Rust source plus that test only; the test is not part of the delivery.
