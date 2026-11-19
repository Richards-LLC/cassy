### Fixed — Tasks

- **A task's start brief names the newest published artifact** — Was: after several artifacts were published for one task, `task start` and `task show` named whichever receipt note sat last in the history, so a synced or re-ordered note made it point at an older artifact (cas-8156 named the Nov 13 one after four newer ones). Now: the brief names the receipt with the latest creation time, and a receipt without a time never beats one that has it.
