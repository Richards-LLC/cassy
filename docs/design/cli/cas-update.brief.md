# Brief: `cas update` (human mode)

| Field | Sentence |
| --- | --- |
| First two lines | Whether anything is pending — a newer binary, unapplied migrations — or, after a refresh, whether every project converged. |
| Scannable | `--check`: two labelled rows (Binary, Schema). After a refresh: the verdict line with counts and the project table with one mark per phase. |
| Readable | Per-project detail lines, only for a project whose phase was not `✓`. |
| Machine output | `--json`: one receipt document (`--check` emits the version/migration object; a refresh emits the combined receipt); progress never appears. |
| Omitted | Successful phases' transcripts and the `Run … to …` sentences: the remedy is the command itself under `→`. |

## Rendering decisions

- `--check` opens with `✓ up to date · Cassy 3.17.2 · schema v254` or
  `⚠ update available · 3.17.2 → 3.18.0`, then `Binary` and `Schema` rows, then one remedy.
- The refresh banner became a verdict line: `✓ complete`, `⚠ N not refreshed`, or
  `✗ N projects failed`, with the unchanged count grammar as its detail.
- A failed post-swap refresh names each failing project and its failed phase, then gives one
  copyable remedy (`cas update --all-projects`); the old "outcome is unknown" wording remains
  only when the child truly produced no receipt.
- The `Current version: / Latest version:` pairs, each in accent colour, are gone; versions
  are plain text in the row.

### Service-managed hub refresh

After a binary swap, the hub restart is represented in JSON as `hub_restart` with
`from_version`, `to_version`, and `via` (`service` or `detached`). This receipt is included
in both the combined update document and the post-swap project-refresh document, so the
operator can verify that the installed service picked up the new binary.

## Full update and `--all-projects` (cas-0aa5)

| Field | Sentence |
| --- | --- |
| First two lines | The binary phase line says whether Cassy is current or what it was updated to; the projects phase line says how many projects failed, warned or were skipped. A current binary never reads as a failed update. |
| Scannable | Two phase lines with the mark first, a `[n/N]` counter, a fixed label column and right-aligned elapsed time. Then one finding per failure or warning: mark, project, phase, cause, and one `→` remedy. Healthy projects collapse to one counted line. |
| Readable | The wrapped cause of each finding, and the names under a finding shared by several projects. |
| Machine output | `--json` is unchanged: the combined receipt (`binary_updated`, `version`, `hub_restart`), then the refresh receipt (`projects[]`, `refresh_status`, `user_level_store`, `skipped_unregistered`). No phase lines, spinner or findings. |
| Omitted | The release check's own chatter (`Checking target-arch…`), every phase's captured transcript, the per-project phase table, repeated conflict paths and Cloud row explanations. The project table and transcripts appear under `--verbose`; a dry run keeps its table, because the plan is its result. |

### Rendering decisions

- **A phase owns its line.** On a terminal, the line redraws in place: an accent spinner, the
  counter, the label, what the phase is doing now (`7 of 12 · gabber-studio`), and elapsed
  time. Everything the phase prints meanwhile is captured, so nothing interleaves with the line.
  This fixes `Updating Cassy binaryChecking target-arch...` and
  `Refreshing all local Cassy projects  project`. When the phase ends, its result replaces the
  line in the same columns, so the `✓ 1.4s` and `✗ 41.8s` marks no longer land on lines of
  their own.
- **Piped, nothing redraws.** Each phase appends one start line (`→ [1/2] Cassy binary …`) and
  one result line. Marks pad to the widest mark the mode draws, so `[OK]` and `[ERROR]` rows
  align.
- **Interactive runs keep the terminal.** A run that may prompt (no `-y` on a TTY) prints its
  start line, lets the confirmation prompt through, then prints the result. A run that cannot
  prompt turns off the release check's chatter.
- **After a swap,** the installed binary draws phase `[2/2]` on the same terminal, after the
  parent's binary line. The hub restart order is unchanged (cas-91ba).
- **Findings are derived from the receipt data, not the transcript.** A failed phase's detail
  splits into cause and remedy (`…; run X` gives `→ X`). Cloud rows read their own summary
  (`push incomplete: 2 rejected by cloud (invalid_json_nul ×2)` → `cas cloud queue --retry`).
  The hub's Serve warning is a finding with its remedy. Identical findings across projects
  collapse to one row with a count and the names beneath it.
- **Names are never cut.** A project is named by the shortest trailing path that tells it
  apart (`gabber-studio`, `Richards LLC/Accounting`). A name too wide for the column moves to
  its own line. A path too long even for that loses its start, never its end. Commands and
  short parentheticals never wrap.
