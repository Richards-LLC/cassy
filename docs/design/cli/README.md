# CLI craft: briefs and captures

Concept briefs and terminal-qa captures for the commands re-rendered under the `cas-cli-craft`
skill (`cas-cli/src/builtins/skills/cas-cli-craft/`). Each command has a brief beside this file
and a before/after capture set under `captures/`.

| Command | Brief | Before | After |
| --- | --- | --- | --- |
| `cas doctor` | [cas-doctor.brief.md](cas-doctor.brief.md) | [captures/before/cas-doctor](captures/before/cas-doctor/report.md) | [captures/after/cas-doctor](captures/after/cas-doctor/report.md) |
| `cas known-repos prune-missing` | [cas-known-repos.brief.md](cas-known-repos.brief.md) | — | [captures/after/cas-known-repos/report.md](captures/after/cas-known-repos/report.md) |
| `cas update --check` | [cas-update.brief.md](cas-update.brief.md) | [captures/before/cas-update-check](captures/before/cas-update-check/report.md) | [captures/after/cas-update-check](captures/after/cas-update-check/report.md) |
| `cas update` (partial refresh) | [cas-update.brief.md](cas-update.brief.md) | [captures/before/cas-update-partial-refresh](captures/before/cas-update-partial-refresh/report.md) | [captures/after/cas-update-partial-refresh](captures/after/cas-update-partial-refresh/report.md) |
| `cas update` (all current) | [cas-update.brief.md](cas-update.brief.md) | [captures/before/cas-update-success](captures/before/cas-update-success/report.md) | [captures/after/cas-update-success](captures/after/cas-update-success/report.md) |
| `cas update` (projects and user store failing) | [cas-update.brief.md](cas-update.brief.md) | — | [captures/after/cas-update-mixed-failure](captures/after/cas-update-mixed-failure/report.md) |
| `cas update` interactive (native pty) | [cas-update.brief.md](cas-update.brief.md) | [captures/before/cas-update-interactive](captures/before/cas-update-interactive/README.md) | [captures/after/cas-update-interactive](captures/after/cas-update-interactive/README.md) |
| `cas artifact list` | [cas-artifact.brief.md](cas-artifact.brief.md) | — | [captures/after/cas-artifact-list](captures/after/cas-artifact-list/report.md) |
| `cas artifact show` | [cas-artifact.brief.md](cas-artifact.brief.md) | — | [captures/after/cas-artifact-show](captures/after/cas-artifact-show/report.md) |
| `cas factory status` | [cas-factory-status.brief.md](cas-factory-status.brief.md) | [captures/before/cas-factory-status](captures/before/cas-factory-status/report.md) | [captures/after/cas-factory-status](captures/after/cas-factory-status/report.md) |
| `cas` error renderer | [cas-error.brief.md](cas-error.brief.md) | — | [captures/after/cas-error-renderer](captures/after/cas-error-renderer/report.md) |
| Supervisor pane routine cards (Codex PTY) | [supervisor-routine-cards.brief.md](supervisor-routine-cards.brief.md) | [captures/before/supervisor-routine-cards](captures/before/supervisor-routine-cards/codex-pane-80.txt) | [captures/after/supervisor-routine-cards](captures/after/supervisor-routine-cards/codex-pane-80.txt) |

Each capture directory keeps the 80-column `.html` that stacks the four palettes (open it in a
browser to see the light and dark renders side by side), the ANSI-stripped 80-column dark and
light `.txt`, and `report.md`, whose first line is the receipt. The gate also writes the raw
`.ansi` streams, the 120-column, Solarized, piped, `NO_COLOR`, C-locale and `--json` captures,
and `report.json`; those are regenerated on demand and not committed.

## Regenerate

From the project root of a Cassy-initialized checkout (the commands read the local store):

```bash
node scripts/terminal-qa.mjs --label cas-doctor --escape-flag --verbose --json-flag --json \
  --out docs/design/cli/captures/after/cas-doctor -- cas doctor
node scripts/terminal-qa.mjs --label cas-update-check --json-flag --json \
  --out docs/design/cli/captures/after/cas-update-check -- cas update --check
for f in partial-refresh success mixed-failure; do
  node scripts/terminal-qa.mjs --label cas-update-$f --escape-flag --verbose \
    --out docs/design/cli/captures/after/cas-update-$f -- \
    cas update -y --render-fixture docs/design/cli/fixtures/cas-update-$f.json
done
node scripts/terminal-qa.mjs --label cas-factory-status --json-flag --json \
  --out docs/design/cli/captures/after/cas-factory-status -- cas factory status
node scripts/terminal-qa.mjs --label cas-artifact-list --json-flag --json \
  --out docs/design/cli/captures/after/cas-artifact-list -- cas artifact list --task <task-id>
node scripts/terminal-qa.mjs --label cas-artifact-show --json-flag --json \
  --out docs/design/cli/captures/after/cas-artifact-show -- cas artifact show <artifact-id>
node scripts/terminal-qa.mjs --label cas-error-renderer \
  --out docs/design/cli/captures/after/cas-error-renderer -- \
  cas config import /tmp/cas-d0b1-missing-config.toml
```

The `before` set was produced the same way from the binary built at `eda3dfd1`, the last
commit before the re-render. The two `cas update` sets replay recorded updates with the
hidden `--render-fixture` flag, which reads no store and changes nothing; their `before`
captures come from `ab3f679af`, the seam commit that still carried the old renderer.
Rerun them whenever update output changes, and before each release. The interactive set is
native, because a prompting path cannot be replayed. These are the approved cells of an
independent QA run that the supervisor executed with factory identity preserved; its README
names the binary, the driver and the environment each cell recorded.
