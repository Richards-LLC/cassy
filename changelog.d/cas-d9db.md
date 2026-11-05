### Changed
- Agent instructions are about a third shorter and open with one rule: build the simplest complete thing, hit every mark, delete before you add, and treat speed as part of done. Supervisor, worker and reviewer instructions no longer restate gates that the tools already refuse and explain.
- The task verifier asks whether work is overbuilt, whether a smaller change would fully solve it, and whether it slows the flow without catching a real failure. A named smaller sufficient change, or an unearned wait, is rejected like broken work.
- `/finish` has each worker push and park before shutdown, and never forces past unsaved work without the operator.
- `cas ledger report` shows commit-to-main and main-to-publish times, plus the complexity a release adds: lines added and removed, new config keys, new checks.
- The supervisor's "no wake source" warning now appears only when the session has open work.

### Fixed
- Agent instructions now agree on who closes a task: the worker forwards a verification request once, then retries close itself. The wake rule now says the send result reports whether the recipient woke.
