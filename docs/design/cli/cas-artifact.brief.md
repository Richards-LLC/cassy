# `cas artifact` CLI brief

| Field | Contract |
| --- | --- |
| **First two lines** | A committed `publish` opens with `[OK] published <name> - <size> - <artifact_id>`; a retained local record opens with `[WARN] local-only` and its id. The next line names the storage outcome, including remote failure status when available. Only a completed Cloud commit claims publication; a local record remains citable on this machine. A refusal states what is wrong and where publishing is allowed. |
| **Scannable** | `list` prints one fixed-column row per artifact — name, size, status, id — sorted newest first, so the row just published is the first one. `show` is a labelled term/value block in a stable order (task, type, sha256, status, then any cloud and Slack references). Sizes use decimal units, matching the file dialog the operator read the size from. |
| **Readable** | The storage line states Cloud availability or local-only retention. A failed upload preserves its HTTP status and complete cause without a success headline; the local artifact id remains available for inspection. MCP uses the same distinction: `Published` only after commit, otherwise `Local-only artifact`. |
| **Machine output** | `--json` is one object per command: `publish` returns `artifact_id`, `task_id`, `name`, `mime`, `size_bytes`, `sha256`, `status`, `storage` (`committed` / `not_live` / `not_logged_in` / `upload_failed`), `storage_detail`, `cloud_url`, `source`; `show` returns the stored record; `list` returns an array of records. The human verdict line never appears on stdout in JSON mode. |
| **Omitted** | The pre-signed upload URL, in every mode — it is a credential, so it is never printed, never persisted, and never included in a failure report. The full digest is shortened to `b94d27b9…cde9` in `publish` (whole in `show` and `--json`), and per-step upload timings are not reported at all. |

## Fetch

| Field | Contract |
| --- | --- |
| **First two lines** | `[OK] verified <name> - <id>` proves the download matched the published bytes; the next line is the local file to read. |
| **Scannable** | One verdict followed by one path; no table or transfer progress. |
| **Readable** | A refusal names authentication, a missing reference, or the size/digest disagreement before a local copy is saved. |
| **Machine output** | One object: `artifact_id`, `path`, `sha256`, `size_bytes`; no human banner. |
| **Omitted** | Signed view URLs and credentials stay in memory; artifact metadata is available through `show`. |

Like publish, fetch mutates disk and the local ledger. Its real CLI and MCP
paths share the verified download implementation; the portable-proof handler
regression covers both surfaces, including corruption refusal.

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
`captures/after/`. `publish` is not gated because it mutates: each run mints a
new record and a new id, so the capture would not be reproducible. Its two
rendered lines are the same verdict-plus-detail shape as `show`, which is
gated, and its refusal paths are covered by
`cas-cli/tests/artifact_publish_test.rs`.
