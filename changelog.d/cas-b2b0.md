### Fixed — Factory

- **A dead supervisor's unfinished train-merge queue no longer blocks the new train owner** — Was: after a train changed owners, the ended supervisor's held queue refused the new live owner's `train_merge` with an owner-generation error and held single merges behind it until the state file was edited by hand. Now: once the queue's recorded supervisor is no longer live, the current train owner adopts the queue, settles entries already merged and continues; a live prior owner still holds it.
