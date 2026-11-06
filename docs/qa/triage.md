# Severity × reach

Declare a project's main flows with `main_journeys: [save, chat]` at the top of its existing journey registry. Each ID must name a live journey in that registry. Other journeys are secondary; an undeclared main journey cannot claim main reach.

File a finding using the existing task labels: `severity:money|data_loss|broken|awkward|cosmetic`, `reach:main|secondary|theoretical`, and `journey:<id>`. Supply the existing `repro` field with the command or steps and observed wrong outcome, plus `affected`. Intake records runtime evidence; labels and priority are normalized together. Update the labels to reclassify a finding; a manual priority cannot override the axes. Ordinary tasks without triage keep their current priority.

Verification and QA issue objects carry `triage`, for example:

```json
{"triage":{"severity":"broken","reach":"main","journey":"save","repro":"Save an idea; it asks a dead-end question"}}
```

Missing runtime reproduction means theoretical, even when the caller asks for main reach. Older unclassified findings default to theoretical. QA retains pre-existing findings as linked backlog tasks rather than discarding them.

| Severity | Main, reproduced | Secondary, reproduced | Theoretical |
| --- | --- | --- | --- |
| Money or data loss | P0 | P1 | P4; P2 if the threatened journey is declared main |
| Broken output or flow | P1 | P2 | P4 |
| Awkward | P2 | P3 | P4 |
| Cosmetic | P3 | P4 | P4 |

The existing ready queue sorts the derived priority. Its main-journey failures come before theoretical findings. Staffing theoretical work while a main-journey FAIL is unowned returns a nudge naming the unowned work; it adds no gate.

Set persistent operator intent with `cas config set factory.focus_mode "visible chunks first"`. Workers, queued assignments, recycled workers and verifier handoffs inherit it; independent QA uses the same worker startup path. Clearing the setting restores briefs without a focus line.

The gabber Nov 4–5 replay lives in `gabber_findings_derive_priority_and_ready_order`: canned writer lines, dead-end save questions, leaked prompt text and the chat composer rank ahead of crash windows, contradictory reader output, doubtful corrections and unreachable voice paths. Money/data-loss hypotheses on main journeys retain the explicit P2 exception.
