# Severity × reach

Declare a project's main flows with `main_journeys: [save, chat]` at the top of its existing journey registry. Each ID must name a live journey in that registry. Other journeys are secondary; an undeclared main journey cannot claim main reach.

Main flows can be factory or CLI workflows as well as browser routes. Cassy's registry declares `FACTORY-SUPERVISE`, `FACTORY-MERGE`, `FACTORY-CLOSE`, `CLI-RELEASE`, `FACTORY-SPAWN` and `FACTORY-QA-RECEIPT`. A reproduced broken supervisor workflow uses `severity:broken,reach:main,journey:FACTORY-SUPERVISE` and derives P1. Only reproduced money or data loss on a declared main journey derives P0; requesting P0 alone cannot promote a broken workflow.

File a finding using the existing task labels: `severity:money|data_loss|broken|awkward|cosmetic`, `reach:main|secondary|theoretical`, and `journey:<id>`. Supply the existing `repro` field with the command or steps and observed wrong outcome, plus `affected`. Intake records runtime evidence; labels and priority are normalized together. Update the labels to reclassify a finding; a manual priority cannot override the axes. Ordinary tasks without triage keep their current priority.

On `task action=update`, ordinary labels are additive; a supplied `severity:`, `reach:` or `journey:` label replaces the previous value for that axis. Omitted axes stay unchanged. An explicit empty `labels=""` clears user labels and resets an existing finding to broken/theoretical, retaining `from-finding` provenance and runtime evidence from its notes. Normalization restores the canonical axes, so clearing labels cannot turn a finding into manually prioritized work. To correct a stale P4 finding with a recorded repro, update `labels="severity:broken,reach:main,journey:FACTORY-SUPERVISE"`; the stored priority becomes P1. CLI workflow findings use the same MCP task handler and registry.

Requesting `reach:main` on update requires a `journey:<id>` naming a declared live main journey, either supplied in that update or retained from the task. Missing, empty or undeclared journeys return `finding_main_journey_required` with `axis: journey` before any task fields change. Every successful update, including an unchanged request, prints the resulting priority; structured output includes the task `id` and numeric `priority`. A recorded creation-time reproduction remains available when correcting or clearing labels.

An explicit priority that disagrees with normalized finding labels rejects the entire update with `INVALID_PARAMS` and `rule_id: finding_priority_derived`. The error includes requested, derived and current priority, the labels used for derivation, and `applied_fields: []`; accompanying design, notes and label changes do not save. Retry without `priority` to save the other fields, or change the severity, reach and journey labels to reclassify the finding. An explicit priority matching the derived value remains valid.

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
