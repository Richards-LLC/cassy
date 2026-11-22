### Fixed — Factory daemon

- **CI watch no longer holds the store writer for seconds on a large project** — Was: each CI poll read every task, closed ones included, while holding the daemon's single store writer, up to 5.9 s on a 7,000-task project, and every other daemon write waited behind it. Now: it reads only open, in-progress, blocked and awaiting-merge tasks (271 of 6,986 on the same project), the only ones a merge-timeout escalation can concern.
