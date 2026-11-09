### Fixed — Factory

- **Check the final worker delivery at first park** — A fresh close now uses the pushed tip identified by the task's recorded start, so an earlier failing-test commit cannot hide the final changelog or passing test receipt. Previously parked deliveries keep their ownership and anchor protections.
