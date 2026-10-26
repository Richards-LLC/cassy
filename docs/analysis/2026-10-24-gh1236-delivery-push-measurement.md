# GH #1236: live factory delivery-push measurement

Task `cas-07e5` measures the hosted lane round trip left open by the v61
release-speed work. The lane starts from v63 epic commit `6c75ef1388e5` with no
source diff or natural test failure to repair. No synthetic failure or repair
push was staged. Under the supervisor's fallback, this report is a genuine
task delivery push; the exact hosted run and phase timings are recorded in the
`cas-07e5` proof artifact and the follow-up comment on [GH #1236](https://github.com/Richards-LLC/cassy/issues/1236).

At capture, the repository has `CAS_LANE_SLIM=true` and
`CASSY_LANE_SELF_HOSTED=enabled`. A factory branch push enters `.github/workflows/lane.yml`.
The slim lane classifies the task delta and, for a docs-only delta, runs Docs
Lint. The docs-only sample gives a real hosted commit-to-green interval and
shows the actual classifier/job selection and any duplicated work on this
lane. It does not exercise the test-only repair predicate, Commander web tests,
journeys, or Rust checks. Do not treat this sample as a direct measurement of
test-only repair duration or as evidence that those check workloads shrank.

The issue update separates hosted queue/admission time, classifier time,
selected checks, and skipped/redundant work. It leaves the repair-specific
wall-clock question open if the measured delta cannot establish it.
