### Fixed — Close gates

- **Park and the capped worker-check name the same changed tests** — Was: after a queued successor rebased onto a merged predecessor, park diffed from a stale local target ref and demanded the predecessor's unchanged tests, which the worker-check command could not cover; rerunning one test also dropped the five-pass credit of every other test in the same receipt. Now: both read one shared proof base (the newest merge-base with the target), and a rerun withdraws credit only from the tests it reran.
