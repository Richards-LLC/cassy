### Added — Factory

- **Drop one fix off the release train** — `cas factory train drop <task> --reason "<failure>"` reverts that child's merge on the train with a new revert commit (no history rewrite), returns the task to its worker with the failure and how to restore the change, and re-enqueues the train PR. Each train gets one repair cycle: a second drop after a re-enqueue changes nothing and goes to the operator as a P0.
- **Trains are cut early and often** — The supervisor gets one "train ready to cut" notice when the train has reviewed fixes merged, no child in progress and no queue run in flight, and one more when the train reaches `factory.release_train_max_age_hours` (default 3) since its first fix. Each notice names the release command. Cassy never cuts a release on its own.
