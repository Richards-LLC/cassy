//! Backlog hygiene (cas-e8d1, GH #1233): open work the record says is done.
//!
//! The 2026-10-16 cleanup found about 60 stale open tasks, about 25 of them
//! already delivered on the default branch, and 11 epics whose children had
//! all closed. Nothing flagged any of them. This module finds these kinds:
//!
//! 1. an open task whose delivery commit is on the default branch: a
//!    non-merge commit whose subject names the task;
//! 2. an open epic whose children have all closed or been cancelled;
//! 3. an open task idle for [`IDLE_DAYS`] or more;
//! 4. an open task whose cited code is all gone (cas-d9038): every file path
//!    or qualified symbol it names is absent from the default branch, and git
//!    history dates each removal. Citations git never saw (planned code, typos)
//!    do not count either way.
//! 5. an open task whose latest named effect is a revert, or whose changed
//!    paths have been completely restored on the default branch.
//!
//! Each finding carries its evidence. Kinds 1 and 2 also carry the one call
//! that closes them through the ordinary close gates; nothing here closes,
//! reopens or edits a task. `cas doctor` and a supervisor's SessionStart
//! render the same report.

use std::path::Path;

use chrono::{DateTime, Utc};

use crate::types::{Dependency, DependencyType, Task, TaskType};

/// A task untouched this long is reported as idle.
pub(crate) const IDLE_DAYS: i64 = 14;

/// Commits read from the default branch, newest first. Bounds the scan on
/// very long histories; the `--since` floor normally ends it much sooner.
const MAX_COMMITS: usize = 20_000;

/// One non-merge commit on the default branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitRecord {
    pub sha: String,
    pub committed_at: DateTime<Utc>,
    pub subject: String,
}

/// Kind 1: an open task delivered on the default branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeliveredOpen {
    pub task_id: String,
    pub title: String,
    pub status: String,
    pub commit: CommitRecord,
    /// Further subject-matching commits beyond `commit`.
    pub more_commits: usize,
}

/// Kind 2: an open epic whose children have all ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FinishedEpic {
    pub epic_id: String,
    pub title: String,
    pub children: usize,
    pub last_closed_at: Option<DateTime<Utc>>,
}

/// Kind 3: an open task untouched for [`IDLE_DAYS`] or more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdleOpen {
    pub task_id: String,
    pub title: String,
    pub status: String,
    pub assignee: Option<String>,
    pub idle_days: i64,
}

/// What a cited piece of code is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CitationKind {
    File,
    Symbol,
}

/// One cited path or symbol that the default branch no longer has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GoneCitation {
    pub kind: CitationKind,
    /// The path, or the symbol as the task wrote it.
    pub cited: String,
    /// The commit that removed it, short form.
    pub removed_in: String,
    /// That commit's date, `YYYY-MM-DD`.
    pub removed_on: String,
}

/// Kind 4: an open task whose every dated citation is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StaleCitations {
    pub task_id: String,
    pub title: String,
    pub status: String,
    pub gone: Vec<GoneCitation>,
}

/// How much kind 4 may spend. Tree checks are one `ls-tree` and one batched
/// `grep`; each symbol's removal date is a `git log -S` bounded to the task's
/// lifetime, and those are capped and timed.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CitationBudget {
    /// Open tasks whose text is read for citations.
    pub max_tasks: usize,
    /// `git log -S` lookups for removed symbols.
    pub max_symbol_lookups: usize,
    /// Wall time for all removal lookups.
    pub lookup_deadline: std::time::Duration,
}

impl CitationBudget {
    /// `cas doctor`: thorough.
    pub(crate) const DOCTOR: Self = Self {
        max_tasks: 500,
        max_symbol_lookups: 40,
        lookup_deadline: std::time::Duration::from_secs(10),
    };
    /// A supervisor's SessionStart: a fraction of the hook's budget.
    pub(crate) const SESSION_START: Self = Self {
        max_tasks: 200,
        max_symbol_lookups: 6,
        lookup_deadline: std::time::Duration::from_millis(1_500),
    };
}

