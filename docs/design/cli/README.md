# CLI craft: briefs and captures

Concept briefs and terminal-qa captures for the commands re-rendered under the `cas-cli-craft`
skill (`cas-cli/src/builtins/skills/cas-cli-craft/`). Each command has a brief beside this file
and a before/after capture set under `captures/`.

| Command | Brief | Before | After |
| --- | --- | --- | --- |
| `cas doctor` | [cas-doctor.brief.md](cas-doctor.brief.md) | [captures/before/cas-doctor](captures/before/cas-doctor/report.md) | [captures/after/cas-doctor](captures/after/cas-doctor/report.md) |
| `cas update --check` | [cas-update.brief.md](cas-update.brief.md) | [captures/before/cas-update-check](captures/before/cas-update-check/report.md) | [captures/after/cas-update-check](captures/after/cas-update-check/report.md) |
| `cas factory status` | [cas-factory-status.brief.md](cas-factory-status.brief.md) | [captures/before/cas-factory-status](captures/before/cas-factory-status/report.md) | [captures/after/cas-factory-status](captures/after/cas-factory-status/report.md) |

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
node scripts/terminal-qa.mjs --label cas-factory-status --json-flag --json \
  --out docs/design/cli/captures/after/cas-factory-status -- cas factory status
```

The `before` set was produced the same way from the binary built at `eda3dfd1`, the last
commit before the re-render.
