//! Doctor command - diagnostics and repair

use clap::Args;
use std::collections::BTreeMap;
use std::path::Path;

use crate::config::Config;
use crate::migration::{
    check_migrations,
    detector::{SchemaSummary, get_schema_summary},
    run_migrations,
};
use crate::store::{StoreType, detect_store_type, open_rule_store, open_store, open_task_store};
use crate::types::RuleStatus;
use crate::ui::components::Formatter;
use crate::ui::theme::ActiveTheme;
use cas_core::SearchIndex;

use crate::cli::Cli;

#[derive(Args, Debug, Clone)]
pub struct DoctorArgs {
    /// Attempt safe automatic fixes (initialize CAS and apply pending schema migrations)
    #[arg(long)]
    pub fix: bool,

    /// Report cross-project ("foreign") task rows in this project's database
    /// in full detail, instead of running the other diagnostics (cas-fc6fa /
    /// GH #133). Read-only: every database is opened read-only and nothing is
    /// deleted. Rows are matched on `(id, title)` — never on id alone, because
    /// 4-hex task ids collide across projects.
    #[arg(long)]
    pub foreign_rows: bool,
}

struct Check {
    name: String,
    status: CheckStatus,
    message: String,
}

enum CheckStatus {
    Ok,
    Warning,
    Error,
}

// A missing history table is not an empty index. Each table below is created
// unconditionally by the current migration chain, so its absence means the
// store is behind on schema and doctor must say so. In particular, older
// installs without the symbol or epoch migrations should warn until their
// migrations are applied; silently accepting them would make unsupported
// history queries look merely quiet.
const EXPECTED_TABLES: &[&str] = &[
    "entries",
    "tasks",
    "rules",
    "skills",
    "agents",
    "task_leases",
    "history_commits",
    "history_commit_files",
    "history_index_state",
    "history_docs",
    "history_commit_symbols",
    "history_epochs",
    "code_vector_queue",
    "code_index_state",
];

/// Pure schema verdict so the missing-table path is exercised directly in
/// tests rather than inferred from a source-code string.
fn schema_tables_check(summary: &SchemaSummary) -> Check {
    let table_count = summary.tables.len();
    let total_columns: usize = summary.tables.iter().map(|t| t.columns.len()).sum();
    let total_rows: i64 = summary.tables.iter().map(|t| t.row_count).sum();
    let missing_tables: Vec<&str> = EXPECTED_TABLES
        .iter()
        .filter(|table| !summary.tables.iter().any(|found| found.name == **table))
        .copied()
        .collect();

    if missing_tables.is_empty() {
        Check {
            name: "tables".to_string(),
            status: CheckStatus::Ok,
            message: format!(
                "{table_count} tables, {total_columns} columns, {total_rows} rows total"
            ),
        }
    } else {
        Check {
            name: "tables".to_string(),
            status: CheckStatus::Warning,
            message: format!(
                "{} tables ({} missing: {})",
                table_count,
                missing_tables.len(),
                missing_tables.join(", ")
            ),
        }
    }
}

