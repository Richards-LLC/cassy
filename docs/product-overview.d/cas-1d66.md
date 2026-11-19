## Core Concepts

- **Release freeze starts its proof** — `train_freeze` immediately dispatches or adopts one `full-validation.yml` run at the current frozen train tip. It returns the run URL or an explicit skip/failure reason; an absent run carries no release ETA. Repeating the freeze shares dispatch history with the daemon, which retains its single replacement for a run queued without jobs for ninety seconds. A repair tip gets its own run.