/// What one scan found.
#[derive(Debug, Clone, Default)]
pub(crate) struct BacklogReport {
    /// The ref kind 1 was measured against, e.g. `origin/main`.
    pub default_ref: Option<String>,
    /// Why kind 1 could not be measured, when it could not.
    pub delivery_unknown: Option<String>,
    pub delivered: Vec<DeliveredOpen>,
    /// Latest task effect was a revert, or its changed paths are restored.
    /// These findings never carry a close call.
    pub reverted: Vec<DeliveredOpen>,
    pub finished_epics: Vec<FinishedEpic>,
    pub idle: Vec<IdleOpen>,
    /// Kind 4 (cas-d9038).
    pub stale_citations: Vec<StaleCitations>,
    /// Open tasks kind 4 could not finish within its budget.
    pub citations_unchecked: usize,
}

impl BacklogReport {
    pub(crate) fn is_clean(&self) -> bool {
        self.delivered.is_empty()
            && self.reverted.is_empty()
            && self.finished_epics.is_empty()
            && self.idle.is_empty()
            && self.stale_citations.is_empty()
            && self.delivery_unknown.is_none()
    }
}

/// File paths (with at least one directory) and qualified symbols cited in
/// `text`, each once, in order of first mention. A `:line` suffix is dropped;
/// URLs and absolute paths are not citations of the tree.
pub(crate) fn extract_citations(text: &str) -> (Vec<String>, Vec<String>) {
    use std::sync::LazyLock;
    // A path starts after a delimiter, so the middle of a URL or an absolute
    // path never matches; it needs a directory and an extension.
    static FILE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r#"(?:^|[\s`'"(\[<,;])((?:[A-Za-z0-9_.-]+/)+[A-Za-z0-9_.-]+\.[A-Za-z0-9]{1,6})(?::\d+(?:-\d+)?)?"#,
        )
        .expect("file citation pattern")
    });
    static SYMBOL: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?:^|[^A-Za-z0-9_:])((?:[A-Za-z_][A-Za-z0-9_]*::)+[A-Za-z_][A-Za-z0-9_]*)")
            .expect("symbol citation pattern")
    });
    let mut files: Vec<String> = Vec::new();
    for capture in FILE.captures_iter(text) {
        let path = capture[1].trim_start_matches("./").to_string();
        if !path.contains("..") && !files.contains(&path) {
            files.push(path);
        }
    }
    let mut symbols: Vec<String> = Vec::new();
    for capture in SYMBOL.captures_iter(text) {
        let symbol = capture[1].to_string();
        if symbol_is_distinctive(&symbol) && !symbols.contains(&symbol) {
            symbols.push(symbol);
        }
    }
    (files, symbols)
}

/// The identifier a qualified symbol is looked up by: its last segment.
fn symbol_identifier(symbol: &str) -> &str {
    symbol.rsplit("::").next().unwrap_or(symbol)
}

/// Short or plain last segments (`fmt`, `User`, `new`) occur everywhere, so
/// their presence proves nothing; only distinctive ones are tested.
fn symbol_is_distinctive(symbol: &str) -> bool {
    let identifier = symbol_identifier(symbol);
    identifier.contains('_') || identifier.chars().count() >= 8
}

