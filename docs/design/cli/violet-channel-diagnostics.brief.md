# Configured channel diagnostics

| Field | Contract |
| --- | --- |
| First two lines | Doctor names whether configured channels need attention; the next action is `cas violet channels` for full details and refreshed evidence. |
| Scannable | A doctor integration row summarizes configured findings; session start lists unchecked, stale or flagged channel IDs from local evidence. |
| Readable | Stale evidence carries its age policy explicitly; flags keep missing mappings, unknown authority and unverified Cloud bindings distinct. |
| Machine output | Doctor retains its existing checks JSON with status, message and remediation; `cas violet channels --json` retains its existing report. Session-start uses the hook's additionalContext. |
| Omitted | No credential, activation, consumer exemption or claim authority is inferred from a local diagnostic snapshot. Healthy cached channels add no startup context. |

Doctor checks at most 16 configured channels under a shared three-second HTTP
budget, using GETs only. A caller deadline also bounds blocking DNS resolution;
probes that finish after the deadline do not refresh cached evidence. Extra configured channels remain unchecked. It does not
enumerate Cloud-only channels or create a supervisor registration. The full
`cas violet channels` inventory remains the detailed read-only diagnostic.

Both diagnostics save `.cas/violet-channel-check.json` atomically. It contains
project/team identity, observation time and configured channel flags, with no
credentials. Session start only reads local files. Missing, invalid or differently
scoped evidence and newly configured channels are unchecked. Evidence at least
24 hours old, or dated in the future, is stale. Stale flags are retained and
labeled stale; current bindings require another check.

## Critique

terminal-qa: PASS channel-startup-complete · 11 runs · 0 fail · 0 warn · 0 allowed

Receipt: task `cas-cf25`, `terminal-qa/channel-startup-complete/report.json`.
The capture extracts the complete diagnostic section from the real candidate's
SessionStart JSON; service responses are isolated HTTP fixtures. Existing
personal-pattern and suggestion reads are recorded separately from channel
reads. This does not establish deployed binding health or consumer authority.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | First line gives the affected count and cached-evidence label; line two gives the refresh command. |
| Fit | 4 | Healthy cached channels add no warning; stale or flagged IDs get one row each, bounded to eight rows. |
| Craft | 4 | Wrapped flags retain a hanging indent; the full stale finding remains visible at 80 columns. |
| Theme safety | 5 | Plain text passes light, dark, Solarized, NO_COLOR and C-locale captures. |
| Machine contract | 5 | Actual channels/doctor/hook invocations retain one JSON document; extraction is confined to the QA harness. |

Scored by the implementing worker on 2026-10-25; independent review is separate.