pub fn execute(args: &DoctorArgs, cli: &Cli, cas_root: Option<&Path>) -> anyhow::Result<()> {
    let mut checks = Vec::new();
    let mut resolved_cas_root = cas_root.map(Path::to_path_buf);

    if args.fix && cli.json && resolved_cas_root.is_none() {
        anyhow::bail!(
            "`cas doctor --fix --json` is not supported before initialization. Run `cas init --yes` first or omit `--json`."
        );
    }

    if args.fix {
        if resolved_cas_root.is_none() {
            // doctor --fix runs init non-interactively in the background;
            // `no_integrations: true` ensures no platform MCP calls or
            // prompts are issued during a diagnostic run.
            let init_args = crate::cli::init::InitArgs {
                yes: true,
                no_integrations: true,
                ..Default::default()
            };
            match crate::cli::init::execute(&init_args, cli) {
                Ok(()) => {
                    resolved_cas_root = crate::store::find_cas_root().ok();
                    if let Some(path) = &resolved_cas_root {
                        checks.push(Check {
                            name: "auto-fix".to_string(),
                            status: CheckStatus::Ok,
                            message: format!("Initialized CAS at {}", path.display()),
                        });
                    } else {
                        checks.push(Check {
                            name: "auto-fix".to_string(),
                            status: CheckStatus::Warning,
                            message: "Initialization ran but CAS root could not be resolved."
                                .to_string(),
                        });
                    }
                }
                Err(e) => {
                    checks.push(Check {
                        name: "auto-fix".to_string(),
                        status: CheckStatus::Error,
                        message: format!("Failed to initialize CAS: {e}"),
                    });
                    return output_checks(&checks, cli);
                }
            }
        }

        if let Some(path) = &resolved_cas_root {
            match check_migrations(path) {
                Ok(status) if status.has_pending() => match run_migrations(path, false) {
                    Ok(applied) => checks.push(Check {
                        name: "auto-fix".to_string(),
                        status: CheckStatus::Ok,
                        message: format!(
                            "Applied {} pending schema migration(s)",
                            applied.applied_count
                        ),
                    }),
                    Err(e) => checks.push(Check {
                        name: "auto-fix".to_string(),
                        status: CheckStatus::Warning,
                        message: format!("Failed to apply pending migrations: {e}"),
                    }),
                },
                Ok(_) => {}
                Err(e) => checks.push(Check {
                    name: "auto-fix".to_string(),
                    status: CheckStatus::Warning,
                    message: format!("Could not check migrations before fix: {e}"),
                }),
            }
        }
    }

    // Check 1: .cas directory exists
    let cas_root = match resolved_cas_root {
        Some(path) => {
            checks.push(Check {
                name: "cas directory".to_string(),
                status: CheckStatus::Ok,
                message: format!("Found at {}", path.display()),
            });
            path
        }
        None => {
            checks.push(Check {
                name: "cas directory".to_string(),
                status: CheckStatus::Error,
                message: "Not found. Run 'cas init' (or 'cas doctor --fix').".to_string(),
            });

            return output_checks(&checks, cli);
        }
    };

    // Check 2: Store type and database
    let store_type = detect_store_type(&cas_root);
    match store_type {
        StoreType::Sqlite => {
            let db_path = cas_root.join("cas.db");
            if db_path.exists() {
                checks.push(Check {
                    name: "database".to_string(),
                    status: CheckStatus::Ok,
                    message: "SQLite database found".to_string(),
                });
            } else {
                checks.push(Check {
                    name: "database".to_string(),
                    status: CheckStatus::Error,
                    message: "SQLite database missing".to_string(),
                });
            }
        }
        StoreType::Markdown => {
            checks.push(Check {
                name: "database".to_string(),
                status: CheckStatus::Warning,
                message: "Using legacy markdown storage. Consider migrating with 'cas migrate'."
                    .to_string(),
            });
        }
    }

    // Check 3: Schema migrations
    match check_migrations(&cas_root) {
        Ok(status) => {
            if status.has_pending() {
                checks.push(Check {
                    name: "schema".to_string(),
                    status: CheckStatus::Warning,
                    message: format!(
                        "v{} ({} migration(s) pending). Run 'cas update --schema-only'",
                        status.current_version,
                        status.pending_count()
                    ),
                });
            } else {
                checks.push(Check {
                    name: "schema".to_string(),
                    status: CheckStatus::Ok,
                    message: format!("v{} (up to date)", status.current_version),
                });
            }
        }
        Err(e) => {
            checks.push(Check {
                name: "schema".to_string(),
                status: CheckStatus::Error,
                message: format!("Cannot check migrations: {e}"),
            });
        }
    }

    // Check 3a: Undelivered supervisor lifecycle relays (cas-7787, GH #160).
    //
    // A relay that dies without transport is a factory failure that used to
    // leave no trace at all: the durable event looked healthy (stamped
    // `prompt_delivered_at`), the queue row read `suppressed_idle` like any
    // benign dedup, and nothing told anyone the supervisor had not been
    // reached. Surfacing it here — as a WARNING, not an Ok line — is what
    // makes "the relay is silent" distinguishable from "there was nothing to
    // relay".
    {
        match crate::store::open_prompt_queue_store(&cas_root)
            .map_err(|e| e.to_string())
            .and_then(|queue| {
                queue
                    .list_undelivered_lifecycle_relays(50)
                    .map_err(|e| e.to_string())
            }) {
            Ok(relays) if relays.is_empty() => checks.push(Check {
                name: "supervisor relay".to_string(),
                status: CheckStatus::Ok,
                message: "no undelivered lifecycle relays".to_string(),
            }),
            Ok(relays) => {
                let sample = relays
                    .iter()
                    .take(3)
                    .filter_map(|relay| relay.summary.as_deref())
                    .collect::<Vec<_>>()
                    .join(", ");
                checks.push(Check {
                    name: "supervisor relay".to_string(),
                    status: CheckStatus::Warning,
                    message: format!(
                        "{} lifecycle relay(s) expired without ever reaching the supervisor{}{}. \
                         Those lanes may still be waiting — open each task directly.",
                        relays.len(),
                        if sample.is_empty() { "" } else { ": " },
                        sample
                    ),
                });
            }
            // Fail loud rather than silently reporting health: this check
            // exists precisely because an unreadable failure signal reads as
            // success.
            Err(e) => checks.push(Check {
                name: "supervisor relay".to_string(),
                status: CheckStatus::Warning,
                message: format!("cannot check undelivered lifecycle relays: {e}"),
            }),
        }
    }

    // Check 3a-ii: messages the factory keeps failing to hand over
    // (cas-94a1, GH #169).
    //
    // The read side that makes `delivery_attempts` worth writing. A row still
    // pending after several spent attempts is the earliest honest signal that
    // a recipient is unreachable — visible here BEFORE the row exhausts its
    // budget and dies, which is the only window in which anyone can act.
    {
        const RETRY_WARN_THRESHOLD: u32 = 3;
        match crate::store::open_prompt_queue_store(&cas_root)
            .map_err(|e| e.to_string())
            .and_then(|queue| {
                queue
                    .list_most_retried_pending(RETRY_WARN_THRESHOLD, 5)
                    .map_err(|e| e.to_string())
            }) {
            Ok(rows) if rows.is_empty() => checks.push(Check {
                name: "delivery retries".to_string(),
                status: CheckStatus::Ok,
                message: format!("no pending message has spent {RETRY_WARN_THRESHOLD}+ attempts"),
            }),
            Ok(rows) => {
                let worst = rows
                    .iter()
                    .take(3)
                    .map(|row| {
                        format!(
                            "#{} -> {} ({} attempts{})",
                            row.prompt_id,
                            row.target,
                            row.delivery_attempts,
                            row.reason.map(|r| format!(", {r}")).unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                checks.push(Check {
                    name: "delivery retries".to_string(),
                    status: CheckStatus::Warning,
                    message: format!(
                        "{} pending message(s) have spent {RETRY_WARN_THRESHOLD}+ transport \
                         attempts: {worst}. The recipient is likely unreachable — check the \
                         pane before the row exhausts its budget.",
                        rows.len()
                    ),
                });
            }
            Err(e) => checks.push(Check {
                name: "delivery retries".to_string(),
                status: CheckStatus::Warning,
                message: format!("cannot check delivery retry counts: {e}"),
            }),
        }
    }

    // Check 3b: Schema details (tables and columns). An unreadable schema is a
    // warning, not a skipped check: silence here would look exactly like all
    // required tables being present.
    match get_schema_summary(&cas_root) {
        Ok(summary) => checks.push(schema_tables_check(&summary)),
        Err(error) => checks.push(Check {
            name: "tables".to_string(),
            status: CheckStatus::Warning,
            message: format!("cannot check expected tables: {error}"),
        }),
    }

    // Check 4: Store can be opened
    match open_store(&cas_root) {
        Ok(store) => match store.list() {
            Ok(entries) => {
                checks.push(Check {
                    name: "entry store".to_string(),
                    status: CheckStatus::Ok,
                    message: format!("{} entries accessible", entries.len()),
                });
            }
            Err(e) => {
                checks.push(Check {
                    name: "entry store".to_string(),
                    status: CheckStatus::Error,
                    message: format!("Cannot list entries: {e}"),
                });
            }
        },
        Err(e) => {
            checks.push(Check {
                name: "entry store".to_string(),
                status: CheckStatus::Error,
                message: format!("Cannot open store: {e}"),
            });
        }
    }

    // Check 4: Search index
    let index_dir = cas_root.join("index/tantivy");
    if index_dir.exists() {
        match SearchIndex::open(&index_dir) {
            Ok(_) => {
                checks.push(Check {
                    name: "search index".to_string(),
                    status: CheckStatus::Ok,
                    message: "Tantivy index accessible".to_string(),
                });
            }
            Err(e) => {
                checks.push(Check {
                    name: "search index".to_string(),
                    status: CheckStatus::Warning,
                    message: format!("Index may need rebuild: {e}"),
                });
            }
        }
    } else {
        checks.push(Check {
            name: "search index".to_string(),
            status: CheckStatus::Warning,
            message: "Index not found. Will be created on first search.".to_string(),
        });
    }

    // Check 4b: symbol index lag (cas-499c).
    //
    // The daemon only indexes code while it is idle (operator ruling: that gate stays), so on a
    // busy machine catch-up can trail by days. Without a line here that lag is invisible and
    // `code_search` returning thin results looks like a bug rather than a queue.
    checks.push(symbol_index_check(
        gather_symbol_index_state(&cas_root),
        chrono::Utc::now(),
    ));

    // Check 4c: the embedding drain (EPIC cas-6212 / cas-db6e, M7).
    //
    // The drain runs on a daemon tick, so its failures have no command output to
    // appear in. Without this line a drain that has been 400ing for a week looks
    // exactly like one with nothing to do — which is the cas-a924 failure shape,
    // rebuilt for a different corpus.
    checks.push(embedding_drain_check(gather_embedding_drain_state(
        &cas_root,
    )));

    // Check 4d: the structural git-history index (EPIC cas-6212 / cas-35b8,
    // spec §10.1 — "never silently stale").
    //
    // The index answers queries whether or not it is current, so staleness has
    // no natural symptom: a thin result set from a week-old watermark looks
    // exactly like a repository where nothing happened. This line is where that
    // difference becomes visible, in commits AND seconds, alongside the
    // measured provenance coverage the answers are only as good as.
    checks.push(history_index_check(gather_history_index_state(&cas_root)));

    // Check 5: Config
    match Config::load(&cas_root) {
        Ok(config) => {
            checks.push(Check {
                name: "configuration".to_string(),
                status: CheckStatus::Ok,
                message: format!(
                    "Loaded (sync: {})",
                    if config.sync.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    }
                ),
            });
        }
        Err(_) => {
            checks.push(Check {
                name: "configuration".to_string(),
                status: CheckStatus::Warning,
                message: "Using defaults (no config.toml found)".to_string(),
            });
        }
    }

    // Check 6: Sync target
    let config = Config::load(&cas_root).unwrap_or_default();
    if config.sync.enabled {
        let project_root = cas_root.parent().unwrap_or(Path::new("."));
        let sync_target = project_root.join(&config.sync.target);

        if sync_target.exists() {
            let rule_count = std::fs::read_dir(&sync_target)
                .map(|entries| {
                    entries
                        .filter_map(|e| e.ok())
                        .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
                        .count()
                })
                .unwrap_or(0);

            checks.push(Check {
                name: "sync target".to_string(),
                status: CheckStatus::Ok,
                message: format!("{} rules synced to {}", rule_count, config.sync.target),
            });
        } else {
            checks.push(Check {
                name: "sync target".to_string(),
                status: CheckStatus::Ok,
                message: format!("Will sync to {} (not yet created)", config.sync.target),
            });
        }
    }

    // Check 7: Memory statistics by type
    if let Ok(store) = open_store(&cas_root) {
        if let Ok(entries) = store.list() {
            // BTreeMap, not HashMap: doctor output is snapshot-tested (GH #92)
            // and these breakdowns are printed by iteration order.
            let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
            let mut by_tier: BTreeMap<String, usize> = BTreeMap::new();
            let mut compressed_count = 0;
            let mut helpful_count = 0;
            let mut harmful_count = 0;

            for entry in &entries {
                *by_type.entry(entry.entry_type.to_string()).or_insert(0) += 1;
                *by_tier.entry(entry.memory_tier.to_string()).or_insert(0) += 1;
                if entry.compressed {
                    compressed_count += 1;
                }
                if entry.helpful_count > 0 {
                    helpful_count += 1;
                }
                if entry.harmful_count > 0 {
                    harmful_count += 1;
                }
            }

            let type_summary: String = by_type
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join(", ");

            let tier_summary: String = by_tier
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join(", ");

            checks.push(Check {
                name: "memory stats".to_string(),
                status: CheckStatus::Ok,
                message: format!(
                    "{} total ({}) | tiers: {} | compressed: {} | helpful: {} | harmful: {}",
                    entries.len(),
                    type_summary,
                    tier_summary,
                    compressed_count,
                    helpful_count,
                    harmful_count
                ),
            });
        }
    }

    // Check 8: Rule status check
    if let Ok(rule_store) = open_rule_store(&cas_root) {
        if let Ok(rules) = rule_store.list() {
            let mut by_status: BTreeMap<String, usize> = BTreeMap::new();
            let mut stale_count = 0;

            for rule in &rules {
                *by_status.entry(rule.status.to_string()).or_insert(0) += 1;
                if rule.status == RuleStatus::Stale {
                    stale_count += 1;
                }
            }

            let status_summary: String = by_status
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join(", ");

            if stale_count > 0 {
                checks.push(Check {
                    name: "rules".to_string(),
                    status: CheckStatus::Warning,
                    message: format!(
                        "{} rules ({}) - {} stale rules need review",
                        rules.len(),
                        status_summary,
                        stale_count
                    ),
                });
            } else {
                checks.push(Check {
                    name: "rules".to_string(),
                    status: CheckStatus::Ok,
                    message: format!("{} rules ({})", rules.len(), status_summary),
                });
            }
        }
    }

    // Check 9: Task health check
    if let Ok(task_store) = open_task_store(&cas_root) {
        if let Ok(tasks) = task_store.list(None) {
            use crate::types::TaskStatus;
            let mut by_status: BTreeMap<String, usize> = BTreeMap::new();
            let open_count = tasks
                .iter()
                .filter(|t| matches!(t.status, TaskStatus::Open | TaskStatus::InProgress))
                .count();
            let blocked_count = task_store.list_blocked().map(|b| b.len()).unwrap_or(0);

            for task in &tasks {
                *by_status.entry(task.status.to_string()).or_insert(0) += 1;
            }

            let status_summary: String = by_status
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join(", ");

            // Check for orphaned dependencies
            let deps = task_store.list_dependencies(None).unwrap_or_default();
            let task_ids: std::collections::HashSet<_> = tasks.iter().map(|t| &t.id).collect();
            let orphaned_deps = deps
                .iter()
                .filter(|d| !task_ids.contains(&d.from_id) || !task_ids.contains(&d.to_id))
                .count();

            if orphaned_deps > 0 {
                checks.push(Check {
                    name: "tasks".to_string(),
                    status: CheckStatus::Warning,
                    message: format!(
                        "{} tasks ({}) | {} open, {} blocked | {} orphaned dependencies",
                        tasks.len(),
                        status_summary,
                        open_count,
                        blocked_count,
                        orphaned_deps
                    ),
                });
            } else {
                checks.push(Check {
                    name: "tasks".to_string(),
                    status: CheckStatus::Ok,
                    message: format!(
                        "{} tasks ({}) | {} open, {} blocked",
                        tasks.len(),
                        status_summary,
                        open_count,
                        blocked_count
                    ),
                });
            }
        }
    }

    // Check 10: Vector store / embeddings
    let vectors_path = cas_root.join("vectors.hnsw");
    if vectors_path.exists() {
        checks.push(Check {
            name: "embeddings".to_string(),
            status: CheckStatus::Ok,
            message: "Vector store present".to_string(),
        });
    } else {
        checks.push(Check {
            name: "embeddings".to_string(),
            status: CheckStatus::Ok,
            message: "No local vector embeddings (semantic search uses cloud).".to_string(),
        });
    }

    // Check 11: Models directory
    let models_path = cas_root.join("models");
    if models_path.exists() {
        let model_count = std::fs::read_dir(&models_path)
            .map(|entries| entries.filter_map(|e| e.ok()).count())
            .unwrap_or(0);

        if model_count > 0 {
            checks.push(Check {
                name: "models".to_string(),
                status: CheckStatus::Ok,
                message: format!("{model_count} cached model(s)"),
            });
        }
    }

    // Check 12: Claude Code MCP configuration
    let project_root = cas_root.parent().unwrap_or(Path::new("."));
    let mcp_check = check_claude_code_mcp(project_root);
    checks.push(mcp_check);

    // Check 13: Integration ID staleness (vercel/neon/github)
    // ------------------------------------------------------------------
    // Phase 3 / cas-3efe: surface stale platform IDs without the user
    // having to remember `cas integrate <p> verify`. Severity capped at
    // Warning so an MCP outage doesn't fail `cas doctor` in CI.
    for row in integration_checks(project_root) {
        checks.push(Check {
            name: row.name,
            status: match row.severity {
                crate::cli::integrate::doctor::DoctorSeverity::Ok => CheckStatus::Ok,
                crate::cli::integrate::doctor::DoctorSeverity::Warning => CheckStatus::Warning,
            },
            message: row.message,
        });
    }

    // Check 14: cloud canonical id — which bucket this project syncs into,
    // and whether any other known local project lands in the same bucket
    // (cas-f699 / GH #134).
    checks.extend(canonical_id_checks(
        &cas_root,
        collect_local_root_identities(),
    ));

    // Check 15: residual cross-project contamination from the cas-ed15 pull
    // leak (cas-fc6fa / GH #133). Read-only comparison of this project's task
    // rows against every other known project database on the host, keyed on
    // `(id, title)`.
    if cas_root.join("cas.db").is_file() {
        let report = crate::cli::foreign_rows::scan(&cas_root);
        if args.foreign_rows {
            return output_foreign_rows_detail(report, cli);
        }
        checks.push(foreign_rows_check(report.as_ref()));
    } else if args.foreign_rows {
        anyhow::bail!(
            "`cas doctor --foreign-rows` needs a SQLite database at {}; this project uses legacy \
             markdown storage. Migrate with `cas migrate` first.",
            cas_root.join("cas.db").display()
        );
    }

    output_checks(&checks, cli)
}

/// Turn a contamination scan into a single `cas doctor` row.
///
/// A failed scan is reported as a **named skip**, never as silence: an absent
/// warning on this surface reads as "no contamination", which is the exact
/// wrong answer for the user consulting doctor because they suspect it.
fn foreign_rows_check(
    report: Result<&crate::cli::foreign_rows::ForeignRowReport, &anyhow::Error>,
) -> Check {
    let report = match report {
        Ok(report) => report,
        Err(e) => {
            return Check {
                name: "cross-project rows".to_string(),
                status: CheckStatus::Warning,
                message: format!(
                    "Could not scan for cross-project task rows: {e} — contamination check \
                     SKIPPED. This is not a clean result: rows belonging to other projects may \
                     be resident here and go unreported."
                ),
            };
        }
    };

    let mut message = report.summary();
    if !report.peers_unreadable.is_empty() {
        let named = report
            .peers_unreadable
            .iter()
            .map(|p| format!("{} ({})", p.project, p.error))
            .collect::<Vec<_>>()
            .join(", ");
        message.push_str(&format!(
            ". {} project DB(s) could NOT be read and were not compared: {named}",
            report.peers_unreadable.len()
        ));
    }

    let status = if report.is_clean() && report.peers_unreadable.is_empty() {
        CheckStatus::Ok
    } else {
        CheckStatus::Warning
    };
    if !report.is_clean() {
        message.push_str(&format!(". {}", report.remediation()));
    }

    Check {
        name: "cross-project rows".to_string(),
        status,
        message,
    }
}

/// `cas doctor --foreign-rows`: the full read-only contamination listing.
fn output_foreign_rows_detail(
    report: anyhow::Result<crate::cli::foreign_rows::ForeignRowReport>,
    cli: &Cli,
) -> anyhow::Result<()> {
    let report = report?;

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report.to_json())?);
        return Ok(());
    }

    let theme = ActiveTheme::default();
    let mut out = std::io::stdout();
    let mut fmt = Formatter::stdout(&mut out, theme);

    fmt.subheading("cross-project rows")?;
    fmt.write_muted(&"─".repeat(50))?;
    fmt.newline()?;
    fmt.write_muted(&format!(
        "project `{}` — {} local task row(s) compared against {} other project DB(s) on (id, title)",
        report.local_project,
        report.local_task_count,
        report.peers_compared.len()
    ))?;
    fmt.newline()?;

    for peer in &report.peers_unreadable {
        fmt.warning(&format!(
            "NOT COMPARED: {} ({}) — {}",
            peer.project,
            peer.db_path.display(),
            peer.error
        ))?;
    }

    if report.foreign.is_empty() {
        fmt.success("no rows attributable to another project")?;
    } else {
        fmt.newline()?;
        fmt.warning(&format!(
            "{} foreign row(s) — {} not closed, {} closed",
            report.foreign.len(),
            report.foreign_open(),
            report.foreign_closed()
        ))?;
        for row in &report.foreign {
            fmt.write_raw(&format!(
                "    [{}] {} {} → {}",
                row.id,
                if row.closed { "closed  " } else { "NOT CLOSED" },
                truncate(&row.title, 60),
                row.home_project
            ))?;
            fmt.newline()?;
        }
    }

    if !report.unattributed.is_empty() {
        fmt.newline()?;
        fmt.warning(&format!(
            "{} replicated row(s) with no activity evidence in any project — home unknown, \
             {} not closed",
            report.unattributed.len(),
            report.unattributed_open()
        ))?;
        for row in &report.unattributed {
            fmt.write_raw(&format!(
                "    [{}] {} {} (also in: {})",
                row.id,
                if row.closed { "closed  " } else { "NOT CLOSED" },
                truncate(&row.title, 60),
                row.present_in.join(", ")
            ))?;
            fmt.newline()?;
        }
    }

    if !report.collisions.is_empty() {
        fmt.newline()?;
        fmt.warning(&format!(
            "{} id collision(s) — same id, DIFFERENT task. Deleting by id alone destroys real work:",
            report.collisions.len()
        ))?;
        for c in &report.collisions {
            fmt.write_raw(&format!(
                "    [{}] here: {} | {}: {}",
                c.id,
                truncate(&c.local_title, 45),
                c.other_project,
                truncate(&c.other_title, 45)
            ))?;
            fmt.newline()?;
        }
    }

    fmt.newline()?;
    fmt.subheading("knowledge-page attribution")?;
    fmt.write_muted(&format!(
        "{} local page row(s) checked by durable origin + the cloud-pull project predicate",
        report.local_knowledge_page_count
    ))?;
    fmt.newline()?;
    if report.foreign_knowledge_pages.is_empty() {
        fmt.success("no cloud-pulled pages attributed to another project")?;
    } else {
        fmt.warning(&format!(
            "{} foreign cloud-pulled knowledge page(s)",
            report.foreign_knowledge_pages.len()
        ))?;
        for page in &report.foreign_knowledge_pages {
            fmt.write_raw(&format!(
                "    [{}] {} ({}) → {}",
                page.id,
                truncate(&page.title, 55),
                page.rel_path,
                page.origin_project_id.as_deref().unwrap_or("<missing>")
            ))?;
            fmt.newline()?;
        }
    }
    if !report.unattributed_knowledge_pages.is_empty() {
        fmt.warning(&format!(
            "{} knowledge page(s) have unauditable provenance",
            report.unattributed_knowledge_pages.len()
        ))?;
        for page in &report.unattributed_knowledge_pages {
            fmt.write_raw(&format!(
                "    [{}] {} ({}) — {}",
                page.id,
                truncate(&page.title, 55),
                page.rel_path,
                page.reason
            ))?;
            fmt.newline()?;
        }
    }

    fmt.newline()?;
    if report.is_clean() {
        fmt.success("no cross-project contamination detected")?;
    } else {
        fmt.warning(&report.remediation())?;
    }

    Ok(())
}

