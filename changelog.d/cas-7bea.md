### Fixed — Factory test proof

- **Playwright setup projects no longer block stability proof** — With the standard pattern of a journeys project that depends on a setup project, the capped Playwright runner now accepts setup tests that ran once, as Playwright schedules them, beside the five repeats of each changed journey. A failing setup test still fails the run. A changed setup test still needs five passes of its own, and the refusal names the capped command that runs its file.
