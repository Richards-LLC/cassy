### Fixed — Factory

- **The first merge into a train from another machine passes premerge verification** — Was: when the train existed only on origin, premerge verification compared it against a local branch that did not exist yet and failed with `unknown revision`, so the first merge needed a retry. Now: with no local branch, origin's copy is the only authority and the candidate builds on the first call; a diverged local branch is still refused.