fn truncate(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Resolve every known local CAS root to its canonical id + repository
/// identity, for the collision check.
///
/// Returns `Err` when the host known-repos registry cannot be read. It is
/// deliberately NOT mapped to an empty list: an empty list is indistinguishable
/// from "checked everything, found no collisions", and a silently-skipped
/// collision check on the one surface a contamination-suspicious user consults
/// is the same reassuring-zero failure mode this epic exists to kill. The check
/// stays advisory — the caller reports the skip as a warning rather than
/// failing `cas doctor`.
fn collect_local_root_identities() -> Result<Vec<crate::cloud::LocalRootIdentity>, String> {
    let repos = crate::worktree::discovery::list_tracked_repos().map_err(|e| e.to_string())?;
    Ok(repos
        .into_iter()
        .filter(|repo| repo.healthy)
        .filter_map(|repo| {
            let project_root = repo.path.canonicalize().unwrap_or(repo.path);
            let cas_root = project_root.join(".cas");
            let canonical_id = crate::cloud::resolve_canonical_id(&cas_root)?;
            Some(crate::cloud::LocalRootIdentity {
                git_remote: crate::cloud::derive_canonical_id_from_git_remote(&cas_root),
                project_root,
                canonical_id,
            })
        })
        .collect())
}

/// Build the canonical-id doctor rows. Pure given the resolved root list, so
/// the collision warning is testable without touching the host registry.
///
/// `known_roots` carries the registry read outcome, not just its rows: an
/// `Err` becomes a Warning row naming the failure, so a skipped collision
/// check can never masquerade as a clean one.
fn canonical_id_checks(
    cas_root: &Path,
    known_roots: Result<Vec<crate::cloud::LocalRootIdentity>, String>,
) -> Vec<Check> {
    let mut checks = Vec::new();

    let Some((canonical_id, source)) = crate::cloud::resolve_canonical_id_with_source(cas_root)
    else {
        return checks;
    };

    let mut message = format!("Cloud bucket `{canonical_id}` (from {})", source.label());
    // The read chain consults the git remote ahead of the folder name
    // (cas-f699). On a project that predates that change and was never
    // pinned, the bucket moves — say so, and name the exact command that
    // restores the old one, rather than letting sync quietly re-home.
    if source == crate::cloud::CanonicalIdSource::GitRemote
        && let Some(folder) = crate::cloud::canonical_id_from_cas_root(cas_root)
        && folder != canonical_id
    {
        message.push_str(&format!(
            ". Earlier releases used the folder name `{folder}`; if that is where \
             your synced data lives, pin it with `cas cloud project set {folder}`"
        ));
    }
    checks.push(Check {
        name: "canonical id".to_string(),
        status: CheckStatus::Ok,
        message,
    });

    let known_roots = match known_roots {
        Ok(roots) => roots,
        Err(e) => {
            checks.push(Check {
                name: "canonical id collision".to_string(),
                status: CheckStatus::Warning,
                message: format!(
                    "Could not read the known-repos registry: {e} — canonical-id collision \
                     check SKIPPED. This is not a clean result: other local projects may share \
                     bucket `{canonical_id}` and go unreported. Run `cas known-repos list` to \
                     confirm the registry is readable."
                ),
            });
            return checks;
        }
    };

    for collision in crate::cloud::detect_canonical_id_collisions(&known_roots) {
        let roots = collision
            .roots
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        checks.push(Check {
            name: "canonical id collision".to_string(),
            status: CheckStatus::Warning,
            message: format!(
                "DIFFERENT repositories share cloud bucket `{}`: {roots}. Every sync merges \
                 them into each other. Give each one its own id with `cas cloud project set \
                 <unique-id>` (run it inside each project).",
                collision.canonical_id
            ),
        });
    }

    checks
}

/// Observed state of the tree-sitter symbol index for the current project (cas-499c).
#[derive(Debug, Clone, Default)]
struct SymbolIndexState {
    /// `code.enabled` as resolved from config (defaults to true since cas-499c).
    enabled: bool,
    /// Whether `<cas_root>/index/code` exists — i.e. whether `code_search` can answer at all.
    searchable: bool,
    /// Files indexed **for this repository**. Scoped, because "is my project searchable" is the
    /// question being asked; a sibling repo's rows must not answer it.
    files: usize,
    /// Symbols in the store across every indexed repository. `CodeStore` has no per-repository
    /// symbol count, so this is labelled as a total rather than silently implying scope it
    /// does not have.
    symbols: usize,
    /// Newest `code_files.updated` for this repository: the catch-up watermark.
    last_indexed: Option<chrono::DateTime<chrono::Utc>>,
    eligible_files: usize,
    indexed_files: usize,
    failed_files: usize,
    vector_eligible: usize,
    vectorized: usize,
    vector_pending: usize,
    vector_failed: usize,
    head_lag: Option<bool>,
    scan_error: Option<String>,
    /// Set when the state could not be read; reported instead of silently skipped.
    error: Option<String>,
}

/// Anything older than this and the index is "behind" rather than merely "settling".
const SYMBOL_INDEX_LAG_WARN_SECS: i64 = 24 * 60 * 60;

fn gather_symbol_index_state(cas_root: &Path) -> SymbolIndexState {
    let enabled = Config::load(cas_root)
        .map(|config| config.code().enabled)
        .unwrap_or(true);
    let searchable = crate::hybrid_search::code::code_search_available(cas_root);

    let project_root = cas_root.parent().unwrap_or(cas_root);
    // Same derivation the indexer writes with, or the lookup would miss every row.
    let (_repo_root, repository) = crate::daemon::indexing::resolve_repository(project_root);

    let store = match crate::store::open_code_store(cas_root) {
        Ok(store) => store,
        Err(e) => {
            return SymbolIndexState {
                enabled,
                searchable,
                error: Some(e.to_string()),
                ..Default::default()
            };
        }
    };

    let files = match store.list_files(&repository, None) {
        Ok(files) => files,
        Err(e) => {
            return SymbolIndexState {
                enabled,
                searchable,
                error: Some(e.to_string()),
                ..Default::default()
            };
        }
    };

    let vector_store = match cas_store::SqliteCodeVectorStore::open(cas_root) {
        Ok(store) => store,
        Err(e) => {
            return SymbolIndexState {
                enabled,
                searchable,
                files: files.len(),
                symbols: store.count_symbols().unwrap_or(0),
                last_indexed: files.iter().map(|file| file.updated).max(),
                error: Some(e.to_string()),
                ..Default::default()
            };
        }
    };
    let vectors = vector_store.stats().unwrap_or_default();
    let scan = vector_store.index_state(&repository).ok().flatten();
    let current_head = crate::daemon::indexing::resolve_repository(project_root)
        .0
        .as_deref()
        .and_then(crate::daemon::indexing::head_commit);
    let head_lag = scan.as_ref().and_then(|scan| {
        current_head
            .as_ref()
            .zip(scan.last_head.as_ref())
            .map(|(current, indexed)| current != indexed)
    });

    SymbolIndexState {
        enabled,
        searchable,
        files: files.len(),
        symbols: store.count_symbols().unwrap_or(0),
        last_indexed: files.iter().map(|file| file.updated).max(),
        eligible_files: scan.as_ref().map(|scan| scan.eligible_files).unwrap_or(0),
        indexed_files: scan.as_ref().map(|scan| scan.indexed_files).unwrap_or(0),
        failed_files: scan.as_ref().map(|scan| scan.failed_files).unwrap_or(0),
        vector_eligible: vectors.eligible,
        vectorized: vectors.vectorized,
        vector_pending: vectors.pending,
        vector_failed: vectors.failed,
        head_lag,
        scan_error: scan.and_then(|scan| scan.last_error),
        error: None,
    }
}

fn symbol_index_check(state: SymbolIndexState, now: chrono::DateTime<chrono::Utc>) -> Check {
    let name = "symbol index".to_string();

    if let Some(error) = state.error {
        return Check {
            name,
            status: CheckStatus::Warning,
            message: format!("cannot check symbol index lag: {error}"),
        };
    }

    if !state.enabled {
        return Check {
            name,
            status: CheckStatus::Warning,
            message: "code indexing is disabled (`cas config set code.enabled true`); \
                      `code_search` will keep returning nothing"
                .to_string(),
        };
    }

    let file_lag = state.eligible_files.saturating_sub(state.indexed_files);
    if state.scan_error.is_some()
        || state.failed_files > 0
        || file_lag > 0
        || state.head_lag == Some(true)
        || state.vector_failed > 0
    {
        return Check {
            name,
            status: CheckStatus::Warning,
            message: format!(
                "symbol index coverage is incomplete: {}/{} eligible file(s), {} file(s) lagging, {} file failure(s), HEAD {}; code vectors {}/{} vectorized, {} pending, {} failed{}. Run `cas index code` to reconcile now.",
                state.indexed_files,
                state.eligible_files,
                file_lag,
                state.failed_files,
                match state.head_lag {
                    Some(true) => "behind",
                    Some(false) => "current",
                    None => "unknown",
                },
                state.vectorized,
                state.vector_eligible,
                state.vector_pending,
                state.vector_failed,
                state
                    .scan_error
                    .as_deref()
                    .map(|error| format!("; last error: {error}"))
                    .unwrap_or_default(),
            ),
        };
    }

    if state.files == 0 || !state.searchable {
        return Check {
            name,
            status: CheckStatus::Warning,
            message: format!(
                "nothing indexed for this project ({} symbol(s) in the store across all \
                 repositories){}. The daemon only indexes while idle — run `cas index code` to \
                 catch up now.",
                state.symbols,
                if state.searchable {
                    ""
                } else {
                    "; the code search index is missing"
                }
            ),
        };
    }

    let lag_secs = state
        .last_indexed
        .map(|last| (now - last).num_seconds().max(0))
        .unwrap_or(i64::MAX);

    if lag_secs >= SYMBOL_INDEX_LAG_WARN_SECS {
        Check {
            name,
            status: CheckStatus::Warning,
            message: format!(
                "symbol index is behind: {} file(s) from this project ({} symbol(s) stored in \
                 total), newest entry {}. The daemon indexes only while idle — run \
                 `cas index code` to catch up now.",
                state.files,
                state.symbols,
                format_lag(lag_secs),
            ),
        }
    } else {
        Check {
            name,
            status: CheckStatus::Ok,
            message: format!(
                "{} file(s) from this project indexed ({} symbol(s) stored in total), newest \
                 entry {}; code vectors {}/{} vectorized, {} pending, {} failed; HEAD {}",
                state.files,
                state.symbols,
                format_lag(lag_secs),
                state.vectorized,
                state.vector_eligible,
                state.vector_pending,
                state.vector_failed,
                match state.head_lag {
                    Some(true) => "behind",
                    Some(false) => "current",
                    None => "unknown",
                }
            ),
        }
    }
}

/// What `cas doctor` needs to say something true about the embedding drain.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct EmbeddingDrainState {
    /// Whether an embedder exists at all (i.e. the user is logged in). `false`
    /// is a declared boundary, not a failure.
    capability: bool,
    /// Knowledge pages awaiting a vector.
    pages_pending: usize,
    /// History commits awaiting a vector.
    commits_pending: i64,
    /// History docs awaiting a vector.
    docs_pending: i64,
    /// `history_index_state('embeddings').last_error` — what the last tick hit.
    last_error: Option<String>,
    /// `history_index_state('embeddings').last_attempt_at` — evidence the arm
    /// is running at all. `None` means it has never completed a pass, which is
    /// a different fact from "there was nothing to do".
    last_attempt: Option<String>,
}

