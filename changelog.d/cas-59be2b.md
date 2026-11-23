### Fixed — Factory close proof

- **Merging the released trunk no longer adds other tests to a task's proof** — When a worker on an epic lane is told to merge `main`, close no longer counts the released commits that the merge brings in as the task's own work. Those commits often have no task reference. The task now owes five-pass stability only for the tests it changed itself, before or after the sync.