- **The receipt keeps the grammar scripts grep:** `Cassy 3.62.0 · 12 projects refreshed ·
  2 failed · 1 skipped · 41.8s` (it was `· N unregistered store(s) not refreshed`). It wraps
  between fields, never inside one. The `--verbose` escape is named on the next line whenever
  something was shortened or a finding exists.
- **The exit reason carries both verdicts:** `✗ 2 of 12 projects not fully refreshed; Cassy
  3.62.0 is current`. The exit status stays nonzero.

### Review captures

`cas update --render-fixture <json>` (hidden) replays a recorded update through the shipping
phases and renderer. It reads no store and changes nothing, so it runs anywhere, including a
factory worker. The fixtures live in [fixtures/](fixtures/):
`cas-update-partial-refresh.json` is the operator's 2026-10-17 transcript, **reconstructed**
from task evidence (no raw capture was saved). `cas-update-success.json` is its all-healthy
counterpart.

Before a release, or after changing any update output, capture both fixtures at 80 and 120
columns, piped, `NO_COLOR` and C-locale:

```bash
for f in partial-refresh success; do
  node scripts/terminal-qa.mjs --label cas-update-$f --escape-flag --verbose \
    --out docs/design/cli/captures/after/cas-update-$f -- \
    cas update -y --render-fixture docs/design/cli/fixtures/cas-update-$f.json
done
```

Open each `.80.html` to read the light and dark palettes side by side, and the `.txt` captures
as a pipe would. The `before` captures came from `ab3f679af`, the seam commit with the pre-change
renderer.

## Critique

Before (build `eda3dfd1`): `terminal-qa: FAIL cas-update-check · 12 runs · 33 fail · 0 warn` — 32 contrast, 1 unicode-without-fallback.

After: `terminal-qa: PASS cas-update-check · 12 runs · 0 fail · 0 warn · 0 allowed · docs/design/cli/captures/after/cas-update-check/report.md`

Post-swap failure review: `terminal-qa: PASS cas-update · 12 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-a461/terminal-qa/report.json`; the plain-mode
failure test keeps the first line actionable by naming the failed project and the one rerun command.

Service-refresh receipt review: `cas-cli` scoped update tests pass 41/41, including the
old-to-new version transition and `via: service` contract; the real-build terminal capture
for the adjacent doctor/service warning is recorded under
`/home/pippenz/.cas/artifacts/cas-47f9/terminal-qa/`.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | verdict first; post-swap failures name the project before the one rerun command |
| Fit | 4 | two rows for a two-fact answer; partial failures retain structured project evidence |
| Craft | 4 | fixed label column; failure guidance uses one copyable remedy |
| Theme safety | 5 | marks only; four palettes pass |
| Machine contract | 5 | `--check --json` is one object |

### Full update review (cas-0aa5)

Both runs replay the same fixtures. Before is `ab3f679af` with the old renderer; after is
`695bf6f10`.

Before: `terminal-qa: FAIL cas-update-partial-refresh · 11 runs · 145 fail · 0 warn · 0 allowed`
and `terminal-qa: FAIL cas-update-success · 11 runs · 81 fail · 0 warn · 0 allowed`. Both show
the transcript's defects as rendered: `→ [1/2] Updating Cassy binaryChecking target-arch...`,
`→ [2/2] Refreshing all local Cassy projects  project …`, the orphan `✓ 1.7s` and `✗ 4.3s` marks on lines of their own,
cut names (`Petrastella/gabber-…`), raw errors in details blocks, and a final
`✗ one or more projects were not fully refreshed`.

After: `terminal-qa: PASS cas-update-partial-refresh · 11 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a/cas-0aa5/terminal-qa/after/cas-update-partial-refresh/report.json`

After: `terminal-qa: PASS cas-update-success · 11 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a/cas-0aa5/terminal-qa/after/cas-update-success/report.json`

`--json` for the partial fixture prints the same two documents before and after, byte for byte
once parsed, and exits 1 both times.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | Line 1 is `✓ [1/2] Cassy binary  3.62.0 is current`; line 2 is `✗ [2/2] Projects  2 of 12 failed · 4 warnings · 1 skipped`; each finding carries one `→` remedy; the receipt is last |
| Fit | 4 | Six healthy projects collapse to one line, and raw phase output is gone; two Cloud rows differ only by a count and stay separate |
| Craft | 4 | Marks, counters, labels and right-aligned times align, causes hang under one column, and commands never wrap; piped start lines pad the arrow to the `[ERROR]` column |
| Theme safety | 5 | Four palettes pass; only marks, the spinner and muted text carry colour; `NO_COLOR` and `LC_ALL=C` read the same in brackets |
| Machine contract | 4 | `--json` is unchanged and the exit status is still 1, but a full update still prints two documents (pre-existing; not changed here) |

Scored by happy-tiger-95 on 2026-10-17; floor holds.
