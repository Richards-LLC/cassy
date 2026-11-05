### Fixed — Factory

- **Reuse the queue proof at close** — Was: direct-to-main deliveries needed a supervisor to repeat scoped tests after the merge queue had proved their tree. Now: close verifies the successful queue run and its exact landed tree, discovered from the recorded landing or cited by run ID, while retaining the risk, content, CI and QA gates.
