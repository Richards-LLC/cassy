# Brief: `cas worktree pr-body`

| Field | Contract |
| --- | --- |
| First two lines | The Markdown starts with Summary and its visual; successful `--output` emits no human status banner into the PR body. |
| Scannable | Summary shows the immutable comparison and changed files; Evidence pairs before/after and references the QA bundle; Merge Danger names recorded risk and door. |
| Readable | Git comparison is a shallow text tree; absent observations say “behavior run not supplied”, and absent metadata says “not declared”. |
| Machine output | Stdout is the Markdown document, or `--output` writes that same document for `gh --body-file`; there is no JSON mode or terminal control sequence. |
| Omitted | No assumed rollback classification, inferred test success, full task-note history or policy decision is added. |

## Critique

This command produces a Markdown artifact for GitHub, with literal Git paths
and user-supplied evidence. The task-show MCP response adds recorded metadata
using existing labelled lines. Neither introduces colour, tables or terminal
redraws. Real CLI fixture proof and terminal captures need the assembled binary;
worker Rust execution is deferred to the supervisor's assembly. The Python
release renderer is exercised through the real release stage and shares exact
body fixtures with the Rust renderer.
