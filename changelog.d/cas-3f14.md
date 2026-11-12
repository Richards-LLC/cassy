### Fixed — Factory guards

- **Playwright's `--shuffle <seed>` no longer trips the browser test tier** — Was: a named single-spec run with `--shuffle 42` was refused because `42` was read as a file filter. Now: the optional seed is consumed with its flag, unfiltered runs and `--full` are still refused, and a refusal names the token it could not read.
