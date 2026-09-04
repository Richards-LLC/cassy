//! `cas history` — drive and inspect the structural git-history index
//! (EPIC cas-6212 / cas-7a21).
//!
//! `backfill` runs an indexing pass (full or delta, decided by the watermark);
//! `status` only reads. The split matters: spec §4.2 rule 5 says a freshness
//! check must never index, so `status` never shells out to anything that writes.

use std::path::Path;

use clap::{Args, Subcommand};

use crate::history::symbols::{self as symbol_map, DEFAULT_MAP_LIMIT};
use crate::history::{self, WalkMode};

#[derive(Debug, Clone, Subcommand)]
pub enum HistoryCommands {
    /// Index commits into the history tables (full backfill, or a delta from
    /// the watermark when one exists)
    Backfill(BackfillArgs),
    /// Index GitHub issues/PRs/comments and CHANGELOG release sections
    /// (incremental; absent GitHub data is a declared boundary, not a failure)
    Docs(DocsArgs),
    /// Report the watermark, indexed counts and lag without indexing anything
    Status(StatusArgs),
    /// Search indexed commits by text, path and time window
    Search(SearchArgs),
    /// Embed everything still awaiting a vector — code history AND knowledge
    /// pages — now, instead of waiting for the daemon tick that normally does it
    Embed(EmbedArgs),
    /// Map commits to the symbols their changed lines touch (M3)
    Symbols(SymbolsArgs),
    /// Reconstruct missing commit → session links from the populated
    /// provenance edges (spec §5.3). Never overwrites a link the PostToolUse
    /// hook observed directly.
    RepairLinks(RepairLinksArgs),
    /// List the running-binary timeline, or backfill it from daemon records
    Epochs(EpochsArgs),
    /// Answer "is symptom X fixed" against the running-binary timeline
    Verdict(VerdictArgs),
}

#[derive(Debug, Clone, Args)]
pub struct EmbedArgs {
    /// Units to embed in this run (costs ceil(n / 32) requests). Defaults to
    /// the daemon's per-tick budget.
    #[arg(long)]
    pub limit: Option<usize>,

    /// Re-arm every unit the provider previously refused, then drain.
    ///
    /// Quarantined units are not retried on their own — the same payload would
    /// be refused again. Use this once the cause is gone: the provider raised
    /// its cap, or a client upgrade now truncates the oversized text.
    #[arg(long)]
    pub retry_quarantined: bool,

