# JavaScript stability specimens

These files are negative-control data, outside ordinary test discovery. Copy `package.json` and each `*.fixture` file into a disposable package, removing the `.fixture` suffix before running it with its pinned runner.

Select `deterministic` for five passing executions. Select `seeded one in three` with a fresh `STABILITY_FIXTURE_STATE` path outside the checkout for Vitest/Jest; the third invocation fails. Playwright uses `repeatEachIndex`, so `--repeat-each 5 --retries 0 --max-failures 1 --workers 1` stops after the third execution. These cases need no browser.
