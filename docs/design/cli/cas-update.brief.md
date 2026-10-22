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

### Binary installation failure JSON (cas-929d, cas-a99a)

Before a binary swap, a failed updater setup, release lookup, download or extraction
emits one JSON object and exits nonzero. It retains `binary_updated: false` and
`version` for the running binary, adds `binary_status: "failed"`,
`refresh_status: "not_run"`, and `error: {phase: "binary", message: <cause>}`.
No project, hub or refresh result is claimed. Stderr retains the existing diagnostic.
Successful updates and post-swap failures retain their existing receipts; a failure
after a successful swap must never be described as `binary_updated: false`.
This also covers a full `cas --json update --yes` with no pinned version: if
the selected release asset fails extraction, refresh does not run and the
receipt reports the current binary and the extraction cause.

### Service-managed hub refresh

After a binary swap, the hub restart is represented in JSON as `hub_restart` with
`from_version`, `to_version`, and `via` (`service` or `detached`). This receipt is included
in both the combined update document and the post-swap project-refresh document, so the
operator can verify that the installed service picked up the new binary.

## Full update and `--all-projects` (cas-0aa5, cas-06b0, cas-70a8)

This section describes the accepted renderer as merged at `dc1665877`. The update renderer
files (`cli/update.rs`, `cli/update/`, `ui/components`, `ui/theme`) are byte-identical to
`4cedda1f0`, the candidate that passed independent QA.

| Field | Sentence |
| --- | --- |
| First two lines | The binary phase line says whether Cassy is current, what it was updated to, or that an install was cancelled. The projects phase line says how many projects failed, warned or were skipped, with a failed user-level store named on its own. A current binary never reads as a failed update. |
| Scannable | Two phase lines with the mark first, a `[n/N]` counter, a fixed label column and right-aligned elapsed time. Then one finding per failure or warning: mark, who, phase, cause, and one `→` remedy. Healthy projects collapse to one counted line. |
| Readable | The wrapped cause of each finding, and the names under a finding shared by several projects. |
| Machine output | `--json` emits one combined receipt: binary state (`binary_updated`, `version`, `hub_restart`) together with the refresh receipt (`projects[]`, `refresh_status`, `user_level_store`, `skipped_unregistered`). Pre-install errors use the binary-failure contract above. No phase lines, spinner, prompt or findings. |
| Omitted | self_update's own output: release-check chatter, its release-status block and prompt, and its download bar. Also every phase's captured transcript, the per-project phase table, repeated conflict paths and Cloud explanations that only restate a reason. The table and transcripts appear under `--verbose`; a dry run keeps its table, because the plan is its result. |

### Rendering decisions

- **A phase owns its line.** On a terminal the line redraws in place: an accent spinner, the
  counter, the label, what the phase is doing now (`7 of 12 · gabber-studio`), and elapsed
  time. Everything the phase prints meanwhile is captured, so nothing interleaves with the line.
  This fixes `Updating Cassy binaryChecking target-arch...` and
  `Refreshing all local Cassy projects  project`. When the phase ends, its result replaces the
  line in the same columns, so the `✓ 1.4s` and `✗ 41.8s` marks no longer land on lines of
  their own.
- **Piped, nothing redraws.** Each phase appends one start line (`→ [1/2] Cassy binary …`) and
  one result line. Marks pad to the widest mark the mode draws, so `[OK]` and `[ERROR]` rows
  align.
- **Interactive runs ask in the phase grammar (cas-70a8).** self_update never prints. A run that
  must ask (a TTY, no `-y`) prints its start line, then `ℹ Cassy 3.62.0 → 9.9.9 is available` /
  `Download and install it now? [Y/n]`. As before, `Enter` or `y` installs and anything else
  cancels.
  - After `y`, the same phase line redraws `downloading Cassy 9.9.9` until the download
    returns, and its result replaces it.
  - A declined install reads `⚠ [1/2] Cassy binary  cancelled · 3.62.0 unchanged` and still
    exits nonzero.
  - When no update is available there is no prompt.
  - The hub's printed verification is held behind the phase line even when the phase cannot be
    captured. A hub that is not running is said once, as its finding.
