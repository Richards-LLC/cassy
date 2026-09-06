# `cas doctor` check matrix

This is the contract for the checks assembled by `cas-cli/src/cli/doctor.rs`.
`scope` describes where the evidence lives; `class` describes what `--fix`
may do. `info` rows are represented by an OK verdict with explanatory text.
Consent and human rows are never changed by `--fix`.

| Check(s) | Scope | Class | Fix or remedy | Test seam |
| --- | --- | --- | --- | --- |
| `cas directory`, `database`, `schema`, `tables`, `entry store` | project | info / auto-fix | initialize with `cas doctor --fix`; apply schema migrations | doctor store and migration tests |
| `supervisor relay`, `factory session*`, `factory session overlap`, `delivery retries` | project | human | inspect the named session or recipient | factory/relay doctor tests |
| `legacy search index` | project | auto-fix | bounded legacy-root repair; rerun doctor | `doctor_reports_legacy_tantivy_root_before_repair_and_clean_after`, lock-budget test |
| `pre-versioned search index`, `search index` | project | auto-fix | migrate/rebuild `index/tantivy-v15` and backfill entries, tasks, rules, skills, and artifacts | search-index migration tests |
| `symbol index` | project | auto-fix / info | run full code reconciliation and reconcile the vector queue; 0/0 eligible files is info | `symbol_index_check_*`, code-index tests |
| `embedding drain`, `embeddings` | project | info / human | inspect provider capability and named drain error | embedding drain tests |
| `code history index` | project | human | run `cas history backfill`; investigate named source errors | history-index tests |
| `configuration`, `issue repositories`, `mcp config`, `mcp stdio upstreams`, `sync target`, `models` | project | info / human | restore config, repair named upstream, or run the named sync command | config, proxy, and MCP doctor tests |
| `rules`, `memory stats`, `tasks` | project | auto-fix / human | prune only genuinely dangling local dependency rows; cloud/quarantined rows require review | dependency health and task tests |
| platform `integration*` rows | project | human | run the platform-specific `cas integrate …` remedy | integration doctor tests |
| canonical ID, project aliases, cloud identity metadata | project | human | explicitly adopt aliases or purge/re-register cloud scope | canonical/cloud identity tests |
| cloud sync queue, cross-project rows, foreign knowledge pages, foreign project rows | project | consent-fix | preview by default; apply quarantine/release or purge only with its explicit consent flag and `--yes` | cloud quarantine and foreign-row tests |
| `user-level store` | host | info / human | initialize the host registry or repair its schema | host-store check |
| `known repos` | host | auto-fix | `cas doctor --fix` reuses `known-repos prune-missing`; repository files are never removed | known-repos prune tests |
| `host proxy` | host | human | configure or repair the user-scoped proxy and credentials | MechaCassy doctor tests |
| `hub service` | host | info / human | inspect `cas hub service status` and the runtime path | hub runtime/service tests |
| `registered project roots` | host | human | remove a disposable registration with `cas known-repos forget`; unlink remote data first when applicable | registered-root tests |
| `host user skills` | host | human | review stale user-level skills and delete their directories deliberately | user-skill scan tests |
| `root projections` | project | auto-fix / human | regenerate current managed `.claude`, `.codex`, and `.grok` projections; preserved customizations remain reviewable | builtin preview/sync tests |

Project runs report host findings once as `host: N findings — see `cas doctor
--host``. `cas doctor --full` expands that row into the host rows. `cas doctor
--host` never runs project checks. A plain TTY may offer the safe auto-fix set
with one key; piped, JSON, and explicit consent-fix modes remain non-interactive.
