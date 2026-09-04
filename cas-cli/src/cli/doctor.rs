//! Doctor command - diagnostics and repair

use clap::Args;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::hybrid_search::SearchIndex;
use crate::migration::{
    check_migrations,
    detector::{SchemaSummary, get_schema_summary},
    run_migrations,
};
use crate::store::{
    StoreType, detect_store_type, open_agent_store, open_rule_store, open_store, open_task_store,
};
use crate::types::RuleStatus;
use crate::ui::components::Formatter;
use crate::ui::theme::{ActiveTheme, Icons};

use crate::cli::Cli;

#[derive(Args, Debug, Clone)]
pub struct DoctorArgs {
    /// Attempt safe automatic fixes (initialize Cassy and apply pending schema migrations)
    #[arg(long)]
    pub fix: bool,

    /// Report cross-project ("foreign") task rows in this project's database
    /// in full detail, instead of running the other diagnostics (cas-fc6fa /
    /// GH #133). Read-only: every database is opened read-only and nothing is
    /// deleted. Rows are matched on `(id, title)` — never on id alone, because
    /// 4-hex task ids collide across projects.
    #[arg(long)]
    pub foreign_rows: bool,

    /// Quarantine this project's unattributed task rows (cas-4342 / GH #701):
    /// rows replicated here by the `cas-ed15` pull leak whose home project
    /// cannot be established from any database on this host. Quarantine is
    /// local and reversible — the row is hidden from the board and never
    /// pushed, but is left byte-for-byte intact and stays readable by id.
    ///
    /// Reports the plan and changes nothing unless `--yes` is also passed.
    #[arg(long)]
    pub fix_cloud_rows: bool,

    /// Release every locally quarantined row (the reverse of
    /// `--fix-cloud-rows`). Reports the plan; applies only with `--yes`.
    #[arg(long)]
    pub release_cloud_rows: bool,

    /// Apply the reported `--fix-cloud-rows` / `--release-cloud-rows` plan
    /// instead of only printing it.
    #[arg(long)]
    pub yes: bool,
}

struct Check {
    name: String,
    status: CheckStatus,
    message: String,
}

/// Wall time attributed to one check (cas-ba01 / GH #700).
///
/// `doctor` is a sequence of blocks that push checks onto one vector, so the
/// unit that can honestly be measured is the block, not the individual check.
/// When a block emits several checks they all carry the block's duration and
/// `shared` is set, because claiming the same 60 seconds three times over
/// without saying so would be a worse lie than the missing timing this fixes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CheckTiming {
    phase: String,
    duration: Duration,
    /// How many checks this phase produced. `1` means the duration is this
    /// check's alone.
    checks_in_phase: usize,
}

impl CheckTiming {
    fn shared(&self) -> bool {
        self.checks_in_phase > 1
    }

    /// `(1.2s)`, or `(1.2s for 3 checks)` when the phase is shared.
    fn label(&self) -> String {
        if self.shared() {
            format!(
                "({} for {} checks)",
                duration_label(self.duration),
                self.checks_in_phase
            )
        } else {
            format!("({})", duration_label(self.duration))
        }
    }
}

/// One measured block of `execute`, including blocks that produced no check.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Phase {
    label: String,
    duration: Duration,
    checks: usize,
}

/// Stopwatch over `execute`'s sequential blocks.
///
/// Deliberately not a closure-wrapping API: `execute` is one long imperative
/// function with early returns, and threading every block through a closure
/// would restructure code this task has no business restructuring. `mark` is
/// called after a block and attributes the time since the previous mark to
/// whatever checks appeared in that window.
struct PhaseRecorder {
    last: Instant,
    phases: Vec<Phase>,
    /// One entry per check, in check order.
    per_check: Vec<CheckTiming>,
}

impl PhaseRecorder {
    fn new() -> Self {
        Self::new_at(Instant::now())
    }

    /// Start the clock at an explicit instant so timing behaviour can be
    /// asserted exactly instead of with a tolerance window.
    fn new_at(start: Instant) -> Self {
        Self {
            last: start,
            phases: Vec::new(),
            per_check: Vec::new(),
        }
    }

    /// Close the current block, attributing its elapsed time to the checks
    /// pushed since the last `mark`.
    fn mark(&mut self, label: &str, checks: &[Check]) {
        self.mark_at(label, checks, Instant::now())
    }

    fn mark_at(&mut self, label: &str, checks: &[Check], now: Instant) {
        let duration = now.saturating_duration_since(self.last);
        self.last = now;
        let new_checks = checks.len().saturating_sub(self.per_check.len());
        self.phases.push(Phase {
            label: label.to_string(),
            duration,
            checks: new_checks,
        });
        for _ in 0..new_checks {
            self.per_check.push(CheckTiming {
                phase: label.to_string(),
                duration,
                checks_in_phase: new_checks,
            });
        }
    }

    fn per_check(&self) -> &[CheckTiming] {
        &self.per_check
    }

    /// Phases that produced no check still spent real time; a slowest-phase
    /// table that dropped them could not account for the total.
    fn phases(&self) -> &[Phase] {
        &self.phases
    }

    /// Phases worth naming, slowest first.
    fn slowest(&self, threshold: Duration, limit: usize) -> Vec<&Phase> {
        let mut phases: Vec<&Phase> = self
            .phases
            .iter()
            .filter(|phase| phase.duration >= threshold)
            .collect();
        phases.sort_by(|a, b| b.duration.cmp(&a.duration));
        phases.truncate(limit);
        phases
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckGroup {
    Store,
    Indexes,
    Cloud,
    Config,
    Integrations,
}

impl CheckGroup {
    fn label(self) -> &'static str {
        match self {
            Self::Store => "Store",
            Self::Indexes => "Indexes",
            Self::Cloud => "Cloud",
            Self::Config => "Config",
            Self::Integrations => "Integrations",
        }
    }

    fn json_name(self) -> &'static str {
        match self {
            Self::Store => "store",
            Self::Indexes => "indexes",
            Self::Cloud => "cloud",
            Self::Config => "config",
            Self::Integrations => "integrations",
        }
    }

    fn for_name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "legacy search index"
            | "pre-versioned search index"
            | "search index"
            | "symbol index"
            | "embedding drain"
            | "embeddings"
            | "code history index" => Self::Indexes,
            "canonical id"
            | "canonical id collision"
            | "project aliases"
            | "cloud sync queue"
            | "cross-project rows"
            | "foreign knowledge pages"
            | "supervisor relay"
            | "delivery retries" => Self::Cloud,
            "configuration" | "mcp config" | "mcp stdio upstreams" | "sync target" | "models" => {
                Self::Config
            }
            "integrations" | "mecha-cassy" => Self::Integrations,
            "user skills" => Self::Config,
            name if name.starts_with("integration") => Self::Integrations,
            _ => Self::Store,
        }
    }
}

impl Check {
    fn new(name: impl Into<String>, status: CheckStatus, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status,
            message: message.into(),
        }
    }

    fn group(&self) -> CheckGroup {
        CheckGroup::for_name(&self.name)
    }

    /// Split operator guidance from the short diagnostic while retaining the
    /// original message for verbose output. Existing checks historically
    /// embedded their actionable command in `message`; keeping that source
    /// shape here lets every producer migrate without changing its verdict.
    fn parts(&self) -> (String, Option<String>) {
        const REMEDIATION_MARKERS: &[&str] = &[
            "; Run `",
            ". Run `",
            ". Review ",
            ". Do not treat ",
            "; repair ",
            "; run `",
            ". run `",
            "— run `",
        ];
        let Some((index, _marker)) = REMEDIATION_MARKERS
            .iter()
            .filter_map(|marker| self.message.find(marker).map(|index| (index, *marker)))
            .min_by_key(|(index, _)| *index)
        else {
            return (self.message.clone(), None);
        };
        let message = self.message[..index].trim_end().to_string();
        let remediation = self.message[index..]
            .trim_start_matches([';', '.', ' ', '—'])
            .to_string();
        (message, (!remediation.is_empty()).then_some(remediation))
    }
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

// ---------------------------------------------------------------------------
// Stray user-level skills (cas-332f)
// ---------------------------------------------------------------------------

/// Skills that were once hand-installed into a user skills directory and are
/// now owned by a builtin. Each entry names the builtin that supersedes it.
///
/// This list exists because [`crate::builtins::prune_stale_cas_skill_dirs`]
/// only ever removes `cas-*` directories, so a hand-installed skill without
/// that prefix is never written by `cas update` **and** never pruned by it —
/// it simply persists forever, unreachable by any test in this repo. That is
/// exactly how `mecha-cassy-post` kept documenting a retired hub tool contract
/// after every in-repo copy had been corrected.
const RETIRED_USER_SKILLS: &[(&str, &str)] = &[("mecha-cassy-post", "mecha-cassy")];

/// Why a user-level skill directory should not be on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StrayReason {
    /// Retired in favour of the named builtin, which now owns the contract.
    RetiredBy(&'static str),
    /// Carries the `managed_by: cas` marker but is not in the builtin catalog,
    /// so `cas update` will never refresh it again.
    OrphanedManagedCopy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StrayUserSkill {
    name: String,
    path: PathBuf,
    reason: StrayReason,
}

/// Decide a single skill directory's fate from its name and `SKILL.md` body.
///
/// A user skill that merely shares a name with a builtin is **not** flagged:
/// that is the normal case for a builtin projected into the user directory,
/// and calling it a shadow would make this check cry wolf on every machine.
fn classify_user_skill(
    name: &str,
    content: &str,
    builtin_skill_names: &std::collections::HashSet<String>,
) -> Option<StrayReason> {
    if let Some((_, superseded_by)) = RETIRED_USER_SKILLS.iter().find(|(n, _)| *n == name) {
        return Some(StrayReason::RetiredBy(superseded_by));
    }
    if crate::builtins::is_managed_by_cas(content) && !builtin_skill_names.contains(name) {
        return Some(StrayReason::OrphanedManagedCopy);
    }
    None
}

/// Skill names one catalog ships, taken from the embedded catalog rather than
/// from disk, so the comparison is against what this binary would actually
/// write.
///
/// Per-catalog and not merged: each harness gets its own set. Codex ships
/// skills Claude does not (`cas-codex-supervisor-checklist`), so comparing a
/// `~/.codex/skills` directory against the Claude catalog reports a perfectly
/// current builtin as an orphan.
fn catalog_skill_names(catalog: &[crate::builtins::BuiltinFile]) -> std::collections::HashSet<String> {
    catalog
        .iter()
        .filter_map(|file| file.path.strip_prefix("skills/"))
        .filter_map(|rest| rest.split('/').next())
        .map(str::to_string)
        .collect()
}

/// Scan the given user skills directories. Results are deduplicated by
/// **canonical** path: several Claude account directories symlink a single
/// shared `skills/` directory, so a naive walk reports the same file three
/// times and an operator "fixes" one file over and over.
fn scan_user_skill_dirs(targets: &[(PathBuf, std::collections::HashSet<String>)]) -> Vec<StrayUserSkill> {
    let mut seen = std::collections::HashSet::new();
    let mut strays = Vec::new();
    for (dir, builtin_skill_names) in targets {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let skill_file = path.join("SKILL.md");
            let canonical = skill_file
                .canonicalize()
                .unwrap_or_else(|_| skill_file.clone());
            if !seen.insert(canonical) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let content = fs::read_to_string(&skill_file).unwrap_or_default();
            if let Some(reason) = classify_user_skill(name, &content, builtin_skill_names) {
                strays.push(StrayUserSkill {
                    name: name.to_string(),
                    path: skill_file,
                    reason,
                });
            }
        }
    }
    strays.sort_by(|a, b| a.path.cmp(&b.path));
    strays
}

/// Every user-level skills directory this machine might carry.
///
/// `cas update --user` writes into `~/.claude`, `~/.codex` and `~/.grok`, and
/// a Claude install may additionally use per-account directories selected by
/// `CLAUDE_CONFIG_DIR`. Several of those commonly symlink one shared
/// `skills/`, which [`scan_user_skill_dirs`] deduplicates.
fn user_skill_scan_targets() -> Vec<(PathBuf, std::collections::HashSet<String>)> {
    let claude = catalog_skill_names(crate::builtins::BUILTIN_SKILLS);
    let codex = catalog_skill_names(crate::builtins::CODEX_BUILTIN_SKILLS);
    let grok = catalog_skill_names(crate::builtins::GROK_BUILTIN_SKILLS);

    let mut targets = Vec::new();
    if let Some(configured) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        targets.push((PathBuf::from(configured).join("skills"), claude.clone()));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        targets.push((home.join(".claude").join("skills"), claude.clone()));
        targets.push((home.join(".codex").join("skills"), codex));
        targets.push((home.join(".grok").join("skills"), grok));
        // Per-account Claude profiles (`~/.claude-alt`, `~/.claude-<email>`).
        if let Ok(entries) = fs::read_dir(&home) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".claude-"))
                {
                    targets.push((path.join("skills"), claude.clone()));
                }
            }
        }
    }
    targets
}