    /// Emit JSON instead of prose
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct EpochsArgs {
    /// Backfill historical epochs from `daemon_instances` before listing
    #[arg(long)]
    pub backfill: bool,

    /// Only epochs starting at or after this bound (14d, 2026-08-01, RFC3339)
    #[arg(long)]
    pub since: Option<String>,

    /// Maximum epochs to list
    #[arg(long, default_value_t = 20)]
    pub limit: usize,

    /// Emit JSON instead of prose
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct RepairLinksArgs {
    /// Commits to examine in this pass
    #[arg(long, default_value_t = crate::history::provenance::REPAIR_BATCH)]
    pub limit: usize,

    /// Keep repairing until a pass finds nothing left to do
    #[arg(long)]
    pub all: bool,
}

#[derive(Debug, Clone, Args)]
pub struct VerdictArgs {
    /// Symptom to look for: a substring of an event type or summary
    pub symptom: Vec<String>,

    /// Commit carrying the fix (full SHA or prefix); resolved to its commit
    /// time, which is a *build* time, never the start of post-fix data
    #[arg(long)]
    pub fix_commit: Option<String>,

    /// The fix's build time, if there is no indexed commit for it
    #[arg(long)]
    pub fix_at: Option<String>,

    /// Post-boundary observations required before absence counts as verified
    #[arg(long, default_value_t = crate::history::epochs::DEFAULT_SAMPLE_THRESHOLD)]
    pub threshold: i64,

    /// Emit JSON instead of prose
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct SearchArgs {
    /// Free text matched against commit subject and body. Optional: a query
    /// with only `--path`/`--since` is a legitimate structural question.
    pub query: Vec<String>,

    /// Only commits touching paths containing this substring
    #[arg(long)]
    pub path: Option<String>,

    /// Only commits touching this exact qualified symbol. Commits whose symbol
    /// mapping is incomplete are returned with their explicit mapping verdict.
    #[arg(long)]
    pub symbol: Option<String>,

    /// Lower bound: 14d, 2w, 6h, 45m, 2026-08-01, or RFC3339
    #[arg(long)]
    pub since: Option<String>,

    /// Upper bound, same formats as --since
    #[arg(long)]
    pub until: Option<String>,

    /// Doc class: commit (supported), issue/pr/changelog (M6; declared)
    #[arg(long)]
    pub kind: Option<String>,

    /// Include merge commits (excluded by default: their message is noise)
    #[arg(long)]
    pub include_merges: bool,

    /// Resolve each commit's task/session edges with their link method and
    /// confidence (spec §5.2)
    #[arg(long)]
    pub include_provenance: bool,

    /// Only commits attributable to this task
    #[arg(long)]
    pub task_id: Option<String>,

    /// Only commits attributable to this session
    #[arg(long)]
    pub session_id: Option<String>,

    /// Maximum commits to return
    #[arg(long, default_value_t = 10)]
    pub limit: usize,

    /// Emit JSON instead of prose
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct DocsArgs {
    /// Index GitHub only (default: both sources)
    #[arg(long)]
    pub github: bool,

    /// Index the CHANGELOG only (default: both sources)
    #[arg(long)]
    pub changelog: bool,

    /// Ignore the GitHub cursor and re-fetch every issue and pull request
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Clone, Args)]
pub struct BackfillArgs {
    /// Discard the watermark and re-walk the whole history
    #[arg(long)]
    pub force: bool,

    /// Report what the pass would do without writing rows
    #[arg(long)]
    pub dry_run: bool,

    /// Skip the symbol-mapping pass that normally follows indexing
    #[arg(long)]
    pub no_symbols: bool,
}

#[derive(Debug, Clone, Args)]
pub struct SymbolsArgs {
    /// Maximum commits to map in this pass
    #[arg(long, default_value_t = DEFAULT_MAP_LIMIT)]
    pub limit: usize,
}

#[derive(Debug, Clone, Args)]
pub struct StatusArgs {
    /// Emit JSON instead of prose
    #[arg(long)]
    pub json: bool,
}

pub fn execute(
    command: &HistoryCommands,
    _cli: &crate::cli::Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    match command {
        HistoryCommands::Backfill(args) => execute_backfill(args, cas_root),
        HistoryCommands::Docs(args) => execute_docs(args, cas_root),
        HistoryCommands::Status(args) => execute_status(args, cas_root),
        HistoryCommands::Search(args) => execute_search(args, cas_root),
        HistoryCommands::Embed(args) => execute_embed(args, cas_root),
        HistoryCommands::Symbols(args) => execute_symbols(args, cas_root),
        HistoryCommands::RepairLinks(args) => execute_repair_links(args, cas_root),
        HistoryCommands::Epochs(args) => execute_epochs(args, cas_root),
        HistoryCommands::Verdict(args) => execute_verdict(args, cas_root),
    }
}

/// Force the drain the daemon runs on a tick (EPIC cas-6212 / cas-db6e).
///
/// Exists for the same reason `cas index code` does: the automatic path is the
/// normal one, and a human who wants the backlog gone *now* should not have to
/// wait out an interval or — the defect M7 removes — reach for `cas cloud sync`
/// and hope the embedding half comes along for the ride.
fn execute_embed(args: &EmbedArgs, cas_root: &Path) -> anyhow::Result<()> {
    let limit = args.limit.unwrap_or(crate::cloud::DRAIN_BATCH);

    let requeued = if args.retry_quarantined {
        use cas_store::HistoryStore;
        let store = cas_store::SqliteHistoryStore::open(cas_root)?;
        store.init()?;
        store.requeue_quarantined_embeddings()?
    } else {
        0
    };

    let report = crate::cloud::drain_all_pending(cas_root, limit)?;
    let (quarantined_commits, quarantined_docs) = {
        use cas_store::HistoryStore;
        cas_store::SqliteHistoryStore::open(cas_root)
            .and_then(|store| store.count_quarantined_embedding())
            .unwrap_or((0, 0))
    };
    let quarantined_total = quarantined_commits + quarantined_docs;

    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "capability_absent": report.capability_absent,
                "embedded": report.embedded(),
                "skipped": report.skipped(),
                "requests": report.requests(),
                "pending_after": report.pending_after(),
                "quarantined_this_run": report.quarantined(),
                "quarantined_total": quarantined_total,
                "requeued": requeued,
                "problems": report.problems(),
            })
        );
        return Ok(());
    }

    if requeued > 0 {
        println!("re-armed {requeued} previously refused unit(s) before draining");
    }

    if report.capability_absent {
        // A boundary of the installation, stated plainly — not an error, and
        // not a silent no-op either.
        println!(
            "no cloud embedding capability configured; nothing was embedded and no vector \
             store was created"
        );
        return Ok(());
    }

    println!(
        "embedded {} unit(s) in {} request(s); {} skipped, {} still awaiting a vector",
        report.embedded(),
        report.requests(),
        report.skipped(),
        report.pending_after(),
    );
    // Quarantine is reported apart from the backlog on purpose: these units are
    // not waiting their turn, they need a decision. Naming the provider's own
    // words is what turns the count into something actionable.
    if quarantined_total > 0 {
        println!(
            "{quarantined_total} unit(s) quarantined — the provider refused them and a retry              would be refused again; re-arm with `cas history embed --retry-quarantined`"
        );
        for (id, message) in report
            .history
            .as_ref()
            .map(|r| r.quarantine_errors.as_slice())
            .unwrap_or_default()
        {
            println!("  - {id}: {message}");
        }
    }
    for problem in report.problems() {
        println!("  ! {problem}");
    }
    Ok(())
}