fn git_stdout(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|error| format!("cannot run git {}: {error}", args.first().unwrap_or(&"")))?;
    // `git grep` exits 1 when nothing matched.
    if output.status.success() || (args.first() == Some(&"grep") && output.status.code() == Some(1)) {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// `(short sha, date)` of the newest commit on `reference` that `args`
/// select, or `None` when git saw no such commit.
fn dated_commit(repo: &Path, reference: &str, args: &[&str]) -> Option<(String, String)> {
    let mut full = vec!["log", "-1", "--format=%h%x1f%cs", reference];
    full.extend_from_slice(args);
    let output = git_stdout(repo, &full).ok()?;
    let (sha, date) = output.trim().split_once('\u{1f}')?;
    (!sha.is_empty()).then(|| (sha.to_string(), date.to_string()))
}

/// Kind 4 against `reference` in `repo`.
///
/// A task counts when it cites code, none of what it cites is on
/// `reference`, and git dates at least one removal: a file by its deleting
/// commit, a symbol by the newest commit that changed how often its
/// identifier occurs (`git log -S`, bounded to the task's lifetime). A
/// citation git never saw, such as code a task plans to write, is set aside
/// rather than called gone. Returns the findings and how many open tasks the
/// budget left unchecked.
pub(crate) fn stale_citations(
    repo: &Path,
    reference: &str,
    tasks: &[Task],
    budget: CitationBudget,
) -> (Vec<StaleCitations>, usize) {
    let open: Vec<&Task> = tasks
        .iter()
        .filter(|task| task.is_open() && task.task_type != TaskType::Epic)
        .collect();
    let mut unchecked = open.len().saturating_sub(budget.max_tasks);
    let cited: Vec<(&Task, Vec<String>, Vec<String>)> = open
        .into_iter()
        .take(budget.max_tasks)
        .filter_map(|task| {
            let (files, symbols) = extract_citations(&format!(
                "{}\n{}\n{}",
                task.title, task.description, task.notes
            ));
            (!files.is_empty() || !symbols.is_empty()).then_some((task, files, symbols))
        })
        .collect();
    if cited.is_empty() {
        return (Vec::new(), unchecked);
    }
    let Ok(tree) = git_stdout(repo, &["ls-tree", "-r", "--name-only", reference]) else {
        return (Vec::new(), unchecked + cited.len());
    };
    let tree: std::collections::HashSet<&str> = tree.lines().collect();
    let mut identifiers: Vec<&str> = cited
        .iter()
        .flat_map(|(_, _, symbols)| symbols.iter().map(|symbol| symbol_identifier(symbol)))
        .collect();
    identifiers.sort_unstable();
    identifiers.dedup();
    let present: std::collections::HashSet<String> = if identifiers.is_empty() {
        Default::default()
    } else {
        let mut args = vec!["grep", "-o", "-h", "-w", "-F"];
        for identifier in &identifiers {
            args.push("-e");
            args.push(identifier);
        }
        args.push(reference);
        args.push("--");
        match git_stdout(repo, &args) {
            Ok(output) => output.lines().map(str::to_string).collect(),
            Err(_) => return (Vec::new(), unchecked + cited.len()),
        }
    };
    let started = std::time::Instant::now();
    let mut lookups = 0usize;
    let mut found = Vec::new();
    'tasks: for (task, files, symbols) in cited {
        let live = files.iter().any(|file| tree.contains(file.as_str()))
            || symbols.iter().any(|symbol| present.contains(symbol_identifier(symbol)));
        if live {
            continue;
        }
        let mut gone = Vec::new();
        for file in &files {
            if let Some((sha, date)) =
                dated_commit(repo, reference, &["--diff-filter=D", "--", file.as_str()])
            {
                gone.push(GoneCitation {
                    kind: CitationKind::File,
                    cited: file.clone(),
                    removed_in: sha,
                    removed_on: date,
                });
            }
        }
        let since = format!(
            "--since={}",
            (task.created_at - chrono::Duration::days(30)).to_rfc3339()
        );
        for symbol in &symbols {
            if lookups >= budget.max_symbol_lookups || started.elapsed() >= budget.lookup_deadline {
                unchecked += 1;
                continue 'tasks;
            }
            lookups += 1;
            let pickaxe = format!("-S{}", symbol_identifier(symbol));
            if let Some((sha, date)) = dated_commit(repo, reference, &[&pickaxe, &since]) {
                gone.push(GoneCitation {
                    kind: CitationKind::Symbol,
                    cited: symbol.clone(),
                    removed_in: sha,
                    removed_on: date,
                });
            }
        }
        if !gone.is_empty() {
            found.push(StaleCitations {
                task_id: task.id.clone(),
                title: task.title.clone(),
                status: task.status.to_string(),
                gone,
            });
        }
    }
    found.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    (found, unchecked)
}

