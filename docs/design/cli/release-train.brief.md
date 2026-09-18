# Release-train stage output brief

**First two lines** — The stage either passed or names its blocker, and the next copyable command is visible immediately.

**Scannable** — One verdict line followed by stage, branch, receipt, and pull-request rows; warnings are prefixed with `WARN` and failures with `ERROR`.

**Readable** — A short cause and one remedy explain missing drafts, partial receipts, and queue failures; detailed adapter output remains in the run directory.

**Machine output** — The stage scripts currently expose line-oriented receipts rather than `--json`; receipt files use stable uppercase keys and no credential values.

**Omitted** — Slack message bodies, report bytes, and credentials never print in the terminal; they remain in the draft, adapter receipt, report files, or configured secret store.

## Critique

terminal-qa: PASS release-train-print-run-dir · 11 runs · 0 fail · 0 warn · 0 allowed · .cas/artifacts/terminal-qa/release-train-print-run-dir/report.json
| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | The stage verdict and receipt path are printed together; `--print-run-dir` is intentionally a single receipt line. |
| Fit | 4 | Stage output uses compact verdict/remedy lines while report and adapter detail stay in the run directory. |
| Craft | 4 | Paths, branch names, and SHAs remain intact without decorative wrapping in the terminal gate. |
| Theme safety | 4 | The command emits plain text and no terminal colour, so light, dark, and `NO_COLOR` captures agree. |
| Machine contract | 4 | Piped output is stable line-oriented text and durable receipts carry stable uppercase fields. |

Scored by Codex on 2026-09-18; floor holds.
