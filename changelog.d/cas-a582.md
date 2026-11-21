### Fixed — Factory

- **Failed Playwright evidence survives worker-check** — A capped Playwright run now writes its traces, error contexts and JSON reports under the task's artifact directory. When the run fails, worker-check keeps that directory and names it in the failure, so the paths Playwright printed stay readable after the command returns. Before, they pointed into a temporary directory that was deleted on return. Passing runs still clean up after themselves.
