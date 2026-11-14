### Fixed — Factory

- **QA candidate build failures keep their evidence** — When the journey snapshot step fails, `cas qa candidate build` copies its build log under the task's artifacts directory before removing scratch and prints that path. A step refused by memory admission is reported as not started, with the sampled budget, estimate and reserve, instead of as a build failure.
