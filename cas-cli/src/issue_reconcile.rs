//! Reconcile tasks with the GitHub issues they fix (cas-dd56).
//!
//! On 2026-10-14, 12 issues (#1158–#1171) were still open although their fix
//! tasks had closed and shipped in v3.50.0 and v3.51.0. Tasks cited issues
//! only in prose, PRs carried no `Fixes #N`, and nothing compared shipped
//! tasks with open issues. This module gives three surfaces the comparison:
//!
//! - **Task create:** a task whose title cites exactly one issue records it as
//!   `external_ref`. A task that cites issues only in its description, or
//!   several, gets a hint to set it ([`external_ref_hint`]).
//! - **Epic close:** the receipt warns about issues its children reference
//!   that are still open ([`epic_open_issue_warning`]).
//! - **`cas doctor`:** a row lists issues that are still open although every
//!   task referencing them is closed and its delivery is in a release tag
//!   ([`shipped_open_issues`]).
//!
//! The issue *state* comes from `gh`, bounded and offline-tolerant: a failed
//! or slow `gh` is reported as "could not check", never as an empty list.
//! Closing issues automatically belongs to the release train and is out of
//! scope here (rule-176).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use cas_types::{Task, TaskStatus};

use crate::bounded_process::{Deadline, run_command};
use crate::github_issue_attach::{GH_BIN_ENV, IssueRef, parse_issue_refs, task_brief_text};

/// Total time one reconciliation may spend in `gh`.
pub const GH_BUDGET: Duration = Duration::from_secs(6);
/// Most repositories one reconciliation lists.
const MAX_REPOS: usize = 3;

/// `owner/repo` in lower case, the key issue states are grouped by.
fn repo_key(owner: &str, repo: &str) -> String {
    format!(
        "{}/{}",
        owner.to_ascii_lowercase(),
        repo.to_ascii_lowercase()
    )
}

fn issue_url(issue: &IssueRef) -> String {
    format!(
        "https://github.com/{}/{}/issues/{}",
        issue.owner, issue.repo, issue.number
    )
}

/// The open issue numbers of GitHub repositories. Production asks `gh`;
/// tests inject a fake.
pub trait IssueStates {
    /// Open issue numbers of `owner/repo`, or why they could not be read.
    fn open_issues(&self, repo: &str) -> Result<HashSet<u64>, String>;
}

/// `gh issue list` with the caller's credentials, under one deadline.
pub struct GhIssueStates {
    binary: PathBuf,
    deadline: Deadline,
}

impl GhIssueStates {
    pub fn new(budget: Duration) -> Self {
        let binary = std::env::var_os(GH_BIN_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("gh"));
        Self {
            binary,
            deadline: Deadline::after(budget),
        }
    }
}

impl IssueStates for GhIssueStates {
    fn open_issues(&self, repo: &str) -> Result<HashSet<u64>, String> {
        let mut command = Command::new(&self.binary);
        command
            .args([
                "issue", "list", "--repo", repo, "--state", "open", "--limit", "1000", "--json",
                "number",
            ])
            .env("GH_PROMPT_DISABLED", "1");
        let output = run_command(&mut command, self.deadline, GH_BUDGET)
            .map_err(|error| format!("gh issue list --repo {repo} did not complete: {error:?}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "gh issue list --repo {repo} failed: {}",
                stderr.lines().next().unwrap_or("").trim()
            ));
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&output.stdout).map_err(|error| {
                format!("gh issue list --repo {repo} returned unreadable JSON: {error}")
            })?;
        Ok(rows
            .iter()
            .filter_map(|row| row.get("number").and_then(serde_json::Value::as_u64))
            .collect())
    }
}

/// Issues `task` references: its `external_ref` and its brief text.
pub fn task_issue_refs(task: &Task, default_repo: Option<&str>) -> Vec<IssueRef> {
    parse_issue_refs(&task_brief_text(task), default_repo)
}

/// What task create does about a task's cited issues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalRefHint {
    /// The title cites exactly one issue: record it as `external_ref`.
    Set {
        url: String,
        issue: String,
    },
    /// Issues are cited, but not as one title issue: suggest setting it.
    Suggest {
        issues: Vec<String>,
    },
    None,
}