impl EmbeddingDrainState {
    fn total_pending(&self) -> i64 {
        self.pages_pending as i64 + self.commits_pending + self.docs_pending
    }
}

fn gather_embedding_drain_state(cas_root: &Path) -> EmbeddingDrainState {
    use cas_store::{HistoryStore, KnowledgeStore, SOURCE_EMBEDDINGS};

    let capability = crate::cloud::CloudConfig::load()
        .map(|config| crate::cloud::KnowledgeEmbedder::from_config(&config).is_some())
        .unwrap_or(false);

    let pages_pending = cas_store::SqliteKnowledgeStore::open(cas_root)
        .ok()
        .and_then(|store| store.count_pending_embedding().ok())
        .unwrap_or(0);

    let mut state = EmbeddingDrainState {
        capability,
        pages_pending,
        ..Default::default()
    };

    if let Ok(store) = cas_store::SqliteHistoryStore::open(cas_root) {
        if let Ok((commits, docs)) = store.count_pending_embedding() {
            state.commits_pending = commits;
            state.docs_pending = docs;
        }
        if let Ok(repo_root) = crate::history::repo_root_for(cas_root) {
            let repository = crate::history::repository_id(&repo_root);
            if let Ok(Some(ledger)) = store.index_state(&repository, SOURCE_EMBEDDINGS) {
                state.last_error = ledger.last_error;
                state.last_attempt = ledger.last_attempt_at;
            }
        }
    }

    state
}

fn embedding_drain_check(state: EmbeddingDrainState) -> Check {
    let name = "embedding drain".to_string();
    let pending = state.total_pending();

    // A real failure outranks everything else: it is the reason the queue is
    // not moving, and it must never be summarised away as a backlog.
    if let Some(error) = &state.last_error {
        return Check {
            name,
            status: CheckStatus::Warning,
            message: format!(
                "last drain reported: {error} ({pending} unit(s) still awaiting a vector)"
            ),
        };
    }

    if !state.capability {
        // Not logged in. A boundary of the installation, so this is only worth
        // a warning when there is actually a queue going nowhere.
        return Check {
            name,
            status: if pending > 0 {
                CheckStatus::Warning
            } else {
                CheckStatus::Ok
            },
            message: if pending > 0 {
                format!(
                    "no cloud embedding capability (not logged in): {pending} unit(s) will stay \
                     unembedded and semantic search stays absent"
                )
            } else {
                "no cloud embedding capability (not logged in); nothing is queued".to_string()
            },
        };
    }

    if pending == 0 {
        return Check {
            name,
            status: CheckStatus::Ok,
            message: match &state.last_attempt {
                Some(at) => format!("nothing pending (last drain {at})"),
                None => "nothing pending".to_string(),
            },
        };
    }

    // A queue with a capability present and no error is the drain doing its
    // job across ticks — say so, and say how deep it is.
    Check {
        name,
        status: CheckStatus::Ok,
        message: format!(
            "{pending} unit(s) queued ({} page(s), {} commit(s), {} doc(s)); the daemon drains \
             them on its tick{}",
            state.pages_pending,
            state.commits_pending,
            state.docs_pending,
            match &state.last_attempt {
                Some(at) => format!(" — last drain {at}"),
                None => " — no drain has completed yet".to_string(),
            }
        ),
    }
}

/// What `cas doctor` needs to say something true about the structural
/// git-history index (EPIC cas-6212 / cas-35b8, spec §10.1).
///
/// Gathered separately from the verdict so the verdict is a pure function of
/// observed state: staleness can then be *seeded* in a test rather than waited
/// for, which is the only way to assert "a stale index is loudly visible"
/// without a test that sleeps.
#[derive(Debug, Clone, Default, PartialEq)]
struct HistoryIndexHealth {
    /// The read itself failed. Reported rather than skipped: an unreadable
    /// health signal reads as health, which is the exact failure §10.1 exists
    /// to prevent.
    error: Option<String>,
    /// `index_history` — whether the daemon arm runs at all.
    enabled: bool,
    /// Commits between the watermark and HEAD. `None` when the watermark is
    /// unusable (never run, or no longer an ancestor of HEAD) — which is a
    /// different fact from 0 and must not be rendered as "fresh".
    lag_commits: Option<i64>,
    /// Wall-clock age since the last successful index observation while a
    /// non-zero lag exists. `None` when it cannot be established honestly.
    lag_seconds: Option<i64>,
    /// False means the watermark is not on HEAD's ancestry — §10.2 row 3, a
    /// re-run condition, never a silent gap.
    watermark_is_ancestor: bool,
    /// Whether the initial backfill ever finished.
    backfill_complete: bool,
    /// Has the git arm ever produced a watermark at all?
    ever_indexed: bool,
    indexed_commits: i64,
    repo_commits: i64,
    /// `(source, last_error)` for every ledger row carrying a failure — git,
    /// github, changelog, embeddings. Ordered as gathered; the check names at
    /// most three, per §10.1.
    failing_sources: Vec<(String, String)>,
    /// One daemon tick, read from the daemon's own default rather than
    /// hardcoded, so a retuned interval cannot leave doctor asserting a
    /// threshold the daemon does not use.
    tick_interval_secs: u64,
    /// M5's measured ledger (spec §10.1): the high-confidence figure and the
    /// any-edge figure. Both, deliberately — publishing only the second makes
    /// a substring-grade corpus look solved.
    provenance_coverage_pct: Option<f64>,
    provenance_any_coverage_pct: Option<f64>,
    /// Why the coverage measurement is incomplete, when it is. Carried rather
    /// than flattened to a bool because M5 sets this for *partial* measurement
    /// too — a store that can read only some edges must not publish a number
    /// that reads as complete.
    provenance_unmeasurable_reason: Option<String>,
}