/// Render the scan as a doctor row. Warning, never Error: a stale skill misleads
/// an agent but breaks nothing on its own, and the fix is a deletion the
/// operator must make deliberately.
fn stray_user_skills_check(strays: &[StrayUserSkill]) -> Check {
    if strays.is_empty() {
        return Check::new(
            "user skills",
            CheckStatus::Ok,
            "no stale or orphaned user-level skills",
        );
    }
    let detail = strays
        .iter()
        .map(|stray| match &stray.reason {
            StrayReason::RetiredBy(builtin) => {
                format!("{} (retired; {builtin} owns it now)", stray.path.display())
            }
            StrayReason::OrphanedManagedCopy => {
                format!("{} (managed_by: cas but no longer a builtin)", stray.path.display())
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    Check::new(
        "user skills",
        CheckStatus::Warning,
        format!(
            "{} stale user-level skill file(s) no `cas update` will ever refresh: {detail}. \
             Review then delete the directory",
            strays.len()
        ),
    )
}

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
        Check::new(
            "tables",
            CheckStatus::Ok,
            format!("{table_count} tables, {total_columns} columns, {total_rows} rows total"),
        )
    } else {
        Check::new(
            "tables",
            CheckStatus::Warning,
            format!(
                "{} tables ({} missing: {})",
                table_count,
                missing_tables.len(),
                missing_tables.join(", ")
            ),
        )
    }
}

fn memory_decay_check(cas_root: &Path) -> Check {
    let message = crate::daemon::MemoryDecayStatus::read(cas_root)
        .map(|status| {
            format!(
                "Memory decay (last cycle): protected={} promoted_on_access={} recorded_at={}",
                status.curated_entries_protected,
                status.promoted_on_access,
                status.recorded_at.to_rfc3339()
            )
        })
        .unwrap_or_else(|_| {
            "Memory decay (last cycle): unavailable (no completed decay cycle recorded)".to_string()
        });
    Check::new("memory decay", CheckStatus::Ok, message)
}

/// Report the routable supervisor population for every factory session that
/// still has at least one live registered agent. Stale historical supervisor
/// rows do not make an active session ambiguous.
fn factory_supervisor_checks(agents: &[crate::types::Agent]) -> Vec<Check> {
    use crate::types::AgentRole;

    let mut sessions: BTreeMap<&str, Vec<&crate::types::Agent>> = BTreeMap::new();
    for agent in agents.iter().filter(|agent| agent.is_alive()) {
        if let Some(session) = agent.factory_session.as_deref() {
            sessions.entry(session).or_default().push(agent);
        }
    }

    sessions
        .into_iter()
        .map(|(session, members)| {
            let supervisors: Vec<_> = members
                .into_iter()
                .filter(|agent| agent.role == AgentRole::Supervisor)
                .collect();
            let count = supervisors.len();
            let status = if count == 1 {
                CheckStatus::Ok
            } else {
                CheckStatus::Warning
            };
            let detail = match count {
                0 => "expected exactly 1; `target=supervisor` cannot resolve in this session"
                    .to_string(),
                1 => format!("{} ({})", supervisors[0].name, supervisors[0].id),
                _ => format!(
                    "expected exactly 1; found {}",
                    supervisors
                        .iter()
                        .map(|agent| format!("{} ({})", agent.name, agent.id))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            Check::new(
                format!("factory session {session}"),
                status,
                format!("supervisors: {count}; {detail}"),
            )
        })
        .collect()
}

pub fn execute(args: &DoctorArgs, cli: &Cli, cas_root: Option<&Path>) -> anyhow::Result<()> {
    let started = Instant::now();
    let mut checks = Vec::new();
    // GH #700: doctor took 76s across 30 checks with no way to tell which one
    // spent it. Every block below closes with a `mark`, so `--verbose` and
    // `--json` can name the cost instead of leaving the operator to guess.
    let mut recorder = PhaseRecorder::new();
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
                            message: format!("Initialized Cassy at {}", path.display()),
                        });
                    } else {
                        checks.push(Check {
                            name: "auto-fix".to_string(),
                            status: CheckStatus::Warning,
                            message: "Initialization ran but Cassy root could not be resolved."
                                .to_string(),
                        });
                    }
                }
                Err(e) => {
                    checks.push(Check {
                        name: "auto-fix".to_string(),
                        status: CheckStatus::Error,
                        message: format!("Failed to initialize Cassy: {e}"),
                    });
                    return output_checks(
                        &checks,
                        cli,
                        started.elapsed(),
                        resolved_cas_root.as_deref(),
                    );
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

            if let Some(check) = legacy_index_autofix(path) {
                checks.push(check);
            }

            if let Some(check) = dangling_dependency_autofix(path) {
                checks.push(check);
            }
        }
    }

    recorder.mark("startup", &checks);
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

            return output_checks(
                &checks,
                cli,
                started.elapsed(),
                resolved_cas_root.as_deref(),
            );
        }
    };

    recorder.mark("cas directory", &checks);
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

    recorder.mark("store and database", &checks);
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

    recorder.mark("schema migrations", &checks);
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

    // Every active factory session needs exactly one live durable supervisor
    // row. Otherwise logical handoffs and verification recovery have nowhere
    // to route even while the supervisor pane itself exists.
    match open_agent_store(&cas_root).and_then(|store| Ok(store.list(None)?)) {
        Ok(agents) => {
            checks.extend(factory_supervisor_checks(&agents));
            // The per-session checks above each pass in isolation while two
            // supervisors quietly share one clone's `.cas/` state, which is
            // exactly the reap-the-other's-workers hazard (GH #699). Cross
            // the sessions and say so.
            if let Some(warning) = crate::factory_supervisor_overlap::shared_clone_warning(
                &agents,
                &cas_root,
                chrono::Utc::now(),
            ) {
                checks.push(Check {
                    name: "factory session overlap".to_string(),
                    status: CheckStatus::Warning,
                    message: warning,
                });
            }
        }
        Err(error) => checks.push(Check {
            name: "factory supervisors".to_string(),
            status: CheckStatus::Warning,
            message: format!("cannot inspect registered factory supervisors: {error}"),
        }),
    }

    recorder.mark("supervisor relays", &checks);
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

    recorder.mark("message handoff", &checks);
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

    recorder.mark("schema tables", &checks);
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

    recorder.mark("store open", &checks);
    // Check 4: Search index
    checks.push(legacy_search_index_check(&cas_root));
    if let Some(check) = legacy_versioned_search_index_check(&cas_root) {
        checks.push(check);
    }
    let index_dir = crate::hybrid_search::tantivy_index_dir(&cas_root);
    if index_dir.exists() {
        match SearchIndex::open(&index_dir) {
            Ok(_) => {
                checks.push(Check {
                    name: "search index".to_string(),
                    status: CheckStatus::Ok,
                    message: format!("Tantivy index accessible at {}", index_dir.display()),
                });
            }
            Err(e) => {
                checks.push(Check {
                    name: "search index".to_string(),
                    status: CheckStatus::Warning,
                    message: format!("Index may need rebuild: {e}; Run a search to rebuild it"),
                });
            }
        }
    } else {
        checks.push(Check {
            name: "search index".to_string(),
            status: CheckStatus::Warning,
            message: format!(
                "Index not found at {}. Will be created on first search; Run a search to build it",
                index_dir.display()
            ),
        });
    }

    recorder.mark("search index", &checks);
    // Check 4b: symbol index lag (cas-499c).
    //
    // The daemon only indexes code while it is idle (operator ruling: that gate stays), so on a
    // busy machine catch-up can trail by days. Without a line here that lag is invisible and
    // `code_search` returning thin results looks like a bug rather than a queue.
    checks.push(symbol_index_check(
        gather_symbol_index_state(&cas_root),
        chrono::Utc::now(),
    ));

    recorder.mark("symbol index", &checks);
    // Check 4c: the embedding drain (EPIC cas-6212 / cas-db6e, M7).
    //
    // The drain runs on a daemon tick, so its failures have no command output to
    // appear in. Without this line a drain that has been 400ing for a week looks
    // exactly like one with nothing to do — which is the cas-a924 failure shape,
    // rebuilt for a different corpus.
    checks.push(embedding_drain_check(gather_embedding_drain_state(
        &cas_root,
    )));

    recorder.mark("embedding drain", &checks);
    // Check 4d: the structural git-history index (EPIC cas-6212 / cas-35b8,
    // spec §10.1 — "never silently stale").
    //
    // The index answers queries whether or not it is current, so staleness has
    // no natural symptom: a thin result set from a week-old watermark looks
    // exactly like a repository where nothing happened. This line is where that
    // difference becomes visible, in commits AND seconds, alongside the
    // measured provenance coverage the answers are only as good as.
    checks.push(history_index_check(gather_history_index_state(&cas_root)));

    recorder.mark("history index", &checks);
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

    #[cfg(feature = "mcp-proxy")]
    checks.push(proxy_stdio_commands_check(&cas_root));

    recorder.mark("config and proxy", &checks);
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

    recorder.mark("sync target", &checks);
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

            checks.push(memory_decay_check(&cas_root));
        }
    }

    recorder.mark("memory statistics", &checks);
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

    recorder.mark("rule status", &checks);
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

            // Dependency endpoints: a quarantined (hidden but present) endpoint
            // is not an orphan, and only a genuinely absent id warrants a WARN
            // (cas-095c).
            let deps = task_store.list_dependencies(None).unwrap_or_default();
            let health = dependency_endpoint_health(task_store.as_ref(), &tasks, &deps);
            checks.push(task_health_check(
                tasks.len(),
                &status_summary,
                open_count,
                blocked_count,
                &health,
            ));
        }
    }

    recorder.mark("task health", &checks);
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

    recorder.mark("vector store", &checks);
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

    recorder.mark("models directory", &checks);
    // Check 12: Claude Code MCP configuration
    let project_root = cas_root.parent().unwrap_or(Path::new("."));
    let mcp_check = check_claude_code_mcp(project_root);
    checks.push(mcp_check);

    recorder.mark("mcp config", &checks);
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

    recorder.mark("integrations", &checks);
    // Check 13b: MechaCassy hub reachability (cas-8fad). Machine-scoped, so
    // it is not part of `integration_checks` (which walks per-project keep
    // blocks). Unlike the platform rows this one *can* be an Error: a missing
    // variable, a rejected bearer, or a drifted tool contract each mean the
    // next release post will fail, and each has an exact remedy.
    #[cfg(feature = "mcp-proxy")]
    {
        let project_proxy = cas_root.join("proxy.toml");
        if let Some(row) = crate::cli::integrate::mecha_cassy::doctor_row_from_env(
            project_proxy.is_file().then_some(project_proxy.as_path()),
        ) {
            checks.push(Check {
                name: "mecha-cassy".to_string(),
                status: match row.severity {
                    crate::cli::integrate::mecha_cassy::DoctorSeverity::Ok => CheckStatus::Ok,
                    crate::cli::integrate::mecha_cassy::DoctorSeverity::Warning => {
                        CheckStatus::Warning
                    }
                    crate::cli::integrate::mecha_cassy::DoctorSeverity::Error => CheckStatus::Error,
                },
                message: row.message,
            });
        }
    }

    recorder.mark("mechacassy hub", &checks);

    // Check 13c: stale user-level skills (cas-332f). `cas update` only prunes
    // `cas-*` directories, so a hand-installed skill without that prefix is
    // never refreshed and never removed — it just keeps giving an agent stale
    // instructions that no test in this repo can reach.
    checks.push(stray_user_skills_check(&scan_user_skill_dirs(
        &user_skill_scan_targets(),
    )));

    // Its own phase: this check walks user-level skill directories on disk,
    // which is a different cost from the hub probe before it and from the
    // canonical-id queries after it. Folding it into either would misattribute
    // whichever one later shows up as slow.
    recorder.mark("user skills", &checks);

    // Check 14: cloud canonical id — which bucket this project syncs into,
    // and whether any other known local project lands in the same bucket
    // (cas-f699 / GH #134).
    checks.extend(canonical_id_checks(
        &cas_root,
        collect_local_root_identities(),
    ));
    checks.extend(canonical_alias_checks(&cas_root));

    recorder.mark("canonical id", &checks);
    // Check 15: residual cross-project contamination from the cas-ed15 pull
    // leak (cas-fc6fa / GH #133). Read-only comparison of this project's task
    // rows against every other known project database on the host, keyed on
    // `(id, title)`.
    if cas_root.join("cas.db").is_file() {
        let (report, sync_warnings) =
            crate::cloud::collect_sync_warnings(|| crate::cli::foreign_rows::scan(&cas_root));
        let (purge_analysis, purge_analysis_error) = match (
            report.as_ref(),
            crate::cloud::resolve_canonical_id(&cas_root),
        ) {
            (Ok(report), Some(current_project)) => {
                match crate::cli::cloud::purge_analysis_for_report(
                    &cas_root,
                    &current_project,
                    report,
                ) {
                    Ok(analysis) => (Some(analysis), None),
                    Err(error) => (None, Some(error.to_string())),
                }
            }
            (Ok(_), None) => (
                None,
                Some("current project canonical id could not be resolved".to_string()),
            ),
            (Err(_), _) => (None, None),
        };
        if args.fix_cloud_rows || args.release_cloud_rows {
            return run_cloud_row_quarantine(&cas_root, report, args, cli);
        }
        if args.foreign_rows {
            return output_foreign_rows_detail(
                report,
                purge_analysis.as_ref(),
                purge_analysis_error.as_deref(),
                cli,
            );
        }
        checks.push(cloud_queue_check(&cas_root));
        checks.extend(sync_warning_checks(&sync_warnings));
        // Quarantine is local state, so an unreadable ledger reports zero
        // rather than failing the whole check: the check's job is to describe
        // contamination, not to depend on the remedy's bookkeeping.
        let quarantined_count = crate::cloud::SyncQueue::open(&cas_root)
            .and_then(|queue| queue.quarantined_count(crate::cloud::QUARANTINE_TASK))
            .unwrap_or(0);
        let foreign_check = match purge_analysis_error.as_deref() {
            Some(error) => foreign_rows_check_with_classifier_error(
                report.as_ref(),
                purge_analysis.as_ref(),
                Some(error),
                quarantined_count,
            ),
            None => foreign_rows_check(report.as_ref(), purge_analysis.as_ref(), quarantined_count),
        };
        checks.push(foreign_check);
    } else if args.foreign_rows {
        anyhow::bail!(
            "`cas doctor --foreign-rows` needs a SQLite database at {}; this project uses legacy \
             markdown storage. Migrate with `cas migrate` first.",
            cas_root.join("cas.db").display()
        );
    }

    recorder.mark("foreign rows and cloud queue", &checks);

    output_checks_timed(
        &checks,
        recorder.per_check(),
        recorder.phases(),
        cli,
        started.elapsed(),
        Some(&cas_root),
    )
}

// ---------------------------------------------------------------------------
// Task dependency health (cas-095c)
// ---------------------------------------------------------------------------

/// Dependency rows whose endpoints are not on the board, split by cause.
///
/// The board is the *quarantine-filtered* task list, so a dependency touching a
/// row that `cas doctor --fix-cloud-rows` quarantined looks orphaned to a check
/// that compares against `list` alone — which is precisely the bug: the operator
/// did what doctor told them and was warned for it forever after. The
/// quarantined row is present, intact and reversible; only an endpoint that is
/// absent from the `tasks` table entirely is a fault.
#[derive(Debug, Default, PartialEq, Eq)]
struct DependencyEndpointHealth {
    /// Rows with an endpoint that is hidden by quarantine but still present.
    quarantined_endpoint_rows: usize,
    /// `(from_id, to_id)` of rows with an endpoint no longer in `tasks`.
    dangling: Vec<(String, String)>,
}

/// Classify every dependency row against the board.
///
/// `still_present` resolves an id that is not on the board. In production it is
/// `TaskStore::get`, which deliberately does not filter quarantine — that is the
/// seam that answers "does this row exist?" without leaking a hidden row onto
/// any list surface.
fn classify_dependency_endpoints(
    tasks: &[crate::types::Task],
    deps: &[crate::types::Dependency],
    mut still_present: impl FnMut(&str) -> bool,
) -> DependencyEndpointHealth {
    let board: std::collections::HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    // One resolve per distinct off-board id, not per row: a quarantine sweep
    // leaves dozens of rows pointing at the same handful of ids.
    let mut resolved: BTreeMap<String, bool> = BTreeMap::new();
    let mut health = DependencyEndpointHealth::default();

    for dep in deps {
        let mut hidden = false;
        let mut missing = false;
        for endpoint in [&dep.from_id, &dep.to_id] {
            if board.contains(endpoint.as_str()) {
                continue;
            }
            let present = *resolved
                .entry(endpoint.clone())
                .or_insert_with(|| still_present(endpoint));
            if present {
                hidden = true;
            } else {
                missing = true;
            }
        }

        // A row with one missing endpoint is dangling whatever the other
        // endpoint is: the fault dominates.
        if missing {
            health
                .dangling
                .push((dep.from_id.clone(), dep.to_id.clone()));
        } else if hidden {
            health.quarantined_endpoint_rows += 1;
        }
    }

    health
}

/// [`classify_dependency_endpoints`] against a live store.
fn dependency_endpoint_health(
    store: &dyn crate::store::TaskStore,
    tasks: &[crate::types::Task],
    deps: &[crate::types::Dependency],
) -> DependencyEndpointHealth {
    classify_dependency_endpoints(tasks, deps, |id| store.get(id).is_ok())
}

/// The `tasks` row: counts, plus dependency endpoints reported for what they
/// are.
fn task_health_check(
    total_tasks: usize,
    status_summary: &str,
    open_count: usize,
    blocked_count: usize,
    health: &DependencyEndpointHealth,
) -> Check {
    let mut message = format!(
        "{total_tasks} tasks ({status_summary}) | {open_count} open, {blocked_count} blocked"
    );

    if health.quarantined_endpoint_rows > 0 {
        message.push_str(&format!(
            " | {} dependency row(s) reference quarantined tasks",
            health.quarantined_endpoint_rows
        ));
    }

    if health.dangling.is_empty() {
        return Check {
            name: "tasks".to_string(),
            status: CheckStatus::Ok,
            message,
        };
    }

    let sample = health
        .dangling
        .iter()
        .take(3)
        .map(|(from, to)| format!("{from} -> {to}"))
        .collect::<Vec<_>>()
        .join(", ");
    let remainder = health.dangling.len().saturating_sub(3);
    let remainder = if remainder > 0 {
        format!(", +{remainder} more")
    } else {
        String::new()
    };
    message.push_str(&format!(
        " | {} dependency row(s) point at a task id that is not in this database ({sample}{remainder}); Run `cas doctor --fix` to prune them",
        health.dangling.len()
    ));

    Check {
        name: "tasks".to_string(),
        status: CheckStatus::Warning,
        message,
    }
}

/// Delete dependency rows whose endpoints are absent from `tasks`.
///
/// Quarantined endpoints are left strictly alone: quarantine is reversible, and
/// pruning its edges would make releasing a row restore it without its graph.
fn prune_dangling_dependencies(store: &dyn crate::store::TaskStore) -> anyhow::Result<usize> {
    let tasks = store.list(None)?;
    let deps = store.list_dependencies(None)?;
    let health = dependency_endpoint_health(store, &tasks, &deps);

    for (from, to) in &health.dangling {
        store.remove_dependency(from, to)?;
    }
    Ok(health.dangling.len())
}

/// The `cas doctor --fix` dangling-dependency prune, as one renderable Check.
///
/// Returns `None` when there is nothing to prune — the clean case adds no row.
///
/// Uses the *local* store on purpose: a locally dangling edge may simply be one
/// whose task has not been pulled yet, so the prune must not push a delete that
/// would take a live edge out of the cloud. A later pull can restore it.
fn dangling_dependency_autofix(cas_root: &Path) -> Option<Check> {
    let store = crate::store::open_task_store_local(cas_root).ok()?;
    match prune_dangling_dependencies(store.as_ref()) {
        Ok(0) => None,
        Ok(pruned) => Some(Check {
            name: "auto-fix".to_string(),
            status: CheckStatus::Ok,
            message: format!(
                "Pruned {pruned} dependency row(s) pointing at task ids that are not in this database"
            ),
        }),
        Err(error) => Some(Check {
            name: "auto-fix".to_string(),
            status: CheckStatus::Warning,
            message: format!("Failed to prune dangling dependency rows: {error}"),
        }),
    }
}

/// The `cas doctor --fix` legacy-index repair step, as one renderable Check.
///
/// Extracted from `execute` so it can be driven by a test: cas-25a9 AC1
/// requires that a held lock make `--fix` return *bounded* with a Warning
/// rather than hang, and that is a behaviour, not a code shape.
///
/// Returns `None` when there is no stray root — the clean case adds no row.
fn legacy_index_autofix(path: &Path) -> Option<Check> {
    // Interactive: drain the whole legacy root rather than the daemon's
    // bounded slice, but still bound the WALL CLOCK so a blocked reader in
    // another process cannot hang the command (cas-25a9).
    let outcome = open_store(path).and_then(|store| {
        crate::hybrid_search::repair_legacy_index_bounded(
            path,
            store,
            crate::hybrid_search::LegacyRepairLimits::unbounded(),
            crate::hybrid_search::DOCTOR_REPAIR_BUDGET,
        )
    });

    match outcome {
        Ok(crate::hybrid_search::LegacyRepairOutcome::Repaired(repair)) => {
            let mut message = format!(
                "Repaired legacy Tantivy root: {} document(s), {} entry row(s) re-queued, {} pending entry row(s) indexed",
                repair.legacy_documents, repair.requeued_entries, repair.indexed_entries
            );
            let mut status = CheckStatus::Ok;
            if !repair.retired_non_entry_documents.is_empty() {
                status = CheckStatus::Warning;
                let dropped = repair
                    .retired_non_entry_documents
                    .iter()
                    .map(|(doc_type, count)| format!("{count} {doc_type}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                message.push_str(&format!(
                    "; retired non-entry document(s) with no re-queue path ({dropped}) — reindex those document types to restore them"
                ));
            }
            if !repair.unswept_files.is_empty() {
                status = CheckStatus::Warning;
                message.push_str(&format!(
                    "; {} file(s) could not be removed — re-run `cas doctor --fix` to finish the sweep",
                    repair.unswept_files.len()
                ));
            }
            if !repair.errors.is_empty() {
                status = CheckStatus::Warning;
                message.push_str(&format!(
                    "; {} entry row(s) failed to index and stay queued for retry",
                    repair.errors.len()
                ));
            }
            Some(Check {
                name: "auto-fix".to_string(),
                status,
                message,
            })
        }
        Ok(crate::hybrid_search::LegacyRepairOutcome::NoLegacyRoot) => None,
        Ok(crate::hybrid_search::LegacyRepairOutcome::Busy { reason }) => Some(Check {
            name: "auto-fix".to_string(),
            status: CheckStatus::Warning,
            message: format!(
                "Legacy Tantivy root is busy ({reason}); stop any running `cas serve`/daemon and re-run `cas doctor --fix`"
            ),
        }),
        Err(error) => Some(Check {
            name: "auto-fix".to_string(),
            status: CheckStatus::Warning,
            message: format!("Failed to repair legacy Tantivy root: {error}"),
        }),
    }
}