- **After a swap,** the installed binary draws phase `[2/2]` on the same terminal, after the
  parent's binary line. The hub restart order is unchanged (cas-91ba).
- **Findings are derived from the receipt data, not the transcript.** A failed phase's detail
  splits into cause and remedy (`…; run X` gives `→ X`). Cloud rows read their own summary.
  - An explanation that names a link or a command becomes the remedy and overrides the blanket
    `cas cloud queue --retry`. `invalid_json_nul` therefore reads `push incomplete: 2 rejected
    by cloud (invalid_json_nul ×2); invalid_json_nul: PostgreSQL jsonb cannot store U+0000;
    payload retained unchanged`, then `→ Follow
    https://github.com/Richards-LLC/petra-stella-cloud/issues/184 for lossless roundtrip
    support, then` `` `cas cloud queue --retry --retry-reason invalid_json_nul` `` (cas-7c6b's
    guidance, kept whole).
  - An explanation that only restates its reason stays under `--verbose`.
  - The hub's Serve warning is a finding with its remedy.
  - Identical findings across projects collapse to one row with a count and the names beneath
    it.
- **Names are never cut.** A project is named by the shortest trailing path that tells it
  apart (`gabber-studio`, `Richards LLC/Accounting`). A name too wide for the column moves to
  its own line. A path too long even for that loses its start, never its end. Commands and
  short parentheticals never wrap.
- **The user-level store is host state, never a project (cas-06b0).**
  - "N of M" and the receipt's "N failed" count projects only.
  - A failed user-level store is named on its own: on the phase line (`1 of 3 failed ·
    user-level store failed`), as its own finding, and in the receipt (`12 projects refreshed ·
    0 failed · user-level store failed`).
  - `--json` counts are unchanged.
- **The receipt keeps the grammar scripts grep:** `Cassy 3.62.0 · 12 projects refreshed ·
  2 failed · 1 skipped · 41.8s` (it was `· N unregistered store(s) not refreshed`).
  - It is laid out for the width (cas-70a8). That grammar stays whole on one line, and the
    elapsed time joins it or is dropped (the phase line shows it), never alone.
  - The user-store and skipped counts follow on their own line.
  - The `--verbose` escape is named on the next line whenever something was shortened or a
    finding exists.
- **The exit reason carries both verdicts:** `✗ 2 of 12 projects not fully refreshed; Cassy
  3.62.0 is current`.
  - It stays on one line when it fits beside `[ERROR]`; otherwise there is one clause per line
    (`2 of 12 projects not fully refreshed` / `the user-level store was not refreshed` /
    `Cassy 3.62.0 is current`).
  - A failed install says what failed, its cause without self_update's `UpdateError:` prefix,
    and ``Cassy 3.62.0 is unchanged; rerun `cas update` to retry``. On unix the old binary is
    provably untouched by any self_update error: self-replace's rename is its last fallible
    step.
  - The exit status stays nonzero.
- **Reserved:** the cas-4585 config-cleanup success row is not captured here yet; it is added
  when that delivery is verified and merged.

### Review captures

`cas update --render-fixture <json>` (hidden) replays a recorded update through the shipping
phases and renderer. It reads no store and changes nothing, so it runs anywhere, including a
factory worker. Fixture replays prove the renderer, not the live sweep.

The fixtures live in [fixtures/](fixtures/):

- `cas-update-partial-refresh.json` is the operator's 2026-10-17 transcript, **reconstructed**
  from task evidence (no raw capture was saved). It carries the real cas-7c6b `invalid_json_nul`
  hint.
- `cas-update-success.json` is its all-healthy counterpart.
- `cas-update-mixed-failure.json` is the partial fixture with the user-level store failing too
  (synthetic, cas-06b0).