/// cas-dd56 item 1: the `external_ref` a new task should carry. Only a title
/// that cites exactly one issue sets it; description mentions are often
/// context ("related to GH #1210"), so they only produce a hint.
pub fn external_ref_hint(
    title: &str,
    description: &str,
    external_ref: Option<&str>,
    default_repo: Option<&str>,
) -> ExternalRefHint {
    if external_ref.is_some_and(|value| !value.trim().is_empty()) {
        return ExternalRefHint::None;
    }
    let in_title = parse_issue_refs(title, default_repo);
    if let [issue] = in_title.as_slice() {
        return ExternalRefHint::Set {
            url: issue_url(issue),
            issue: issue.display(),
        };
    }
    let mut cited = in_title;
    for issue in parse_issue_refs(description, default_repo) {
        if !cited.contains(&issue) {
            cited.push(issue);
        }
    }
    if cited.is_empty() {
        ExternalRefHint::None
    } else {
        ExternalRefHint::Suggest {
            issues: cited.iter().map(IssueRef::display).collect(),
        }
    }
}

/// The receipt line for an [`ExternalRefHint`], empty for none.
pub fn external_ref_hint_note(hint: &ExternalRefHint) -> String {
    match hint {
        ExternalRefHint::Set { url, issue } => format!(
            "\n\n🔗 external_ref set to {url}: the title cites {issue}. Release and doctor \
             issue checks use it to tell when the issue has shipped."
        ),
        ExternalRefHint::Suggest { issues } => format!(
            "\n\n🔗 This task cites {} but has no external_ref. If it fixes one of them, set \
             `external_ref=<issue URL>` so release and doctor issue checks can find it.",
            issues.join(", ")
        ),
        ExternalRefHint::None => String::new(),
    }
}

/// Open state of every issue in `issues`, grouped by repository. A
/// repository whose state could not be read maps to its error.
fn open_states(
    issues: &[IssueRef],
    states: &dyn IssueStates,
) -> BTreeMap<String, Result<HashSet<u64>, String>> {
    let repos: BTreeSet<String> = issues
        .iter()
        .map(|issue| repo_key(&issue.owner, &issue.repo))
        .collect();
    repos
        .into_iter()
        .take(MAX_REPOS)
        .map(|repo| {
            let open = states.open_issues(&repo);
            (repo, open)
        })
        .collect()
}

/// cas-dd56 item 3: the epic-close receipt warning about issues the epic's
/// children reference that are still open, or `None` when there are none.
pub fn epic_open_issue_warning(
    children: &[Task],
    default_repo: Option<&str>,
    states: &dyn IssueStates,
) -> Option<String> {
    let mut referenced: Vec<(IssueRef, Vec<String>)> = Vec::new();
    for child in children {
        for issue in task_issue_refs(child, default_repo) {
            match referenced.iter_mut().find(|(seen, _)| *seen == issue) {
                Some((_, tasks)) => tasks.push(child.id.clone()),
                None => referenced.push((issue, vec![child.id.clone()])),
            }
        }
    }
    if referenced.is_empty() {
        return None;
    }
    let issues: Vec<IssueRef> = referenced.iter().map(|(issue, _)| issue.clone()).collect();
    let states = open_states(&issues, states);
    let mut open = Vec::new();
    let mut unchecked = Vec::new();
    for (issue, tasks) in &referenced {
        match states.get(&repo_key(&issue.owner, &issue.repo)) {
            Some(Ok(numbers)) if numbers.contains(&issue.number) => {
                open.push(format!("{} ({})", issue.display(), tasks.join(", ")));
            }
            Some(Ok(_)) => {}
            Some(Err(error)) => unchecked.push(format!("{} ({error})", issue.display())),
            None => unchecked.push(format!("{} (repository limit reached)", issue.display())),
        }
    }
    if open.is_empty() && unchecked.is_empty() {
        return None;
    }
    let mut warning = String::new();
    if !open.is_empty() {
        warning.push_str(&format!(
            "\n\n⚠️ {} issue(s) this epic's children reference are still open:\n  - {}\n\
             Close each one whose fix has shipped, with the release version (`gh issue close \
             <N> --comment ...`), or keep it open if work remains.",
            open.len(),
            open.join("\n  - ")
        ));
    }
    if !unchecked.is_empty() {
        warning.push_str(&format!(
            "\n\nCould not check whether {} referenced issue(s) are still open:\n  - {}",
            unchecked.len(),
            unchecked.join("\n  - ")
        ));
    }
    Some(warning)
}

