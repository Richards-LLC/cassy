### Fixed — Factory

- **Train notices reach only the train's owner** — Was: a supervisor that did not own a release train still got its cut-ready and post-merge close recovery notices, with the owner refusal quoted in the body, even after it cleared its epic focus. Now: only the owning supervisor is notified, and a supervisor on another machine that does not own the train gets nothing.
