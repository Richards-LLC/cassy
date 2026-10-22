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

## Commit guard uncertainty (cas-bb3c)

A guard config read failure keeps the repair staged and adds one short ASCII
warning naming the unreadable commit-guard config, the staged outcome and the
retry action. The existing staged receipt supplies `git diff --cached`.
`managed_files.commit_warning` carries the same warning in JSON; it is distinct
from `commit_error`, so this safe refusal exits zero and remains a warning in
compact sweeps. The guard uses the CAS root passed to sync, including worktrees
without a local `.cas`. Detached HEAD and an unidentifiable default branch keep
repairs staged. This adds no automatic push, reset or config rewrite.

Fresh terminal evidence for this addition belongs to the cas-bb3c task's
commit-bound ledger; earlier captures below cover only their named changes.

## Local-only commits (cas-3b812)

A committed repair keeps the managed-files verdict and adds one ASCII next-step
line: the commit is local-only; push it to origin and open a PR. It fits 80
columns without hiding the action. Staged/current/conflict outcomes do not get
this line. The compact multi-project sweep keeps a committed repair as a warning
with the same pending-PR action, so successful-phase elision cannot hide it.
JSON retains the existing `managed_files` outcome and commit fields and carries
the sweep warning in its existing skills phase shape.
Doctor checks the default branch against its configured upstream, even from a
worker checkout, and warns on Cassy-authored reconcile commits still ahead.
Its existing check JSON carries the same cause and PR remedy. Spawn and lane
refusals name reconcile commits only when all commits on the implicated side
are Cassy-authored managed-file repairs; refs remain untouched.

See the cas-3b812 task's commit-bound `LEDGER.md` for fresh terminal evidence
and critique of this addition. Earlier receipts below cover their named earlier
changes only.

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

### cas-bb3c commit guard warning

`terminal-qa: PASS cas-bb3c-config-error · 12 runs · 0 fail · 0 warn · 0 allowed`

`terminal-qa: PASS cas-bb3c-detached · 12 runs · 0 fail · 0 warn · 0 allowed`

```text
Managed files: staged; review with git diff --cached
WARN: Commit guard config unreadable; repair staged. Fix config, then retry.
```

Supervisor-run candidate `cas 3.72.0 (230f8dc 2026-10-22)` maps to
`230f8dc50c4de89e4d6a88b184f7040829d278ae`. The isolated actual-CLI probe
keeps the caller's factory role and emits the native changed rows; full output,
Git refs, staged paths and config-obstruction readbacks are retained in cas-bb3c
artifacts. `terminal-config-error/report.json` and `terminal-detached/report.json`
cover 80/120 columns, four palettes, pipe, C locale, NO_COLOR and JSON. All 27
readbacks passed, including normal work-branch, staging and unknown-default JSON
controls. No live project or factory configuration was rewritten by the probe.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | The staged verdict gives `git diff --cached`; line two names the config cause and retry action. |
| Fit | 5 | One warning follows the existing receipt; detached HEAD needs only the staged row. |
| Craft | 4 | The warning is 76 ASCII cells at 80 columns, with no wrapping or elision. |
| Theme safety | 5 | Uncolored native text is identical across the four palettes and monochrome captures. |
| Machine contract | 5 | JSON is one document with staged outcome, null commit and optional `commit_warning`; all safe refusals exit zero. |

Scoped text and mechanical captures reviewed by fierce-marten-72 on 2026-10-22;
this does not grade the unchanged full sync inventory or claim a GUI screenshot.
