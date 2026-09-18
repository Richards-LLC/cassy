# `cas knowledge` CLI brief

| Field | Contract |
| --- | --- |
| **First two lines** | The build opens with a verdict and failed-source count; a failed or timed-out run immediately names the source and the one retry command. Status opens with page/source counts and the next detail command. |
| **Scannable** | Build uses one compact summary followed by verbose source rows in source order; status groups counts, then shows one failed-source row per ledger entry when `--full` is requested. |
| **Readable** | `--verbose` explains which source is being distilled and whether it completed; `status --full` prints the persisted failure cause beside each source path, while timeout errors name the in-flight source and elapsed seconds. |
| **Machine output** | `--json` is one stable object with `sources_scanned`, `sources_distilled`, `sources_failed`, `pages_written`, and source rows containing `path`, `status`, and optional `error`; progress and banners never appear on stdout. |
| **Omitted** | Successful per-source rows and provider timing detail stay hidden in the default build view; `--verbose` is the escape, while status omits failure causes unless `--full` is supplied. |

## Critique

Pending implementation and terminal-qa evidence.
