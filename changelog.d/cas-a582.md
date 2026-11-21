### Fixed — Factory

- **Failed Playwright evidence survives worker-check** — When a capped Playwright run fails, worker-check now copies its traces, error contexts and JSON reports into the task's artifact directory and names that directory in the failure. Before, the paths Playwright printed pointed into a temporary directory that was deleted as soon as worker-check returned. Passing runs keep nothing.