fn legacy_search_index_check(cas_root: &Path) -> Check {
    match crate::hybrid_search::inspect_legacy_index(cas_root) {
        Ok(Some(state)) => Check {
            name: "legacy search index".to_string(),
            status: CheckStatus::Warning,
            message: format!(
                "{} document(s), including {} memory entry id(s), are stranded in `.cas/index/` and invisible to search; run `cas doctor --fix`",
                state.documents, state.entry_documents
            ),
        },
        Ok(None) => Check {
            name: "legacy search index".to_string(),
            status: CheckStatus::Ok,
            message: "no stray Tantivy root below `.cas/index/`".to_string(),
        },
        Err(error) => Check {
            name: "legacy search index".to_string(),
            status: CheckStatus::Warning,
            message: format!("cannot inspect stray Tantivy root: {error}"),
        },
    }
}

/// Report the pre-versioned canonical path separately from the older stray
/// root handled by [`legacy_search_index_check`]. This check is read-only: the
/// next explicit reindex performs the one-time migration or quarantine.
fn legacy_versioned_search_index_check(cas_root: &Path) -> Option<Check> {
    let legacy_dir = crate::hybrid_search::legacy_tantivy_index_dir(cas_root);
    if !legacy_dir.join("meta.json").is_file() {
        return None;
    }

    Some(Check {
        name: "pre-versioned search index".to_string(),
        status: CheckStatus::Warning,
        message: format!(
            "legacy Tantivy index at {}; current versioned path is {}; run the reindex maintenance action (`mcp__cas__system action=reindex bm25=true`) from an agent session to migrate it safely",
            legacy_dir.display(),
            crate::hybrid_search::tantivy_index_dir(cas_root).display()
        ),
    })
}

/// Turn a contamination scan into a single `cas doctor` row.
///
/// A failed scan is reported as a **named skip**, never as silence: an absent
/// warning on this surface reads as "no contamination", which is the exact
/// wrong answer for the user consulting doctor because they suspect it.
fn foreign_rows_check(
    report: Result<&crate::cli::foreign_rows::ForeignRowReport, &anyhow::Error>,
    purge_analysis: Option<&crate::cli::cloud::PurgeForeignAnalysis>,
    quarantined_count: usize,
) -> Check {
    foreign_rows_check_with_classifier_error(report, purge_analysis, None, quarantined_count)
}

fn foreign_rows_check_with_classifier_error(
    report: Result<&crate::cli::foreign_rows::ForeignRowReport, &anyhow::Error>,
    purge_analysis: Option<&crate::cli::cloud::PurgeForeignAnalysis>,
    purge_analysis_error: Option<&str>,
    quarantined_count: usize,
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
    if let Some(analysis) = purge_analysis {
        let evidence_count = analysis.foreign_task_count;
        let purge_count = analysis.delete_set.tasks.len();
        message.push_str(&format!(
            ". foreign evidence: {evidence_count} task row(s); purge delete set: {purge_count} task row(s)"
        ));
        if evidence_count == purge_count {
            message.push_str(" — counts agree");
        } else if evidence_count > purge_count {
            // GH #697 (cas-a869): the printed number must describe the list
            // that follows it. This used to print `evidence_count -
            // purge_count` and then list `retained_foreign_tasks`, a different
            // set, so an operator read "cannot reach 4" above six ids and had
            // no way to tell which number was wrong. When the two genuinely
            // disagree, both are real and both are stated — neither stands in
            // for the other.
            let gap = evidence_count - purge_count;
            let named = analysis.retained_foreign_tasks.len();
            if named > 0 {
                message.push_str(&format!(" — purge cannot reach {named} evidence row(s)"));
                let retained = analysis
                    .retained_foreign_tasks
                    .iter()
                    .map(|row| format!("{} ({})", row.id, row.reason))
                    .collect::<Vec<_>>()
                    .join(", ");
                message.push_str(&format!(": {retained}"));
                if named != gap {
                    message.push_str(&format!(
                        " (delete-set shortfall is {gap} row(s); the {named} named above are the rows purge identified and retained)"
                    ));
                }
            } else {
                message.push_str(&format!(
                    " — purge cannot reach {gap} evidence row(s); none were individually identified"
                ));
            }
        } else {
            message.push_str(&format!(
                " — purge includes {} task row(s) attributed by origin_project but absent from peer evidence",
                purge_count - evidence_count
            ));
        }
        if analysis.unattributed_task_count > 0 || analysis.collision_count > 0 {
            // Deliberately no longer says "neither category is deletable" and
            // stops there: unattributed rows now have a reversible local
            // remedy, and the remediation clause below states it (cas-4342).
            message.push_str(&format!(
                ". purge excludes {} unattributed task row(s) and {} id collision(s) — deleting either would destroy real work",
                analysis.unattributed_task_count,
                analysis.collision_count
            ));
        }
    }
    if let Some(remediation) = cloud_row_remediation_summary(report, quarantined_count) {
        message.push_str(&remediation);
    }
    if let Some(error) = purge_analysis_error {
        message.push_str(&format!(
            ". purge delete-set classifier unavailable: {error}; foreign-count comparison is incomplete"
        ));
    }
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

    let purge_counts_disagree = purge_analysis
        .is_some_and(|analysis| analysis.foreign_task_count != analysis.delete_set.tasks.len());
    let status = if report.is_clean()
        && report.peers_unreadable.is_empty()
        && purge_analysis_error.is_none()
        && !purge_counts_disagree
    {
        CheckStatus::Ok
    } else {
        CheckStatus::Warning
    };
    if purge_counts_disagree {
        message.push_str(
            ". Do not treat purge-foreign as complete remediation: inspect the retained evidence rows and their stated reasons before taking action.",
        );
    } else if !report.is_clean() {
        message.push_str(&format!(". {}", report.remediation()));
    }

    Check {
        name: "cross-project rows".to_string(),
        status,
        message,
    }
}

/// `cas doctor --foreign-rows`: the full read-only contamination listing.
/// Rows this project would quarantine: unattributed and not already closed.
///
/// A closed unattributed row is not lying to anyone — it is not in a ready
/// queue and nobody is going to pick it up — so quarantining it would be churn
/// with no operator benefit. Collisions are deliberately absent: a collision
/// means two rows share an id while their titles *differ*, so the peer row is
/// no evidence about the local one, and hiding a row that may be this
/// project's real work is exactly the harm GH #133 documented. Those need an
/// id rekey, which is a separate, non-destructive piece of work.
pub(crate) fn quarantine_candidates(
    report: &crate::cli::foreign_rows::ForeignRowReport,
) -> Vec<&crate::cli::foreign_rows::UnattributedRow> {
    report
        .unattributed
        .iter()
        .filter(|row| !row.closed)
        .collect()
}

/// One sentence of remediation state for the `cross-project rows` check:
/// how many rows are unattributed, how many are already quarantined, and why
/// collisions are excluded.
pub(crate) fn cloud_row_remediation_summary(
    report: &crate::cli::foreign_rows::ForeignRowReport,
    quarantined: usize,
) -> Option<String> {
    if report.unattributed.is_empty() && report.collisions.is_empty() && quarantined == 0 {
        return None;
    }
    let mut message = String::new();
    if !report.unattributed.is_empty() || quarantined > 0 {
        message.push_str(&format!(
            ". unattributed: {} row(s) ({} open), {quarantined} quarantined locally — quarantined rows are hidden from the board and never pushed, and the row itself is untouched (`cas doctor --fix-cloud-rows --yes` to quarantine the open ones, `--release-cloud-rows --yes` to reverse)",
            report.unattributed.len(),
            report.unattributed_open(),
        ));
    }
    if !report.collisions.is_empty() {
        message.push_str(&format!(
            ". id collisions: {} — two rows sharing an id whose titles differ, so a peer row is not evidence about the local row; these are neither deletable nor quarantinable and need an id rekey (GH #133)",
            report.collisions.len()
        ));
    }
    Some(message)
}

/// `cas doctor --fix-cloud-rows` / `--release-cloud-rows`.
///
/// Report-then-apply: without `--yes` this prints exactly what it would change
/// and touches nothing, because the operator is being asked to accept a
/// judgement about rows whose owner nobody can name.
fn run_cloud_row_quarantine(
    cas_root: &Path,
    report: anyhow::Result<crate::cli::foreign_rows::ForeignRowReport>,
    args: &DoctorArgs,
    cli: &Cli,
) -> anyhow::Result<()> {
    let report = report?;
    let queue = crate::cloud::SyncQueue::open(cas_root)?;
    queue.init()?;
    let already = queue.quarantined_ids(crate::cloud::QUARANTINE_TASK)?;

    let (action, planned): (&str, Vec<(String, String)>) = if args.release_cloud_rows {
        (
            "release",
            queue
                .quarantined_rows(crate::cloud::QUARANTINE_TASK)?
                .into_iter()
                .map(|row| (row.entity_id, row.reason))
                .collect(),
        )
    } else {
        (
            "quarantine",
            quarantine_candidates(&report)
                .into_iter()
                .filter(|row| !already.contains(&row.id))
                .map(|row| (row.id.clone(), row.title.clone()))
                .collect(),
        )
    };

    let applied = if args.yes {
        let mut applied = 0usize;
        for (id, _) in &planned {
            let changed = if args.release_cloud_rows {
                queue.release_quarantined_row(crate::cloud::QUARANTINE_TASK, id)?
            } else {
                queue.quarantine_row(
                    crate::cloud::QUARANTINE_TASK,
                    id,
                    "unattributed cloud row (cas doctor --fix-cloud-rows)",
                )?
            };
            if changed {
                applied += 1;
            }
        }
        Some(applied)
    } else {
        None
    };
    let quarantined_now = queue.quarantined_count(crate::cloud::QUARANTINE_TASK)?;

    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "action": action,
                "applied": applied.is_some(),
                "planned": planned.iter().map(|(id, detail)| serde_json::json!({
                    "id": id,
                    "detail": detail,
                })).collect::<Vec<_>>(),
                "planned_count": planned.len(),
                "changed_count": applied,
                "quarantined_before": already.len(),
                "quarantined_after": quarantined_now,
                "unattributed_total": report.unattributed.len(),
                "unattributed_open": report.unattributed_open(),
                "id_collisions": report.collisions.len(),
            }))?
        );
        return Ok(());
    }

    let theme = ActiveTheme::default();
    let mut out = std::io::stdout();
    let mut fmt = Formatter::stdout(&mut out, theme);
    fmt.subheading(&format!("cloud rows — {action}"))?;
    fmt.write_muted(&"─".repeat(50))?;
    fmt.newline()?;
    fmt.write_muted(&format!(
        "project `{}`: {} unattributed row(s) ({} open), {} already quarantined, {} id collision(s) excluded (they need an id rekey, not a purge)",
        report.local_project,
        report.unattributed.len(),
        report.unattributed_open(),
        already.len(),
        report.collisions.len(),
    ))?;
    fmt.newline()?;

    for (id, detail) in planned.iter().take(20) {
        fmt.write_muted(&format!("  {id}  {detail}"))?;
        fmt.newline()?;
    }
    if planned.len() > 20 {
        fmt.write_muted(&format!("  … and {} more", planned.len() - 20))?;
        fmt.newline()?;
    }

    match applied {
        Some(changed) => fmt.success(&format!(
            "{action}d {changed} row(s); {quarantined_now} row(s) now quarantined. Quarantine is local state: it is reapplied to no row and survives every pull, because the pull never writes this ledger."
        ))?,
        None => fmt.warning(&format!(
            "DRY RUN — nothing changed. {} row(s) would be {action}d; re-run with --yes to apply.",
            planned.len()
        ))?,
    }
    Ok(())
}