/// `cas history repair-links` (EPIC cas-6212 / cas-519f, spec §5.3).
fn execute_repair_links(args: &RepairLinksArgs, cas_root: &Path) -> anyhow::Result<()> {
    let repo_root = history::repo_root_for(cas_root)?;
    let started = std::time::Instant::now();

    let mut total = history::provenance::RepairOutcome::default();
    // The work list is "indexed commits with no link", and a commit no edge can
    // resolve never leaves it. So `--all` walks the list with an offset that
    // advances by however many rows STAYED unlinked, rather than restarting
    // from the top and stalling on the unresolvable head.
    let mut offset = 0usize;
    loop {
        let pass = history::provenance::repair_commit_links_from(
            cas_root, &repo_root, args.limit, offset,
        )?;
        total.examined += pass.examined;
        total.written += pass.written;
        total.no_session_edge += pass.no_session_edge;
        total.skipped_ambiguous += pass.skipped_ambiguous;
        total.already_present += pass.already_present;
        offset += pass.examined - pass.written;
        // A short pass means the end of the list, not a barren stretch of it.
        if !args.all || pass.is_noop() || pass.examined < args.limit.max(1) {
            break;
        }
    }

    println!(
        "examined {} unlinked commit(s); reconstructed {} link(s) in {:.1}s",
        total.examined,
        total.written,
        started.elapsed().as_secs_f64()
    );
    // Every skip is named. A pass that reports only what it wrote reads as
    // "the rest were fine", when in fact most commits simply have no edge that
    // names a session — which is the measured state of this corpus, not a bug.
    println!(
        "  no session-bearing edge  {}\n             ambiguous prefix (skipped, never guessed)  {}\n             already linked (raced a direct observation)  {}",
        total.no_session_edge, total.skipped_ambiguous, total.already_present
    );
    Ok(())
}

