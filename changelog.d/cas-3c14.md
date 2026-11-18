### Added — Factory

- **Park proof runs lane CI's own selection** — `cas factory worker-check -- lane-impact` asks the lane CI impact script which tests this diff selects (own tests, then the ring), runs them once under the usual build caps, and records a passing receipt for each changed crate. A bounce that lane CI would have caught in a test the worker never ran now shows up before review. Named-test checks still work; a plan lane CI leaves to the train's full set is reported instead of passing with zero tests.
