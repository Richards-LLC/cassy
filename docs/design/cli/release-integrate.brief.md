# Release assembly metadata errors

| Field | Contract |
| --- | --- |
| First two lines | State that assembly failed and name the metadata blocker before listing paths. |
| Scannable | List unsupported paths separately, then preservation and fresh-worktree guidance. |
| Readable | Explain that prep carries prior receipts after assembly; show Git's command, exit code and diagnostics when Git fails. |
| Machine output | This script has no JSON mode; exit status remains 0 for assembly success and 1 for refusal. |
| Omitted | Build timing and publication status belong to the train receipts, not the metadata error. |

The terminal capture scope is the unsupported-metadata refusal. Git commands
and Git's own diagnostics retain their complete bytes for recovery, including
long absolute paths; subprocess contracts cover their content and restoration.
QA fixtures invoke the production train and integration scripts against local
Git repositories; they never publish, announce or run Cargo.

## Critique

terminal-qa unavailable: GNU script runner on BSD macOS; checked by hand

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | The first two lines state failure and name unsupported release paths. |
| Fit | 5 | All 11 nonempty captures fit 80 columns without truncation. |
| Craft | 4 | Relative paths form a separate list; preservation and recovery follow. |
| Theme safety | 5 | Default foreground, ASCII and no escape controls in every palette. |
| Machine contract | 4 | Exit status and stderr behavior are unchanged; no JSON mode exists. |

Scored by wise-raven-87, 2026-10-03. Captures and the fallback ledger belong to
task cas-8b34 under terminal-qa/. The unmodified runner awarded a false PASS to
empty BSD captures and discarded piped stderr; follow-up cas-c161 owns that bug.
