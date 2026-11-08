### Fixed — Release latency receipt

- **The release receipt measures first commit to main for an untracked train** — Was: a train cut without a tracked release receipt printed `FIRST_CHILD_TO_MAIN_SECONDS=unavailable`, so the thirty-minute commit-to-main budget could not be checked. Now: the receipt falls back to the earliest commit of the merged release PR and prints the measured seconds.
