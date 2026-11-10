### Fixed — Factory

- **Released deliveries close without metadata overrides** — Was: post-release close could refuse changelog fragments consumed on a release train and codemaps regenerated during integration. Now: close follows the full release history, records the promoting release commit and accepts the generated codemap while continuing to reject missing source or unpromoted changelog entries.
