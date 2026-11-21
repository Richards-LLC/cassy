### Fixed — Release trains

- **Release readiness follows the supervisor that froze the train** — After a train is handed to a supervisor on another machine and that supervisor freezes it, the freeze moves the train's ownership to it, so auto-cut and cut notices act on its machine. A refusal now names the owner and its machine. The cut notice counts every delivery merged into the train, including those still waiting for their verdict or batch QA.
