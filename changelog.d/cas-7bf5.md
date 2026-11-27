### Fixed — Factory

- **Workers can inspect local dependency symlinks** — The credential guard no longer mistakes quoted `find -lname` link-target patterns for credential reads. Direct credential-store access remains blocked.