fn output_foreign_rows_detail(
    report: anyhow::Result<crate::cli::foreign_rows::ForeignRowReport>,
    purge_analysis: Option<&crate::cli::cloud::PurgeForeignAnalysis>,
    purge_analysis_error: Option<&str>,
    cli: &Cli,
) -> anyhow::Result<()> {
    let report = report?;

    if cli.json {
        let mut output = report.to_json();
        if let Some(analysis) = purge_analysis {
            output["purge_foreign"] = serde_json::json!({
                "foreign_task_evidence_count": analysis.foreign_task_count,
                "delete_set": analysis.delete_set.to_json(),
                "retained_foreign_tasks": analysis.retained_foreign_tasks.iter().map(|row| {
                    serde_json::json!({
                        "id": row.id,
                        "title": row.title,
                        "reason": row.reason,
                    })
                }).collect::<Vec<_>>(),
                "unattributed_task_count": analysis.unattributed_task_count,
                "id_collision_count": analysis.collision_count,
            });
        }
        if let Some(error) = purge_analysis_error {
            output["purge_foreign_error"] = serde_json::json!({
                "message": format!(
                    "purge delete-set classifier unavailable: {error}; foreign-count comparison is incomplete"
                ),
            });
        }
        println!("{}", serde_json::to_string_pretty(&output)?);
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

    if let Some(analysis) = purge_analysis {
        fmt.write_muted(&format!(
            "doctor evidence: {} foreign task row(s); purge delete set: {} task row(s)",
            analysis.foreign_task_count,
            analysis.delete_set.tasks.len()
        ))?;
        fmt.newline()?;
        if analysis.foreign_task_count == analysis.delete_set.tasks.len() {
            fmt.success("doctor evidence and purge delete-set counts agree")?;
        } else {
            fmt.warning(&format!(
                "purge cannot reach {} doctor evidence row(s) — see retained proposal rows below",
                analysis
                    .foreign_task_count
                    .saturating_sub(analysis.delete_set.tasks.len())
            ))?;
        }
        for row in &analysis.retained_foreign_tasks {
            fmt.warning(&format!(
                "    retained [{}] {} — {}",
                row.id,
                truncate(&row.title, 55),
                row.reason
            ))?;
        }
    }
    if let Some(error) = purge_analysis_error {
        fmt.warning(&format!(
            "purge delete-set classifier unavailable: {error}; foreign-count comparison is incomplete"
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
    let purge_counts_disagree = purge_analysis
        .is_some_and(|analysis| analysis.foreign_task_count != analysis.delete_set.tasks.len());
    if report.is_clean() && purge_analysis_error.is_none() && !purge_counts_disagree {
        fmt.success("no cross-project contamination detected")?;
    } else if purge_counts_disagree {
        fmt.warning(
            "Do not treat purge-foreign as complete remediation: inspect retained evidence rows above.",
        )?;
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

/// Resolve every known local Cassy root to its canonical id + repository
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
                git_remote: crate::cloud::derive_canonical_id_from_git_remote(&cas_root)
                    .and_then(|remote| crate::cloud::canonical_project_id(&remote)),
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
        && crate::cloud::canonical_project_id(&folder).as_deref() != Some(canonical_id.as_str())
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

/// Report persisted task origins that are equivalent to the current project
/// but retain a legacy spelling. Doctor is intentionally report-only; users
/// opt into the local rewrite with `cas cloud project --adopt-aliases`.
fn canonical_alias_checks(cas_root: &Path) -> Vec<Check> {
    let Some(current_project) = crate::cloud::resolve_canonical_id(cas_root) else {
        return Vec::new();
    };
    let mut checks = registered_alias_checks(cas_root, &current_project);
    let db_path = cas_root.join("cas.db");
    if !db_path.is_file() {
        return checks;
    }
    let conn = match rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(conn) => conn,
        Err(error) => {
            checks.push(Check {
                name: "project aliases".to_string(),
                status: CheckStatus::Warning,
                message: format!("Could not inspect task project aliases: {error}"),
            });
            return checks;
        }
    };
    let has_origin_project = conn
        .prepare("PRAGMA table_info(tasks)")
        .and_then(|mut stmt| {
            let names = stmt.query_map([], |row| row.get::<_, String>(1))?;
            names.collect::<Result<Vec<_>, _>>()
        })
        .map(|columns| columns.iter().any(|column| column == "origin_project"))
        .unwrap_or(false);
    if !has_origin_project {
        return checks;
    }

    let mut stmt = match conn.prepare(
        "SELECT origin_project FROM tasks
         WHERE NULLIF(trim(origin_project), '') IS NOT NULL
         ORDER BY origin_project",
    ) {
        Ok(stmt) => stmt,
        Err(error) => {
            checks.push(Check {
                name: "project aliases".to_string(),
                status: CheckStatus::Warning,
                message: format!("Could not inspect task project aliases: {error}"),
            });
            return checks;
        }
    };
    let rows = match stmt.query_map([], |row| row.get::<_, String>(0)) {
        Ok(rows) => rows,
        Err(error) => {
            checks.push(Check {
                name: "project aliases".to_string(),
                status: CheckStatus::Warning,
                message: format!("Could not inspect task project aliases: {error}"),
            });
            return checks;
        }
    };
    let origins = rows.filter_map(Result::ok).collect::<Vec<_>>();
    let registered = crate::cloud::project_aliases_from_config_toml(cas_root);
    checks.extend(
        canonical_alias_counts(&origins, &current_project, &registered)
            .into_iter()
            .map(|(alias, count)| Check {
                name: "project aliases".to_string(),
                status: CheckStatus::Warning,
                message: format!(
                    "{count} rows use alias `{alias}` of this project; run `cas cloud project \
                     --adopt-aliases` to rewrite and enqueue them"
                ),
            }),
    );
    checks
}

/// Report the cloud's per-project `aliases` record as mirrored into
/// `.cas/config.toml` (GH #669), so a reader can see *why* rows spelled
/// `ozer-health` are counted as this project's own rather than as foreign.
fn registered_alias_checks(cas_root: &Path, current_project: &str) -> Vec<Check> {
    let registered = crate::cloud::project_aliases_from_config_toml(cas_root);
    if registered.is_empty() {
        return Vec::new();
    }
    vec![Check {
        name: "project aliases".to_string(),
        status: CheckStatus::Ok,
        message: format!(
            "Cloud registry folds {} alias spelling(s) into `{current_project}`: {}. Rows \
             carrying them are this project's own, not foreign.",
            registered.len(),
            registered.join(", ")
        ),
    }]
}

/// Count task rows whose persisted `origin_project` is a *different spelling*
/// of the current project.
///
/// `registered` is the cloud's alias record for this project. Without it only
/// the syntactic remote/bare-slug rule applies, which cannot see a renamed
/// repository (`ozer-health` under `ozer`) — those rows would keep being
/// counted as another project's (GH #669).
fn canonical_alias_counts(
    origins: &[String],
    current_project: &str,
    registered: &[String],
) -> BTreeMap<String, usize> {
    let mut class = registered.to_vec();
    class.push(current_project.to_string());
    origins
        .iter()
        .filter(|origin| {
            crate::cloud::project_ids_match_with_aliases(origin, current_project, &class)
                && origin.trim() != current_project
        })
        .fold(BTreeMap::new(), |mut counts, origin| {
            *counts.entry(origin.clone()).or_default() += 1;
            counts
        })
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
    skipped_files: usize,
    skipped_detail: Option<String>,
    /// Symbols eligible for a vector, counted in `code_symbols` — the table the
    /// indexer writes — not in the queue. A queue-derived denominator moves
    /// whenever the queue is re-armed or lost, which is how two runs 80s apart
    /// reported 13,545 and then 11,535 eligible (GH #696).
    vector_eligible: usize,
    /// Eligible symbols whose *current* content hash is recorded vectorized:
    /// the drain's own completion condition, so doctor and the drain cannot
    /// disagree about what is done.
    vectorized: usize,
    /// Eligible symbols still awaiting a vector, including those with no queue
    /// row at all. Never a count of queue rows: an empty queue with unvectorized
    /// symbols is 0 rows and N pending, and doctor must say N.
    vector_pending: usize,
    vector_failed: usize,
    /// Eligible symbols the indexer never queued. Part of `vector_pending`,
    /// surfaced separately because it indicts the indexer, not the drain.
    vector_unqueued: usize,
    /// Queue rows describing symbols that no longer exist — pending work that
    /// no drain tick can complete.
    vector_orphaned: usize,
    /// Set when the current generation of the code-vector cache replaced an
    /// older one. A reset that is named is not a reset that lies.
    vector_rebuild: Option<crate::cloud::embeddings::CacheRebuild>,
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
    // Coverage, not queue rows: `stats()` reports what the queue happens to
    // contain, which reads as "0 pending" for a store whose queue was emptied
    // while thousands of symbols still have no vector (GH #696).
    let vectors = vector_store.coverage().unwrap_or_default();
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
        skipped_files: scan.as_ref().map(|scan| scan.skipped_files).unwrap_or(0),
        skipped_detail: scan.as_ref().and_then(|scan| scan.skipped_detail.clone()),
        vector_eligible: vectors.eligible,
        vectorized: vectors.vectorized,
        vector_pending: vectors.pending,
        vector_failed: vectors.failed,
        vector_unqueued: vectors.unqueued,
        vector_orphaned: vectors.orphaned,
        vector_rebuild: crate::cloud::embeddings::KnowledgeVectorCache::code_cache_rebuild(
            cas_root,
        ),
        head_lag,
        scan_error: scan.and_then(|scan| scan.last_error),
        error: None,
    }
}

/// The skipped-files clause, or empty when nothing was skipped.
///
/// GH #698: skipped files are named, and deliberately carry NO remediation.
/// They are excluded from the eligible denominator precisely because no rerun
/// can change them, and printing "run `cas index code`" beside them is how the
/// old warning trained operators to ignore doctor.
fn skipped_files_clause(state: &SymbolIndexState) -> String {
    if state.skipped_files == 0 {
        return String::new();
    }
    let detail = state
        .skipped_detail
        .as_deref()
        .map(|detail| format!(" ({detail})"))
        .unwrap_or_default();
    format!(
        " {} file(s) skipped as undecodable and excluded from the eligible count{detail};          no action needed — converting them to UTF-8 is the only way to index them.",
        state.skipped_files
    )
}

/// One rendering of the code-vector counters, shared by every branch of the
/// symbol-index check.
///
/// All four figures come from [`cas_store::CodeVectorCoverage`], so the line is
/// internally consistent by construction: `vectorized + pending + failed`
/// always equals `eligible`. The trailing clauses exist because a bare set of
/// counters cannot distinguish "the drain is behind" from "the queue lost its
/// rows" from "the cache was rebuilt and everything is legitimately starting
/// over" — and an operator reading a reset needs to be told which one it is.
fn code_vector_summary(state: &SymbolIndexState) -> String {
    let mut summary = format!(
        "code vectors {}/{} vectorized, {} pending, {} failed",
        state.vectorized, state.vector_eligible, state.vector_pending, state.vector_failed,
    );
    if state.vector_unqueued > 0 {
        summary.push_str(&format!(
            " ({} never queued — run `cas index code` to re-arm them)",
            state.vector_unqueued
        ));
    }
    if state.vector_orphaned > 0 {
        summary.push_str(&format!(
            "; {} queue row(s) name symbols that no longer exist",
            state.vector_orphaned
        ));
    }
    if let Some(rebuild) = &state.vector_rebuild {
        summary.push_str(&format!(
            "; vector index rebuilt at {} ({}), vectors regenerating",
            rebuild.rebuilt_at.format("%Y-%m-%d %H:%M UTC"),
            rebuild.reason,
        ));
    }
    summary
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
        // Symbols with no queue row, and queue rows with no symbol, are the two
        // ways the semantic corpus goes quietly wrong. Both are reconciled by
        // `cas index code`, which is what this branch already tells the
        // operator to run.
        || state.vector_unqueued > 0
        || state.vector_orphaned > 0
    {
        let vectors = code_vector_summary(&state);
        return Check {
            name,
            status: CheckStatus::Warning,
            message: format!(
                "symbol index coverage is incomplete: {}/{} eligible file(s), {} file(s) lagging, {} file failure(s), HEAD {}; {vectors}{}. Run `cas index code` to reconcile now.",
                state.indexed_files,
                state.eligible_files,
                file_lag,
                state.failed_files,
                match state.head_lag {
                    Some(true) => "behind",
                    Some(false) => "current",
                    None => "unknown",
                },
                state
                    .scan_error
                    .as_deref()
                    .map(|error| format!("; last error: {error}"))
                    .unwrap_or_default(),
            ) + &skipped_files_clause(&state),
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
                 entry {}; {}; HEAD {}",
                state.files,
                state.symbols,
                format_lag(lag_secs),
                code_vector_summary(&state),
                match state.head_lag {
                    Some(true) => "behind",
                    Some(false) => "current",
                    None => "unknown",
                }
            ) + &skipped_files_clause(&state),
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
    /// Units the provider refused, retired from the queue with the refusal
    /// stored on the row. These are NOT pending: they are waiting on a
    /// decision, not on a tick, and reporting them inside the backlog is how
    /// a permanent refusal hides as an ordinary queue (GH #695).
    quarantined: i64,
    /// The provider's own words for the most recent refusal.
    quarantine_error: Option<String>,
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
        if let Ok((commits, docs)) = store.count_quarantined_embedding() {
            state.quarantined = commits + docs;
        }
        if let Ok(error) = store.last_quarantined_embedding_error() {
            state.quarantine_error = error;
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

/// One clause naming the refused units and how to re-arm them, or nothing.
///
/// Kept separate from the pending backlog in every branch: a quarantined unit
/// is not draining on the next tick, and folding the two counts together is
/// what let a permanent provider refusal read as an ordinary queue for three
/// days (GH #695).
fn quarantine_clause(state: &EmbeddingDrainState) -> String {
    if state.quarantined == 0 {
        return String::new();
    }
    let reason = match &state.quarantine_error {
        Some(error) => format!(" — the provider said: {error}"),
        None => String::new(),
    };
    format!(
        "; {} unit(s) quarantined after the provider refused them{reason};          Run `cas history embed --retry-quarantined` once the cause is fixed",
        state.quarantined
    )
}

fn embedding_drain_check(state: EmbeddingDrainState) -> Check {
    let name = "embedding drain".to_string();
    let pending = state.total_pending();
    let quarantined = quarantine_clause(&state);

    // A real failure outranks everything else: it is the reason the queue is
    // not moving, and it must never be summarised away as a backlog.
    if let Some(error) = &state.last_error {
        return Check {
            name,
            status: CheckStatus::Warning,
            message: format!(
                "last drain reported: {error} ({pending} unit(s) still awaiting a vector){quarantined}"
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
        let drained = match &state.last_attempt {
            Some(at) => format!("nothing pending (last drain {at})"),
            None => "nothing pending".to_string(),
        };
        return Check {
            name,
            // An empty queue with refused units is not a clean bill of health:
            // part of the corpus has no vector and never will until someone
            // acts. Saying "nothing pending" alone would be true and useless.
            status: if state.quarantined > 0 {
                CheckStatus::Warning
            } else {
                CheckStatus::Ok
            },
            message: format!("{drained}{quarantined}"),
        };
    }

    // A queue with a capability present and no error is the drain doing its
    // job across ticks — say so, and say how deep it is.
    Check {
        name,
        status: if state.quarantined > 0 {
            CheckStatus::Warning
        } else {
            CheckStatus::Ok
        },
        message: format!(
            "{pending} unit(s) queued ({} page(s), {} commit(s), {} doc(s)); the daemon drains \
             them on its tick{}{quarantined}",
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

fn sync_warning_checks(warnings: &[crate::cloud::SyncWarningSummary]) -> Vec<Check> {
    warnings
        .iter()
        .map(|warning| {
            let name = if warning.entity_kind.contains("knowledge") {
                "foreign knowledge pages"
            } else {
                "foreign project rows"
            };
            Check::new(
                name,
                CheckStatus::Warning,
                format!("{} skipped ({})", warning.count, warning.project),
            )
        })
        .collect()
}

/// Surface the exact content queue rows that keep `purge-foreign` fail-closed.
/// The remediation is intentionally executable in order: reset terminal rows,
/// push them, then preview the purge again. A count from the generic queue
/// stats would include knowledge pages, so this reuses the purge's own content
/// predicate instead.
fn cloud_queue_check(cas_root: &Path) -> Check {
    let db_path = cas_root.join("cas.db");
    let conn = match rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(conn) => conn,
        Err(error) => {
            return Check {
                name: "cloud sync queue".to_string(),
                status: CheckStatus::Warning,
                message: format!("cannot count queued content changes: {error}"),
            };
        }
    };

    let pending = match crate::cli::cloud::pending_content_pushes(&conn) {
        Ok(pending) => pending,
        Err(error) => {
            return Check {
                name: "cloud sync queue".to_string(),
                status: CheckStatus::Warning,
                message: format!("cannot count queued content changes: {error}"),
            };
        }
    };

    let mut by_type = BTreeMap::<String, usize>::new();
    for (entity_type, _) in &pending {
        *by_type.entry(entity_type.clone()).or_default() += 1;
    }
    let breakdown = by_type
        .iter()
        .map(|(entity_type, count)| format!("{entity_type}: {count}"))
        .collect::<Vec<_>>()
        .join(", ");
    let remediation = "Run `cas cloud queue --retry`, then `cas cloud push`, then `cas cloud purge-foreign --dry-run`; repeat the push until this count reaches 0.";
    let rejections = cloud_queue_rejections(&conn);

    if pending.is_empty() && rejections.is_empty() {
        Check {
            name: "cloud sync queue".to_string(),
            status: CheckStatus::Ok,
            message: format!("0 queued content change(s) block purge-foreign; {remediation}"),
        }
    } else if pending.is_empty() {
        Check {
            name: "cloud sync queue".to_string(),
            status: CheckStatus::Warning,
            message: format!(
                "0 queued content change(s) block purge-foreign, but the cloud refused {} parked row(s): {}",
                rejections.iter().map(|(_, count)| count).sum::<usize>(),
                describe_queue_rejections(&rejections)
            ),
        }
    } else {
        Check {
            name: "cloud sync queue".to_string(),
            status: CheckStatus::Warning,
            message: format!(
                "{} queued content change(s) block purge-foreign ({breakdown}); {remediation}{}",
                pending.len(),
                if rejections.is_empty() {
                    String::new()
                } else {
                    format!(
                        " The cloud refused {} parked row(s): {}",
                        rejections.iter().map(|(_, count)| count).sum::<usize>(),
                        describe_queue_rejections(&rejections)
                    )
                }
            ),
        }
    }
}

/// Terminal queue rows the cloud itself refused, grouped by its reason.
///
/// A database written by a client that predates the per-row verdict columns
/// simply reports nothing here: doctor must not turn a missing column into a
/// warning about rejections that were never recorded.
fn cloud_queue_rejections(conn: &rusqlite::Connection) -> Vec<(String, usize)> {
    let has_columns: bool = conn
        .query_row(
            "SELECT COUNT(*) = 2 FROM pragma_table_info('sync_queue') WHERE name IN ('last_outcome', 'last_reason')",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !has_columns {
        return Vec::new();
    }

    let Ok(mut stmt) = conn.prepare(
        "SELECT COALESCE(NULLIF(TRIM(last_reason), ''), 'unspecified') AS reason, COUNT(*)
         FROM sync_queue
         WHERE last_outcome = 'rejected'
         GROUP BY reason",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
    }) else {
        return Vec::new();
    };
    let mut rejections = rows.filter_map(Result::ok).collect::<Vec<_>>();
    rejections.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    rejections
}

/// Name each refusal with the move that clears it. A reason without its
/// remediation leaves an operator holding a count and no next step.
fn describe_queue_rejections(rejections: &[(String, usize)]) -> String {
    rejections
        .iter()
        .map(|(reason, count)| {
            format!(
                "{reason} ×{count} — {}",
                crate::cloud::push_reason_hint(reason)
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(feature = "mcp-proxy")]
fn proxy_stdio_commands_check(cas_root: &Path) -> Check {
    let proxy_path = cas_root.join("proxy.toml");
    let (config, sources) = match cmcp_core::config::Config::load_merged_with_sources(
        proxy_path.exists().then_some(proxy_path.as_path()),
    ) {
        Ok(loaded) => loaded,
        Err(error) => {
            return Check {
                name: "MCP stdio upstreams".to_string(),
                status: CheckStatus::Warning,
                message: format!("cannot validate registered stdio commands: {error}"),
            };
        }
    };

    let mut commands = config
        .servers
        .iter()
        .filter_map(|(name, config)| match config {
            cmcp_core::config::ServerConfig::Stdio { command, .. } => {
                Some((name.as_str(), command.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    commands.sort_unstable_by_key(|(name, _)| *name);
    let missing = commands
        .iter()
        .filter(|(_, command)| cmcp_core::resolve_stdio_executable(command).is_none())
        .map(|(name, command)| {
            let source = sources
                .get(*name)
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "unknown configuration source".to_string());
            format!("{name} = {command} (from {source})")
        })
        .collect::<Vec<_>>();

    if missing.is_empty() {
        Check {
            name: "MCP stdio upstreams".to_string(),
            status: CheckStatus::Ok,
            message: format!(
                "{} registered command(s) resolve to executable files",
                commands.len()
            ),
        }
    } else {
        let mut source_paths = commands
            .iter()
            .filter(|(_, command)| cmcp_core::resolve_stdio_executable(command).is_none())
            .filter_map(|(name, _)| sources.get(*name))
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        source_paths.sort();
        source_paths.dedup();
        let remediation = if source_paths.is_empty() {
            "repair the registering configuration file".to_string()
        } else {
            format!("repair {}", source_paths.join(", "))
        };
        Check {
            name: "MCP stdio upstreams".to_string(),
            status: CheckStatus::Warning,
            message: format!(
                "{} of {} registered command(s) do not resolve: {}; {remediation} before restarting cas serve",
                missing.len(),
                commands.len(),
                missing.join(", ")
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn grouped_report_uses_sections_remediation_and_a_verbatim_summary() {
        let checks = vec![
            Check::new("database", CheckStatus::Ok, "SQLite database found"),
            Check::new("entries", CheckStatus::Ok, "1234567 entries accessible"),
            Check::new(
                "search index",
                CheckStatus::Warning,
                "index is stale; Run `cas index`",
            ),
        ];

        let report = render_report_plain(
            &checks,
            &[],
            &[],
            "example/project",
            "3.10.1",
            std::time::Duration::from_millis(123),
            true,
            80,
        );

        assert!(report.contains("cas doctor · example/project · 3.10.1"));
        assert!(report.contains("Store"));
        assert!(report.contains("[OK] database"));
        assert!(report.contains("[OK] entries"));
        assert!(report.contains("[WARN] search index"));
        assert!(report.contains("  → Run `cas index`"));
        // GH #697 (cas-a869): counts render verbatim. The digit-grouping
        // pass that produced `1,234,567` here also produced `cas-7,791` and
        // comma-riddled UUIDs on real reports, so it is gone rather than
        // narrowed.
        assert!(report.contains("1234567 entries accessible"));
        assert!(!report.contains("1,234,567"));
        assert!(report.contains("2 ok · 1 warnings · 0 errors · 123ms"));
    }

    #[test]
    fn mixed_sections_pack_ok_checks_on_the_section_line() {
        let checks = vec![
            Check::new("legacy search index", CheckStatus::Ok, "available"),
            Check::new(
                "search index",
                CheckStatus::Warning,
                "index is stale; Run `cas index`",
            ),
            Check::new("symbol index", CheckStatus::Ok, "available"),
        ];

        let report = render_report_plain(
            &checks,
            &[],
            &[],
            "example/project",
            "3.10.1",
            Duration::from_millis(1),
            false,
            100,
        );

        let indexes_line = report
            .lines()
            .find(|line| line.starts_with("Indexes"))
            .expect("mixed section line");
        assert!(indexes_line.contains("[OK] legacy search index"));
        assert!(indexes_line.contains("[OK] symbol index"));
        assert!(!indexes_line.contains("[WARN]"));
        assert!(report.lines().any(|line| {
            line.starts_with("  [WARN] search index") && line.contains("index is stale")
        }));
    }

    #[test]
    fn non_ok_messages_wrap_without_truncation() {
        let message = "573 foreign task row(s) from 9 other project(s) (Accounting, Penguinz, Woodworking, abundant details that operators need)";
        let checks = vec![Check::new(
            "cross-project rows",
            CheckStatus::Warning,
            message,
        )];

        let report = render_report_plain(
            &checks,
            &[],
            &[],
            "example/project",
            "3.10.1",
            Duration::from_millis(1),
            false,
            60,
        );
        let compact_report: String = report.split_whitespace().collect();
        let compact_message: String = message.split_whitespace().collect();

        assert!(
            compact_report.contains(&compact_message),
            "full diagnostic should survive wrapping:\n{report}"
        );
        assert!(!report.contains('…'), "diagnostic was truncated:\n{report}");
    }

    #[test]
    fn narrow_or_unknown_width_uses_eighty_column_layout() {
        let checks = vec![
            Check::new("database", CheckStatus::Ok, "SQLite database found"),
            Check::new("entries", CheckStatus::Ok, "1234567 entries accessible"),
        ];

        let expected = render_report_plain(
            &checks,
            &[],
            &[],
            "example/project",
            "3.10.1",
            Duration::from_millis(1),
            false,
            80,
        );
        for width in [0, 1, 39] {
            assert_eq!(
                render_report_plain(
                    &checks,
                    &[],
                    &[],
                    "example/project",
                    "3.10.1",
                    Duration::from_millis(1),
                    false,
                    width,
                ),
                expected,
                "width {width} should use the 80-column fallback"
            );
        }
    }

    #[test]
    fn doctor_json_is_a_superset_with_group_and_remediation() {
        let checks = vec![Check::new(
            "symbol index",
            CheckStatus::Warning,
            "coverage incomplete; Run `cas index code`",
        )];

        let json = serialize_checks(&checks, &[]);
        assert_eq!(json[0]["name"], "symbol index");
        assert_eq!(json[0]["status"], "warning");
        assert_eq!(json[0]["message"], "coverage incomplete");
        assert_eq!(json[0]["group"], "indexes");
        assert_eq!(json[0]["remediation"], "Run `cas index code`");
        assert!(
            json[0].get("duration_ms").is_none(),
            "an unmeasured check must not claim a duration: {}",
            json[0]
        );
    }

    /// GH #700: 30 checks, 76 seconds, and no way to tell which check spent it.
    /// The recorder attributes a block's wall time to the checks that block
    /// produced.
    #[test]
    fn phase_recorder_attributes_each_block_to_the_checks_it_produced() {
        let mut checks = Vec::new();
        let start = Instant::now();
        let mut recorder = PhaseRecorder::new_at(start);

        checks.push(Check::new("database", CheckStatus::Ok, "found"));
        recorder.mark_at("store", &checks, start + Duration::from_millis(10));

        checks.push(Check::new("foreign rows", CheckStatus::Ok, "clean"));
        recorder.mark_at("foreign rows", &checks, start + Duration::from_secs(62));

        let timings = recorder.per_check();
        assert_eq!(timings.len(), checks.len());
        assert_eq!(timings[0].phase, "store");
        assert_eq!(timings[0].duration, Duration::from_millis(10));
        assert_eq!(timings[1].phase, "foreign rows");
        assert_eq!(
            timings[1].duration,
            Duration::from_secs(62) - Duration::from_millis(10),
            "each phase is measured from the previous mark, not from the start"
        );
        assert!(!timings[1].shared());
    }

    /// A block that emits several checks cannot claim its whole cost for each
    /// one silently — the shared label says so out loud.
    #[test]
    fn phase_recorder_marks_a_shared_phase_and_keeps_empty_phases() {
        let mut checks = Vec::new();
        let start = Instant::now();
        let mut recorder = PhaseRecorder::new_at(start);

        recorder.mark_at("migrations", &checks, start + Duration::from_millis(5));
        checks.push(Check::new("a", CheckStatus::Ok, "ok"));
        checks.push(Check::new("b", CheckStatus::Ok, "ok"));
        recorder.mark_at("integrations", &checks, start + Duration::from_secs(9));

        let timings = recorder.per_check();
        assert_eq!(timings.len(), 2);
        assert!(timings[0].shared() && timings[1].shared());
        assert_eq!(timings[0].checks_in_phase, 2);
        assert_eq!(timings[0].label(), "(9.0s for 2 checks)");

        let phases = recorder.phases();
        assert_eq!(phases.len(), 2);
        assert_eq!(phases[0].label, "migrations");
        assert_eq!(
            phases[0].checks, 0,
            "a phase that produced no check still spent time and must be kept"
        );
        assert_eq!(
            recorder
                .slowest(Duration::from_millis(100), 5)
                .iter()
                .map(|phase| phase.label.as_str())
                .collect::<Vec<_>>(),
            vec!["integrations"],
            "slowest() ranks by duration and drops phases under the threshold"
        );
    }

    /// The operator-facing half: `--verbose` must print the duration beside
    /// the check, and the slowest phases as a table.
    #[test]
    fn verbose_report_prints_per_check_durations_and_a_slowest_phase_table() {
        let checks = vec![
            Check::new("database", CheckStatus::Ok, "SQLite database found"),
            Check::new("foreign rows", CheckStatus::Warning, "12 peer databases"),
        ];
        let timings = vec![
            CheckTiming {
                phase: "store".into(),
                duration: Duration::from_millis(12),
                checks_in_phase: 1,
            },
            CheckTiming {
                phase: "foreign rows".into(),
                duration: Duration::from_secs(62),
                checks_in_phase: 1,
            },
        ];
        let phases = vec![
            Phase {
                label: "store".into(),
                duration: Duration::from_millis(12),
                checks: 1,
            },
            Phase {
                label: "foreign rows".into(),
                duration: Duration::from_secs(62),
                checks: 1,
            },
        ];

        let report = render_report_plain(
            &checks,
            &timings,
            &phases,
            "example/project",
            "0.0.0-test",
            Duration::from_secs(62),
            true,
            100,
        );

        assert!(
            report.contains("(62.0s)"),
            "the slow check must carry its duration: {report}"
        );
        assert!(
            report.contains("slowest"),
            "verbose must rank the phases: {report}"
        );
        let table_line = report
            .lines()
            .find(|line| line.contains("foreign rows") && line.contains("62.0s"))
            .unwrap_or_default();
        assert!(
            !table_line.is_empty(),
            "the slowest phase must be named with its cost: {report}"
        );
    }

    /// Timing is diagnostic detail, not the default report's business.
    #[test]
    fn non_verbose_report_stays_free_of_timing_noise() {
        let checks = vec![Check::new("database", CheckStatus::Ok, "found")];
        let timings = vec![CheckTiming {
            phase: "store".into(),
            duration: Duration::from_secs(62),
            checks_in_phase: 1,
        }];
        let phases = vec![Phase {
            label: "store".into(),
            duration: Duration::from_secs(62),
            checks: 1,
        }];

        let report = render_report_plain(
            &checks,
            &timings,
            &phases,
            "example/project",
            "0.0.0-test",
            Duration::from_secs(62),
            false,
            100,
        );
        assert!(!report.contains("slowest"), "report: {report}");
        assert!(!report.contains("(62.0s)"), "report: {report}");
    }

    /// Automation reads JSON, and a per-check duration there is what makes a
    /// regression in doctor's own cost detectable in CI.
    #[test]
    fn json_carries_the_measured_duration_and_phase_per_check() {
        let checks = vec![Check::new("foreign rows", CheckStatus::Ok, "clean")];
        let timings = vec![CheckTiming {
            phase: "foreign rows".into(),
            duration: Duration::from_millis(62_100),
            checks_in_phase: 2,
        }];

        let json = serialize_checks(&checks, &timings);
        assert_eq!(json[0]["duration_ms"], 62_100);
        assert_eq!(json[0]["phase"], "foreign rows");
        assert_eq!(
            json[0]["duration_shared"], true,
            "a shared phase duration must be labelled as shared: {}",
            json[0]
        );
    }

    /// cas-25a9 AC1, behaviourally: `cas doctor --fix` against a held lock must
    /// return BOUNDED with a Warning, not hang.
    ///
    /// Before the fix the repair called `acquire_lock(&META_LOCK)`, an
    /// unbounded blocking flock, so this scenario hung the command outright.
    /// The assertion is on wall clock as well as on the rendered Check: a
    /// regression that reintroduces the block fails here on elapsed time.
    #[test]
    fn doctor_fix_against_a_held_legacy_lock_warns_within_a_bounded_time() {
        use std::time::{Duration, Instant};

        let temp = TempDir::new().expect("tempdir");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("create .cas");
        let store = open_store(&cas_root).expect("store");
        let entry = crate::types::Entry::new(
            "doctor-locked-entry".to_string(),
            "doctor legacy index held lock".to_string(),
        );
        store.add(&entry).expect("add entry");
        {
            let legacy = SearchIndex::open(&cas_root.join("index")).expect("legacy index");
            legacy.index_entry(&entry).expect("index legacy entry");
        }
        store.mark_indexed(&entry.id).expect("mark indexed");

        // Another process (a pre-fix `cas serve`) is holding the legacy root.
        use tantivy::directory::Directory;
        let holder = tantivy::Index::open_in_dir(cas_root.join("index")).expect("open legacy root");
        let _held = holder
            .directory()
            .acquire_lock(&tantivy::directory::META_LOCK)
            .expect("hold the meta lock");

        let started = Instant::now();
        let check = legacy_index_autofix(&cas_root).expect("a busy root must render a row");
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(30),
            "`doctor --fix` must not block on a held legacy lock; took {elapsed:?}"
        );
        assert!(
            matches!(check.status, CheckStatus::Warning),
            "a busy legacy root is a Warning, got: {}",
            check.message
        );
        assert!(
            check.message.contains("busy") && check.message.contains("cas doctor --fix"),
            "the warning must name the condition and the remedy: {}",
            check.message
        );

        // Release the lock BEFORE inspecting: `inspect_legacy_index` opens an
        // `IndexReader`, which acquires META_LOCK itself, so inspecting while
        // still holding it deadlocks the test against itself — the same trap
        // documented in `hybrid_search::legacy_index`.
        drop(_held);

        // And the root is left intact for the retry the warning recommends.
        assert!(
            crate::hybrid_search::inspect_legacy_index(&cas_root)
                .expect("inspect")
                .is_some(),
            "a refused repair must not have half-retired the root"
        );
    }

    #[test]
    fn doctor_reports_legacy_tantivy_root_before_repair_and_clean_after() {
        let temp = TempDir::new().expect("tempdir");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("create .cas");
        let store = open_store(&cas_root).expect("store");
        let entry = crate::types::Entry::new(
            "doctor-legacy-entry".to_string(),
            "doctor legacy index repair".to_string(),
        );
        store.add(&entry).expect("add entry");
        {
            let legacy = SearchIndex::open(&cas_root.join("index")).expect("legacy index");
            legacy.index_entry(&entry).expect("index legacy entry");
        }
        store.mark_indexed(&entry.id).expect("mark indexed");

        let before = legacy_search_index_check(&cas_root);
        assert!(matches!(before.status, CheckStatus::Warning));
        assert!(
            before.message.contains("1 document(s)"),
            "{}",
            before.message
        );
        assert!(
            before.message.contains("cas doctor --fix"),
            "{}",
            before.message
        );

        assert!(matches!(
            crate::hybrid_search::repair_legacy_index(
                &cas_root,
                store.as_ref(),
                crate::hybrid_search::LegacyRepairLimits::unbounded(),
            )
            .expect("repair"),
            crate::hybrid_search::LegacyRepairOutcome::Repaired(_)
        ));
        let after = legacy_search_index_check(&cas_root);
        assert!(matches!(after.status, CheckStatus::Ok));
        assert!(
            after.message.contains("no stray Tantivy root"),
            "{}",
            after.message
        );
    }

    #[test]
    fn doctor_reports_memory_decay_counters_from_the_last_cycle() {
        let temp = TempDir::new().unwrap();
        let cas_root = temp.path().join(".cas");
        fs::create_dir_all(&cas_root).unwrap();

        crate::daemon::MemoryDecayStatus::write(&cas_root, 4, 2).unwrap();

        let check = memory_decay_check(&cas_root);
        assert_eq!(check.name, "memory decay");
        assert!(matches!(check.status, CheckStatus::Ok));
        assert!(check.message.contains("protected=4"), "{}", check.message);
        assert!(
            check.message.contains("promoted_on_access=2"),
            "{}",
            check.message
        );
    }

    fn factory_agent(
        id: &str,
        name: &str,
        session: &str,
        role: crate::types::AgentRole,
    ) -> crate::types::Agent {
        let mut agent = crate::types::Agent::new(id.to_string(), name.to_string());
        agent.factory_session = Some(session.to_string());
        agent.role = role;
        agent
    }

    #[test]
    fn factory_supervisor_check_reports_one_per_active_session() {
        use crate::types::AgentRole;
        let agents = vec![
            factory_agent("sup-a", "supervisor-a", "factory-a", AgentRole::Supervisor),
            factory_agent("worker-a", "worker-a", "factory-a", AgentRole::Worker),
        ];
        let checks = factory_supervisor_checks(&agents);
        assert_eq!(checks.len(), 1);
        assert!(matches!(checks[0].status, CheckStatus::Ok));
        assert_eq!(checks[0].name, "factory session factory-a");
        assert!(checks[0].message.contains("supervisors: 1"));
        assert!(checks[0].message.contains("supervisor-a"));
    }

    #[test]
    fn factory_supervisor_check_flags_zero_and_multiple() {
        use crate::types::AgentRole;
        let agents = vec![
            factory_agent("worker-a", "worker-a", "factory-zero", AgentRole::Worker),
            factory_agent(
                "sup-b1",
                "supervisor-b1",
                "factory-many",
                AgentRole::Supervisor,
            ),
            factory_agent(
                "sup-b2",
                "supervisor-b2",
                "factory-many",
                AgentRole::Supervisor,
            ),
        ];
        let checks = factory_supervisor_checks(&agents);
        assert_eq!(checks.len(), 2);
        assert!(matches!(checks[0].status, CheckStatus::Warning));
        assert!(checks[0].message.contains("supervisors: 2"));
        assert!(matches!(checks[1].status, CheckStatus::Warning));
        assert!(checks[1].message.contains("supervisors: 0"));
    }

    #[test]
    fn factory_supervisor_check_ignores_stale_historical_rows() {
        use crate::types::{AgentRole, AgentStatus};
        let current = factory_agent(
            "sup-current",
            "supervisor-current",
            "factory-restarted",
            AgentRole::Supervisor,
        );
        let mut predecessor = factory_agent(
            "sup-old",
            "supervisor-old",
            "factory-restarted",
            AgentRole::Supervisor,
        );
        predecessor.status = AgentStatus::Stale;
        let checks = factory_supervisor_checks(&[current, predecessor]);
        assert_eq!(checks.len(), 1);
        assert!(matches!(checks[0].status, CheckStatus::Ok));
        assert!(checks[0].message.contains("supervisors: 1"));
    }

    /// GH #699: this is the reported shape — every per-session check is green
    /// while two supervisors share one clone and either can reap the other's
    /// workers. The per-session verdicts must stay green (they are correct in
    /// isolation) and the cross-session pass must add the warning.
    /// GH #697 (cas-a869): identifiers and timestamps must survive rendering
    /// byte-for-byte. The report used to run a digit-grouping pass over the
    /// whole rendered line, so `cas-7791` printed as `cas-7,791`, a UUID
    /// became unpasteable, and an RFC3339 timestamp grew three commas — it
    /// corrupted exactly the tokens an operator copies into the next command.
    #[test]
    fn rendered_messages_never_group_digits_inside_ids_uuids_or_timestamps() {
        let check = Check {
            name: "cross-project rows".to_string(),
            status: CheckStatus::Warning,
            message: "cas-7791 held by befc4155-89ca-4fb3-9b05-65323a4bf357 \
                      recorded_at=2026-09-03T18:59:18.226617643+00:00 across 3240 rows"
                .to_string(),
        };

        let rendered = full_message(&check);

        assert!(
            !rendered.contains(','),
            "no separator may be injected anywhere in a rendered line: {rendered}"
        );
        assert!(rendered.contains("cas-7791"), "{rendered}");
        assert!(
            rendered.contains("befc4155-89ca-4fb3-9b05-65323a4bf357"),
            "{rendered}"
        );
        assert!(
            rendered.contains("2026-09-03T18:59:18.226617643+00:00"),
            "{rendered}"
        );
        assert!(rendered.contains("3240 rows"), "{rendered}");
    }

    /// The JSON surface renders through its own path, so pin it separately —
    /// a machine reader is exactly who cannot tolerate `cas-7,791`.
    #[test]
    fn serialized_checks_keep_identifiers_verbatim() {
        let checks = vec![Check {
            name: "factory session".to_string(),
            status: CheckStatus::Ok,
            message: "supervisors: 1; noble-koala-5 (befc4155-89ca-4fb3-9b05-65323a4bf357)"
                .to_string(),
        }];

        let serialized = serialize_checks(&checks, &[]);
        let message = serialized[0]["message"].as_str().expect("message string");
        assert_eq!(
            message,
            "supervisors: 1; noble-koala-5 (befc4155-89ca-4fb3-9b05-65323a4bf357)"
        );
    }

    /// GH #697 defect (b): the line said "cannot reach 4 evidence row(s)" and
    /// then listed six ids, because the number was an arithmetic gap over one
    /// set while the ids came from another. The printed count must describe
    /// the list actually printed.
    #[test]
    fn unreachable_row_count_matches_the_rows_it_lists() {
        use crate::cli::cloud::{
            PurgeDeleteSet, PurgeEntity, PurgeForeignAnalysis, PurgeRetainedTask,
        };
        use crate::cli::foreign_rows::{ForeignRow, ForeignRowReport};

        let report = ForeignRowReport {
            local_project: "gabber-studio".to_string(),
            local_task_count: 900,
            peers_compared: vec!["cas-src".to_string()],
            foreign: (0..10)
                .map(|index| ForeignRow {
                    id: format!("cas-f{index:03}"),
                    title: format!("Foreign row {index}"),
                    closed: false,
                    origin_project: None,
                    home_project: "cas-src".to_string(),
                    also_present_in: Vec::new(),
                })
                .collect(),
            ..Default::default()
        };

        // Ten rows of evidence, six of which purge cannot reach, but a delete
        // set of six — so the arithmetic gap (4) and the retained list (6)
        // disagree. Both numbers are real; neither may stand in for the other.
        let analysis = PurgeForeignAnalysis {
            foreign_task_count: 10,
            delete_set: PurgeDeleteSet {
                tasks: (0..6)
                    .map(|index| {
                        PurgeEntity::with_evidence(
                            "task",
                            &format!("cas-d{index:03}"),
                            "Deletable foreign row",
                            "peer-evidence",
                            "cas-src",
                        )
                    })
                    .collect(),
                ..Default::default()
            },
            retained_foreign_tasks: (0..6)
                .map(|index| PurgeRetainedTask {
                    id: format!("cas-r{index:03}"),
                    title: format!("Retained row {index}"),
                    reason: "id collision".to_string(),
                })
                .collect(),
            unattributed_task_count: 0,
            collision_count: 0,
        };

        let check = foreign_rows_check(Ok(&report), Some(&analysis), 0);

        let listed = check.message.matches("cas-r").count();
        assert_eq!(listed, 6, "fixture must list six ids: {}", check.message);
        assert!(
            check.message.contains("purge cannot reach 6 evidence row(s)"),
            "the printed count must describe the list it prints: {}",
            check.message
        );
        assert!(
            !check.message.contains("cannot reach 4"),
            "the arithmetic gap must not masquerade as the listed count: {}",
            check.message
        );
    }

    /// GH #701 (cas-4342): the check has to say how many rows are
    /// unattributed, how many are already quarantined, and why collisions are
    /// excluded — the old wording stopped at "neither category is deletable",
    /// which left the operator with a count and no move.
    #[test]
    fn cross_project_check_reports_quarantine_counts_and_the_collision_rekey_recommendation() {
        use crate::cli::foreign_rows::{ForeignRowReport, IdCollision, UnattributedRow};

        let report = ForeignRowReport {
            local_project: "cas-src".to_string(),
            local_task_count: 900,
            peers_compared: vec!["gabber-studio".to_string()],
            unattributed: vec![
                UnattributedRow {
                    id: "cas-u001".to_string(),
                    title: "Open replica nobody can place".to_string(),
                    closed: false,
                    present_in: vec!["gabber-studio".to_string()],
                },
                UnattributedRow {
                    id: "cas-u002".to_string(),
                    title: "Finished replica".to_string(),
                    closed: true,
                    present_in: vec!["gabber-studio".to_string()],
                },
            ],
            collisions: vec![IdCollision {
                id: "cas-c001".to_string(),
                local_title: "Real local work".to_string(),
                other_project: "gabber-studio".to_string(),
                other_title: "A different real task".to_string(),
            }],
            ..Default::default()
        };

        let check = foreign_rows_check(Ok(&report), None, 1);

        assert!(
            check.message.contains("unattributed: 2 row(s) (1 open), 1 quarantined locally"),
            "{}",
            check.message
        );
        assert!(
            check.message.contains("cas doctor --fix-cloud-rows --yes"),
            "the reversible remedy must be named: {}",
            check.message
        );
        assert!(
            check.message.contains("--release-cloud-rows"),
            "the reversal must be named too: {}",
            check.message
        );
        assert!(
            check.message.contains("id collisions: 1") && check.message.contains("id rekey"),
            "collisions must carry the rekey recommendation, not a purge: {}",
            check.message
        );
        assert!(
            !check.message.contains("neither category is deletable"),
            "the pre-remediation wording must be gone: {}",
            check.message
        );
    }

    /// A clean project must not grow a remediation clause it does not need.
    #[test]
    fn cross_project_check_stays_silent_about_quarantine_when_there_is_nothing_to_quarantine() {
        use crate::cli::foreign_rows::ForeignRowReport;

        let report = ForeignRowReport {
            local_project: "cas-src".to_string(),
            local_task_count: 12,
            peers_compared: vec!["gabber-studio".to_string()],
            ..Default::default()
        };
        let check = foreign_rows_check(Ok(&report), None, 0);
        assert!(
            !check.message.contains("quarantined locally"),
            "{}",
            check.message
        );
    }

    /// Only open unattributed rows are candidates: a closed one is not in
    /// anybody's ready queue, and a collision must never be hidden.
    #[test]
    fn quarantine_candidates_are_open_unattributed_rows_only() {
        use crate::cli::foreign_rows::{ForeignRowReport, IdCollision, UnattributedRow};

        let report = ForeignRowReport {
            unattributed: vec![
                UnattributedRow {
                    id: "cas-open".to_string(),
                    title: "Open replica".to_string(),
                    closed: false,
                    present_in: Vec::new(),
                },
                UnattributedRow {
                    id: "cas-closed".to_string(),
                    title: "Closed replica".to_string(),
                    closed: true,
                    present_in: Vec::new(),
                },
            ],
            collisions: vec![IdCollision {
                id: "cas-collide".to_string(),
                local_title: "Real local work".to_string(),
                other_project: "gabber-studio".to_string(),
                other_title: "Different task, same id".to_string(),
            }],
            ..Default::default()
        };

        let ids: Vec<&str> = quarantine_candidates(&report)
            .into_iter()
            .map(|row| row.id.as_str())
            .collect();
        assert_eq!(ids, vec!["cas-open"]);
    }

    #[test]
    fn two_live_supervisor_sessions_add_an_overlap_warning_beside_green_session_checks() {
        use crate::types::AgentRole;
        let agents = vec![
            factory_agent(
                "sup-incumbent",
                "noble-koala-5",
                "gabber-gentle-hawk-71",
                AgentRole::Supervisor,
            ),
            factory_agent(
                "sup-newcomer",
                "gentle-falcon-66",
                "gabber-witty-panda-98",
                AgentRole::Supervisor,
            ),
        ];

        let checks = factory_supervisor_checks(&agents);
        assert_eq!(checks.len(), 2);
        let rendered: Vec<String> = checks
            .iter()
            .map(|check| format!("{}: {}", check.name, check.message))
            .collect();
        assert!(
            checks.iter().all(|c| matches!(c.status, CheckStatus::Ok)),
            "each session alone is well-formed: {rendered:?}"
        );

        let warning = crate::factory_supervisor_overlap::shared_clone_warning(
            &agents,
            Path::new("/home/pippenz/Petrastella/gabber-studio/.cas"),
            chrono::Utc::now(),
        )
        .expect("two live supervisor sessions on one clone must warn");
        assert!(warning.contains("2 live supervisors share this clone"));
        assert!(warning.contains("/home/pippenz/Petrastella/gabber-studio"));
        assert!(warning.contains("gabber-gentle-hawk-71/noble-koala-5"));
        assert!(warning.contains("gabber-witty-panda-98/gentle-falcon-66"));
        assert!(warning.contains("reap the other's workers"));
    }

    #[test]
    fn one_live_supervisor_session_adds_no_overlap_warning() {
        use crate::types::AgentRole;
        let agents = vec![
            factory_agent(
                "sup-a",
                "supervisor-a",
                "factory-a",
                AgentRole::Supervisor,
            ),
            factory_agent("worker-a", "worker-a", "factory-a", AgentRole::Worker),
        ];
        assert!(
            crate::factory_supervisor_overlap::shared_clone_warning(
                &agents,
                Path::new("/repo/.cas"),
                chrono::Utc::now(),
            )
            .is_none()
        );
    }

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
                    origin_project: None,
                    home_project: "accounting".to_string(),
                    also_present_in: Vec::new(),
                },
                ForeignRow {
                    id: "cas-0002".to_string(),
                    title: "Finished months ago".to_string(),
                    closed: true,
                    origin_project: None,
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

        let check = foreign_rows_check(Ok(&report), None, 0);

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
    fn foreign_rows_check_explains_when_purge_cannot_reach_evidence_rows() {
        use crate::cli::cloud::{PurgeDeleteSet, PurgeEntity, PurgeForeignAnalysis};
        use crate::cli::foreign_rows::{ForeignRow, ForeignRowReport};

        let report = ForeignRowReport {
            local_project: "cas-src".to_string(),
            local_task_count: 3,
            peers_compared: vec!["accounting".to_string()],
            foreign: vec![
                ForeignRow {
                    id: "cas-0001".to_string(),
                    title: "Backfilled foreign task".to_string(),
                    closed: false,
                    origin_project: Some("cas-src".to_string()),
                    home_project: "accounting".to_string(),
                    also_present_in: Vec::new(),
                },
                ForeignRow {
                    id: "cas-0002".to_string(),
                    title: "Accepted proposal".to_string(),
                    closed: false,
                    origin_project: Some("accounting".to_string()),
                    home_project: "accounting".to_string(),
                    also_present_in: Vec::new(),
                },
            ],
            ..Default::default()
        };
        let analysis = PurgeForeignAnalysis {
            delete_set: PurgeDeleteSet {
                tasks: vec![PurgeEntity::with_evidence(
                    "task",
                    "cas-0001",
                    "Backfilled foreign task",
                    "peer-evidence",
                    "accounting",
                )],
                ..Default::default()
            },
            foreign_task_count: 2,
            retained_foreign_tasks: vec![crate::cli::cloud::PurgeRetainedTask {
                id: "cas-0002".to_string(),
                title: "Accepted proposal".to_string(),
                reason: "accepted proposal materialized for this project".to_string(),
            }],
            unattributed_task_count: 0,
            collision_count: 0,
        };

        let check = foreign_rows_check(Ok(&report), Some(&analysis), 0);

        assert!(
            check.message.contains("foreign evidence: 2"),
            "{}",
            check.message
        );
        assert!(
            check.message.contains("purge delete set: 1"),
            "{}",
            check.message
        );
        assert!(
            check.message.contains("cannot reach 1"),
            "{}",
            check.message
        );
        assert!(
            check.message.contains("accepted proposal"),
            "{}",
            check.message
        );
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

        let check = foreign_rows_check(Ok(&report), None, 0);

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
        let check = foreign_rows_check(Err(&err), None, 0);

        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(check.message.contains("SKIPPED"), "{}", check.message);
        assert!(
            check.message.contains("disk I/O error"),
            "{}",
            check.message
        );
    }

    #[test]
    fn doctor_queue_check_names_retry_push_purge_and_exact_blocking_counts() {
        use rusqlite::Connection;

        let temp = TempDir::new().unwrap();
        let cas_root = temp.path().join(".cas");
        fs::create_dir_all(&cas_root).unwrap();
        let conn = Connection::open(cas_root.join("cas.db")).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE sync_queue (
                id INTEGER PRIMARY KEY,
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                operation TEXT NOT NULL,
                payload TEXT,
                team_id TEXT,
                project_id TEXT,
                created_at TEXT NOT NULL,
                retry_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT
            );
            INSERT INTO sync_queue
                (id, entity_type, entity_id, operation, created_at, retry_count)
            VALUES
                (1, 'entry', 'entry-a', 'upsert', '2026-09-01T00:00:00Z', 0),
                (2, 'entry', 'entry-b', 'upsert', '2026-09-01T00:00:01Z', 5),
                (3, 'task', 'task-a', 'upsert', '2026-09-01T00:00:02Z', 0),
                (4, 'knowledge_page', 'page-a', 'upsert', '2026-09-01T00:00:03Z', 0);
            "#,
        )
        .unwrap();

        let check = cloud_queue_check(&cas_root);
        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(check.message.contains("3 queued content change(s)"));
        assert!(check.message.contains("entry: 2"));
        assert!(check.message.contains("task: 1"));
        let retry = check.message.find("cas cloud queue --retry").unwrap();
        let push = check.message.find("cas cloud push").unwrap();
        let purge = check
            .message
            .find("cas cloud purge-foreign --dry-run")
            .unwrap();
        assert!(
            retry < push && push < purge,
            "{message}",
            message = check.message
        );
    }

    /// GH #668: doctor names each cloud refusal and its repair instead of
    /// folding every parked row into one queue count. A legacy database with
    /// no verdict columns must still read clean rather than warn.
    #[test]
    fn doctor_queue_check_names_cloud_rejections_by_reason_with_remediation() {
        use rusqlite::Connection;

        let temp = TempDir::new().unwrap();
        let cas_root = temp.path().join(".cas");
        fs::create_dir_all(&cas_root).unwrap();
        let conn = Connection::open(cas_root.join("cas.db")).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE sync_queue (
                id INTEGER PRIMARY KEY,
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                operation TEXT NOT NULL,
                payload TEXT,
                team_id TEXT,
                project_id TEXT,
                created_at TEXT NOT NULL,
                retry_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                last_outcome TEXT,
                last_reason TEXT,
                failed_client_version TEXT
            );
            INSERT INTO sync_queue
                (id, entity_type, entity_id, operation, created_at, retry_count, last_outcome, last_reason)
            VALUES
                (1, 'entry', 'entry-a', 'upsert', '2026-09-01T00:00:00Z', 5, 'rejected', 'project_mismatch'),
                (2, 'entry', 'entry-b', 'upsert', '2026-09-01T00:00:01Z', 5, 'rejected', 'project_mismatch'),
                (3, 'task', 'task-a', 'upsert', '2026-09-01T00:00:02Z', 5, NULL, NULL);
            "#,
        )
        .unwrap();

        let check = cloud_queue_check(&cas_root);
        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(
            check.message.contains("project_mismatch ×2"),
            "{}",
            check.message
        );
        assert!(
            check.message.contains("cas cloud link"),
            "{}",
            check.message
        );
        assert!(
            !check.message.contains("×3"),
            "a row with no cloud verdict is not a rejection: {}",
            check.message
        );
    }

    #[test]
    fn doctor_queue_check_is_quiet_on_databases_without_the_verdict_columns() {
        use rusqlite::Connection;

        let temp = TempDir::new().unwrap();
        let cas_root = temp.path().join(".cas");
        fs::create_dir_all(&cas_root).unwrap();
        let conn = Connection::open(cas_root.join("cas.db")).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE sync_queue (
                id INTEGER PRIMARY KEY,
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                operation TEXT NOT NULL,
                payload TEXT,
                team_id TEXT,
                project_id TEXT,
                created_at TEXT NOT NULL,
                retry_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT
            );
            "#,
        )
        .unwrap();

        let check = cloud_queue_check(&cas_root);
        assert!(matches!(check.status, CheckStatus::Ok), "{}", check.message);
        assert!(!check.message.contains("refused"), "{}", check.message);
    }

    #[cfg(feature = "mcp-proxy")]
    #[test]
    fn doctor_proxy_stdio_check_names_missing_commands_and_passes_resolved_commands() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|_| {
            let temp = TempDir::new().unwrap();
            let cas_root = temp.path().join(".cas");
            fs::create_dir_all(&cas_root).unwrap();
            let executable = temp.path().join("stdio-server");
            fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
            let stale_interpreter = temp.path().join("stale-interpreter");
            fs::write(&stale_interpreter, "#!/missing/interpreter\nexit 0\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut permissions = fs::metadata(&executable).unwrap().permissions();
                permissions.set_mode(0o755);
                fs::set_permissions(&executable, permissions).unwrap();
                let mut permissions = fs::metadata(&stale_interpreter).unwrap().permissions();
                permissions.set_mode(0o755);
                fs::set_permissions(&stale_interpreter, permissions).unwrap();
            }

            let mut config = cmcp_core::config::Config::default();
            config.add_server(
                "working".to_string(),
                cmcp_core::config::ServerConfig::Stdio {
                    command: executable.to_string_lossy().into_owned(),
                    args: Vec::new(),
                    env: BTreeMap::new().into_iter().collect(),
                },
            );
            config.add_server(
                "missing".to_string(),
                cmcp_core::config::ServerConfig::Stdio {
                    command: temp
                        .path()
                        .join("removed-interpreter")
                        .to_string_lossy()
                        .into_owned(),
                    args: Vec::new(),
                    env: BTreeMap::new().into_iter().collect(),
                },
            );
            config.add_server(
                "stale-interpreter".to_string(),
                cmcp_core::config::ServerConfig::Stdio {
                    command: stale_interpreter.to_string_lossy().into_owned(),
                    args: Vec::new(),
                    env: BTreeMap::new().into_iter().collect(),
                },
            );
            config.save_to(&cas_root.join("proxy.toml")).unwrap();

            let check = proxy_stdio_commands_check(&cas_root);
            assert!(matches!(check.status, CheckStatus::Warning));
            assert!(check.message.contains("missing"), "{}", check.message);
            assert!(
                check.message.contains("removed-interpreter"),
                "{}",
                check.message
            );
            assert!(
                check.message.contains("stale-interpreter"),
                "{}",
                check.message
            );
            assert!(!check.message.contains("working ="), "{}", check.message);
        });
    }

    #[cfg(feature = "mcp-proxy")]
    #[test]
    fn doctor_proxy_stdio_check_names_user_config_source_for_missing_command() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let xdg_config_home = env.home().join("xdg");
        env.set("XDG_CONFIG_HOME", &xdg_config_home);
        let user_path = xdg_config_home.join("code-mode-mcp/config.toml");
        let temp = TempDir::new().unwrap();
        let cas_root = temp.path().join(".cas");
        fs::create_dir_all(&cas_root).unwrap();

        let mut config = cmcp_core::config::Config::default();
        config.add_server(
            "user-missing".to_string(),
            cmcp_core::config::ServerConfig::Stdio {
                command: "/definitely/missing-user-command".to_string(),
                args: Vec::new(),
                env: BTreeMap::new().into_iter().collect(),
            },
        );
        config.save_to(&user_path).unwrap();

        let check = proxy_stdio_commands_check(&cas_root);
        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(
            check
                .message
                .contains(&format!("from {}", user_path.display())),
            "{}",
            check.message
        );
        assert!(
            check
                .message
                .contains(&format!("repair {}", user_path.display())),
            "{}",
            check.message
        );
    }

    #[cfg(feature = "mcp-proxy")]
    #[test]
    fn doctor_proxy_stdio_check_names_project_source_for_overridden_command() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let xdg_config_home = env.home().join("xdg");
        env.set("XDG_CONFIG_HOME", &xdg_config_home);
        let user_path = xdg_config_home.join("code-mode-mcp/config.toml");
        let temp = TempDir::new().unwrap();
        let cas_root = temp.path().join(".cas");
        fs::create_dir_all(&cas_root).unwrap();
        let project_path = cas_root.join("proxy.toml");

        let mut user_config = cmcp_core::config::Config::default();
        user_config.add_server(
            "shared".to_string(),
            cmcp_core::config::ServerConfig::Stdio {
                command: "/definitely/missing-user-command".to_string(),
                args: Vec::new(),
                env: BTreeMap::new().into_iter().collect(),
            },
        );
        user_config.save_to(&user_path).unwrap();

        let mut project_config = cmcp_core::config::Config::default();
        project_config.add_server(
            "shared".to_string(),
            cmcp_core::config::ServerConfig::Stdio {
                command: "/definitely/missing-project-command".to_string(),
                args: Vec::new(),
                env: BTreeMap::new().into_iter().collect(),
            },
        );
        project_config.save_to(&project_path).unwrap();

        let check = proxy_stdio_commands_check(&cas_root);
        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(
            check
                .message
                .contains(&format!("from {}", project_path.display())),
            "{}",
            check.message
        );
        assert!(
            !check
                .message
                .contains(&format!("from {}", user_path.display())),
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

        let check = foreign_rows_check(Ok(&report), None, 0);

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
    fn alias_doctor_counts_case_and_remote_spellings_but_not_owned_rows() {
        let origins = vec![
            "gabber-studio".to_string(),
            "GABBER-STUDIO".to_string(),
            "git@GitHub.com:Richards-LLC/gabber-studio.git".to_string(),
            "github.com/other/pixel-hive".to_string(),
        ];

        let counts = canonical_alias_counts(&origins, "gabber-studio", &[]);

        assert_eq!(counts.get("GABBER-STUDIO"), Some(&1));
        assert_eq!(
            counts.get("git@GitHub.com:Richards-LLC/gabber-studio.git"),
            Some(&1)
        );
        assert!(!counts.contains_key("gabber-studio"));
        assert!(!counts.contains_key("github.com/other/pixel-hive"));
    }

    /// GH #669: a *renamed* repository shares no path segment with its
    /// canonical id, so only the cloud's alias record can attribute it. Before
    /// the record is consumed those rows are counted as another project's.
    #[test]
    fn alias_doctor_attributes_a_renamed_repository_only_through_the_registered_record() {
        let origins = vec![
            "ozer-health".to_string(),
            "github.com/richards-llc/ozer-health".to_string(),
            "penguinz".to_string(),
        ];

        let without_record = canonical_alias_counts(&origins, "ozer", &[]);
        assert!(without_record.is_empty(), "got: {without_record:?}");

        let registered = vec![
            "ozer-health".to_string(),
            "github.com/richards-llc/ozer-health".to_string(),
        ];
        let with_record = canonical_alias_counts(&origins, "ozer", &registered);
        assert_eq!(with_record.get("ozer-health"), Some(&1));
        assert_eq!(
            with_record.get("github.com/richards-llc/ozer-health"),
            Some(&1)
        );
        // `penguinz` is an unmapped legacy bucket: never folded in.
        assert!(!with_record.contains_key("penguinz"));
    }

    #[test]
    fn registered_alias_check_names_the_folded_spellings() {
        let temp = TempDir::new().unwrap();
        let cas_root = temp.path().join("ozer/.cas");
        fs::create_dir_all(&cas_root).unwrap();
        fs::write(
            cas_root.join("config.toml"),
            "[project]\ncanonical_id = \"ozer\"\naliases = [\"ozer-health\"]\n",
        )
        .unwrap();

        let checks = registered_alias_checks(&cas_root, "ozer");

        assert_eq!(checks.len(), 1);
        assert!(matches!(checks[0].status, CheckStatus::Ok));
        assert!(
            checks[0].message.contains("ozer-health"),
            "got: {}",
            checks[0].message
        );
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

    // -----------------------------------------------------------------
    // Stale user-level skills (cas-332f)
    // -----------------------------------------------------------------

    fn claude_names() -> std::collections::HashSet<String> {
        catalog_skill_names(crate::builtins::BUILTIN_SKILLS)
    }

    fn write_skill(dir: &Path, name: &str, body: &str) -> PathBuf {
        let skill_dir = dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        let file = skill_dir.join("SKILL.md");
        fs::write(&file, body).unwrap();
        file
    }

    /// A retired skill is named even though it carries no `managed_by: cas`
    /// marker — which is the whole point, since the marker-based pruner is
    /// exactly what failed to see `mecha-cassy-post` for its entire life.
    #[test]
    fn retired_user_skill_is_reported_with_the_builtin_that_replaced_it() {
        let dir = TempDir::new().unwrap();
        let skills = dir.path().join("skills");
        let retired = write_skill(
            &skills,
            "mecha-cassy-post",
            "---\nname: mecha-cassy-post\n---\n\nPost release notes.\n",
        );

        let strays = scan_user_skill_dirs(&[(skills, claude_names())]);
        assert_eq!(
            strays,
            vec![StrayUserSkill {
                name: "mecha-cassy-post".to_string(),
                path: retired,
                reason: StrayReason::RetiredBy("mecha-cassy"),
            }]
        );

        let check = stray_user_skills_check(&strays);
        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(check.message.contains("mecha-cassy-post"), "{}", check.message);
        assert!(check.message.contains("mecha-cassy owns it now"), "{}", check.message);
        assert_eq!(check.group(), CheckGroup::Config);
        // The guidance must reach doctor's remediation column, not stay buried.
        assert!(check.parts().1.is_some(), "{:?}", check.parts());
    }

    /// A builtin projected into the user directory is the normal case and must
    /// never be flagged, or this check cries wolf on every machine.
    #[test]
    fn a_current_builtin_present_at_user_scope_is_not_a_stray() {
        let dir = TempDir::new().unwrap();
        let skills = dir.path().join("skills");
        let builtin_name = claude_names()
            .into_iter()
            .next()
            .expect("catalog ships at least one skill");
        write_skill(
            &skills,
            &builtin_name,
            "---\nname: x\nmanaged_by: cas\n---\n\nbody\n",
        );
        // An unrelated hand-written skill with no cas marker is the user's own
        // business and is also left alone.
        write_skill(&skills, "my-own-notes", "---\nname: my-own-notes\n---\n");

        let strays = scan_user_skill_dirs(&[(skills, claude_names())]);
        assert!(strays.is_empty(), "{strays:?}");
        assert!(matches!(
            stray_user_skills_check(&strays).status,
            CheckStatus::Ok
        ));
    }

    /// A directory carrying `managed_by: cas` that the catalog no longer ships
    /// will never be refreshed again, so it is reported even though its name is
    /// not on the retired list.
    #[test]
    fn orphaned_managed_copy_is_reported_even_without_a_cas_prefix() {
        let dir = TempDir::new().unwrap();
        let skills = dir.path().join("skills");
        write_skill(
            &skills,
            "some-retired-builtin",
            "---\nname: some-retired-builtin\nmanaged_by: cas\n---\n\nbody\n",
        );

        let strays = scan_user_skill_dirs(&[(skills, claude_names())]);
        assert_eq!(strays.len(), 1, "{strays:?}");
        assert_eq!(strays[0].reason, StrayReason::OrphanedManagedCopy);
        assert!(
            stray_user_skills_check(&strays)
                .message
                .contains("no longer a builtin")
        );
    }

    /// Codex ships skills Claude does not. Comparing a `~/.codex/skills`
    /// directory against the Claude catalog reported the perfectly current
    /// `cas-codex-supervisor-checklist` as an orphan on a real machine, so the
    /// catalog must be chosen per harness.
    #[test]
    fn a_codex_only_builtin_is_not_an_orphan_against_the_codex_catalog() {
        let dir = TempDir::new().unwrap();
        let skills = dir.path().join("skills");
        write_skill(
            &skills,
            "cas-codex-supervisor-checklist",
            "---\nname: cas-codex-supervisor-checklist\nmanaged_by: cas\n---\n",
        );

        let codex = catalog_skill_names(crate::builtins::CODEX_BUILTIN_SKILLS);
        assert!(
            codex.contains("cas-codex-supervisor-checklist"),
            "fixture assumes this ships in the Codex catalog"
        );
        assert!(
            scan_user_skill_dirs(&[(skills.clone(), codex)]).is_empty(),
            "a current Codex builtin must not be flagged"
        );
        // …and the same directory judged against the wrong catalog is exactly
        // the false positive this guards against.
        assert_eq!(scan_user_skill_dirs(&[(skills, claude_names())]).len(), 1);
    }

    /// Several Claude account directories symlink one shared `skills/`. A naive
    /// walk reports the same file once per account and an operator "fixes" the
    /// same file repeatedly; the scan must dedupe by canonical path.
    #[test]
    fn a_skills_directory_shared_by_symlink_is_reported_once() {
        let dir = TempDir::new().unwrap();
        let real = dir.path().join("real-account").join("skills");
        write_skill(
            &real,
            "mecha-cassy-post",
            "---\nname: mecha-cassy-post\n---\n",
        );
        let linked = dir.path().join("linked-account-skills");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &linked).unwrap();
        #[cfg(not(unix))]
        return;

        let strays = scan_user_skill_dirs(&[(real.clone(), claude_names()), (linked, claude_names())]);
        assert_eq!(strays.len(), 1, "symlinked duplicate double-counted: {strays:?}");
    }

    /// `cas-8fad`: the machine-scoped MechaCassy row must land in the
    /// Integrations group (not the Store catch-all) and its "Run `cas integrate
    /// mecha-cassy`" guidance must split into doctor's remediation column
    /// rather than staying buried in the diagnostic text.
    #[test]
    fn mecha_cassy_row_groups_under_integrations_and_exposes_its_remedy() {
        let check = Check::new(
            "mecha-cassy",
            CheckStatus::Warning,
            "not registered on this machine (/tmp/config.toml has no mecha-cassy server). \
             Run `cas integrate mecha-cassy`",
        );
        assert_eq!(check.group(), CheckGroup::Integrations);
        let (message, remediation) = check.parts();
        assert!(message.contains("not registered on this machine"), "{message}");
        assert_eq!(
            remediation.as_deref(),
            Some("Run `cas integrate mecha-cassy`")
        );
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
            assert!(
                check.message.contains(expected),
                "missing {expected}: {}",
                check.message
            );
        }
    }

    /// GH #696: an empty queue with unvectorized symbols used to read as
    /// "0/0 vectorized, 0 pending" and pass. Coverage puts those symbols in the
    /// denominator, so doctor now names the hole and points at the fix.
    #[test]
    fn symbol_index_check_reports_symbols_that_were_never_queued() {
        let now = chrono::Utc::now();
        let state = SymbolIndexState {
            vector_eligible: 11_535,
            vectorized: 0,
            vector_pending: 11_535,
            vector_unqueued: 11_535,
            last_indexed: Some(now),
            ..healthy_state()
        };

        let check = symbol_index_check(state, now);
        assert!(
            matches!(check.status, CheckStatus::Warning),
            "a corpus with no vectors and no queued work must not pass: {}",
            check.message
        );
        for expected in [
            "0/11535 vectorized",
            "11535 pending",
            "11535 never queued",
            "cas index code",
        ] {
            assert!(
                check.message.contains(expected),
                "missing {expected}: {}",
                check.message
            );
        }
    }

    /// The inverse lie: queue rows outliving their symbols are reported as
    /// ghosts rather than folded into pending work.
    #[test]
    fn symbol_index_check_names_orphaned_queue_rows() {
        let now = chrono::Utc::now();
        let state = SymbolIndexState {
            vector_eligible: 900,
            vectorized: 900,
            vector_orphaned: 2_010,
            last_indexed: Some(now),
            ..healthy_state()
        };

        let check = symbol_index_check(state, now);
        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(
            check
                .message
                .contains("2010 queue row(s) name symbols that no longer exist"),
            "message: {}",
            check.message
        );
    }

    /// A reset that is named is not a reset that lies: after the vector cache
    /// is rebuilt, the check says so instead of silently reporting that every
    /// vector disappeared.
    #[test]
    fn symbol_index_check_labels_a_vector_cache_rebuild() {
        let now = chrono::Utc::now();
        let rebuilt_at = chrono::DateTime::parse_from_rfc3339("2026-09-03T19:18:50Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let state = SymbolIndexState {
            vector_eligible: 11_535,
            vectorized: 0,
            vector_pending: 11_535,
            vector_rebuild: Some(crate::cloud::embeddings::CacheRebuild {
                rebuilt_at,
                reason: "embedding model changed from p/m1 (3d) to p/m2 (4d)".into(),
            }),
            last_indexed: Some(now),
            ..healthy_state()
        };

        let check = symbol_index_check(state, now);
        for expected in [
            "vector index rebuilt at 2026-09-03 19:18 UTC",
            "embedding model changed",
            "vectors regenerating",
        ] {
            assert!(
                check.message.contains(expected),
                "missing {expected}: {}",
                check.message
            );
        }
    }

    /// The acceptance shape from GH #696: nothing happened between two reads,
    /// so the two messages must be identical — including the vector line.
    #[test]
    fn symbol_index_check_is_identical_across_two_reads_of_one_state() {
        let now = chrono::Utc::now();
        let state = SymbolIndexState {
            vector_eligible: 13_545,
            vectorized: 603,
            vector_pending: 12_942,
            last_indexed: Some(now),
            ..healthy_state()
        };

        let first = symbol_index_check(state.clone(), now);
        let second = symbol_index_check(state, now);
        assert_eq!(first.message, second.message);
        assert!(
            first.message.contains("603/13545 vectorized, 12942 pending"),
            "message: {}",
            first.message
        );
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

    /// GH #695: refused units are reported apart from the backlog, name the
    /// provider's reason, and carry the command that re-arms them. An empty
    /// queue with refusals in it is not a clean bill of health.
    #[test]
    fn embedding_drain_check_separates_quarantined_units_from_the_backlog() {
        let check = embedding_drain_check(EmbeddingDrainState {
            capability: true,
            commits_pending: 12,
            quarantined: 5,
            quarantine_error: Some(
                "Embedding request rejected with status 502: {\"error\":\"Embedding provider returned 400\"}"
                    .to_string(),
            ),
            last_attempt: Some("2026-09-03T21:00:00Z".to_string()),
            ..Default::default()
        });
        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(check.message.contains("12 unit(s) queued"), "{}", check.message);
        assert!(
            check.message.contains("5 unit(s) quarantined"),
            "the refused units must not be folded into the backlog: {}",
            check.message
        );
        assert!(
            check.message.contains("provider returned 400"),
            "{}",
            check.message
        );
        assert!(
            check.message.contains("cas history embed --retry-quarantined"),
            "a count without a move is not actionable: {}",
            check.message
        );

        // Drained to zero, but part of the corpus has no vector and never will
        // until someone acts: that is a warning, not "nothing pending".
        let drained = embedding_drain_check(EmbeddingDrainState {
            capability: true,
            quarantined: 2,
            last_attempt: Some("2026-09-03T21:00:00Z".to_string()),
            ..Default::default()
        });
        assert!(matches!(drained.status, CheckStatus::Warning));
        assert!(drained.message.contains("nothing pending"), "{}", drained.message);
        assert!(
            drained.message.contains("2 unit(s) quarantined"),
            "{}",
            drained.message
        );

        // No refusals: the line stays exactly as it was.
        let clean = embedding_drain_check(EmbeddingDrainState {
            capability: true,
            last_attempt: Some("2026-09-03T21:00:00Z".to_string()),
            ..Default::default()
        });
        assert!(matches!(clean.status, CheckStatus::Ok));
        assert!(!clean.message.contains("quarantined"), "{}", clean.message);
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
        assert!(matches!(
            history_index_check(state).status,
            CheckStatus::Warning
        ));

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
        assert!(
            check.message.contains("unknown rather than fresh"),
            "{}",
            check.message
        );
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

    // -----------------------------------------------------------------
    // Task dependency health (cas-095c)
    // -----------------------------------------------------------------

    /// A task store shaped like the real one: quarantine-filtered lists over an
    /// unfiltered SQLite table, which is exactly the pairing that made doctor
    /// call quarantined endpoints "orphaned".
    fn task_health_store() -> (
        TempDir,
        std::sync::Arc<crate::cloud::SyncQueue>,
        std::sync::Arc<dyn crate::store::TaskStore>,
    ) {
        let temp = TempDir::new().unwrap();
        let inner = crate::store::SqliteTaskStore::open(temp.path()).unwrap();
        crate::store::TaskStore::init(&inner).unwrap();
        let queue = std::sync::Arc::new(crate::cloud::SyncQueue::open(temp.path()).unwrap());
        queue.init().unwrap();
        let store: std::sync::Arc<dyn crate::store::TaskStore> =
            std::sync::Arc::new(crate::store::QuarantineFilteringTaskStore::new(
                std::sync::Arc::new(inner),
                std::sync::Arc::clone(&queue),
            ));
        (temp, queue, store)
    }

    fn seed_pair(store: &dyn crate::store::TaskStore) {
        use crate::types::{Dependency, DependencyType, Task};
        store
            .add(&Task::new("cas-aaaa".to_string(), "Child".to_string()))
            .unwrap();
        store
            .add(&Task::new("cas-bbbb".to_string(), "Blocker".to_string()))
            .unwrap();
        store
            .add_dependency(&Dependency::new(
                "cas-aaaa".to_string(),
                "cas-bbbb".to_string(),
                DependencyType::Blocks,
            ))
            .unwrap();
    }

    fn health_of(store: &dyn crate::store::TaskStore) -> DependencyEndpointHealth {
        let tasks = store.list(None).unwrap();
        let deps = store.list_dependencies(None).unwrap();
        dependency_endpoint_health(store, &tasks, &deps)
    }

    fn check_for(store: &dyn crate::store::TaskStore) -> Check {
        let tasks = store.list(None).unwrap();
        task_health_check(tasks.len(), "Open: 2", 2, 0, &health_of(store))
    }

    /// The reported bug: quarantining a task (which `cas doctor
    /// --fix-cloud-rows` tells the operator to do) must not turn every
    /// dependency touching it into an "orphan" the operator cannot clear.
    #[test]
    fn dependency_rows_pointing_at_quarantined_tasks_are_ok_and_counted_cas_095c() {
        let (_temp, queue, store) = task_health_store();
        seed_pair(store.as_ref());

        assert!(
            queue
                .quarantine_row(
                    crate::cloud::QUARANTINE_TASK,
                    "cas-bbbb",
                    "unattributed cloud row"
                )
                .unwrap()
        );

        let health = health_of(store.as_ref());
        assert_eq!(health.quarantined_endpoint_rows, 1);
        assert!(health.dangling.is_empty(), "{health:?}");

        let check = check_for(store.as_ref());
        assert!(
            matches!(check.status, CheckStatus::Ok),
            "quarantine is not a fault: {}",
            check.message
        );
        assert!(
            check
                .message
                .contains("1 dependency row(s) reference quarantined tasks"),
            "{}",
            check.message
        );
        assert!(
            !check.message.contains("orphaned"),
            "quarantined endpoints must not be reported as orphans: {}",
            check.message
        );
    }

    /// A row whose endpoint is gone from the `tasks` table entirely is still a
    /// fault — and the warning has to name a command that clears it.
    #[test]
    fn genuinely_dangling_dependency_rows_warn_and_name_a_command_cas_095c() {
        let (temp, _queue, store) = task_health_store();
        seed_pair(store.as_ref());
        delete_task_row_only(temp.path(), "cas-bbbb");

        let health = health_of(store.as_ref());
        assert_eq!(health.quarantined_endpoint_rows, 0);
        assert_eq!(
            health.dangling,
            vec![("cas-aaaa".to_string(), "cas-bbbb".to_string())]
        );

        let check = check_for(store.as_ref());
        assert!(
            matches!(check.status, CheckStatus::Warning),
            "{}",
            check.message
        );
        let (_message, remediation) = check.parts();
        let remediation = remediation.expect("a dangling-row warning must carry remediation");
        assert!(
            remediation.contains("cas doctor --fix"),
            "remediation must name the command that clears it: {remediation}"
        );
        assert!(
            check.message.contains("cas-aaaa -> cas-bbbb"),
            "the warning must name the offending row: {}",
            check.message
        );
    }

    /// The command the warning names must actually clear the warning.
    #[test]
    fn doctor_fix_prunes_dangling_dependency_rows_and_clears_the_warning_cas_095c() {
        let (temp, queue, store) = task_health_store();
        seed_pair(store.as_ref());
        // One genuinely dangling row, one quarantined endpoint: the prune must
        // take the first and leave the second strictly alone.
        store
            .add(&crate::types::Task::new(
                "cas-cccc".to_string(),
                "Quarantined".to_string(),
            ))
            .unwrap();
        store
            .add_dependency(&crate::types::Dependency::new(
                "cas-aaaa".to_string(),
                "cas-cccc".to_string(),
                crate::types::DependencyType::Related,
            ))
            .unwrap();
        queue
            .quarantine_row(
                crate::cloud::QUARANTINE_TASK,
                "cas-cccc",
                "unattributed cloud row",
            )
            .unwrap();
        delete_task_row_only(temp.path(), "cas-bbbb");

        assert!(matches!(
            check_for(store.as_ref()).status,
            CheckStatus::Warning
        ));

        let pruned = prune_dangling_dependencies(store.as_ref()).unwrap();
        assert_eq!(pruned, 1);

        let check = check_for(store.as_ref());
        assert!(
            matches!(check.status, CheckStatus::Ok),
            "the named command did not clear the warning: {}",
            check.message
        );
        assert!(
            check
                .message
                .contains("1 dependency row(s) reference quarantined tasks"),
            "the quarantined row must survive the prune and stay reported: {}",
            check.message
        );
        assert_eq!(store.list_dependencies(None).unwrap().len(), 1);
    }

    /// A healthy board says exactly what it said before this fix.
    #[test]
    fn a_board_with_no_missing_endpoints_reports_the_plain_summary_cas_095c() {
        let (_temp, _queue, store) = task_health_store();
        seed_pair(store.as_ref());

        let check = check_for(store.as_ref());
        assert!(matches!(check.status, CheckStatus::Ok));
        assert_eq!(check.message, "2 tasks (Open: 2) | 2 open, 0 blocked");
    }

    /// Delete only the task row, leaving its dependency edges behind. The store
    /// API cascades, so the state doctor actually meets in the wild (edges
    /// pulled from cloud whose task never arrived) has to be built by hand.
    fn delete_task_row_only(cas_dir: &Path, id: &str) {
        let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
        conn.execute("DELETE FROM tasks WHERE id = ?", [id]).unwrap();
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

/// Rendered report text is emitted verbatim (GH #697 / cas-a869).
///
/// A digit-grouping pass used to run over each finished line, with no way to
/// know whether a digit run was a count or part of an identifier. It turned
/// `cas-7791` into `cas-7,791`, made UUIDs and RFC3339 timestamps unpasteable,
/// and so corrupted precisely the tokens an operator copies into the next
/// command. Grouping was cosmetic; the corruption was not, so the pass is
/// gone and counts render as plain integers. Do not reintroduce a
/// post-processing pass over rendered lines — any future grouping must happen
/// where the number is still a number, before it becomes prose.
fn status_label(status: &CheckStatus, styled: bool) -> &'static str {
    if styled {
        match status {
            CheckStatus::Ok => Icons::CHECK,
            CheckStatus::Warning => Icons::WARNING,
            CheckStatus::Error => Icons::CROSS,
        }
    } else {
        match status {
            CheckStatus::Ok => "[OK]",
            CheckStatus::Warning => "[WARN]",
            CheckStatus::Error => "[ERROR]",
        }
    }
}

fn status_name(status: &CheckStatus) -> &'static str {
    match status {
        CheckStatus::Ok => "ok",
        CheckStatus::Warning => "warning",
        CheckStatus::Error => "error",
    }
}

fn full_message(check: &Check) -> String {
    check.message.clone()
}

/// Serialize the checks, adding the measured duration where one exists.
///
/// `timings` is positional: entry `i` measures check `i`. An unmeasured check
/// (an early return before the recorder ran) omits the fields rather than
/// reporting a zero, because "not measured" and "took no time" are different
/// facts and automation must be able to tell them apart.
fn serialize_checks(checks: &[Check], timings: &[CheckTiming]) -> Vec<serde_json::Value> {
    checks
        .iter()
        .enumerate()
        .map(|(index, check)| {
            let (message, remediation) = check.parts();
            let mut value = serde_json::json!({
                "name": check.name,
                "status": status_name(&check.status),
                "message": message,
                "group": check.group().json_name(),
                "remediation": remediation,
            });
            if let (Some(timing), Some(object)) = (timings.get(index), value.as_object_mut()) {
                object.insert(
                    "duration_ms".to_string(),
                    serde_json::json!(timing.duration.as_millis() as u64),
                );
                object.insert("phase".to_string(), serde_json::json!(timing.phase));
                object.insert(
                    "duration_shared".to_string(),
                    serde_json::json!(timing.shared()),
                );
            }
            value
        })
        .collect()
}

fn duration_label(duration: Duration) -> String {
    if duration.as_secs() == 0 {
        format!("{}ms", duration.as_millis())
    } else {
        format!("{:.1}s", duration.as_secs_f64())
    }
}

fn wrap_report_text(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let word_len = word.chars().count();
        if !current.is_empty() && current.chars().count() + 1 + word_len <= width {
            current.push(' ');
            current.push_str(word);
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        for ch in word.chars() {
            if current.chars().count() >= width {
                lines.push(std::mem::take(&mut current));
            }
            current.push(ch);
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn write_report_line(
    fmt: &mut Formatter<'_>,
    status: Option<&CheckStatus>,
    text: &str,
) -> std::io::Result<()> {
    if fmt.is_styled() {
        let color = status.map(|status| match status {
            CheckStatus::Ok => fmt.theme().palette.status_success,
            CheckStatus::Warning => fmt.theme().palette.status_warning,
            CheckStatus::Error => fmt.theme().palette.status_error,
        });
        if let Some(color) = color {
            fmt.write_colored(text, color)?;
        } else {
            fmt.write_primary(text)?;
        }
    } else {
        fmt.write_raw(text)?;
    }
    fmt.newline()
}

fn write_ok_section_line(
    fmt: &mut Formatter<'_>,
    group: CheckGroup,
    checks: &[&Check],
    width: usize,
) -> std::io::Result<()> {
    let label = format!("{:<14}", group.label());
    let marker = status_label(&CheckStatus::Ok, fmt.is_styled());
    let mut line = label.clone();
    let indent = " ".repeat(14);
    for check in checks {
        let pair = format!("{marker} {}", check.name);
        let prefix = if line == label { "" } else { "  " };
        if line.chars().count() + prefix.chars().count() + pair.chars().count() > width
            && line != label
        {
            write_report_line(fmt, Some(&CheckStatus::Ok), &line)?;
            line = format!("{indent}{pair}");
        } else {
            line.push_str(prefix);
            line.push_str(&pair);
        }
    }
    write_report_line(fmt, Some(&CheckStatus::Ok), &line)
}

/// Phases faster than this are noise in the slowest-phase table; the table
/// exists to point at the one check that spent the minute (GH #700).
const SLOW_PHASE_THRESHOLD: Duration = Duration::from_millis(100);

fn render_report(
    fmt: &mut Formatter<'_>,
    checks: &[Check],
    timings: &[CheckTiming],
    phases: &[Phase],
    canonical_id: &str,
    version: &str,
    elapsed: Duration,
    verbose: bool,
) -> std::io::Result<()> {
    fmt.write_bold(&format!("cas doctor · {canonical_id} · {version}"))?;
    fmt.newline()?;
    let width = match fmt.width() as usize {
        width if width < 40 => 80,
        width => width,
    };
    fmt.write_muted(&Icons::SEPARATOR.repeat(width.min(80)))?;
    fmt.newline()?;

    let groups = [
        CheckGroup::Store,
        CheckGroup::Indexes,
        CheckGroup::Cloud,
        CheckGroup::Config,
        CheckGroup::Integrations,
    ];
    for group in groups {
        let section: Vec<&Check> = checks
            .iter()
            .filter(|check| check.group() == group)
            .collect();
        if section.is_empty() {
            continue;
        }
        let all_ok = section
            .iter()
            .all(|check| matches!(check.status, CheckStatus::Ok));
        let ok_checks: Vec<&Check> = section
            .iter()
            .copied()
            .filter(|check| matches!(check.status, CheckStatus::Ok))
            .collect();
        if all_ok {
            write_ok_section_line(fmt, group, &ok_checks, width)?;
            continue;
        }

        if ok_checks.is_empty() {
            fmt.write_bold(group.label())?;
            fmt.newline()?;
        } else {
            write_ok_section_line(fmt, group, &ok_checks, width)?;
        }
        let name_width = section
            .iter()
            .map(|check| check.name.chars().count())
            .max()
            .unwrap_or(0);
        for check in section {
            if matches!(check.status, CheckStatus::Ok) {
                continue;
            }
            let prefix = format!(
                "  {} {:<name_width$} ",
                status_label(&check.status, fmt.is_styled()),
                check.name,
                name_width = name_width
            );
            let available = width.saturating_sub(prefix.chars().count()).max(1);
            let (message, remediation) = check.parts();
            let message_lines = wrap_report_text(&message, available);
            let hanging_indent = " ".repeat(prefix.chars().count());
            for (line_index, message_line) in message_lines.iter().enumerate() {
                let line = if line_index == 0 {
                    format!("{prefix}{message_line}")
                } else {
                    format!("{hanging_indent}{message_line}")
                };
                write_report_line(fmt, Some(&check.status), &line)?;
            }
            if let Some(remediation) = remediation {
                fmt.write_muted(&format!("  {} {}", Icons::ARROW_RIGHT, remediation))?;
                fmt.newline()?;
            }
        }
    }

    if verbose {
        fmt.newline()?;
        fmt.write_bold("verbose")?;
        fmt.newline()?;
        for (index, check) in checks.iter().enumerate() {
            let timing = timings
                .get(index)
                .map(|timing| format!(" {}", timing.label()))
                .unwrap_or_default();
            let line = format!(
                "{} {}: {}{timing}",
                status_label(&check.status, fmt.is_styled()),
                check.name,
                full_message(check)
            );
            write_report_line(fmt, Some(&check.status), &line)?;
        }

        // The per-check line answers "how long did this take"; the table
        // answers the question GH #700 actually asked, which is "which one is
        // eating the minute". Ranking is over phases, including any that
        // produced no check, so the times still add up to the total.
        let mut slowest: Vec<&Phase> = phases
            .iter()
            .filter(|phase| phase.duration >= SLOW_PHASE_THRESHOLD)
            .collect();
        slowest.sort_by(|a, b| b.duration.cmp(&a.duration));
        slowest.truncate(10);
        if !slowest.is_empty() {
            fmt.newline()?;
            fmt.write_bold("slowest phases")?;
            fmt.newline()?;
            let label_width = slowest
                .iter()
                .map(|phase| phase.label.chars().count())
                .max()
                .unwrap_or(0);
            for phase in slowest {
                write_report_line(
                    fmt,
                    None,
                    &format!(
                        "  {:<label_width$}  {:>8}  {} check(s)",
                        phase.label,
                        duration_label(phase.duration),
                        phase.checks,
                        label_width = label_width
                    ),
                )?;
            }
        }
    }

    let ok = checks
        .iter()
        .filter(|check| matches!(check.status, CheckStatus::Ok))
        .count();
    let warnings = checks
        .iter()
        .filter(|check| matches!(check.status, CheckStatus::Warning))
        .count();
    let errors = checks
        .iter()
        .filter(|check| matches!(check.status, CheckStatus::Error))
        .count();
    fmt.newline()?;
    write_report_line(
        fmt,
        None,
        &format!(
            "{ok} ok · {warnings} warnings · {errors} errors · {}",
            duration_label(elapsed)
        ),
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn render_report_plain(
    checks: &[Check],
    timings: &[CheckTiming],
    phases: &[Phase],
    canonical_id: &str,
    version: &str,
    elapsed: Duration,
    verbose: bool,
    width: u16,
) -> String {
    let mut output = Vec::new();
    {
        let mut fmt = Formatter::new(
            &mut output,
            crate::ui::components::OutputMode::Plain,
            ActiveTheme::default(),
            width,
        );
        render_report(
            &mut fmt,
            checks,
            timings,
            phases,
            canonical_id,
            version,
            elapsed,
            verbose,
        )
        .unwrap();
    }
    String::from_utf8(output).expect("doctor report is UTF-8")
}

fn output_checks(
    checks: &[Check],
    cli: &Cli,
    elapsed: Duration,
    cas_root: Option<&Path>,
) -> anyhow::Result<()> {
    output_checks_timed(checks, &[], &[], cli, elapsed, cas_root)
}

fn output_checks_timed(
    checks: &[Check],
    timings: &[CheckTiming],
    phases: &[Phase],
    cli: &Cli,
    elapsed: Duration,
    cas_root: Option<&Path>,
) -> anyhow::Result<()> {
    if cli.json {
        println!(
            "{}",
            serde_json::to_string(&serialize_checks(checks, timings))?
        );
        return Ok(());
    }

    let canonical_id = cas_root
        .and_then(crate::cloud::resolve_canonical_id)
        .unwrap_or_else(|| "<uninitialized>".to_string());
    let mut out = std::io::stdout();
    let mut fmt = Formatter::stdout(&mut out, ActiveTheme::default());
    render_report(
        &mut fmt,
        checks,
        timings,
        phases,
        &canonical_id,
        env!("CARGO_PKG_VERSION"),
        elapsed,
        cli.verbose,
    )?;
    fmt.flush()?;
    Ok(())
}
