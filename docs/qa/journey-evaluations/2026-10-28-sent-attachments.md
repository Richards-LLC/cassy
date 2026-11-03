# Sent attachment evidence correction

The corrected `cas-195a/LEDGER.md` has **32 cells: 31 PASS, 1 FAIL,
0 NOT EXERCISED**. Labels: 7 real-build, 25 fixture. The real-hub table has
7 PASS; the affected-journey table has 24 PASS and 1 FAIL.

The original HUB-J7 failure remains FAIL and now names defect `cas-1224`.
The four-worker run exhausted its 60-second test budget at
`answer-ask.journey.ts:206`, trace action 170. The separate one-worker control
passed all three tests; it does not replace the original failure.

Portable ledger `art-a033f951` supersedes the stale ledger embedded in
`art-542ca852`. The original real-hub captures, candidate receipt, traces and
logs remain in `art-542ca852`; the four-variant HUB-J24 bundle remains in
`art-a6a828f3`. No evidence verdict was changed.

The ledger also records the merged test-only amendment
`e3b242a0783d2f2f7f780919a2877cc55d722174`: one scoped failed-preview route
binding in `paste-content.journey.ts`. Its `typecheck.log` exits zero and its
`HUB-J24.log` records four desktop/390px light/dark cases passing at one worker,
under the task artifact directory's `merged-route-control/` folder.
The original real-hub walks remain bound to candidate
`5fc47b1db169c2522f21fba9cb98d986b6f176b0`; no candidate rebuild is claimed.

This correction addresses the two evidence-gate findings in
`ver-3fb388e8fcbb`: failed-cell ownership and headline arithmetic. A parser
confirmed all 32 eight-column rows, their verdict totals, the label split and
the defect reference. It changes no product code and runs no new browser or
Rust checks. The verifier had stopped before judging acceptance criteria.