/// An open issue whose every referencing task has shipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShippedOpenIssue {
    pub issue: String,
    /// `(task id, release tag containing its delivery)`.
    pub tasks: Vec<(String, String)>,
}

/// Successful shipped-issue findings, plus references that could not be checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShippedIssueReport {
    pub shipped: Vec<ShippedOpenIssue>,
    pub unchecked: Vec<String>,
}

/// The delivery commits recorded on `task`, most specific first.
fn delivery_commits(task: &Task) -> Vec<&str> {
    let deliverables = &task.deliverables;
    [
        deliverables.delivery_pr_merge_commit.as_deref(),
        deliverables.merge_commit.as_deref(),
        deliverables.factory_branch_anchor.as_deref(),
        deliverables.commit_hash.as_deref(),
        deliverables
            .integration_batch
            .as_ref()
            .map(|batch| batch.tip.as_str()),
    ]
    .into_iter()
    .flatten()
    .filter(|sha| !sha.trim().is_empty())
    .collect()
}

/// The earliest `v*` release tag in `repo_root` that contains `commit`.
pub fn release_tag_containing(repo_root: &Path, commit: &str) -> Option<String> {
    if commit.starts_with('-') || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let output = Command::new("git")
        .args([
            "tag",
            "--contains",
            commit,
            "--list",
            "v*",
            "--sort=v:refname",
        ])
        .current_dir(repo_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|tag| !tag.is_empty())
        .map(str::to_string)
}