fn gather_history_index_state(cas_root: &Path) -> HistoryIndexHealth {
    gather_history_index_state_at(cas_root, chrono::Utc::now())
}

fn gather_history_index_state_at(
    cas_root: &Path,
    now: chrono::DateTime<chrono::Utc>,
) -> HistoryIndexHealth {
    let tick_interval_secs =
        crate::mcp::daemon::EmbeddedDaemonConfig::default().history_index_interval_secs;
    let enabled = crate::mcp::daemon::EmbeddedDaemonConfig::default().index_history;
    let base = HistoryIndexHealth {
        enabled,
        tick_interval_secs,
        ..Default::default()
    };

    let repo_root = match crate::history::repo_root_for(cas_root) {
        Ok(root) => root,
        Err(e) => {
            return HistoryIndexHealth {
                error: Some(e.to_string()),
                ..base
            };
        }
    };

    let status = match crate::history::status(cas_root, &repo_root) {
        Ok(status) => status,
        Err(e) => {
            return HistoryIndexHealth {
                error: Some(e.to_string()),
                ..base
            };
        }
    };

    // Every ledger row that carries a failure, named by source. `last_error` is
    // the declared-boundary channel (§10.2 row 2): GitHub being absent is not a
    // git-index failure, and conflating them would hide both.
    let failing_sources: Vec<(String, String)> = [
        ("git", status.state.as_ref()),
        ("github", status.github_state.as_ref()),
        ("changelog", status.changelog_state.as_ref()),
    ]
    .into_iter()
    .filter_map(|(source, state)| {
        state
            .and_then(|s| s.last_error.clone())
            .map(|err| (source.to_string(), err))
    })
    .collect();

    let lag_seconds = status.lag_age_seconds_at(now);

    let coverage = cas_store::SqliteHistoryStore::open(cas_root)
        .ok()
        .and_then(|store| {
            use cas_store::HistoryStore;
            store.provenance_coverage(&status.repository).ok()
        });

    HistoryIndexHealth {
        error: None,
        lag_commits: status.lag_commits,
        lag_seconds,
        watermark_is_ancestor: status.watermark_is_ancestor,
        backfill_complete: status.state.as_ref().is_some_and(|s| s.backfill_complete),
        ever_indexed: status
            .state
            .as_ref()
            .is_some_and(|s| s.last_indexed_sha.is_some()),
        indexed_commits: status.indexed_commits,
        repo_commits: status.repo_commits,
        failing_sources,
        provenance_coverage_pct: coverage.as_ref().and_then(|c| c.coverage_pct),
        provenance_any_coverage_pct: coverage.as_ref().and_then(|c| c.any_coverage_pct),
        provenance_unmeasurable_reason: match coverage.as_ref() {
            Some(c) => c.unmeasurable_reason.clone(),
            // No store at all is itself a reason, and a silent None here would
            // render as a confident "0%".
            None => Some("history store unreadable".to_string()),
        },
        ..base
    }
}

/// The §10.1 verdict. Pure, so staleness is seeded rather than waited for.
fn history_index_check(state: HistoryIndexHealth) -> Check {
    let name = "code history index".to_string();

    // Fail loud rather than silently reporting health — the same reason the
    // supervisor-relay and delivery-retry checks above have this arm.
    if let Some(error) = &state.error {
        return Check {
            name,
            status: CheckStatus::Warning,
            message: format!("cannot check code history index: {error}"),
        };
    }

    if !state.enabled {
        return Check {
            name,
            status: CheckStatus::Warning,
            message: "code history indexing is disabled; `action=history` will keep returning \
                      nothing and provenance cannot be measured"
                .to_string(),
        };
    }

    // Provenance is reported on every arm below, because a coverage figure is
    // only honest next to the freshness of the index it was measured over.
    let provenance = match (
        state.provenance_coverage_pct,
        state.provenance_any_coverage_pct,
        &state.provenance_unmeasurable_reason,
    ) {
        // Both figures, always together. A partial-measurement reason is
        // appended rather than suppressing the numbers: the figures are real,
        // they just are not the whole picture, and saying so is the point.
        (Some(high), Some(any), reason) => format!(
            "; provenance {high:.1}% high-confidence, {any:.1}% any-edge{}",
            reason
                .as_deref()
                .map(|r| format!(" (partial: {})", truncate(r, 60)))
                .unwrap_or_default()
        ),
        (Some(high), None, _) => format!("; provenance {high:.1}% high-confidence"),
        // Unmeasurable is NOT 0%. Rendering it as a number would invent a fact,
        // which is the single dishonesty §10.1 names by hand.
        (None, _, reason) => format!(
            "; provenance coverage unmeasurable{}",
            reason
                .as_deref()
                .map(|r| format!(" ({})", truncate(r, 60)))
                .unwrap_or_default()
        ),
    };

    // A named failure outranks staleness: it is usually the *reason* for the
    // staleness, and summarising it as lag would bury the cause.
    if !state.failing_sources.is_empty() {
        let worst = state
            .failing_sources
            .iter()
            .take(3)
            .map(|(source, err)| format!("{source}: {}", truncate(err, 80)))
            .collect::<Vec<_>>()
            .join("; ");
        return Check {
            name,
            status: CheckStatus::Warning,
            message: format!(
                "{} source(s) reporting errors — {worst}{provenance}",
                state.failing_sources.len()
            ),
        };
    }

    if !state.ever_indexed {
        return Check {
            name,
            status: CheckStatus::Warning,
            message: format!(
                "never indexed: 0 of {} commit(s) — run `cas history backfill`{provenance}",
                state.repo_commits
            ),
        };
    }

    // §10.2 row 3. `lag_commits: None` means the watermark is no longer on
    // HEAD's ancestry (history rewritten, or a branch switch). The declared
    // behaviour is a re-run, and the one thing it must never be is invisible.
    if !state.watermark_is_ancestor || state.lag_commits.is_none() {
        return Check {
            name,
            status: CheckStatus::Warning,
            message: format!(
                "watermark is not an ancestor of HEAD — the indexed range and the current \
                 branch have diverged, so lag is unknown rather than 0. The next pass \
                 re-runs the backfill; run `cas history backfill` to close it \
                 now{provenance}"
            ),
        };
    }

    if !state.backfill_complete {
        return Check {
            name,
            status: CheckStatus::Warning,
            message: format!(
                "backfill incomplete: {} of {} commit(s) indexed{provenance}",
                state.indexed_commits, state.repo_commits
            ),
        };
    }

    let lag_commits = state.lag_commits.unwrap_or(0);
    let Some(lag_secs) = state.lag_seconds else {
        return Check {
            name,
            status: CheckStatus::Warning,
            message: format!(
                "index is behind: {lag_commits} commit(s), but the last successful observation \
                 time is unknown rather than fresh. Run `cas history backfill` to catch up \
                 now{provenance}"
            ),
        };
    };

    // "Under one tick interval" is what separates an index that is *settling*
    // from one that is *behind*. A non-zero lag younger than a tick is simply
    // the window between commits arriving and the daemon's next pass.
    if lag_commits > 0 && lag_secs >= state.tick_interval_secs as i64 {
        return Check {
            name,
            status: CheckStatus::Warning,
            message: format!(
                "index is behind: {lag_commits} commit(s) and {} un-indexed, past the {}s \
                 daemon tick. Run `cas history backfill` to catch up now{provenance}",
                format_lag(lag_secs),
                state.tick_interval_secs
            ),
        };
    }

    Check {
        name,
        status: CheckStatus::Ok,
        message: format!(
            "{} of {} commit(s) indexed, {lag_commits} behind ({}){provenance}",
            state.indexed_commits,
            state.repo_commits,
            format_lag(lag_secs)
        ),
    }
}

fn format_lag(secs: i64) -> String {
    if secs == i64::MAX {
        return "never".to_string();
    }
    if secs < 60 {
        return format!("{secs}s old");
    }
    if secs < 3600 {
        return format!("{}m old", secs / 60);
    }
    if secs < 86_400 {
        return format!("{}h old", secs / 3600);
    }
    format!("{}d old", secs / 86_400)
}