/// `cas history epochs` — the running-binary timeline (spec §9).
fn execute_epochs(args: &EpochsArgs, cas_root: &Path) -> anyhow::Result<()> {
    use cas_store::{HistoryStore, SqliteHistoryStore};

    let store = SqliteHistoryStore::open(cas_root)?;
    let backfill = if args.backfill {
        Some(store.backfill_epochs_from_daemons()?)
    } else {
        None
    };

    let since = args
        .since
        .as_deref()
        .map(history::search::parse_time_bound)
        .transpose()?;
    let epochs = store.list_epochs(since.as_deref(), args.limit)?;

    if args.json {
        let payload = serde_json::json!({
            "backfill": backfill.as_ref().map(|b| serde_json::json!({
                "source_available": b.source_available,
                "scanned": b.scanned,
                "inserted": b.inserted,
                "already_present": b.already_present,
            })),
            "epochs": epochs,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    if let Some(b) = &backfill {
        if b.source_available {
            println!(
                "backfill: {} daemon record(s) scanned, {} epoch(s) added, {} already present",
                b.scanned, b.inserted, b.already_present
            );
        } else {
            // "No daemon records" and "no daemon table" are different facts.
            println!("backfill: no daemon_instances table in this database — nothing to backfill");
        }
    }

    if epochs.is_empty() {
        println!("no binary epochs recorded — start `cas serve`, or run with --backfill");
        return Ok(());
    }
    for e in &epochs {
        let binary_identity = if e.exe_deleted {
            "mtime unknown [running exe replaced/deleted]"
        } else {
            e.binary_mtime.as_deref().unwrap_or("mtime unknown")
        };
        println!(
            "{}  {}  pid {}  {}  binary {}",
            e.started_at,
            e.ended_at.as_deref().unwrap_or("(never seen alive again)"),
            e.pid.map(|p| p.to_string()).unwrap_or_else(|| "?".into()),
            e.version.as_deref().unwrap_or("version unknown"),
            binary_identity,
        );
    }
    Ok(())
}

/// `cas history verdict` — the three-valued is-it-fixed answer (spec §9, §12 Q6).
fn execute_verdict(args: &VerdictArgs, cas_root: &Path) -> anyhow::Result<()> {
    let request = crate::history::epochs::VerdictRequest {
        symptom: args.symptom.join(" "),
        fix_commit: args.fix_commit.clone(),
        fix_at: args.fix_at.clone(),
        threshold: args.threshold,
    };
    let response = crate::history::epochs::run(cas_root, &request)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }

    let a = &response.assessment;
    println!("verdict: {}", response.verdict);
    println!("  {}", a.rationale);
    match (&a.boundary.fix_started_running, &a.boundary.clean_post_from) {
        (Some(fix), Some(clean)) => {
            println!("  fix first observed running: {fix}");
            println!(
                "  clean-post from:            {clean} ({} non-fixed/unknown daemon(s) overlapped)",
                a.boundary.overlapping_nonfixed_epochs
            );
        }
        _ => println!("  no epoch has been observed running the fixed binary"),
    }
    println!(
        "  clean-post: {} match(es) in {} observation(s) (threshold {})",
        a.clean_post_matches, a.clean_post_sample, a.threshold
    );
    println!(
        "  mixed:      {} match(es) in {} observation(s) — excluded from the verdict",
        a.mixed_matches, a.mixed_sample
    );
    println!(
        "  epochs: {} recorded, {} considered, {} with unknown binary ({} stale/deleted executable)",
        response.epochs_recorded,
        a.boundary.epochs_considered,
        a.boundary.epochs_without_binary_identity,
        a.boundary.epochs_with_stale_executable_identity
    );
    Ok(())
}

fn execute_search(args: &SearchArgs, cas_root: &Path) -> anyhow::Result<()> {
    let query = args.query.join(" ");
    let request = history::search::HistorySearchRequest {
        query: (!query.trim().is_empty()).then_some(query),
        path: args.path.clone(),
        symbol: args.symbol.clone(),
        since: args.since.clone(),
        until: args.until.clone(),
        kind: args.kind.clone(),
        task_id: args.task_id.clone(),
        session_id: args.session_id.clone(),
        limit: args.limit,
        include_provenance: args.include_provenance,
        include_merges: args.include_merges,
    };

    // Same entry point the MCP surface uses. Two renderings, one answer.
    let response = history::search::run(cas_root, &request)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }

    let status = &response.index_status;
    if response.results.is_empty() {
        // "Nothing matched" and "nothing is indexed" look identical in an empty
        // list, so the second one says so out loud (spec §10.1).
        if status.indexed_commits == 0 {
            println!(
                "no commits are indexed for {} — run `cas history backfill`",
                response.repository
            );
        } else if let Some(n) = response.filters.identity_filter_matched.filter(|n| *n > 0) {
            // The identity filter DID resolve; something else emptied the
            // answer. Saying "no commits matched" here would report the task or
            // session as having shipped nothing, which is a different and
            // wrong claim. The usual culprit is the default merge exclusion.
            println!(
                "the task/session filter resolved to {n} commit(s), but no result survived the \
                 other filters{}",
                if response.filters.include_merges {
                    ""
                } else {
                    " — merge commits are excluded by default; retry with --include-merges"
                }
            );
        } else {
            println!("no commits matched");
        }
    }

    for hit in &response.results {
        println!(
            "{}  {}  {}",
            hit.short_sha,
            &hit.committed_at.chars().take(10).collect::<String>(),
            hit.subject
        );
        for file in hit.files.iter().take(5) {
            let churn = match (file.insertions, file.deletions) {
                (Some(i), Some(d)) => format!(" (+{i}/-{d})"),
                // Binary files carry no line counts; saying so beats printing
                // "+0/-0", which reads as "nothing changed".
                _ => " (binary)".to_string(),
            };
            println!("    {} {}{}", file.change_type, file.file_path, churn);
        }
        if file_overflow(hit.files.len(), 5) > 0 {
            println!("    … {} more file(s)", file_overflow(hit.files.len(), 5));
        }
        print_provenance(hit);
    }

    if !response.co_changed_files.is_empty() {
        println!("\nusually changes alongside:");
        for co in &response.co_changed_files {
            println!("  {:>4}×  {}", co.commits_together, co.file_path);
        }
    }

    println!(
        "\nindex: {} of {} commit(s), lag {} commit(s), backfill {}",
        status.indexed_commits,
        status.repo_commits,
        status
            .lag_commits
            .map(|l| l.to_string())
            .unwrap_or_else(|| "unknown".into()),
        if status.backfill_complete {
            "complete"
        } else {
            "incomplete"
        }
    );
    match (status.provenance_coverage_pct, status.provenance_any_coverage_pct) {
        // Both numbers, always. A single figure cannot distinguish an exact
        // commit→task edge from a substring coincidence, and spec §10.1 asks
        // for the split precisely so the debt stays visible.
        (Some(high), Some(any)) => println!(
            "provenance: supported — {high:.1}% of indexed commits on the exact edge, \
             {any:.1}% on any populated edge"
        ),
        (Some(high), None) => println!("provenance: supported — {high:.1}% high-confidence"),
        _ => println!("provenance: supported — coverage not measurable here"),
    }
    if let Some(err) = &status.last_error {
        println!("last error: {err}");
    }
    for note in &response.unsupported {
        println!("unsupported: {} — {} (lands in {})", note.feature, note.reason, note.lands_in);
    }

    Ok(())
}

/// Render one commit's provenance edges, or the stated reason it has none.
///
/// The reason is printed, not swallowed: §6.4 Q3 requires an unlinked commit to
/// appear in the answer, and a blank where the provenance should be is
/// indistinguishable from a rendering bug.
fn print_provenance(hit: &history::search::HistoryHit) {
    let Some(edges) = &hit.provenance else {
        return;
    };
    if edges.is_empty() {
        if let Some(reason) = &hit.provenance_reason {
            println!("    provenance: none — {reason}");
        }
        return;
    }
    for edge in edges {
        let who = [
            edge.task_id.as_deref().map(|t| format!("task {t}")),
            edge.session_id.as_deref().map(|s| format!("session {s}")),
            edge.agent_id.as_deref().map(|a| format!("agent {a}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
        println!(
            "    provenance: {} ({}) {}{}",
            edge.link_method,
            edge.confidence,
            if who.is_empty() { "—" } else { &who },
            if edge.ambiguous {
                format!(
                    " ⚠ prefix {} is ambiguous across {} indexed commit(s): {}",
                    edge.matched_prefix.as_deref().unwrap_or("?"),
                    edge.ambiguous_candidates.len(),
                    edge.ambiguous_candidates
                        .iter()
                        .map(|s| s.chars().take(8).collect::<String>())
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            } else {
                String::new()
            }
        );
    }
}

fn file_overflow(total: usize, shown: usize) -> usize {
    total.saturating_sub(shown)
}

/// Which sources a `cas history docs` invocation asked for. Neither flag means
/// both — the common case is "index whatever is available".
fn requested_sources(args: &DocsArgs) -> (bool, bool) {
    match (args.github, args.changelog) {
        (false, false) => (true, true),
        (github, changelog) => (github, changelog),
    }
}

fn execute_docs(args: &DocsArgs, cas_root: &Path) -> anyhow::Result<()> {
    let repo_root = history::repo_root_for(cas_root)?;
    let config = crate::config::Config::load(cas_root).unwrap_or_default();
    let repo = config
        .issues
        .as_ref()
        .and_then(|i| i.repo.as_deref())
        .map(str::trim)
        .filter(|r| !r.is_empty());

    let (want_github, want_changelog) = requested_sources(args);
    let started = std::time::Instant::now();
    let outcome = history::run_docs_pass(
        cas_root,
        &repo_root,
        repo,
        args.force,
        want_github,
        want_changelog,
    );

    if let Some(github) = &outcome.github {
        match github {
            Ok(fetch) => {
                println!(
                    "github {}: {} issue(s), {} pull request(s), {} comment(s) over {} page(s)",
                    if fetch.is_backfill() { "backfill" } else { "delta" },
                    fetch.issues,
                    fetch.pull_requests,
                    fetch.comments,
                    fetch.pages,
                );
                if let Some(cursor) = &fetch.cursor {
                    println!("                cursor now {cursor}");
                }
                // Both of these mean the index is knowingly incomplete. Saying
                // so is the whole of spec §10.1: a partial pass that prints
                // only its successes reads as a complete one.
                if fetch.comments_truncated > 0 {
                    println!(
                        "                {} thread(s) had more comments than one page; \
                         those threads are indexed partially",
                        fetch.comments_truncated
                    );
                }
                if fetch.page_limit_hit {
                    println!(
                        "                page limit reached — the next pass resumes from the cursor"
                    );
                }
            }
            Err(boundary) => println!("github unavailable: {boundary}"),
        }
    }

    if want_changelog {
        match (&outcome.changelog_sections, &outcome.changelog_error) {
            (_, Some(error)) => println!("changelog failed: {error}"),
            (Some(count), _) => println!("changelog: {count} release section(s)"),
            (None, None) => println!("changelog unavailable: no CHANGELOG in the repository root"),
        }
    }

    println!("elapsed {:.1}s", started.elapsed().as_secs_f64());

    // A boundary is not an error exit: the point of §10.2 is that an absent
    // source is a reported state, and `cas history docs` on a machine with no
    // `gh` must not look like a broken command.
    Ok(())
}

fn execute_backfill(args: &BackfillArgs, cas_root: &Path) -> anyhow::Result<()> {
    let repo_root = history::repo_root_for(cas_root)?;

    if args.dry_run {
        let s = history::status(cas_root, &repo_root)?;
        let pending = match s.lag_commits {
            Some(lag) if s.state.as_ref().is_some_and(|st| st.backfill_complete) => lag,
            _ => s.repo_commits,
        };
        println!(
            "dry run: would index {pending} commit(s) ({} already indexed of {} reachable)",
            s.indexed_commits, s.repo_commits
        );
        return Ok(());
    }

    if args.force {
        let store = cas_store::SqliteHistoryStore::open(cas_root)?;
        cas_store::HistoryStore::reset_watermark(
            &store,
            &history::repository_id(&repo_root),
            cas_store::SOURCE_GIT,
        )?;
        println!("watermark cleared; re-walking full history");
    }

    let started = std::time::Instant::now();
    let outcome = history::run_index_pass(cas_root, &repo_root)?;
    let elapsed = started.elapsed();

    if outcome.watermark_reset {
        println!(
            "watermark was not an ancestor of HEAD (rebase/force-push); ran a full re-backfill"
        );
    }

    // The daemon runs the spine repair on the same tick as the index pass
    // (spec §5.3), so the CLI does too — otherwise `cas history backfill` and
    // the daemon leave the database in two different states and only one of
    // them is the one anybody tests.
    match history::provenance::repair_commit_links(
        cas_root,
        &repo_root,
        history::provenance::REPAIR_BATCH,
    ) {
        Ok(repair) if repair.written > 0 => println!(
            "provenance: reconstructed {} commit link(s) from {} unlinked commit(s) examined",
            repair.written, repair.examined
        ),
        Ok(_) => {}
        // A repair failure must not make the index pass look failed: indexing
        // is the load-bearing half and the query surface works without a spine.
        Err(e) => println!("provenance repair skipped: {e}"),
    }

    match outcome.mode {
        WalkMode::UpToDate => println!("history index already current at {}", outcome.head_sha),
        _ => println!(
            "{}: {} commit(s), {} file change(s) in {} chunk(s) — {:.1}s",
            outcome.mode.as_str(),
            outcome.commits_indexed,
            outcome.files_indexed,
            outcome.chunks,
            elapsed.as_secs_f64()
        ),
    }

    // Symbol mapping runs by default rather than behind a flag, for the same
    // reason M2's index was flipped default-on: a stage nobody opts into is a
    // stage that never runs, and `history_commit_symbols` would then read as
    // "these commits touched no symbols" forever.
    if !args.no_symbols {
        report_symbol_pass(cas_root, &repo_root, DEFAULT_MAP_LIMIT)?;
    }

    Ok(())
}

fn execute_symbols(args: &SymbolsArgs, cas_root: &Path) -> anyhow::Result<()> {
    let repo_root = history::repo_root_for(cas_root)?;
    report_symbol_pass(cas_root, &repo_root, args.limit)
}

fn report_symbol_pass(cas_root: &Path, repo_root: &Path, limit: usize) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let outcome = symbol_map::map_symbols(cas_root, repo_root, limit)?;
    let elapsed = started.elapsed();

    if outcome.commits_considered == 0 {
        println!("symbol mapping: nothing to map");
        return Ok(());
    }

    let mut verdicts: Vec<(&str, usize)> = outcome
        .verdicts
        .iter()
        .map(|(name, count)| (*name, *count))
        .collect();
    verdicts.sort();
    let breakdown = verdicts
        .iter()
        .map(|(name, count)| format!("{name} {count}"))
        .collect::<Vec<_>>()
        .join(", ");

    println!(
        "symbol mapping: {} commit(s), {} symbol row(s) in {:.1}s — {breakdown}",
        outcome.commits_considered,
        outcome.symbol_rows,
        elapsed.as_secs_f64()
    );
    let absent = outcome.count(cas_store::SymbolMapping::Absent);
    if absent > 0 {
        println!(
            "  {absent} commit(s) recorded symbol_mapping=absent: the symbol index has no data"
        );
        println!(
            "  for the files they touched. Run `cas index code`, then re-run — absent is retried."
        );
    }

    Ok(())
}

fn execute_status(args: &StatusArgs, cas_root: &Path) -> anyhow::Result<()> {
    let repo_root = history::repo_root_for(cas_root)?;
    let s = history::status(cas_root, &repo_root)?;

    let watermark = s
        .state
        .as_ref()
        .and_then(|st| st.last_indexed_sha.clone())
        .unwrap_or_else(|| "none".to_string());
    let backfill_complete = s.state.as_ref().is_some_and(|st| st.backfill_complete);
    // "unknown" rather than "0": a missing or unreachable watermark means the
    // lag is not known, and printing 0 would read as fresh (spec §10.1).
    let lag = s
        .lag_commits
        .map(|l| l.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let lag_seconds = s.lag_age_seconds_at(chrono::Utc::now());

    if args.json {
        let payload = serde_json::json!({
            "repository": s.repository,
            "head_sha": s.head_sha,
            "watermark": s.state.as_ref().and_then(|st| st.last_indexed_sha.clone()),
            "watermark_is_ancestor": s.watermark_is_ancestor,
            "backfill_complete": backfill_complete,
            "indexed_commits": s.indexed_commits,
            "indexed_commit_file_pairs": s.indexed_pairs,
            "repo_commits": s.repo_commits,
            "lag_commits": s.lag_commits,
            "lag_seconds": lag_seconds,
            "current": s.is_current(),
            "last_indexed_at": s.state.as_ref().and_then(|st| st.last_indexed_at.clone()),
            "last_attempt_at": s.state.as_ref().and_then(|st| st.last_attempt_at.clone()),
            "last_error": s.state.as_ref().and_then(|st| st.last_error.clone()),
            "docs": {
                "counts": s.doc_counts.iter()
                    .map(|(kind, count)| (kind.clone(), serde_json::json!(count)))
                    .collect::<serde_json::Map<String, serde_json::Value>>(),
                "pending_embedding": s.docs_pending_embedding,
                "github": source_json(s.github_state.as_ref()),
                "changelog": source_json(s.changelog_state.as_ref()),
            },
            "symbol_mapping": s.symbol_mapping
                .iter()
                .map(|(name, count)| (name.clone(), serde_json::json!(count)))
                .collect::<serde_json::Map<String, serde_json::Value>>(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!("Code-history index — {}", s.repository);
    println!("  HEAD            {}", s.head_sha);
    println!("  watermark       {watermark}");
    if s.state.is_some() && !s.watermark_is_ancestor {
        println!("                  (not an ancestor of HEAD — next pass re-backfills)");
    }
    println!(
        "  commits         {} indexed of {} reachable",
        s.indexed_commits, s.repo_commits
    );
    println!("  file changes    {}", s.indexed_pairs);
    println!(
        "  lag             {lag} commit(s) behind HEAD ({})",
        lag_seconds
            .map(|seconds| format!("{seconds}s old"))
            .unwrap_or_else(|| "age unknown".to_string())
    );
    println!(
        "  backfill        {}",
        if backfill_complete {
            "complete"
        } else {
            "incomplete"
        }
    );
    if let Some(state) = &s.state {
        if let Some(at) = &state.last_indexed_at {
            println!("  last indexed    {at}");
        }
        if let Some(at) = &state.last_attempt_at {
            println!("  last attempt    {at}");
        }
        if let Some(err) = &state.last_error {
            println!("  last error      {err}");
        }
    } else {
        println!("  status          never indexed — run `cas history backfill`");
    }

    print_docs(&s);
    if s.symbol_mapping.is_empty() {
        println!("  symbol mapping  none recorded");
    } else {
        let breakdown = s
            .symbol_mapping
            .iter()
            .map(|(name, count)| format!("{name} {count}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("  symbol mapping  {breakdown}");
    }

    Ok(())
}

fn source_json(state: Option<&cas_store::HistoryIndexState>) -> serde_json::Value {
    match state {
        // `null` rather than an object of nulls: "this source has never run" and
        // "it ran and found nothing" must stay distinguishable in the JSON too.
        None => serde_json::Value::Null,
        Some(state) => serde_json::json!({
            "cursor": state.last_indexed_at,
            "last_attempt_at": state.last_attempt_at,
            "last_error": state.last_error,
            "items_indexed": state.items_indexed,
            "backfill_complete": state.backfill_complete,
        }),
    }
}

/// The doc half of `cas history status` (M6).
fn print_docs(s: &history::HistoryStatus) {
    println!("  docs");
    if s.doc_counts.is_empty() {
        println!("    indexed       none — run `cas history docs`");
    } else {
        let rendered = s
            .doc_counts
            .iter()
            .map(|(kind, count)| format!("{count} {kind}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("    indexed       {rendered}");
        println!("    pending embed {}", s.docs_pending_embedding);
    }

    for (label, state) in [
        ("github", s.github_state.as_ref()),
        ("changelog", s.changelog_state.as_ref()),
    ] {
        match state {
            None => println!("    {label:<13} never run"),
            Some(state) => {
                let cursor = state.last_indexed_at.as_deref().unwrap_or("none");
                println!("    {label:<13} cursor {cursor}, {} item(s)", state.items_indexed);
                // The declared boundary, printed where an operator will see it
                // rather than only in the JSON (spec §10.2).
                if let Some(error) = &state.last_error {
                    println!("                  unavailable: {error}");
                }
            }
        }
    }
}
