### Fixed — Factory

- **Merge-queue repair cycles survive hand repairs and busy queues** — A task sent back by hand after a merge-queue ejection no longer bounces Open when GitHub lists the old failed run again, and its next real failure starts its one repair cycle instead of paging the operator. When the merge-group entry ahead is still running, Cassy waits one poll; if it is still running or unreadable, the failure is not counted against the task. Repeat ejections reuse the failed run's job and log instead of downloading the log on every poll.