/// cas-dd56 item 4: open issues whose every referencing task is closed with
/// a delivery that a release tag contains. A task with no recorded delivery
/// commit, or one no tag contains, keeps its issue out of the list.
/// Failed repositories are listed alongside successful results; only a check
/// with no successful repository lookup returns an error.
pub fn shipped_open_issues(
    tasks: &[Task],
    default_repo: Option<&str>,
    states: &dyn IssueStates,
    tag_containing: &dyn Fn(&str) -> Option<String>,
) -> Result<ShippedIssueReport, String> {
    let mut referenced: Vec<(IssueRef, Vec<&Task>)> = Vec::new();
    for task in tasks {
        if task.status == TaskStatus::Cancelled {
            continue;
        }
        for issue in task_issue_refs(task, default_repo) {
            match referenced.iter_mut().find(|(seen, _)| *seen == issue) {
                Some((_, linked)) => linked.push(task),
                None => referenced.push((issue, vec![task])),
            }
        }
    }
    let issues: Vec<IssueRef> = referenced.iter().map(|(issue, _)| issue.clone()).collect();
    let states = open_states(&issues, states);
    let unchecked: Vec<String> = issues
        .iter()
        .filter_map(|issue| match states.get(&repo_key(&issue.owner, &issue.repo)) {
            Some(Ok(_)) => None,
            Some(Err(error)) => Some(format!("{} ({error})", issue.display())),
            None => Some(format!("{} (repository limit reached)", issue.display())),
        })
        .collect();
    if !unchecked.is_empty() && !states.values().any(Result::is_ok) {
        return Err(unchecked.join("; "));
    }
    let mut shipped = Vec::new();
    for (issue, linked) in referenced {
        let open = states
            .get(&repo_key(&issue.owner, &issue.repo))
            .and_then(|state| state.as_ref().ok())
            .is_some_and(|numbers| numbers.contains(&issue.number));
        if !open {
            continue;
        }
        let releases: Option<Vec<(String, String)>> = linked
            .iter()
            .map(|task| {
                (task.status == TaskStatus::Closed)
                    .then(|| delivery_commits(task).into_iter().find_map(tag_containing))
                    .flatten()
                    .map(|tag| (task.id.clone(), tag))
            })
            .collect();
        if let Some(tasks) = releases {
            shipped.push(ShippedOpenIssue {
                issue: issue.display(),
                tasks,
            });
        }
    }
    Ok(ShippedIssueReport { shipped, unchecked })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Open issues per repository; a repository missing from the map fails
    /// as if `gh` were offline.
    struct FakeStates {
        open: BTreeMap<String, HashSet<u64>>,
        calls: RefCell<Vec<String>>,
    }

    impl FakeStates {
        fn new(repo: &str, open: &[u64]) -> Self {
            Self {
                open: BTreeMap::from([(repo.to_string(), open.iter().copied().collect())]),
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl IssueStates for FakeStates {
        fn open_issues(&self, repo: &str) -> Result<HashSet<u64>, String> {
            self.calls.borrow_mut().push(repo.to_string());
            self.open
                .get(repo)
                .cloned()
                .ok_or_else(|| "gh: could not resolve host github.com".to_string())
        }
    }

    const REPO: &str = "Richards-LLC/cassy";

    fn task(id: &str, status: TaskStatus, title: &str) -> Task {
        let mut task = Task::new(id.into(), title.into());
        task.status = status;
        task
    }

    #[test]
    fn a_title_issue_sets_external_ref_and_prose_mentions_only_hint() {
        assert_eq!(
            external_ref_hint("GH #1216: epic create ignores target", "", None, Some(REPO)),
            ExternalRefHint::Set {
                url: "https://github.com/Richards-LLC/cassy/issues/1216".into(),
                issue: "Richards-LLC/cassy#1216".into(),
            }
        );
        assert_eq!(
            external_ref_hint(
                "Render scheduler",
                "Fixes GH #1210; see GH #1211.",
                None,
                Some(REPO)
            ),
            ExternalRefHint::Suggest {
                issues: vec![
                    "Richards-LLC/cassy#1210".into(),
                    "Richards-LLC/cassy#1211".into()
                ],
            }
        );
        assert_eq!(
            external_ref_hint("GH #1 and GH #2", "", None, Some(REPO)),
            ExternalRefHint::Suggest {
                issues: vec!["Richards-LLC/cassy#1".into(), "Richards-LLC/cassy#2".into()],
            },
            "two title issues are ambiguous"
        );
        assert_eq!(
            external_ref_hint("GH #5: x", "", Some("https://example/5"), Some(REPO)),
            ExternalRefHint::None,
            "an explicit external_ref wins"
        );
        assert_eq!(
            external_ref_hint("No issue", "none here", None, Some(REPO)),
            ExternalRefHint::None
        );
        assert!(external_ref_hint_note(&ExternalRefHint::None).is_empty());
    }

    #[test]
    fn task_issue_refs_resolves_task_gh_mentions_in_the_default_repo() {
        let task = task(
            "cas-8284",
            TaskStatus::Closed,
            "Fixed cas-397b/GH#188 and cas-cdb0/gh#195; see cas-397b/actual#7",
        );
        let refs = task_issue_refs(&task, Some(REPO));
        assert_eq!(
            refs.iter().map(IssueRef::display).collect::<Vec<_>>(),
            [
                "Richards-LLC/cassy#188",
                "Richards-LLC/cassy#195",
                "cas-397b/actual#7"
            ]
        );
        assert_eq!(
            task_issue_refs(&task, None)
                .iter()
                .map(IssueRef::display)
                .collect::<Vec<_>>(),
            ["cas-397b/actual#7"],
            "unresolved task/GH shorthand must not invent a repository"
        );
    }

    #[test]
    fn epic_close_warns_about_open_referenced_issues_and_names_unchecked_ones() {
        let children = vec![
            task("cas-a", TaskStatus::Closed, "GH #1158: first"),
            task("cas-b", TaskStatus::Closed, "GH #1159: second"),
            task("cas-c", TaskStatus::Closed, "No issue"),
        ];
        let states = FakeStates::new("richards-llc/cassy", &[1158, 2000]);
        let warning = epic_open_issue_warning(&children, Some(REPO), &states).expect("one is open");
        assert!(warning.contains("1 issue(s)"), "{warning}");
        assert!(
            warning.contains("Richards-LLC/cassy#1158 (cas-a)"),
            "{warning}"
        );
        assert!(!warning.contains("#1159"), "{warning}");
        assert_eq!(states.calls.borrow().len(), 1, "one gh call per repository");

        // Offline: say so, never claim the issues are closed.
        let offline = FakeStates::new("other/repo", &[]);
        let warning = epic_open_issue_warning(&children, Some(REPO), &offline).expect("unchecked");
        assert!(warning.contains("Could not check whether 2"), "{warning}");
        assert!(warning.contains("could not resolve host"), "{warning}");

        let quiet = vec![task("cas-c", TaskStatus::Closed, "No issue")];
        assert_eq!(epic_open_issue_warning(&quiet, Some(REPO), &states), None);
    }

    #[test]
    fn shipped_open_issues_require_every_referencing_task_shipped() {
        let mut shipped = task("cas-ship", TaskStatus::Closed, "GH #1158: shipped fix");
        shipped.deliverables.factory_branch_anchor = Some("aaa111".into());
        let mut shipped_too = task("cas-ship2", TaskStatus::Closed, "Follow-up");
        shipped_too.description = "Also for GH #1158.".into();
        shipped_too.deliverables.merge_commit = Some("bbb222".into());
        let mut unreleased = task(
            "cas-unrel",
            TaskStatus::Closed,
            "GH #1159: merged, not released",
        );
        unreleased.deliverables.factory_branch_anchor = Some("ccc333".into());
        let mut half = task("cas-half", TaskStatus::Closed, "GH #1160: part one");
        half.deliverables.factory_branch_anchor = Some("aaa111".into());
        let half_open = task("cas-half2", TaskStatus::InProgress, "GH #1160: part two");
        let no_commit = task(
            "cas-nocommit",
            TaskStatus::Closed,
            "GH #1161: closed, no commit",
        );
        let mut closed_issue = task("cas-closed", TaskStatus::Closed, "GH #1162: already closed");
        closed_issue.deliverables.factory_branch_anchor = Some("aaa111".into());
        let cancelled = task(
            "cas-cancel",
            TaskStatus::Cancelled,
            "GH #1158: abandoned try",
        );
        let tasks = vec![
            shipped,
            shipped_too,
            unreleased,
            half,
            half_open,
            no_commit,
            closed_issue,
            cancelled,
        ];

        let tags = |sha: &str| match sha {
            "aaa111" => Some("v3.50.0".to_string()),
            "bbb222" => Some("v3.51.0".to_string()),
            _ => None,
        };
        let states = FakeStates::new("richards-llc/cassy", &[1158, 1159, 1160, 1161]);
        let found = shipped_open_issues(&tasks, Some(REPO), &states, &tags).unwrap();
        assert!(found.unchecked.is_empty());
        assert_eq!(
            found.shipped,
            vec![ShippedOpenIssue {
                issue: "Richards-LLC/cassy#1158".into(),
                tasks: vec![
                    ("cas-ship".into(), "v3.50.0".into()),
                    ("cas-ship2".into(), "v3.51.0".into()),
                ],
            }],
            "#1159 is unreleased, #1160 has an open task, #1161 has no commit, #1162 is closed"
        );

        let offline = FakeStates::new("other/repo", &[]);
        let error = shipped_open_issues(&tasks, Some(REPO), &offline, &tags).unwrap_err();
        assert!(error.contains("could not resolve host"), "{error}");
    }

    #[test]
    fn release_tag_containing_names_the_earliest_release() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        git(&["commit", "-q", "--allow-empty", "-m", "fix"]);
        let fix = git(&["rev-parse", "HEAD"]);
        git(&["tag", "v3.50.0"]);
        git(&["commit", "-q", "--allow-empty", "-m", "later"]);
        git(&["tag", "v3.51.0"]);
        git(&["commit", "-q", "--allow-empty", "-m", "unreleased"]);
        let unreleased = git(&["rev-parse", "HEAD"]);
        assert_eq!(
            release_tag_containing(dir.path(), &fix).as_deref(),
            Some("v3.50.0")
        );
        assert_eq!(release_tag_containing(dir.path(), &unreleased), None);
        assert_eq!(release_tag_containing(dir.path(), "--help"), None);
    }
}
