# `cas artifact` CLI brief

| Field | Contract |
| --- | --- |
| **First two lines** | A committed `publish` opens with `[OK] published <name> - <size> - <artifact_id>`; a retained local record opens with `[WARN] local-only` and its id. The next line names the storage outcome, including remote failure status when available. Only a completed Cloud commit claims publication; a local record remains citable on this machine. A refusal states what is wrong and where publishing is allowed. |
| **Scannable** | `list` prints one fixed-column row per artifact — name, size, status, id — sorted newest first, so the row just published is the first one. `show` is a labelled term/value block in a stable order (task, type, sha256, status, then any cloud and Slack references). Sizes use decimal units, matching the file dialog the operator read the size from. |
| **Readable** | The storage line states Cloud availability or local-only retention. A failed upload preserves its HTTP status and complete cause without a success headline; the local artifact id remains available for inspection. MCP uses the same distinction: `Published` only after commit, otherwise `Local-only artifact`. |
| **Machine output** | `--json` is one object per command: `publish` returns `artifact_id`, `task_id`, `name`, `mime`, `size_bytes`, `sha256`, `status`, `storage` (`committed` / `not_live` / `not_logged_in` / `upload_failed`), `storage_detail`, `metadata_state` (`task_reference_recorded` / `metadata_unavailable` / `local_only`), `cloud_url`, `source`; `show` returns the local or synced record; `list` returns an array of local ledger records. The human verdict line never appears on stdout in JSON mode. |
| **Omitted** | The pre-signed upload URL, in every mode — it is a credential, so it is never printed, never persisted, and never included in a failure report. `publish` prints the complete 64-character SHA-256 on a 76-column ASCII line; `show` and `--json` also retain the whole digest. Per-step upload timings are not reported. |

## Fetch

| Field | Contract |
| --- | --- |
| **First two lines** | `[OK] verified <name> - <id>` proves the download matched the published bytes; the next line is the local file to read. |
| **Scannable** | One verdict followed by one path; no table or transfer progress. |
| **Readable** | A refusal names authentication, unavailable metadata, or the size/digest disagreement before a local copy is saved. Unavailable metadata does not claim that Cloud bytes are absent. |
| **Machine output** | One object: `artifact_id`, `path`, `sha256`, `size_bytes`; no human banner. |
| **Omitted** | Signed view URLs and credentials stay in memory; artifact metadata is available through `show`. |

Like publish, fetch mutates disk and the local ledger. Its real CLI and MCP
paths share the verified download implementation; the portable-proof handler
regression covers both surfaces, including corruption refusal.

## Peer metadata

Every committed publication records a trusted task reference, including a plain
publish without a prose proof note. Normal task sync carries that reference to
the peer. `show` can inspect it before the first fetch; only a verified fetch
imports the downloaded artifact into the peer's local ledger.

Byte storage and metadata availability are separate outcomes. If the bytes
commit but the task reference cannot be recorded, `storage` remains `committed`
and `metadata_state` is `metadata_unavailable`, with the cause in
`storage_detail`. Missing metadata in `show` or `fetch` returns a nonzero exit
and one JSON error document with `state: metadata_unavailable`, `artifact_id`
and the task ID when known. MCP carries the same state in its error data.
Neither surface concludes that the Cloud object is missing without checking it.

## Why the refusals are this loud

Three of the five outcomes this command has are refusals, and each one is a
thing an agent will get wrong on its first try: a path outside the task's
roots, a file over the ceiling, a symlink that escapes. Each refusal therefore
names the resolved path, the two permitted roots, and — for the ceiling — that
nothing was uploaded, so the next attempt is informed rather than a guess.

## Critique

```text
terminal-qa: PASS cas-artifact-list · 12 runs · 0 fail · 0 warn · 0 allowed
terminal-qa: PASS cas-artifact-show · 12 runs · 0 fail · 0 warn · 0 allowed
```

Both gated at 80 and 120 columns on the dark, light and both Solarized
palettes, plus piped, `NO_COLOR`, `LC_ALL=C` and `--json` runs; captures under
`captures/after/`. `publish` is gated with a fresh private project and ephemeral fake Cloud per
run. This isolates its mutations and gives every receipt a durable record and
wire readback. No inherited account or provider state is used. Its refusal
paths are also covered by `cas-cli/tests/artifact_publish_test.rs`.

### Publish correction (cas-a483)

Candidate `1563617a858295bfe22aee5c381f4730cdb351e3` passed the repository
`scripts/terminal-qa.mjs` gate. The four human receipt flows each ran at 80 and
120 columns on all four palettes, piped, with `NO_COLOR`, and with `LC_ALL=C`.
Every human line fits 76 columns. The unchanged JSON evidence comes from
candidate `8d15b5e2f86387a92a3f41514b06546dffdbbdb7`, as approved by the
supervisor; the correction changes only the human digest line.

```text
terminal-qa: PASS artifact-publish-committed · 11 runs · 0 fail · 0 warn · 0 allowed
terminal-qa: PASS artifact-publish-failed · 11 runs · 0 fail · 0 warn · 0 allowed
terminal-qa: PASS artifact-publish-offline · 11 runs · 0 fail · 0 warn · 0 allowed
terminal-qa: PASS artifact-publish-legacy · 11 runs · 0 fail · 0 warn · 0 allowed
```

The initial candidate exposed a preexisting Unicode digest ellipsis under C
locale (cas-b0ef). Its four failing reports are preserved in the task's
`qa-initial/`; the approved correction prints the complete ASCII digest.
HTML visual inspection is **NOT EXERCISED** because the browser runtime lists
no browser. Supervisor approval #543993 accepts mechanical palette/contrast
checks as the substitute. No visual PASS is claimed.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | `[WARN] local-only` or `[OK] published` leads; the next line names storage, and failure ends with `cas artifact show <id>`. |
| Fit | 4 | One receipt, one storage outcome, full digest; only failed uploads expand into the cause and remedy. |
| Craft | 4 | Full ASCII SHA occupies 76 columns; HTTP403 detail wraps with hanging indents and appears once. |
| Theme safety | 4 | Four palettes, pipe, NO_COLOR and C pass mechanically; plain marks retain their meaning without color. HTML inspection remains unexercised. |
| Machine contract | 4 | Each JSON run is one object with explicit storage/status; exit0 means a usable local record, so callers inspect storage to establish remote delivery. |

Pipe captures for all four outcomes are committed under
`captures/after/cas-artifact-publish/`. Full receipts, isolated wrappers, wire
readbacks, candidate identity and initial failures are under the task's
artifact root and `LEDGER.md`.
