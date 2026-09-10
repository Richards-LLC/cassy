# Scoped admission output brief

**First two lines:** A small delta is either admitted to the bounded proof surface or sent to the full scoped lane, with the changed-file count and reason visible immediately.

**Scannable:** The classifier uses one verdict line with `eligible`, `files`, `max_files`, and `reason`, followed by a compact comparison line with abbreviated `base` and `head` refs.

**Readable:** Operators read the lane verdict and one next-action sentence; a valid supervisor receipt names the exact branch tip, worktree, mapped targets, and content-addressed proof id.

**Machine output:** These shell surfaces intentionally have no `--json` mode; their stable line-oriented `FAST_ADMISSION`, `SCOPED_PROOF`, and `SCOPED_PROOF_RECEIPT` records are the machine contract, while diagnostics remain separate from the verdict fields.

**Omitted:** Full CI transcripts, per-test output, and receipt internals stay in the CI run or durable artifact path; the gate prints only the admission decision and the identifier needed to retrieve evidence.

## Critique

terminal-qa: PASS scoped-admission-classifier · 11 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-2fcf/terminal-qa-scoped-admission-classifier/report.json

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 5 | `FAST_ADMISSION` starts with the eligibility verdict and changed-file count; the second line names the comparison refs. |
| Fit | 5 | Key/value lines scan cleanly in a pipe, while the proof receipt preserves exact fields for a gate. |
| Craft | 5 | Field names and separators are consistent; abbreviated refs fit the 80-column capture without splitting tokens. |
| Theme safety | 5 | Output is monochrome shell text with no SGR, glyph, or background-dependent styling. |
| Machine contract | 4 | Stable line prefixes and exit codes carry the contract; no JSON mode is exposed by these scripts. |

Scored by the worker on 2026-09-10; floor holds with no mechanical findings.