/// One kind-4 line: the task and each removal, with a call to review.
pub(crate) fn stale_citation_line(finding: &StaleCitations) -> String {
    const SHOWN: usize = 3;
    let mut removals: Vec<String> = finding
        .gone
        .iter()
        .take(SHOWN)
        .map(|gone| {
            format!(
                "{} removed in {} ({})",
                clip(&gone.cited, 40),
                gone.removed_in,
                gone.removed_on
            )
        })
        .collect();
    if finding.gone.len() > SHOWN {
        removals.push(format!("+{} more", finding.gone.len() - SHOWN));
    }
    format!(
        "{} ({}) \"{}\": cites only removed code: {}. Worth a review; nothing closes it.",
        finding.task_id,
        finding.status,
        clip(&finding.title, 50),
        removals.join("; "),
    )
}

/// Kind 1 over `commits` (the default branch's non-merge history, newest
/// first). Only a commit SUBJECT that names the task as a whole token counts:
/// workers commit `fix(cas-1234): …`, while a body that mentions a task as
/// context does not deliver it. Epics are kind 2's.
pub(crate) fn delivered_open(tasks: &[Task], commits: &[CommitRecord]) -> Vec<DeliveredOpen> {
    use crate::mcp::tools::core::task::lifecycle::close_ops::message_references_task;
    let mut found: Vec<DeliveredOpen> = tasks
        .iter()
        .filter(|task| task.is_open() && task.task_type != TaskType::Epic)
        .filter_map(|task| {
            let mut matching = commits
                .iter()
                .filter(|commit| message_references_task(&commit.subject, &task.id));
            let commit = matching.next()?.clone();
            // Never fall back to an older delivery when the latest effect
            // naming this task is a revert. A later normal delivery can count.
            if is_revert(&commit.subject) {
                return None;
            }
            Some(DeliveredOpen {
                task_id: task.id.clone(),
                title: task.title.clone(),
                status: task.status.to_string(),
                commit,
                more_commits: matching.count(),
            })
        })
        .collect();
    found.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    found
}

fn is_revert(subject: &str) -> bool {
    subject.starts_with("revert(")
        || subject.starts_with("revert:")
        || subject.starts_with("Revert \"")
}

/// Separate explicit reverts before checking positive delivery candidates.
/// A subject alone cannot establish that a revert-of-a-revert re-delivered
/// the work, so it also requires review rather than an automatic close hint.
fn reverted_open(tasks: &[Task], commits: &[CommitRecord]) -> Vec<DeliveredOpen> {
    use crate::mcp::tools::core::task::lifecycle::close_ops::message_references_task;
    tasks
        .iter()
        .filter(|task| task.is_open() && task.task_type != TaskType::Epic)
        .filter_map(|task| {
            let commit = commits.iter().find(|c| message_references_task(&c.subject, &task.id))?;
            is_revert(&commit.subject).then(|| DeliveredOpen {
                task_id: task.id.clone(),
                title: task.title.clone(),
                status: task.status.to_string(),
                commit: commit.clone(),
                more_commits: 0,
            })
        })
        .collect()
}

type TreeEntries = std::collections::HashMap<Vec<u8>, (String, String)>;

fn git_output(repo: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| format!("cannot inspect delivery tree: {e}"))?;
    if !output.status.success() {
        return Err("cannot inspect delivery tree (missing history or Git failure)".into());
    }
    Ok(output.stdout)
}