Before a release, or after changing any update output, capture every fixture at 80 and 120
columns, piped, `NO_COLOR` and C-locale:

```bash
for f in partial-refresh success mixed-failure; do
  node scripts/terminal-qa.mjs --label cas-update-$f --escape-flag --verbose \
    --out docs/design/cli/captures/after/cas-update-$f -- \
    cas update -y --render-fixture docs/design/cli/fixtures/cas-update-$f.json
done
```

Open each `.80.html` to read the light and dark palettes side by side, and the `.txt` captures
as a pipe would. The `before` fixture captures came from `ab3f679af`, the seam commit with the
pre-change renderer.

The interactive path cannot be replayed, because it prompts. Its captures are **native**, taken
from independent QA round 2 of cas-70a8:

- The supervisor ran the root-built `4cedda1` binary through the reviewer's driver.
- Factory identity and `CAS_CLONE_PATH` were preserved; each cell records them.
- Each run used a fresh scratch HOME and cwd, against a loopback fake release.

See [captures/after/cas-update-interactive](captures/after/cas-update-interactive/README.md).
That round had no up-to-date interactive cell, so the fixture captures stand in for that result.

## Automatic config maintenance (cas-4585)

| Field | Sentence |
| --- | --- |
| First two lines | Existing binary/project verdicts retain their grammar; configuration failures count as a failed refresh and carry the actual file cause. |
| Scannable | One success row after healthy projects says `config  N retired entries removed in M files`, including host files without counting them as projects. |
| Readable | Each failed cleanup retains the existing phase failure and remedy, followed by its config path, safe cause, and a check-and-rerun remedy. |
| Machine output | Existing refresh documents and fields remain; `projects[].retired_toml`, `user_level_store.retired_toml[]`, and `config_maintenance` add statuses, counts, retired key names and restricted backup paths without values. |
| Omitted | Unchanged/missing files add no human row, successful cleanup never creates a warning, and before-copy contents and retired secret values are never printed. |

The success row uses the existing semantic mark and text wrapper. The banner,
phase counter, timings and exit-reason grammar remain with their existing owners.
Backups are private durable TOML copies, not published evidence. Invalid files
remain intact; other projects continue. A dry run describes the retirement and
creates no config lock, backup or replacement.

## Critique

Before (build `eda3dfd1`): `terminal-qa: FAIL cas-update-check · 12 runs · 33 fail · 0 warn` — 32 contrast, 1 unicode-without-fallback.

After: `terminal-qa: PASS cas-update-check · 12 runs · 0 fail · 0 warn · 0 allowed · docs/design/cli/captures/after/cas-update-check/report.md`

Post-swap failure review: `terminal-qa: PASS cas-update · 12 runs · 0 fail · 0 warn · 0 allowed · <artifacts>/cas-a461/terminal-qa/report.json`; the plain-mode
failure test keeps the first line actionable by naming the failed project and the one rerun command.

Service-refresh receipt review: `cas-cli` scoped update tests pass 41/41, including the
old-to-new version transition and `via: service` contract; the real-build terminal capture
for the adjacent doctor/service warning is recorded under
`<artifacts>/cas-47f9/terminal-qa/`.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | verdict first; post-swap failures name the project before the one rerun command |
| Fit | 4 | two rows for a two-fact answer; partial failures retain structured project evidence |
| Craft | 4 | fixed label column; failure guidance uses one copyable remedy |
| Theme safety | 5 | marks only; four palettes pass |
| Machine contract | 5 | `--check --json` is one object |

### Full update review (cas-0aa5, historical)

This review is kept as history: it scored `695bf6f10`, before the QA fixes recorded below.
Both runs replay the same fixtures. Before is `ab3f679af` with the old renderer; after is
`695bf6f10`.

