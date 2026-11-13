## Core Concepts

### Lane failure attribution

Lane CI reuses explicit failing-test proof only for the recorded base SHA and matching package, binary, test, platform, toolchain and profile. Missing proof triggers a bounded replay; unresolved attribution remains red. Failure receipts are published before base preparation, with separate attribution timings and no additional CI job.
