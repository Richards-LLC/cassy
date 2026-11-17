### Fixed — Release

- **A release cut on another machine admits members reviewed elsewhere** — Was: approvals lived only in the reviewing machine's database, so a cut on the other machine refused with REVIEW_MISSING and no supported remedy. Now: an approved pre-merge verdict is also recorded on its task row, which syncs with the task, and the cut admits that exact reviewed delivery when this machine has no verdict rows of its own.
