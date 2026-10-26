# Capture and pair selection

| Field | Contract |
| --- | --- |
| First two lines | Show complete or unavailable evidence with pair counts, then the next command or evidence location. |
| Scannable | One verdict, pair and unpaired counts, then findings and the full-path escape. |
| Readable | Findings name the missing revision, mask, auth or budget condition; a capture remedy stays copyable. |
| Machine output | Global `--json` emits one `cas.deliverable.capture/1` or `cas.deliverable.pairs/1` document with refs, counts, findings and warnings; no progress banners. |
| Omitted | Screenshots, credential paths, server logs and source directories stay out of the plain verdict; full evidence paths are in JSON. |

## Critique

terminal-qa: PASS deliverable-pairs · 12 runs · 0 fail · 0 warn · 0 allowed · cas-266f/terminal-qa/pairs/report.json

terminal-qa: PASS deliverable-capture · 12 runs · 0 fail · 0 warn · 0 allowed · cas-266f/terminal-qa/capture/report.json

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Both first lines give the verdict and pair counts; capture adds elapsed seconds, followed by evidence and the JSON escape. |
| Fit | 5 | One verdict with counts; findings expand only on refusal. |
| Craft | 4 | Same formatter and grammar as brand/finalize; no overflow at 80 columns. |
| Theme safety | 5 | Four palettes, NO_COLOR and C locale pass. |
| Machine contract | 5 | Global JSON is exactly one typed document; no progress on stdout. |

Scored by ready-raven-67 on 2026-10-26 against candidate4ac73527d. Setup failures (wrapper syntax and cwd banner) are retained separately; the final captures start with the command verdict. The CLI implementation is unchanged from the measured candidate.