/// Exact path-entry comparison, including file modes, additions, deletions
/// and both sides of renames. Unrelated later work cannot hide a rollback.
/// This detects complete restoration, not semantic equivalence or a partial
/// revert. Use the first pre-delivery entry of every path across task commits
/// since its last explicit revert, not just the newest commit's parent tree.
fn delivery_tree_restored(
    repo: &Path,
    finding: &DeliveredOpen,
    commits: &[CommitRecord],
    target: &TreeEntries,
    deadline: std::time::Instant,
    cache: &mut std::collections::HashMap<String, Vec<u8>>,
) -> Result<bool, String> {
    use crate::mcp::tools::core::task::lifecycle::close_ops::message_references_task;
    let matching: Vec<_> = commits
        .iter()
        .filter(|c| message_references_task(&c.subject, &finding.task_id))
        .take_while(|c| !is_revert(&c.subject))
        .collect();
    let mut before = TreeEntries::new();
    for commit in matching.into_iter().rev() {
        if std::time::Instant::now() >= deadline {
            return Err("delivery tree check exceeded scan budget; unchecked receipts withheld".into());
        }
        if !cache.contains_key(&commit.sha) {
            let raw = git_output(repo, &["diff-tree", "--root", "--no-commit-id", "--raw",
                "--no-abbrev", "--no-renames", "-r", "-z", &commit.sha])?;
            cache.insert(commit.sha.clone(), raw);
        }
        let fields: Vec<_> = cache[&commit.sha].split(|b| *b == 0).collect();
        for pair in fields.chunks_exact(2) {
            let header = std::str::from_utf8(pair[0]).map_err(|_| "invalid Git diff header")?;
            let parts: Vec<_> = header.split_whitespace().collect();
            if parts.len() != 5 || !parts[0].starts_with(':') {
                return Err("invalid Git diff header".into());
            }
            before.entry(pair[1].to_vec())
                .or_insert_with(|| (parts[0][1..].to_string(), parts[2].to_string()));
        }
    }
    // Empty commits are still subject-attributed bookkeeping, not evidence
    // of a tree rollback. Preserve their pre-existing attribution behavior.
    Ok(!before.is_empty() && before.iter().all(|(path, (mode, oid))| {
        if mode == "000000" {
            !target.contains_key(path)
        } else {
            target.get(path).is_some_and(|entry| &entry.0 == mode && &entry.1 == oid)
        }
    }))
}

fn verify_delivery_trees(
    repo: &Path,
    reference: &str,
    commits: &[CommitRecord],
    report: &mut BacklogReport,
    budget: CitationBudget,
) -> Result<(), String> {
    if report.delivered.is_empty() {
        return Ok(());
    }
    let candidates = std::mem::take(&mut report.delivered);
    let deadline = std::time::Instant::now() + budget.lookup_deadline;
    if git_output(repo, &["rev-parse", "--is-shallow-repository"])? == b"true\n" {
        return Err("delivery tree history is shallow; unchecked receipts withheld".into());
    }
    let raw = git_output(repo, &["ls-tree", "-r", "-z", reference])?;
    let mut target = TreeEntries::new();
    for entry in raw.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let tab = entry.iter().position(|b| *b == b'\t').ok_or("invalid Git tree entry")?;
        let header = std::str::from_utf8(&entry[..tab]).map_err(|_| "invalid Git tree header")?;
        let parts: Vec<_> = header.split_whitespace().collect();
        if parts.len() != 3 {
            return Err("invalid Git tree header".into());
        }
        target.insert(entry[tab + 1..].to_vec(), (parts[0].into(), parts[2].into()));
    }
    let mut cache = std::collections::HashMap::new();
    let mut unknown = None;
    for (index, finding) in candidates.into_iter().enumerate() {
        let result = if index >= budget.max_tasks {
            Err("delivery tree check exceeded task budget; unchecked receipts withheld".into())
        } else {
            delivery_tree_restored(repo, &finding, commits, &target, deadline, &mut cache)
        };
        match result {
            Ok(true) => report.reverted.push(finding),
            Ok(false) => report.delivered.push(finding),
            Err(error) => unknown = Some(error),
        }
    }
    report.reverted.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    unknown.map_or(Ok(()), Err)
}

