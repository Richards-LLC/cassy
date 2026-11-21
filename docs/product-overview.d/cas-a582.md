## Core Concepts

A failed capped Playwright run keeps its evidence. Worker-check copies the run's traces, error contexts and JSON reports to `<artifacts>/<task>/worker-check/playwright-<head>-<time>/` and names that directory in the failure. Passing runs store nothing.
