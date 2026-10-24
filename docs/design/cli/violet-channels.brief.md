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
