### Changed — Factory

- **Workers prove only what they touched** — Worker instructions now say: run the tests that cover the code you touched, plus the new red-green test, which replays the real incident. The full suite runs once on the tree that lands: in the merge queue, else CI, else one supervisor run per epic. The supervisor no longer runs a full-suite precheck before the queue or reruns the whole gate after a fix. Briefs and acceptance criteria name the touched tests, never suites or evals. QA walks affected journeys once per release batch. The new wording is shorter than the old.
