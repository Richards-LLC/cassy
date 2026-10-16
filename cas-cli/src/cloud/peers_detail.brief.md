# `cas peers --detail` ownership board

| Field | Contract |
| --- | --- |
| First two lines | Outcome and peer/live count come first, followed by the repository; partial observations are explicitly labelled in each section. |
| Scannable | Group by owner, then machine/session, claim-owned epic and target, linked tasks, PR observations, workers, and waiting merges. |
| Readable | Each section states private, stale or unavailable with its observation time; claims establish ownership while activity remains observational. |
| Machine output | One PeerReport JSON document retains full values, section states/timestamps, claims, epic targets, tasks, PR heads, worker machines, roots and waiting reasons; no ANSI/banner. |
| Omitted | No Slack threads/asks, control-loop remote crawls, target writer queue, inferred PRs or locally registered remote spawn targets. Long terminal values use the existing --json escape. |

The board is read-only: conflicts are visible, not prevented. Plain default foreground, state words and ASCII labels carry every meaning on light/dark and Solarized terminals. Private claims are not an empty task inventory. Unknown PR records stay unknown even when a branch is known.

## Critique

The actual CLI was captured against a transient Cloud fixture with two machine
identities, private work, old PR evidence, unknown task details and all four
waiting reasons. The terminal gate passed 12 runs at 80/120 columns across dark,
light and both Solarized palettes, including pipe, NO_COLOR, C locale and JSON;
0 failures, 0 warnings, maximum human line width 71 cells. Durable captures and
the final receipt are under the assigned `cas-cd42/terminal-qa/cas-peers-detail/`
artifact directory. Final delivery reruns bind this review to the delivered code.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | First two lines give peer/live counts and repo; each owner and task begins with its outcome, and observation diagnostics follow. |
| Fit | 4 | Detail groups preserve distinct private/stale/unavailable states; parked assignment and claim ownership stay separate. |
| Craft | 4 | Indentation groups machines, roots, tasks and receipts; long values name the existing --json escape. |
| Theme safety | 5 | Default foreground carries every word; state labels remain identical across four palettes, ASCII and NO_COLOR. |
| Machine contract | 5 | One typed PeerReport supplies both outputs; JSON keeps full roots, PR heads, section times and limitations. |

Reviewed by sharp-koala-95, 2026-10-16. The fixture is labelled as fixture evidence;
actual Cloud/empty-board and CLI-help captures are separate real-build evidence.
