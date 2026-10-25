# Configured channel diagnostics

| Field | Contract |
| --- | --- |
| First two lines | Doctor names whether configured channels need attention; the next action is `cas violet channels` for full details and refreshed evidence. |
| Scannable | A doctor integration row summarizes configured findings; session start lists unchecked, stale or flagged channel IDs from local evidence. |
| Readable | Stale evidence carries its age policy explicitly; flags keep missing mappings, unknown authority and unverified Cloud bindings distinct. |
| Machine output | Doctor retains its existing checks JSON with status, message and remediation; `cas violet channels --json` retains its existing report. Session-start uses the hook's additionalContext. |
| Omitted | No credential, activation, consumer exemption or claim authority is inferred from a local diagnostic snapshot. Healthy cached channels add no startup context. |

Doctor checks at most 16 configured channels under a shared three-second HTTP
budget, using GETs only. Extra configured channels remain unchecked. It does not
enumerate Cloud-only channels or create a supervisor registration. The full
`cas violet channels` inventory remains the detailed read-only diagnostic.

Both diagnostics save `.cas/violet-channel-check.json` atomically. It contains
project/team identity, observation time and configured channel flags, with no
credentials. Session start only reads local files. Missing, invalid or differently
scoped evidence and newly configured channels are unchecked. Evidence at least
24 hours old, or dated in the future, is stale. Stale flags are retained and
labeled stale; current bindings require another check.