/// Kind 2 over every task and dependency. An epic counts only when it has
/// children and every one of them is closed or cancelled; a child the store
/// does not hold is not known to have ended, so its epic is left alone.
pub(crate) fn finished_open_epics(tasks: &[Task], deps: &[Dependency]) -> Vec<FinishedEpic> {
    let by_id: std::collections::HashMap<&str, &Task> =
        tasks.iter().map(|task| (task.id.as_str(), task)).collect();
    let mut found: Vec<FinishedEpic> = tasks
        .iter()
        .filter(|task| task.is_open() && task.task_type == TaskType::Epic)
        .filter_map(|epic| {
            let children: Vec<Option<&&Task>> = deps
                .iter()
                .filter(|dep| dep.dep_type == DependencyType::ParentChild && dep.to_id == epic.id)
                .map(|dep| by_id.get(dep.from_id.as_str()))
                .collect();
            if children.is_empty()
                || !children.iter().all(|child| child.is_some_and(|child| child.is_terminal()))
            {
                return None;
            }
            Some(FinishedEpic {
                epic_id: epic.id.clone(),
                title: epic.title.clone(),
                children: children.len(),
                last_closed_at: children
                    .iter()
                    .filter_map(|child| child.and_then(|child| child.closed_at))
                    .max(),
            })
        })
        .collect();
    found.sort_by(|a, b| a.epic_id.cmp(&b.epic_id));
    found
}

/// Kind 3 at `now`: open non-epic tasks untouched for [`IDLE_DAYS`] or more,
/// longest idle first. An epic waits on its children by design.
pub(crate) fn idle_open(tasks: &[Task], now: DateTime<Utc>) -> Vec<IdleOpen> {
    let mut found: Vec<IdleOpen> = tasks
        .iter()
        .filter(|task| task.is_open() && task.task_type != TaskType::Epic)
        .filter_map(|task| {
            let idle_days = (now - task.updated_at).num_days();
            (idle_days >= IDLE_DAYS).then(|| IdleOpen {
                task_id: task.id.clone(),
                title: task.title.clone(),
                status: task.status.to_string(),
                assignee: task.assignee.clone(),
                idle_days,
            })
        })
        .collect();
    found.sort_by(|a, b| b.idle_days.cmp(&a.idle_days).then_with(|| a.task_id.cmp(&b.task_id)));
    found
}

/// Scan the project whose store is `cas_root`. Kinds 2 and 3 need only the
/// task store; kind 1 also reads the default branch of the project's git
/// repository (the store's parent directory), and says why when it cannot.
pub(crate) fn scan(cas_root: &Path, now: DateTime<Utc>) -> Result<BacklogReport, String> {
    scan_with(cas_root, now, CitationBudget::DOCTOR)
}

/// [`scan`] with kind 4 bounded by `budget`.
pub(crate) fn scan_with(
    cas_root: &Path,
    now: DateTime<Utc>,
    budget: CitationBudget,
) -> Result<BacklogReport, String> {
    let store = crate::store::open_task_store_local(cas_root).map_err(|error| error.to_string())?;
    let tasks = store.list(None).map_err(|error| error.to_string())?;
    let deps = store
        .list_dependencies(Some(DependencyType::ParentChild))
        .map_err(|error| error.to_string())?;
    let mut report = BacklogReport {
        finished_epics: finished_open_epics(&tasks, &deps),
        idle: idle_open(&tasks, now),
        ..BacklogReport::default()
    };
    let oldest_open = tasks
        .iter()
        .filter(|task| task.is_open() && task.task_type != TaskType::Epic)
        .map(|task| task.created_at)
        .min();
    let Some(oldest_open) = oldest_open else {
        return Ok(report);
    };
    let repo = cas_root.parent().unwrap_or(cas_root);
    match default_ref(repo) {
        Ok(reference) => {
            match read_commits(repo, &reference, oldest_open - chrono::Duration::days(1)) {
                Ok(commits) => {
                    report.delivered = delivered_open(&tasks, &commits);
                    report.reverted = reverted_open(&tasks, &commits);
                    if let Err(error) = verify_delivery_trees(repo, &reference, &commits, &mut report, budget) {
                        // If even the target tree was unavailable, none of
                        // the positive candidates were verified.
                        report.delivery_unknown = Some(error);
                    }
                }
                Err(error) => report.delivery_unknown = Some(error),
            }
            (report.stale_citations, report.citations_unchecked) =
                stale_citations(repo, &reference, &tasks, budget);
            report.default_ref = Some(reference);
        }
        Err(error) => report.delivery_unknown = Some(error),
    }
    Ok(report)
}

