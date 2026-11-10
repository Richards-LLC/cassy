### Fixed — Factory guidance

- **Rebased-delivery refusals keep merge ownership clear** — When close finds a task's recorded anchor rewritten away, it now tells the worker to park the current task tip and let the supervisor merge it, instead of telling the worker to merge it into the parent branch.
