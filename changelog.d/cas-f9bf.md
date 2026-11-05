### Fixed — Factory

- **Merge auto-retry follows the newest CI run** — When a push cancels the CI run on a lane's last code commit, the automatic merge retry now waits on the new tip's running checks. Before, it stopped with "CI was cancelled and no replacement check is running" and blocked the worker while the replacement run was still going.