/// Helper around `cli::integrate::doctor::collect_reports` +
/// `render_for_doctor`. Lifted out so it can be tested with a synthetic
/// repo root that doesn't need a `.cas` parent.
fn integration_checks(project_root: &Path) -> Vec<crate::cli::integrate::doctor::DoctorRow> {
    let reports = crate::cli::integrate::doctor::collect_reports(project_root);
    crate::cli::integrate::doctor::render_for_doctor(&reports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // ── cas-f699 / GH #134: canonical-id doctor rows ─────────────────────

    // ── cas-fc6fa / GH #133: cross-project contamination doctor row ──────

    #[test]
    fn foreign_rows_check_reports_counts_and_a_safe_remediation_path_cas_fc6fa() {
        use crate::cli::foreign_rows::{DbSnapshot, ForeignRow, ForeignRowReport, IdCollision};

        let report = ForeignRowReport {
            local_project: "cas-src".to_string(),
            local_task_count: 1484,
            peers_compared: vec!["accounting".to_string()],
            foreign: vec![
                ForeignRow {
                    id: "cas-0001".to_string(),
                    title: "Reconcile Q3 payroll".to_string(),
                    closed: false,
                    home_project: "accounting".to_string(),
                    also_present_in: Vec::new(),
                },
                ForeignRow {
                    id: "cas-0002".to_string(),
                    title: "Finished months ago".to_string(),
                    closed: true,
                    home_project: "accounting".to_string(),
                    also_present_in: Vec::new(),
                },
            ],
            collisions: vec![IdCollision {
                id: "cas-0003".to_string(),
                local_title: "Real local work".to_string(),
                other_project: "accounting".to_string(),
                other_title: "A different real task".to_string(),
            }],
            ..Default::default()
        };
        let _ = DbSnapshot::default(); // keep the public snapshot type exercised

        let check = foreign_rows_check(Ok(&report));

        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(
            check.message.contains("2 foreign task row(s)"),
            "{}",
            check.message
        );
        // AC3: the non-closed count is what lies in ready queues.
        assert!(
            check.message.contains("1 of them not closed"),
            "{}",
            check.message
        );
        assert!(check.message.contains("accounting"), "{}", check.message);
        // AC1: a remediation path is named.
        assert!(
            check.message.contains("cas cloud purge-foreign"),
            "{}",
            check.message
        );
        // AC2: the identity constraint is stated where a human would act on it.
        assert!(check.message.contains("(id, title)"), "{}", check.message);
    }

    #[test]
    fn foreign_rows_check_zero_states_its_coverage_never_a_bare_clean_cas_fc6fa() {
        use crate::cli::foreign_rows::ForeignRowReport;

        let report = ForeignRowReport {
            local_project: "cas-src".to_string(),
            local_task_count: 1485,
            peers_compared: vec!["accounting".to_string(), "ozer".to_string()],
            ..Default::default()
        };

        let check = foreign_rows_check(Ok(&report));

        assert!(matches!(check.status, CheckStatus::Ok));
        // An Ok row that just said "clean" would be indistinguishable from a
        // scan that compared nothing at all.
        assert!(
            check.message.contains("0 foreign task row(s)"),
            "{}",
            check.message
        );
        assert!(
            check.message.contains("1485 local row(s)"),
            "{}",
            check.message
        );
        assert!(
            check.message.contains("2 project DB(s)"),
            "{}",
            check.message
        );
        assert!(
            check.message.contains("0 DB(s) unreadable"),
            "{}",
            check.message
        );
    }

    #[test]
    fn foreign_rows_check_names_a_failed_scan_instead_of_reading_clean_cas_fc6fa() {
        // Same reassuring-zero failure mode as the canonical-id registry row:
        // a scan that could not run must not render as "no contamination".
        let err = anyhow::anyhow!("disk I/O error");
        let check = foreign_rows_check(Err(&err));

        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(check.message.contains("SKIPPED"), "{}", check.message);
        assert!(
            check.message.contains("disk I/O error"),
            "{}",
            check.message
        );
    }

    #[test]
    fn foreign_rows_check_warns_when_a_peer_db_could_not_be_read_cas_fc6fa() {
        use crate::cli::foreign_rows::{ForeignRowReport, UnreadablePeer};

        let report = ForeignRowReport {
            local_project: "cas-src".to_string(),
            local_task_count: 10,
            peers_compared: vec!["accounting".to_string()],
            peers_unreadable: vec![UnreadablePeer {
                project: "ozer".to_string(),
                db_path: std::path::PathBuf::from("/home/u/ozer/.cas/cas.db"),
                error: "file is not a database".to_string(),
            }],
            ..Default::default()
        };

        let check = foreign_rows_check(Ok(&report));

        // Clean against what could be read, but partial coverage is not a
        // clean bill of health.
        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(check.message.contains("ozer"), "{}", check.message);
        assert!(
            check.message.contains("could NOT be read"),
            "{}",
            check.message
        );
    }

    fn messages(checks: &[Check], name: &str) -> Vec<String> {
        checks
            .iter()
            .filter(|c| c.name == name)
            .map(|c| c.message.clone())
            .collect()
    }

    #[test]
    fn canonical_id_check_reports_the_resolved_bucket_and_its_source() {
        let temp = TempDir::new().unwrap();
        let cas_root = temp.path().join("gabber-studio/.cas");
        fs::create_dir_all(&cas_root).unwrap();

        let checks = canonical_id_checks(&cas_root, Ok(Vec::new()));
        let msg = messages(&checks, "canonical id").join("");
        assert!(msg.contains("gabber-studio"), "got: {msg}");
        assert!(msg.contains("folder name"), "got: {msg}");
        // No collision row when only one root is known.
        assert!(messages(&checks, "canonical id collision").is_empty());
    }

    #[test]
    fn canonical_id_check_warns_loudly_on_a_shared_bucket() {
        let temp = TempDir::new().unwrap();
        let cas_root = temp.path().join("accounting/.cas");
        fs::create_dir_all(&cas_root).unwrap();

        let known = vec![
            crate::cloud::LocalRootIdentity {
                project_root: "/home/u/client-one/accounting".into(),
                canonical_id: "accounting".to_string(),
                git_remote: Some("github.com/client-one/accounting".to_string()),
            },
            crate::cloud::LocalRootIdentity {
                project_root: "/home/u/client-two/accounting".into(),
                canonical_id: "accounting".to_string(),
                git_remote: Some("gitlab.com/client-two/accounting".to_string()),
            },
        ];
        let checks = canonical_id_checks(&cas_root, Ok(known));
        let collision = checks
            .iter()
            .find(|c| c.name == "canonical id collision")
            .expect("collision row must be present");
        assert!(matches!(collision.status, CheckStatus::Warning));
        assert!(collision.message.contains("client-one/accounting"));
        assert!(collision.message.contains("client-two/accounting"));
        assert!(collision.message.contains("cas cloud project set"));
    }

    #[test]
    fn unreadable_registry_is_named_as_a_skipped_check_never_silence() {
        // The reassuring-zero failure mode: if the known-repos registry can
        // not be read, the collision check does not run — and an absent
        // warning would read as "no collisions" on the exact surface a
        // contamination-suspicious user consults. It must say so out loud.
        let temp = TempDir::new().unwrap();
        let cas_root = temp.path().join("some-project/.cas");
        fs::create_dir_all(&cas_root).unwrap();

        let checks = canonical_id_checks(&cas_root, Err("disk I/O error".to_string()));
        let row = checks
            .iter()
            .find(|c| c.name == "canonical id collision")
            .expect("an unreadable registry must still emit a collision row");
        assert!(matches!(row.status, CheckStatus::Warning));
        assert!(row.message.contains("disk I/O error"), "{}", row.message);
        assert!(row.message.contains("SKIPPED"), "{}", row.message);
        // The bucket row is still reported — the skip is scoped to the
        // collision check, not the whole diagnostic.
        assert_eq!(messages(&checks, "canonical id").len(), 1);
    }

    #[test]
    fn collect_local_root_identities_propagates_a_corrupt_registry() {
        // End-to-end companion to the seam test above: a real `~/.cas/cas.db`
        // that is not a database must surface as Err, not as an empty list.
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let host_cas = home.join(".cas");
            fs::create_dir_all(&host_cas).unwrap();
            fs::write(host_cas.join("cas.db"), b"this is not a sqlite database").unwrap();

            let result = collect_local_root_identities();
            assert!(
                result.is_err(),
                "a corrupt host registry must not read as an empty (=no collisions) list, got {:?}",
                result
            );

            // Fail-closed must not become fail-always: a healthy registry with
            // no rows still reads Ok(empty), which is a genuine "no collisions".
            fs::remove_file(host_cas.join("cas.db")).unwrap();
            crate::store::known_repos::ensure_host_schema().unwrap();
            assert_eq!(collect_local_root_identities().unwrap(), Vec::new());
        });
    }

    #[test]
    fn canonical_id_check_names_the_legacy_folder_bucket_when_the_remote_wins() {
        // Migration safety: an unpinned repo whose bucket moves from the
        // folder name to the git remote must be told where its old data is.
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("legacy-folder");
        let cas_root = project.join(".cas");
        fs::create_dir_all(&cas_root).unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["remote", "add", "origin", "git@github.com:acme/renamed.git"],
        ] {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&project)
                .args(&args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
        }

        let checks = canonical_id_checks(&cas_root, Ok(Vec::new()));
        let msg = messages(&checks, "canonical id").join("");
        assert!(msg.contains("github.com/acme/renamed"), "got: {msg}");
        assert!(
            msg.contains("cas cloud project set legacy-folder"),
            "must name the pin command for the pre-cas-f699 bucket, got: {msg}"
        );
    }

    /// `cas-3efe`: doctor's integrations check on a project with no SKILL.md
    /// files anywhere collapses to a single Ok row stating "no integrations
    /// configured". This is the green-field new-repo case — doctor must not
    /// nag about missing platform configs.
    #[test]
    fn integration_checks_no_integrations_configured_emits_single_ok_row() {
        let repo = TempDir::new().unwrap();
        let rows = integration_checks(repo.path());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "integrations");
        assert!(matches!(
            rows[0].severity,
            crate::cli::integrate::doctor::DoctorSeverity::Ok
        ));
        assert!(rows[0].message.contains("no integrations configured"));
    }

    /// `cas-3efe`: a github SKILL.md with a recorded OWNER/REPO that doesn't
    /// match any local `git remote -v` (no remotes at all in the tempdir)
    /// produces a github "stale" row at Warning severity, with a hint to
    /// run `cas integrate github refresh`.
    #[test]
    fn integration_checks_github_stale_when_recorded_repo_missing_locally() {
        let repo = TempDir::new().unwrap();
        let github_skill = repo.path().join(".claude/skills/github-repo/SKILL.md");
        fs::create_dir_all(github_skill.parent().unwrap()).unwrap();
        fs::write(
            &github_skill,
            "---\nname: github-repo\n---\n\n## Identity\n\
             <!-- keep github-repo -->\n\
             | **Full name** | `someone/some-repo` |\n\
             <!-- /keep github-repo -->\n",
        )
        .unwrap();

        let rows = integration_checks(repo.path());
        // Stale platform's row should be present and at Warning severity.
        let github_row = rows
            .iter()
            .find(|r| r.name.contains("github"))
            .expect("github row");
        assert!(matches!(
            github_row.severity,
            crate::cli::integrate::doctor::DoctorSeverity::Warning
        ));
        assert!(
            github_row.message.contains("stale"),
            "got: {}",
            github_row.message
        );
        assert!(
            github_row.message.contains("cas integrate github refresh"),
            "got: {}",
            github_row.message
        );
    }

    /// `cas-3efe`: when neon is configured but the live client can't reach
    /// the platform (LiveNeonClient is a placeholder that always errors),
    /// every recorded branch becomes McpUnreachable. Doctor reports
    /// "skipped — MCP not configured" at Warning severity rather than
    /// hard-failing — so the whole `cas doctor` run still exits cleanly
    /// in CI environments without an MCP server.
    #[test]
    fn integration_checks_neon_mcp_unreachable_is_skipped_not_error() {
        // The binary installs this before dispatch; this standalone library test
        // exercises reqwest without passing through main.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let repo = TempDir::new().unwrap();
        let neon_skill = repo.path().join(".claude/skills/neon-database/SKILL.md");
        fs::create_dir_all(neon_skill.parent().unwrap()).unwrap();
        fs::write(
            &neon_skill,
            "---\nname: neon-database\n---\n\n\
             <!-- keep neon-ids -->\n\
             | **org_id** | `org-x` |\n\
             | **projectId** | `proj-y` |\n\
             | **databaseName** | `neondb` |\n\
             | **production branchId** | `br-prod` |\n\
             <!-- /keep neon-ids -->\n",
        )
        .unwrap();

        let rows = integration_checks(repo.path());
        let neon_row = rows
            .iter()
            .find(|r| r.name.contains("neon"))
            .expect("neon row");
        assert!(matches!(
            neon_row.severity,
            crate::cli::integrate::doctor::DoctorSeverity::Warning
        ));
        assert!(
            neon_row.message.contains("MCP not configured"),
            "got: {}",
            neon_row.message
        );
    }

    // ===== cas-499c: symbol index lag =====

    fn healthy_state() -> SymbolIndexState {
        SymbolIndexState {
            enabled: true,
            searchable: true,
            files: 120,
            symbols: 3_400,
            last_indexed: None,
            error: None,
            ..Default::default()
        }
    }

    /// A stale watermark must surface as a warning naming the catch-up command; before cas-499c
    /// there was no line at all, so a symbol index days behind looked identical to a healthy one.
    #[test]
    fn symbol_index_check_warns_on_stale_watermark() {
        let now = chrono::Utc::now();
        let state = SymbolIndexState {
            last_indexed: Some(now - chrono::Duration::days(6)),
            ..healthy_state()
        };

        let check = symbol_index_check(state, now);
        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(
            check.message.contains("behind"),
            "message: {}",
            check.message
        );
        assert!(
            check.message.contains("120 file(s) from this project"),
            "message: {}",
            check.message
        );
        assert!(
            check.message.contains("6d old"),
            "message: {}",
            check.message
        );
        assert!(
            check.message.contains("cas index code"),
            "a lag warning must name the catch-up command: {}",
            check.message
        );
    }

    #[test]
    fn symbol_index_check_names_file_vector_and_head_lag() {
        let now = chrono::Utc::now();
        let state = SymbolIndexState {
            eligible_files: 100,
            indexed_files: 96,
            failed_files: 1,
            vector_eligible: 900,
            vectorized: 850,
            vector_pending: 47,
            vector_failed: 3,
            head_lag: Some(true),
            scan_error: Some("one parser failure".into()),
            last_indexed: Some(now),
            ..healthy_state()
        };
        let check = symbol_index_check(state, now);
        assert!(matches!(check.status, CheckStatus::Warning));
        for expected in [
            "96/100 eligible",
            "4 file(s) lagging",
            "HEAD behind",
            "850/900 vectorized",
            "47 pending",
            "3 failed",
            "one parser failure",
        ] {
            assert!(check.message.contains(expected), "missing {expected}: {}", check.message);
        }
    }

    /// A freshly-indexed tree reports Ok with the counts, not a warning.
    #[test]
    fn symbol_index_check_ok_when_fresh() {
        let now = chrono::Utc::now();
        let state = SymbolIndexState {
            last_indexed: Some(now - chrono::Duration::minutes(7)),
            ..healthy_state()
        };

        let check = symbol_index_check(state, now);
        assert!(
            matches!(check.status, CheckStatus::Ok),
            "message: {}",
            check.message
        );
        assert!(
            check.message.contains("120 file(s) from this project"),
            "the count must be scoped to this project, not a bare global: {}",
            check.message
        );
        assert!(
            check.message.contains("3400 symbol(s) stored in total"),
            "the symbol count is store-wide and must say so: {}",
            check.message
        );
    }

    /// Empty index and missing BM25 directory are the "never ran" case, not a silent pass.
    #[test]
    fn symbol_index_check_warns_when_never_indexed() {
        let now = chrono::Utc::now();
        let state = SymbolIndexState {
            files: 0,
            symbols: 0,
            searchable: false,
            ..healthy_state()
        };

        let check = symbol_index_check(state, now);
        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(
            check.message.contains("nothing indexed for this project"),
            "message: {}",
            check.message
        );
        assert!(
            check.message.contains("code search index is missing"),
            "message: {}",
            check.message
        );
        assert!(
            check.message.contains("cas index code"),
            "message: {}",
            check.message
        );
    }

    /// An explicit opt-out must be reported honestly rather than as a healthy index.
    #[test]
    fn symbol_index_check_warns_when_disabled() {
        let now = chrono::Utc::now();
        let state = SymbolIndexState {
            enabled: false,
            ..healthy_state()
        };

        let check = symbol_index_check(state, now);
        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(
            check.message.contains("disabled"),
            "message: {}",
            check.message
        );
    }

    /// A store that cannot be read is a warning that says so — never a silent skip.
    #[test]
    fn symbol_index_check_reports_read_errors() {
        let now = chrono::Utc::now();
        let state = SymbolIndexState {
            error: Some("database is locked".to_string()),
            ..healthy_state()
        };

        let check = symbol_index_check(state, now);
        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(
            check.message.contains("database is locked"),
            "message: {}",
            check.message
        );
    }

    /// A drain failure must reach the operator here (EPIC cas-6212 / cas-db6e).
    /// The tick has no command output of its own, so if this line stays quiet a
    /// permanently-failing drain is indistinguishable from an empty queue —
    /// which is exactly the cas-a924 shape.
    #[test]
    fn embedding_drain_check_surfaces_the_last_failure() {
        let check = embedding_drain_check(EmbeddingDrainState {
            capability: true,
            commits_pending: 40,
            docs_pending: 2,
            last_error: Some("history: Embedding request failed with status 429".to_string()),
            ..Default::default()
        });
        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(check.message.contains("429"), "message: {}", check.message);
        assert!(check.message.contains("42"), "message: {}", check.message);
    }

    #[test]
    fn embedding_drain_check_reports_a_queue_without_calling_it_broken() {
        let check = embedding_drain_check(EmbeddingDrainState {
            capability: true,
            pages_pending: 3,
            commits_pending: 100,
            docs_pending: 7,
            last_attempt: Some("2026-08-08T00:00:00Z".to_string()),
            ..Default::default()
        });
        // A backlog with a working drain is progress, not a fault.
        assert!(matches!(check.status, CheckStatus::Ok));
        assert!(check.message.contains("110"), "message: {}", check.message);
    }

    #[test]
    fn embedding_drain_check_calls_out_a_queue_with_no_capability() {
        let stranded = embedding_drain_check(EmbeddingDrainState {
            capability: false,
            commits_pending: 5,
            ..Default::default()
        });
        assert!(matches!(stranded.status, CheckStatus::Warning));
        assert!(
            stranded.message.contains("not logged in"),
            "message: {}",
            stranded.message
        );

        // Logged out with nothing queued is an ordinary, fully-supported state.
        let idle = embedding_drain_check(EmbeddingDrainState {
            capability: false,
            ..Default::default()
        });
        assert!(matches!(idle.status, CheckStatus::Ok));
    }

    /// End-to-end over a real code store: a seeded stale `code_files.updated` row is what the
    /// doctor line actually reads, so the gather step must find it.
    #[test]
    fn gather_symbol_index_state_reads_seeded_stale_watermark() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let repo = temp.path().join("seeded-repo");
        fs::create_dir_all(repo.join(".git")).expect(".git dir");
        let cas_root = repo.join(".cas");
        fs::create_dir_all(&cas_root).expect("cas root");

        let store = crate::store::open_code_store(&cas_root).expect("code store");
        let stale = chrono::Utc::now() - chrono::Duration::days(9);
        let path = repo.join("src/lib.rs").to_string_lossy().to_string();
        let id = store.generate_file_id_for("seeded-repo", &path);
        store
            .add_file(&cas_code::CodeFile {
                id,
                path,
                repository: "seeded-repo".to_string(),
                language: cas_code::Language::Rust,
                size: 42,
                line_count: 3,
                commit_hash: None,
                content_hash: "deadbeef".to_string(),
                created: stale,
                updated: stale,
                scope: "project".to_string(),
            })
            .expect("seed code file");

        let state = gather_symbol_index_state(&cas_root);
        assert_eq!(
            state.files, 1,
            "seeded row not found (repository derivation drift?)"
        );
        let last = state.last_indexed.expect("watermark");
        assert!(
            (last - stale).num_seconds().abs() <= 1,
            "watermark {last} did not match the seeded {stale}"
        );

        let check = symbol_index_check(state, chrono::Utc::now());
        assert!(matches!(check.status, CheckStatus::Warning));
    }

    // ===================================================================
    // Code history index check (EPIC cas-6212 / cas-35b8, spec §10.1)
    //
    // The verdict is a pure function of gathered state, so every staleness
    // shape below is SEEDED rather than waited for. A test that slept for a
    // tick interval to observe staleness would be the slowest test in the
    // suite and would still only prove one of these arms.
    // ===================================================================

    /// The shape a healthy, caught-up index has. Each test below mutates one
    /// field, so what it is actually asserting is unambiguous.
    fn healthy_history() -> HistoryIndexHealth {
        HistoryIndexHealth {
            error: None,
            enabled: true,
            lag_commits: Some(0),
            lag_seconds: Some(0),
            watermark_is_ancestor: true,
            backfill_complete: true,
            ever_indexed: true,
            indexed_commits: 2_478,
            repo_commits: 2_478,
            failing_sources: vec![],
            tick_interval_secs: 300,
            provenance_coverage_pct: Some(8.9),
            provenance_any_coverage_pct: Some(23.1),
            provenance_unmeasurable_reason: None,
        }
    }

    /// AC1: lag is visible in BOTH commits and seconds, per §10.1.
    #[test]
    fn history_index_check_reports_lag_in_commits_and_seconds() {
        let check = history_index_check(healthy_history());
        assert!(matches!(check.status, CheckStatus::Ok), "{}", check.message);
        assert!(check.message.contains("2478 of 2478"), "{}", check.message);
        assert!(check.message.contains("0 behind"), "{}", check.message);
    }

    /// AC1, the load-bearing one: a stale index must be LOUD. Seeded two days
    /// behind, well past the 300s tick.
    #[test]
    fn history_index_check_warns_loudly_on_a_seeded_stale_index() {
        let check = history_index_check(HistoryIndexHealth {
            lag_commits: Some(41),
            lag_seconds: Some(2 * 24 * 60 * 60),
            indexed_commits: 2_437,
            ..healthy_history()
        });
        assert!(
            matches!(check.status, CheckStatus::Warning),
            "{}",
            check.message
        );
        assert!(check.message.contains("41 commit(s)"), "{}", check.message);
        assert!(check.message.contains("2d old"), "{}", check.message);
        // The remedy is named, not implied.
        assert!(
            check.message.contains("cas history backfill"),
            "{}",
            check.message
        );
    }

    /// The other half of that threshold: lag younger than one tick is the
    /// daemon's normal window, not a fault. Without this, doctor would cry
    /// wolf on every healthy repository between ticks.
    #[test]
    fn history_index_check_is_ok_while_lag_is_younger_than_one_tick() {
        let check = history_index_check(HistoryIndexHealth {
            lag_commits: Some(3),
            lag_seconds: Some(90),
            ..healthy_history()
        });
        assert!(matches!(check.status, CheckStatus::Ok), "{}", check.message);
    }

    /// Production gather regression: close commit timestamps, but an index
    /// observation two days old. Unknown ledger timestamps must remain loud.
    #[test]
    fn doctor_ages_nonzero_history_lag_from_the_last_successful_observation() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let repo = temp.path().join("history-lag-repo");
        fs::create_dir_all(&repo).expect("repo dir");
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .expect("run git");
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "fixture@example.com"]);
        git(&["config", "user.name", "Fixture"]);
        git(&["config", "commit.gpgsign", "false"]);
        fs::write(repo.join("first.rs"), "fn first() {}\n").expect("first file");
        git(&["add", "first.rs"]);
        git(&["commit", "-m", "first"]);

        let cas_root = crate::store::init_cas_dir(&repo).expect("cas root");
        crate::history::run_index_pass(&cas_root, &repo).expect("initial history index");
        let now = chrono::Utc::now();
        let stale = (now - chrono::Duration::days(2)).to_rfc3339();
        rusqlite::Connection::open(cas_root.join("cas.db"))
            .expect("history db")
            .execute(
                "UPDATE history_index_state
                    SET last_indexed_at = ?1, last_attempt_at = ?1
                  WHERE source = 'git'",
                [&stale],
            )
            .expect("seed stale successful observation");
        fs::write(repo.join("second.rs"), "fn second() {}\n").expect("second file");
        git(&["add", "second.rs"]);
        git(&["commit", "-m", "second"]);

        let state = gather_history_index_state_at(&cas_root, now);
        assert_eq!(state.lag_commits, Some(1));
        assert!(state.lag_seconds.unwrap() >= 2 * 24 * 60 * 60 - 5);
        assert!(matches!(history_index_check(state).status, CheckStatus::Warning));

        rusqlite::Connection::open(cas_root.join("cas.db"))
            .expect("history db")
            .execute(
                "UPDATE history_index_state
                    SET last_indexed_at = NULL, last_attempt_at = NULL
                  WHERE source = 'git'",
                [],
            )
            .expect("remove unknown observation");
        let unknown = gather_history_index_state_at(&cas_root, now);
        assert_eq!(unknown.lag_commits, Some(1));
        assert_eq!(unknown.lag_seconds, None);
        let check = history_index_check(unknown);
        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(check.message.contains("unknown rather than fresh"), "{}", check.message);
    }

    /// §10.2 row 3, surfaced. `lag_commits: None` means the watermark left
    /// HEAD's ancestry — the one thing that must never render as "0 behind".
    #[test]
    fn history_index_check_never_renders_a_diverged_watermark_as_fresh() {
        let check = history_index_check(HistoryIndexHealth {
            lag_commits: None,
            lag_seconds: None,
            watermark_is_ancestor: false,
            ..healthy_history()
        });
        assert!(
            matches!(check.status, CheckStatus::Warning),
            "{}",
            check.message
        );
        assert!(
            check.message.contains("not an ancestor"),
            "{}",
            check.message
        );
        assert!(
            check.message.contains("unknown rather than 0"),
            "lag must be declared unknown, not implied fresh: {}",
            check.message
        );
    }

    /// §10.2 row 2: a source-level failure outranks staleness, because it is
    /// usually the cause. Top-3 offenders named, per §10.1.
    #[test]
    fn history_index_check_names_the_offending_sources() {
        let check = history_index_check(HistoryIndexHealth {
            failing_sources: vec![
                ("github".to_string(), "gh: not authenticated".to_string()),
                ("changelog".to_string(), "no CHANGELOG.md".to_string()),
            ],
            ..healthy_history()
        });
        assert!(
            matches!(check.status, CheckStatus::Warning),
            "{}",
            check.message
        );
        assert!(check.message.contains("github"), "{}", check.message);
        assert!(
            check.message.contains("not authenticated"),
            "the declared boundary must be quoted, not summarised: {}",
            check.message
        );
        assert!(check.message.contains("changelog"), "{}", check.message);
    }

    /// An unreadable health signal reads as health. This arm is why the check
    /// never silently skips.
    #[test]
    fn history_index_check_reports_read_errors_rather_than_skipping() {
        let check = history_index_check(HistoryIndexHealth {
            error: Some("no such table: history_commits".to_string()),
            ..healthy_history()
        });
        assert!(
            matches!(check.status, CheckStatus::Warning),
            "{}",
            check.message
        );
        assert!(
            check
                .message
                .starts_with("cannot check code history index:"),
            "{}",
            check.message
        );
    }

    #[test]
    fn history_index_check_warns_when_never_indexed() {
        let check = history_index_check(HistoryIndexHealth {
            ever_indexed: false,
            backfill_complete: false,
            indexed_commits: 0,
            lag_commits: None,
            ..healthy_history()
        });
        assert!(
            matches!(check.status, CheckStatus::Warning),
            "{}",
            check.message
        );
        assert!(check.message.contains("never indexed"), "{}", check.message);
    }

    #[test]
    fn history_index_check_warns_while_the_backfill_is_incomplete() {
        let check = history_index_check(HistoryIndexHealth {
            backfill_complete: false,
            indexed_commits: 1_200,
            ..healthy_history()
        });
        assert!(
            matches!(check.status, CheckStatus::Warning),
            "{}",
            check.message
        );
        assert!(
            check.message.contains("backfill incomplete"),
            "{}",
            check.message
        );
        assert!(check.message.contains("1200 of 2478"), "{}", check.message);
    }

    /// §10.1's actual demand: BOTH coverage figures, on every arm. Reporting
    /// only the any-edge number would make a substring-grade corpus look
    /// solved, which is the specific dishonesty this milestone exists to stop.
    #[test]
    fn history_index_check_publishes_both_coverage_figures() {
        let ok = history_index_check(healthy_history());
        assert!(
            ok.message.contains("8.9% high-confidence"),
            "{}",
            ok.message
        );
        assert!(ok.message.contains("23.1% any-edge"), "{}", ok.message);

        // ...and on a warning arm too, where it would be easiest to drop.
        let stale = history_index_check(HistoryIndexHealth {
            lag_commits: Some(41),
            lag_seconds: Some(2 * 24 * 60 * 60),
            ..healthy_history()
        });
        assert!(
            stale.message.contains("8.9% high-confidence"),
            "coverage must survive the stale arm: {}",
            stale.message
        );
    }

    /// Unmeasurable is not 0%. A store that cannot read the edges must say so
    /// rather than publish a confident number it did not measure.
    #[test]
    fn history_index_check_says_unmeasurable_rather_than_zero_percent() {
        let check = history_index_check(HistoryIndexHealth {
            provenance_coverage_pct: None,
            provenance_any_coverage_pct: None,
            provenance_unmeasurable_reason: Some("no tasks table".to_string()),
            ..healthy_history()
        });
        assert!(check.message.contains("unmeasurable"), "{}", check.message);
        assert!(
            check.message.contains("no tasks table"),
            "{}",
            check.message
        );
        assert!(
            !check.message.contains("0.0%"),
            "unmeasurable must never render as a number: {}",
            check.message
        );
    }

    /// A partial measurement keeps its figures but must be labelled — the
    /// distinction M5 introduced and this surface has to preserve.
    #[test]
    fn history_index_check_labels_a_partial_measurement() {
        let check = history_index_check(HistoryIndexHealth {
            provenance_unmeasurable_reason: Some("commit_links unreadable".to_string()),
            ..healthy_history()
        });
        assert!(
            check.message.contains("8.9% high-confidence"),
            "{}",
            check.message
        );
        assert!(check.message.contains("partial:"), "{}", check.message);
    }

    /// The table guard must actually warn, not merely contain the right names
    /// in a source literal. Exercise the two history migrations most likely to
    /// be absent on an older install, one at a time.
    #[test]
    fn missing_history_tables_produce_a_warning_that_names_each_table() {
        use crate::migration::detector::TableInfo;

        for missing in [
            "history_commit_symbols",
            "history_epochs",
            "code_vector_queue",
            "code_index_state",
        ] {
            let summary = SchemaSummary {
                tables: EXPECTED_TABLES
                    .iter()
                    .filter(|table| **table != missing)
                    .map(|table| TableInfo {
                        name: (*table).to_string(),
                        columns: vec![],
                        row_count: 0,
                    })
                    .collect(),
            };

            let check = schema_tables_check(&summary);
            assert!(
                matches!(check.status, CheckStatus::Warning),
                "omitting {missing} did not warn: {}",
                check.message
            );
            assert!(
                check.message.contains(missing),
                "warning did not name {missing}: {}",
                check.message
            );
        }
    }
}

