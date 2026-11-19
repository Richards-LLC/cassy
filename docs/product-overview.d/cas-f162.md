## Core Concepts

- **Release closeout issue closes** - After a release, closeout closes each GitHub issue its shipped tasks reference. Each close runs under its own 30-second deadline, up to three attempts with a growing pause, with at most four closes in flight; the ownership and task-state checks still run per issue before any write. An issue that still fails is listed by number in the closeout notice and stays open, so the next closeout pass reads it as open and tries it again.