/// The default branch as git knows it: `origin/<branch>` when the remote ref
/// exists, since that is what shipped, else the local branch.
fn default_ref(repo: &Path) -> Result<String, String> {
    let branch = crate::mcp::tools::core::task::repo_context::resolve_default_branch(repo)?;
    let remote = format!("refs/remotes/origin/{branch}");
    let has_remote = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", &remote])
        .output()
        .is_ok_and(|output| output.status.success());
    Ok(if has_remote { format!("origin/{branch}") } else { branch })
}

/// Non-merge commits on `reference` since `since`, newest first.
fn read_commits(
    repo: &Path,
    reference: &str,
    since: DateTime<Utc>,
) -> Result<Vec<CommitRecord>, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "log",
            reference,
            "--no-merges",
            &format!("--max-count={MAX_COMMITS}"),
            &format!("--since={}", since.to_rfc3339()),
            "--format=%H%x1f%cI%x1f%s%x1e",
        ])
        .output()
        .map_err(|error| format!("cannot run git log on {reference}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git log {reference} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split('\u{1e}')
        .filter_map(|record| {
            let mut fields = record.trim().splitn(3, '\u{1f}');
            let sha = fields.next()?.to_string();
            let committed_at = DateTime::parse_from_rfc3339(fields.next()?)
                .ok()?
                .with_timezone(&Utc);
            let subject = fields.next()?.to_string();
            (!sha.is_empty()).then_some(CommitRecord { sha, committed_at, subject })
        })
        .collect())
}

fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut clipped: String = text.chars().take(max.saturating_sub(1)).collect();
    clipped.push('…');
    clipped
}

/// The call that closes a kind-1 finding through the ordinary close gates:
/// the commit receipt is validated against the target there, not here.
pub(crate) fn delivered_close_call(finding: &DeliveredOpen, default_ref: &str) -> String {
    format!(
        "task action=close id={} commit_receipt={} reason=\"delivered on {default_ref} in {}\"",
        finding.task_id,
        finding.commit.sha,
        short(&finding.commit.sha),
    )
}

/// The call that closes a kind-2 finding through the ordinary epic close gates.
pub(crate) fn epic_close_call(finding: &FinishedEpic) -> String {
    format!(
        "task action=close id={} reason=\"all {} children closed\"",
        finding.epic_id, finding.children
    )
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(9)]
}

/// `1 delivered but open, 2 finished epics open, 3 idle 14+ days`.
pub(crate) fn summary_line(report: &BacklogReport) -> String {
    format!(
        "{} delivered but open, {} finished epic{} open, {} idle {IDLE_DAYS}+ days, {} citing removed code, {} delivered then reverted",
        report.delivered.len(),
        report.finished_epics.len(),
        if report.finished_epics.len() == 1 { "" } else { "s" },
        report.idle.len(),
        report.stale_citations.len(),
        report.reverted.len(),
    )
}

/// One kind-1 line: the task, the commit that delivered it, and its close.
pub(crate) fn delivered_line(finding: &DeliveredOpen, default_ref: &str) -> String {
    format!(
        "{} ({}) \"{}\": on {default_ref} as {} {} ({}){}. Close: {}",
        finding.task_id,
        finding.status,
        clip(&finding.title, 50),
        short(&finding.commit.sha),
        clip(&finding.commit.subject, 60),
        finding.commit.committed_at.format("%Y-%m-%d"),
        match finding.more_commits {
            0 => String::new(),
            n => format!(", +{n} more commit{}", if n == 1 { "" } else { "s" }),
        },
        delivered_close_call(finding, default_ref),
    )
}

pub(crate) fn reverted_line(finding: &DeliveredOpen, default_ref: &str) -> String {
    format!(
        "{} ({}) \"{}\": delivered then reverted on {default_ref}; evidence {} {}. Review before re-delivery.",
        finding.task_id, finding.status, clip(&finding.title, 50),
        short(&finding.commit.sha), clip(&finding.commit.subject, 60),
    )
}