/// Check Claude Code MCP configuration
fn check_claude_code_mcp(project_root: &Path) -> Check {
    let mcp_json_path = project_root.join(".mcp.json");

    // Check if .mcp.json exists
    if !mcp_json_path.exists() {
        return Check {
            name: "mcp config".to_string(),
            status: CheckStatus::Warning,
            message: "MCP not configured. Run 'cas init' or add to .mcp.json".to_string(),
        };
    }

    // Read and parse .mcp.json
    let content = match std::fs::read_to_string(&mcp_json_path) {
        Ok(c) => c,
        Err(e) => {
            return Check {
                name: "mcp config".to_string(),
                status: CheckStatus::Warning,
                message: format!("Cannot read .mcp.json: {e}"),
            };
        }
    };

    let config: serde_json::Value = match serde_json::from_str(&content) {
        Ok(c) => c,
        Err(e) => {
            return Check {
                name: "mcp config".to_string(),
                status: CheckStatus::Warning,
                message: format!("Invalid .mcp.json: {e}"),
            };
        }
    };

    // Check for mcpServers.cas entry
    let has_cas = config
        .pointer("/mcpServers/cas")
        .map(|v| v.is_object())
        .unwrap_or(false);

    if !has_cas {
        return Check {
            name: "mcp config".to_string(),
            status: CheckStatus::Warning,
            message: "CAS MCP server not configured. Run 'cas init' to configure".to_string(),
        };
    }

    // Check if the cas config has the correct command
    let correct_command = config
        .pointer("/mcpServers/cas/command")
        .and_then(|v| v.as_str())
        .map(|cmd| cmd == "cas")
        .unwrap_or(false);

    let correct_args = config
        .pointer("/mcpServers/cas/args")
        .and_then(|v| v.as_array())
        .map(|args| args.iter().filter_map(|a| a.as_str()).any(|a| a == "serve"))
        .unwrap_or(false);

    if correct_command && correct_args {
        Check {
            name: "mcp config".to_string(),
            status: CheckStatus::Ok,
            message: "MCP configured in .mcp.json".to_string(),
        }
    } else {
        Check {
            name: "mcp config".to_string(),
            status: CheckStatus::Warning,
            message: "CAS MCP config may be incorrect. Expected: {\"command\": \"cas\", \"args\": [\"serve\"]}".to_string(),
        }
    }
}

