# Violet channel bindings

| Field | Contract |
| --- | --- |
| First two lines | State healthy/attention, channel and finding counts, then the repair entry (`cas update`) or agreement between the stores. |
| Scannable | Sort channel IDs; each has separate Violet project, Cloud project and Cloud verification; findings name missing mappings, unknown reads or conflicting ownership. |
| Readable | Remedies use existing `cas integrate violet --channel` and `cas update`; conflicting ownership requires inspection before any explicit remap. |
| Machine output | `--json` emits one object with ok, complete, project_id, channels and errors; each row keeps raw project IDs, configured, Cloud verification/strict activation, flags and remedy. Exit 1 for findings or incomplete inventory. |
| Omitted | No message bodies, lease tokens, credentials or Cloud/Violet internal enrollment details. Violet listings are discovery, not mapping proof or work authority. |

The read-only command uses an ordinary Cloud read credential. Cloud-only discovery needs an existing supervisor registration belonging to the caller and this project; it never creates one. Missing/refused registration and Cloud's unpageable 100-binding limit are explicit incomplete reports. Known/configured channels are still checked with the ordinary authority GET. `slack.channels` holds C.../G... IDs as a JSON list through `cas config set` or a TOML list in project config.

`cas update` also considers configured channels that Violet has not mapped, and preserves its existing durable intake/drain and OrdinaryBindingClient path. The report never activates, pauses, remaps or changes pending activity. Repair retains Cloud's archival-only pre-binding-history policy; no later-slot backfill is added (supervisor decision #546120).

## Critique

terminal-qa: PASS violet-channels · 12 runs · 0 fail · 0 warn · 0 allowed

Receipt: task `cas-4e0c`, `terminal-qa/violet-channels/report.json`.
The gate drives the source candidate through isolated REST fixtures; it does not
prove Cloud deployment or production bindings. The registration fallback repair
leaves the renderer unchanged; final candidate captures are stored with the task.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Verdict and counts on line one, repair entry on line two. |
| Fit | 4 | Sorted channel blocks keep the two project IDs and verification separate. |
| Craft | 4 | Identical row grammar; mixed findings capture measures at most 70 columns. |
| Theme safety | 5 | Four palettes, C locale and NO_COLOR pass with plain ASCII output. |
| Machine contract | 5 | One JSON report, explicit completeness and per-row flags, findings exit 1. |

Scored by the implementing worker on 2026-10-24. This is self-check evidence;
independent delivery review remains separate.

## Configured-channel evidence

The command now saves a local diagnostic snapshot at
`.cas/violet-channel-check.json`, including incomplete/flagged configured rows.
This is a local evidence write; all service calls remain read-only. Doctor uses
the same inspection with a bounded configured-only scope. Session start reads
only that snapshot and warns about unchecked, stale (24 hours) or flagged
configured channels. See [diagnostic output contract](violet-channel-diagnostics.brief.md).
