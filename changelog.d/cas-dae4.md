### Fixed — Worker replacement

- **Replacement workers are told to keep working instead of blocking for a rebind** — When a replacement worker starts a task whose recorded branch belongs to its predecessor, `task start` now tells it to keep working, commit and push its delivery, and then ask the supervisor for `rebind_branch`. Before, it told the worker to ask for the rebind first, and that rebind refuses until the replacement has delivered. An untouched predecessor branch is still rebound automatically at start (GH #1579).
