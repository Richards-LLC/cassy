//! `cas history` — drive and inspect the structural git-history index
//! (EPIC cas-6212 / cas-7a21).
//!
//! `backfill` runs an indexing pass (full or delta, decided by the watermark);
//! `status` only reads. The split matters: spec §4.2 rule 5 says a freshness
//! check must never index, so `status` never shells out to anything that writes.

use std::path::Path;

use clap::{Args, Subcommand};

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
}

#[derive(Debug, Clone, Args)]
pub struct SearchArgs {
    /// Free text matched against commit subject and body. Optional: a query
    /// with only `--path`/`--since` is a legitimate structural question.
    pub query: Vec<String>,

    /// Only commits touching paths containing this substring
    #[arg(long)]
    pub path: Option<String>,

    /// Only commits touching this symbol (not supported until M3; declared)
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

    /// Ask for provenance (not supported until M5; declared in the response)
    #[arg(long)]
    pub include_provenance: bool,

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
    }
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
        task_id: None,
        session_id: None,
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
    match status.provenance_coverage_pct {
        Some(pct) => println!(
            "provenance: NOT SUPPORTED (M5) — measured high-confidence coverage {pct:.1}%"
        ),
        None => println!("provenance: NOT SUPPORTED (M5) — coverage not measurable here"),
    }
    if let Some(err) = &status.last_error {
        println!("last error: {err}");
    }
    for note in &response.unsupported {
        println!("unsupported: {} — {} (lands in {})", note.feature, note.reason, note.lands_in);
    }

    Ok(())
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
    println!("  lag             {lag} commit(s) behind HEAD");
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
