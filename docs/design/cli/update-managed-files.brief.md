# cas update managed-file repair

| Field | Contract |
| --- | --- |
| First two lines | The managed-files receipt says current, committed, staged, conflicts or commit failed, with the pending action where needed. |
| Scannable | One compact managed-files line per project; doctor has one managed file drift finding. |
| Readable | A refused standalone sync shows its cause and one hook-scope inspection command once, wrapped at 80 columns; a staged repair names `git diff --cached` for review without assuming why the commit was held. |
| Machine output | Update adds managed_files with outcome, untracked, staged, commit and conflicts to its existing JSON receipt; a failed commit adds commit_error and outcome commit_failed, while doctor retains its checks shape. |
| Omitted | Builtin per-path inventories remain in JSON and existing verbose sync details; no push or unrelated project changes enter the repair commit. |

A normal Git hook refusal is a failed managed commit, with Git's stderr, the
effective hooks directory and a read-only hook-scope inspection step. A hook's
presence alone never identifies it as the refusing hook. Standalone sync prints
a short pointer in its list and leaves the cause/remedy to the final error.
Long path tokens are shortened with a `cas update --sync --json` escape; JSON
retains the original cause and directory. The existing failed
project phase supplies the sweep finding and nonzero final status; standalone
sync also fails. It never becomes a user file-conflict warning. Hook enforcement
and the existing summary renderer stay authoritative. User staging remains a
pending commit for review, and no hook is bypassed or rewritten.

Trunk, the configured epic base, and `epic/*` integration branches retain the
repair as staged changes. Their managed-files receipt uses the existing
`staged` outcome and null commit; it never implies that an automatic commit
was attempted. The compact staged line is scoped for terminal QA, with the
operator recovery procedure in ARCHITECTURE.md.

## Critique

The conflict receipt is the scoped terminal surface. A preflight conflict now
returns its own verdict without a surrounding success message, and the sweep
keeps that verdict as a warning. Existing detailed successful-sync output is
outside this renderer change.

`terminal-qa: PASS update-managed-files · 12 runs · 0 fail · 0 warn · 0 allowed`

Captures: task cas-6718 artifacts, `terminal-qa/report.md` (actual native binary,
80/120 columns, four palettes, pipe, C locale, NO_COLOR and JSON).

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | First line says “1 conflicts preserved” and names `cas update --sync --json`. |
| Fit | 5 | One compact conflict row; JSON names the preserved path. |
| Craft | 4 | The 70-column row fits at 80 and uses the same managed-files grammar as other outcomes. |
| Theme safety | 5 | Default foreground and ASCII text pass every palette and monochrome capture. |
| Machine contract | 5 | JSON is one document; human pipes have no redraw or cursor controls. |

### cas-32e3 staged receipt

`terminal-qa: PASS cas-32e3-managed-staged · 12 runs · 0 fail · 0 warn · 0 allowed`

```text
Managed files: staged; review with git diff --cached
```

The capture wrapper runs the actual `cas update --sync` in a fresh Git fixture
on `staging`, verifies that HEAD stays unchanged and a repair remains staged,
then emits only this scoped receipt (or its native `managed_files` JSON).
Full native stdout/stderr and the before/after refs remain in cas-32e3's task
artifacts alongside the 80/120-column, four-palette, pipe, C-locale, NO_COLOR
and JSON captures. This receipt does not grade the unchanged verbose sync list.

Candidate: `target/debug/cas`, `cas 3.70.0 (f0a6b60 2026-10-21)`, built by the
capped named update-sync integration test; binary mtime 2026-10-21 14:36:25 UTC.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | One line says staged and gives the copyable `git diff --cached` review command. |
| Fit | 5 | The 52-cell scoped receipt fits every 80/120 capture without a repeated inventory. |
| Craft | 4 | The established managed-files grammar stays ASCII and needs no wrapping. |
| Theme safety | 5 | Default foreground, no SGR, and identical dark/light/Solarized/monochrome text. |
| Machine contract | 5 | Native JSON remains one document with outcome staged and null commit; every real fixture preserves HEAD. |

Scored by ready-bear-59 on 2026-10-21; all mechanical checks and score floors pass.
