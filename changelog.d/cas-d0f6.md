### Fixed — Factory verification

- **Verifier launch leaves the input loop** — Local verifier requests run their exact dispatch claim on the factory's external job lane, so the input loop's store deadline cannot refuse startup. Shared SQLite contention reports the holding operation and acquisition site, including task close. Late handoff ownership changes report a dispatch conflict; contention waits and dropped queue jobs retain the launch attempt budget. The daemon's store wait ends before the client's request deadline.
