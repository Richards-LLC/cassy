//! Doctor command - diagnostics and repair

use clap::Args;
use std::collections::BTreeMap;
use std::path::Path;

use crate::config::Config;
use crate::migration::{check_migrations, detector::get_schema_summary, run_migrations};
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

    // Check 3b: Schema details (tables and columns)
    if let Ok(summary) = get_schema_summary(&cas_root) {
        let table_count = summary.tables.len();
        let total_columns: usize = summary.tables.iter().map(|t| t.columns.len()).sum();
        let total_rows: i64 = summary.tables.iter().map(|t| t.row_count).sum();

        // Check for expected core tables
        let expected_tables = [
            "entries",
            "tasks",
            "rules",
            "skills",
            "agents",
            "task_leases",
        ];
        let missing_tables: Vec<&str> = expected_tables
            .iter()
            .filter(|t| !summary.tables.iter().any(|st| st.name == **t))
            .copied()
            .collect();

        if missing_tables.is_empty() {
            checks.push(Check {
                name: "tables".to_string(),
                status: CheckStatus::Ok,
                message: format!(
                    "{table_count} tables, {total_columns} columns, {total_rows} rows total"
                ),
            });
        } else {
            checks.push(Check {
                name: "tables".to_string(),
                status: CheckStatus::Warning,
                message: format!(
                    "{} tables ({} missing: {})",
                    table_count,
                    missing_tables.len(),
                    missing_tables.join(", ")
                ),
            });
        }
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

    output_checks(&checks, cli)
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
