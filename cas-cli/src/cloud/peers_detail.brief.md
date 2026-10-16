# `cas peers --detail` ownership board

| Field | Contract |
| --- | --- |
| First two lines | Outcome and peer/live count come first, followed by the repository; partial observations are explicitly labelled in each section. |
| Scannable | Group by owner, then machine/session, claim-owned epic and target, linked tasks, PR observations, workers, and waiting merges. |
| Readable | Each section states private, stale or unavailable with its observation time; claims establish ownership while activity remains observational. |
| Machine output | One PeerReport JSON document retains full values, section states/timestamps, claims, epic targets, tasks, PR heads, worker machines, roots and waiting reasons; no ANSI/banner. |
| Omitted | No Slack threads/asks, control-loop remote crawls, target writer queue, inferred PRs or locally registered remote spawn targets. Long terminal values use the existing --json escape. |

The board is read-only: conflicts are visible, not prevented. Plain default foreground, state words and ASCII labels carry every meaning on light/dark and Solarized terminals. Private claims are not an empty task inventory. Unknown PR records stay unknown even when a branch is known.
