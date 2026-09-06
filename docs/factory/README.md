# Factory model history

`python3 scripts/factory-model-history.py` scans the host's project `.cas/cas.db`
stores, sibling `.cas/logs/*.log` factory records, Codex rollouts, and Claude
transcripts in one read-only pass. It writes dated outputs under
`docs/factory/data/`:

- `factory-model-history-YYYY-MM-DD.csv`: one row per worker session and task;
- `factory-model-scorecard-YYYY-MM-DD.csv`: project and overall model/effort aggregates;
- `factory-model-scorecard-YYYY-MM-DD.md`: the scorecard plus source coverage,
  join keys, miss rates, and unavailable-field policy;
- `factory-model-history-YYYY-MM-DD-sources.md`: the source coverage section alone.

The default database root is `/home/pippenz`. Transcript discovery enumerates
every matching `/home/pippenz/.codex*/sessions` and
`/home/pippenz/.claude*/projects` home. Override them with repeatable
`--codex-root` and `--claude-root` flags for fixture runs. Databases are opened
with SQLite `mode=ro`; worktree, artifact, and `epic-*` database trees are
excluded.

Populate `data/model-prices.json` with the report's verified prices. Each model
entry accepts `input_per_million`, `cached_input_per_million`,
`cache_creation_per_million`, `output_per_million`, and `reasoning_per_million`.
Costs remain blank when a price is absent; the script never infers a price.

Use `--prices /path/to/prices.json` to rerun the same extraction with a filled
price file, without changing the script. `--root`, repeatable `--codex-root`
and `--claude-root`, `--output-dir`, and `--date` likewise make fixture and
historical reruns explicit. The dated sources Markdown lists every session and
scorecard CSV column, source join key, miss rate, and unavailable-field rule.