fn output_checks(checks: &[Check], cli: &Cli) -> anyhow::Result<()> {
    if cli.json {
        let results: Vec<_> = checks
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "status": match c.status {
                        CheckStatus::Ok => "ok",
                        CheckStatus::Warning => "warning",
                        CheckStatus::Error => "error",
                    },
                    "message": c.message
                })
            })
            .collect();
        println!("{}", serde_json::to_string(&results)?);
    } else {
        let theme = ActiveTheme::default();
        let mut out = std::io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);

        fmt.subheading("cas doctor")?;
        fmt.write_muted(&"─".repeat(50))?;
        fmt.newline()?;

        for check in checks {
            match check.status {
                CheckStatus::Ok => {
                    fmt.success(&format!("{}: {}", check.name, check.message))?;
                }
                CheckStatus::Warning => {
                    fmt.warning(&format!("{}: {}", check.name, check.message))?;
                }
                CheckStatus::Error => {
                    fmt.error(&format!("{}: {}", check.name, check.message))?;
                }
            }
        }

        let has_errors = checks
            .iter()
            .any(|c| matches!(c.status, CheckStatus::Error));
        let has_warnings = checks
            .iter()
            .any(|c| matches!(c.status, CheckStatus::Warning));

        fmt.newline()?;
        if has_errors {
            fmt.error("Some checks failed. Please address the errors above.")?;
        } else if has_warnings {
            fmt.warning("All critical checks passed with some warnings.")?;
        } else {
            fmt.success("All checks passed!")?;
        }
    }

    Ok(())
}