Before: `terminal-qa: FAIL cas-update-partial-refresh · 11 runs · 145 fail · 0 warn · 0 allowed`
and `terminal-qa: FAIL cas-update-success · 11 runs · 81 fail · 0 warn · 0 allowed`. Both show
the transcript's defects as rendered: `→ [1/2] Updating Cassy binaryChecking target-arch...`,
`→ [2/2] Refreshing all local Cassy projects  project …`, the orphan `✓ 1.7s` and `✗ 4.3s` marks on lines of their own,
cut names (`Petrastella/gabber-…`), raw errors in details blocks, and a final
`✗ one or more projects were not fully refreshed`.

After: `terminal-qa: PASS cas-update-partial-refresh · 11 runs · 0 fail · 0 warn · 0 allowed · <artifacts>/cas-0aa5/terminal-qa-history/after/cas-update-partial-refresh/report.json`

After: `terminal-qa: PASS cas-update-success · 11 runs · 0 fail · 0 warn · 0 allowed · <artifacts>/cas-0aa5/terminal-qa-history/after/cas-update-success/report.json`

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

### Accepted renderer review (cas-6b56)

Fixture replays run on the accepted candidate `cas 3.62.0 (4cedda1 2026-10-17)`. That is
`cas-70a8/build-r2/cas`, sha256 `9e097923…dcedce`, provenance verified by the supervisor. Its
update renderer files are identical to the merged `dc1665877`.

The committed `after` captures for these three fixtures are these runs:

- `terminal-qa: PASS cas-update-partial-refresh · 11 runs · 0 fail · 0 warn · 0 allowed · <artifacts>/cas-6b56/terminal-qa/cas-update-partial-refresh/report.json`
- `terminal-qa: PASS cas-update-success · 11 runs · 0 fail · 0 warn · 0 allowed · <artifacts>/cas-6b56/terminal-qa/cas-update-success/report.json`
- `terminal-qa: PASS cas-update-mixed-failure · 11 runs · 0 fail · 0 warn · 0 allowed · <artifacts>/cas-6b56/terminal-qa/cas-update-mixed-failure/report.json`

The native interactive captures are the approved cells of independent QA round 2 of cas-70a8.
They come from the same `4cedda1` binary, run by the supervisor with factory identity preserved,
and cover accept with a deliberate extraction failure, a slow download, HTTP 500, decline, `-y`,
pipe, C locale, `NO_COLOR` and colour. Their ledger:
`<artifacts>/cas-70a8/independent-qa/round-2/LEDGER.md`.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | The binary verdict, then the refresh verdict; the reason-specific NUL remedy; the exit reason names both verdicts |
| Fit | 4 | Healthy projects on one line, raw output gone, the user store apart from project counts; two Cloud rows differ only by a count |
| Craft | 4 | Aligned columns; the receipt and exit reason break between fields or clauses, never inside one, and elapsed time is never alone; the interactive prompt and download stay on the phase line |
| Theme safety | 5 | Four palettes pass; in the native cells `LC_ALL=C` is pure ASCII and `NO_COLOR` draws bracket marks with no colour |
| Machine contract | 4 | `--json` and counts are unchanged; a full update still prints two documents (pre-existing) |

Scored by happy-tiger-95 on 2026-10-17; floor holds.


## Dry-run upkeep preview (cas-be63)

| Field | Sentence |
| --- | --- |
| First two lines | Existing update and project verdicts retain their dry-run status; no adoption is claimed as applied. |
| Scannable | Each dry-run project detail block names its planned upkeep stages under migration. |
| Readable | Wrapped upkeep notes explain the observe window and the command to pin an explicit stage. |
| Machine output | The existing single receipt includes projects[].phase_documents.docs_upkeep[].changes with the planned notes; config bytes stay unchanged. |
| Omitted | No extra success transcript, progress line or receipt document is added. |

Schema migrations remain skipped in the native dry run. Upkeep planning runs
with the real dry-run flag after successful or planned schema work; schema
failures still prevent upkeep writes. Human notes fit within the existing
80-column project detail block; JSON retains the complete notes as data.
