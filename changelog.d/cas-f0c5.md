### Fixed — QA

- **A QA ledger scaffold is valid even when the task title quotes a verdict line** — Was: `cas qa ledger init` copied a title such as "CI flake gate prints 'FAIL: 0 flaky test(s)'" into the `scope:` line, and `cas qa ledger check` then read that text as a second FAIL count and refused the ledger with "duplicate headline count". Now: the check reads headline counts only outside the free-text title, `scope:` and `surface:` lines, so the generated scaffold validates without hand edits.
