# Worker release-tag handoff

| Field | Contract |
| --- | --- |
| First two lines | State that publication stopped at a supervisor handoff, then show the exact tag and commit. |
| Scannable | One line identifies the published `origin/main` comparison; one copyable command shows the authorized push. |
| Readable | A worker can see that the local annotated tag exists and no audit build or remote push ran. |
| Machine output | `release.sh` has no `--json` mode; this is human stderr with a nonzero handoff status. |
| Omitted | Secrets, local audit paths and guessed publication status stay out of the handoff. |

## Critique

terminal-qa: PASS release-worker-handoff · 11 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-74e3/terminal-qa/report.json

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | The first line names the handoff or the mismatched commit; tag and commit follow. |
| Fit | 5 | All 80/120-column terminal captures pass; the happy-path fixture also rejects lines over 80 cells. |
| Craft | 4 | One copyable push command follows the verified tag object and commit. |
| Theme safety | 5 | Plain text passes dark, light, Solarized, no-color and C-locale captures. |
| Machine contract | 4 | The nonzero handoff status distinguishes a local tag from publication; no JSON mode exists. |

The terminal capture exercised the worker worktree with HEAD behind
`origin/main`; the shell fixture exercises a matching commit and the local-tag
handoff. Rust hook tests remain for the supervisor's assembly run.