/// One kind-2 line: the epic, its ended children, and its close.
pub(crate) fn epic_line(finding: &FinishedEpic) -> String {
    format!(
        "{} \"{}\": all {} children closed{}. Close: {}",
        finding.epic_id,
        clip(&finding.title, 50),
        finding.children,
        finding
            .last_closed_at
            .map(|at| format!(", last on {}", at.format("%Y-%m-%d")))
            .unwrap_or_default(),
        epic_close_call(finding),
    )
}

/// One kind-3 line: the task and how long it has sat.
pub(crate) fn idle_line(finding: &IdleOpen) -> String {
    format!(
        "{} ({}{}) \"{}\": untouched {} days",
        finding.task_id,
        finding.status,
        finding
            .assignee
            .as_deref()
            .map(|assignee| format!(", {assignee}"))
            .unwrap_or_default(),
        clip(&finding.title, 50),
        finding.idle_days,
    )
}

/// The supervisor SessionStart block: `(full, compact)`, or `None` when the
/// report is clean. The full form shows a few entries of each kind with their
/// evidence and close call, bounded by [`SESSION_START_CAP_BYTES`]; the rest
/// are counted and `cas doctor` lists them all.
pub(crate) fn session_start_block(report: &BacklogReport) -> Option<(String, String)> {
    if report.is_clean() {
        return None;
    }
    let reference = report.default_ref.as_deref().unwrap_or("the default branch");
    let head = format!("Backlog hygiene: {}.", summary_line(report));
    let compact = format!("{head} Run `cas doctor` for the evidence and close calls.");
    let mut lines: Vec<String> = Vec::new();
    let mut omitted = 0usize;
    let mut take = |entries: Vec<String>, limit: usize, lines: &mut Vec<String>| {
        omitted += entries.len().saturating_sub(limit);
        lines.extend(entries.into_iter().take(limit).map(|line| format!("- {line}")));
    };
    take(report.delivered.iter().map(|f| delivered_line(f, reference)).collect(), 3, &mut lines);
    take(report.reverted.iter().map(|f| reverted_line(f, reference)).collect(), 2, &mut lines);
    take(report.finished_epics.iter().map(epic_line).collect(), 2, &mut lines);
    take(report.idle.iter().map(idle_line).collect(), 2, &mut lines);
    take(report.stale_citations.iter().map(stale_citation_line).collect(), 2, &mut lines);
    if let Some(reason) = &report.delivery_unknown {
        lines.push(format!("- Delivery check unknown: {}", clip(reason, 180)));
    }
    let mut full = head.clone();
    let tail_reserve = 80;
    let mut shown = 0usize;
    for line in &lines {
        if full.len() + 1 + line.len() + tail_reserve > SESSION_START_CAP_BYTES {
            break;
        }
        full.push('\n');
        full.push_str(line);
        shown += 1;
    }
    let more = omitted + (lines.len() - shown);
    full.push('\n');
    full.push_str(&if more > 0 {
        format!("+{more} more; run `cas doctor` for the full list. Nothing here closes on its own.")
    } else {
        "Nothing here closes on its own; each close goes through the ordinary gates.".to_string()
    });
    Some((full, compact))
}

/// The SessionStart block for `role`: supervisors only, from a fresh scan.
/// A scan that fails adds nothing; `cas doctor` reports the failure.
pub(crate) fn session_start_block_for_role(
    role: Option<&str>,
    cas_root: &Path,
    now: DateTime<Utc>,
) -> Option<(String, String)> {
    if role != Some("supervisor") {
        return None;
    }
    session_start_block(&scan_with(cas_root, now, CitationBudget::SESSION_START).ok()?)
}

/// The SessionStart block's byte cap.
pub(crate) const SESSION_START_CAP_BYTES: usize = 1_500;

#[cfg(test)]
#[path = "backlog_hygiene_tests.rs"]
mod tests;
