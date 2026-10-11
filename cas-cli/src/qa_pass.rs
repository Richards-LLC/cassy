//! Independent QA and polish pass (cas-619f): eligibility and the QA work
//! item. Storage, the no-self-review rule and round accounting live in
//! `cas_store::qa_pass_store`; this module decides *whether* a delivery
//! needs a pass and what the reviewer is told to do.
//!
//! Design: `docs/qa/independent-qa-pass.md`.

use std::path::Path;
use std::process::Command;

use cas_types::{QaPass, Task, TaskType};

use crate::config::QaConfig;

pub mod github_gate;
pub mod preflight;
pub use github_gate::{github_merge_refusal, merge_request_qa_hold};

/// Label carried by every Cassy-created QA work item.
pub const QA_PASS_LABEL: &str = "qa-pass";

/// Why a delivery needs the independent pass (empty when it does not).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QaEligibility {
    pub reasons: Vec<String>,
}

impl QaEligibility {
    pub fn is_eligible(&self) -> bool {
        !self.reasons.is_empty()
    }
}

/// Paths that never make a delivery user-facing: documentation, tests and
/// test harnesses, fixtures, and CI configuration. A delivery made only of
/// these is never gated, whatever its demo_statement says.
pub fn is_non_surface_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    // Builtin skill examples and token snippets ship as instructions, not
    // rendered application surfaces (GH #1027).
    if lower.starts_with("cas-cli/src/builtins/") {
        return true;
    }
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    let in_dir = |dir: &str| lower.starts_with(&format!("{dir}/")) || lower.contains(&format!("/{dir}/"));
    ["docs", "doc", "tests", "test", "__tests__", "e2e", "fixtures", "testdata", ".github", ".circleci", ".gitlab", ".buildkite"]
        .iter()
        .any(|dir| in_dir(dir))
        || fixture_directory(&lower)
        || [".md", ".mdx", ".rst", ".adoc"].iter().any(|ext| file.ends_with(ext))
        || file.contains(".test.")
        || file.contains(".spec.")
        || file.ends_with("_test.rs")
        || file.ends_with("_tests.rs")
        || file.ends_with("_test.go")
        || (file.starts_with("test_") && file.ends_with(".py"))
        || file.starts_with("playwright.config.")
        || file.starts_with("vitest.config.")
        || file.starts_with("jest.config.")
        || file == ".gitlab-ci.yml"
        || lower.starts_with("scripts/test-")
}

/// cas-e86b: whether a path sits under a test-fixture directory named by a
/// suffix or a JavaScript convention: `scripts/visual-qa-fixtures/`,
/// `golden_fixtures/`, `__fixtures__/`, `fixture/`. A fixture page is input to
/// a checker, not a product surface: `scripts/visual-qa-fixtures/clip-box.html`
/// matched `**/*.html` and gated cas-0d16 behind a QA bundle and an
/// independent round. Only directory segments count, never the file name.
fn fixture_directory(lower: &str) -> bool {
    let mut segments: Vec<&str> = lower.split('/').collect();
    segments.pop();
    segments.iter().any(|segment| {
        matches!(*segment, "__fixtures__" | "fixture" | "__mocks__")
            || segment.ends_with("-fixtures")
            || segment.ends_with("_fixtures")
    })
}

/// Decide whether a parked delivery needs an independent QA pass.
///
/// `changed_paths` is the delivery diff when Cassy could compute it.
/// `journeys` are the catalog journeys (`scripts/journeys-for-diff.py`) its
/// surface paths touch. Eligible when the diff touches a user-facing
/// surface (a catalog journey or a `qa.user_facing_paths` glob) or the task
/// carries a demo_statement — except that a known diff made only of docs,
/// tests or CI is never gated. Epics and QA work items are never eligible.
pub fn delivery_eligibility(
    task: &Task,
    qa: &QaConfig,
    changed_paths: Option<&[String]>,
    journeys: &[String],
) -> QaEligibility {
    if !qa.independent_pass || no_code_without_surface(task, changed_paths) {
        return QaEligibility::default();
    }
    user_facing_reasons(task, qa, changed_paths, journeys)
}

/// The shared user-facing predicate (independent of any gate's on/off flag),
/// so every QA gate agrees on which deliveries are user-facing (cas-619f,
/// cas-0cd5). Epics, QA work items and docs/test/CI-only diffs are never
/// user-facing.
pub fn user_facing_reasons(
    task: &Task,
    qa: &QaConfig,
    changed_paths: Option<&[String]>,
    journeys: &[String],
) -> QaEligibility {
    let mut reasons = Vec::new();
    if task.task_type == TaskType::Epic || task.labels.iter().any(|label| label == QA_PASS_LABEL) {
        return QaEligibility { reasons };
    }
    let surface_paths: Vec<String> = changed_paths
        .unwrap_or(&[])
        .iter()
        .filter(|path| !is_non_surface_path(path))
        .cloned()
        .collect();
    if let Some(paths) = changed_paths
        && !paths.is_empty()
        && surface_paths.is_empty()
    {
        return QaEligibility { reasons };
    }
    if !journeys.is_empty() {
        reasons.push(format!("journeys:{}", journeys.join(",")));
    }
    if let Some((path, glob)) = first_user_facing_path(&surface_paths, &qa.user_facing_paths) {
        reasons.push(format!("path:{path} ({glob})"));
    }
    if !task.demo_statement.trim().is_empty() {
        reasons.push("demo_statement".to_string());
    }
    QaEligibility { reasons }
}

/// Catalog journeys the given paths touch, via the project's own
/// `scripts/journeys-for-diff.py --paths` (cas-9be7). Empty when the project
/// has no catalog or the helper fails; config globs still apply then.
///
/// cas-8132: with `head`, the selector and catalog committed at that delivery
/// are used (as `select_journeys` does since cas-f365), never the store
/// checkout's working tree. The store sat on an old main whose 11-journey
/// catalog marked hub-web surface-wide, so a delivery's "journeys:" reason
/// read HUB-J1..J11 while journey-eval at the tip selected 16 other IDs.
/// Without a head (no delivered commit known) the working tree still serves.
pub fn catalog_journeys_for(repo: &Path, head: Option<&str>, paths: &[String]) -> Vec<String> {
    let surface: Vec<String> = paths.iter().filter(|path| !is_non_surface_path(path)).cloned().collect();
    if surface.is_empty() {
        return Vec::new();
    }
    if let Some(head) = head {
        return crate::qa_evidence::journeys::journeys_for_paths_at(repo, head, &surface).unwrap_or_default();
    }
    let script = repo.join("scripts/journeys-for-diff.py");
    if !script.is_file() || !repo.join("docs/qa/journeys.md").is_file() {
        return Vec::new();
    }
    let output = Command::new("python3")
        .arg(&script)
        .arg("--paths")
        .args(&surface)
        .env("CAS_JOURNEYS_ROOT", repo)
        .current_dir(repo)
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .ok()
        .and_then(|value| value.get("journeys").and_then(|j| j.as_array()).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|journey| journey.get("id").and_then(|id| id.as_str()).map(str::to_string))
        .collect()
}

/// Whether a task is bound by the merge gates: the park (which saw the
/// delivery diff) found it user-facing and opened a round. Deciding from the
/// recorded round keeps docs/test/CI-only deliveries ungated even when they
/// carry a demo_statement.
///
/// cas-5c38: a withdrawn round (its demo_statement was cleared, or a no-code
/// task delivered no user-facing code) no longer binds the delivery.
/// cas-2387: a no-code declaration alone never unbinds a recorded round. A
/// task that kept `execution_note=no-code` while its branch carries
/// user-facing code must still wait for its review.
pub fn gate_applies(task: &Task, qa: &QaConfig, passes: &[QaPass]) -> bool {
    qa.independent_pass
        && task.task_type != TaskType::Epic
        && !task.labels.iter().any(|label| label == QA_PASS_LABEL)
        && passes.iter().any(|pass| !pass.is_withdrawn())
}

/// cas-5c38 (GH #999): an operations/artifact task (`execution_note=no-code`)
/// carries no commits, so there is no build or diff an independent reviewer
/// could walk. Its proof is the `external_ref` the no-code close requires.
pub fn is_no_code(task: &Task) -> bool {
    task.execution_note
        .as_deref()
        .is_some_and(|note| note.trim().eq_ignore_ascii_case("no-code"))
}

/// cas-2387: the independent QA exemption for a no-code task. It holds only
/// while the delivery diff has no user-facing surface path. `None` (no diff
/// could be computed, e.g. no commits at all) is no evidence of code. A
/// no-code declaration on a branch that carries UI code is reviewed like any
/// other delivery.
pub fn no_code_without_surface(task: &Task, changed_paths: Option<&[String]>) -> bool {
    is_no_code(task)
        && changed_paths.is_none_or(|paths| paths.iter().all(|path| is_non_surface_path(path)))
}

/// Merge gate for one exact tip: a passed or waived round must cover `head`.
/// Returns the refusal text when the merge must wait.
pub fn merge_gate(task: &Task, qa: &QaConfig, passes: &[QaPass], head: &str) -> Result<(), String> {
    if !gate_applies(task, qa, passes) {
        return Ok(());
    }
    if passes
        .iter()
        .any(|pass| pass.bound_head == head && pass.state.satisfies_gate())
    {
        return Ok(());
    }
    let head8 = &head[..head.len().min(8)];
    let status = match passes.iter().find(|pass| !pass.is_withdrawn()) {
        Some(latest) => format!(
            "latest round {} ({}) is {} for @{}{}",
            latest.round,
            latest.id,
            latest.state,
            latest.head8(),
            latest
                .qa_task_id
                .as_deref()
                .map(|qa_task| format!(", QA task {qa_task}"))
                .unwrap_or_default()
        ),
        None => "no round has been dispatched yet (the worker's close dispatches it)".to_string(),
    };
    Err(format!(
        "INDEPENDENT QA REQUIRED before {task} merges: no passed or waived QA round covers @{head8}; {status}. \
         Spawn a reviewer who is not the implementer \
         (`{prefix}factory action=spawn_workers lane=taste task_id=<QA task>`), \
         or waive with a logged reason: `{prefix}verification action=qa_waive task_id={task} summary=\"...\"`. \
         Check with: `{prefix}verification action=qa_status task_id={task}`",
        task = task.id,
        prefix = crate::mcp::tools::core::guidance::supervisor_prefix(),
    ))
}

/// The tip a supervisor's `qa_waive` binds to (cas-6c75, GH #1048, #1078).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaiverHead {
    pub head: String,
    /// The recorded tip the waiver moved past, when it bound to a newer one.
    pub advanced_from: Option<String>,
    /// Why the newer tip is the same delivery, in words for the receipt.
    pub why: Option<&'static str>,
}

fn same_sha(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    !a.is_empty() && !b.is_empty() && (a == b || (a.len().min(b.len()) >= 7 && (a.starts_with(b) || b.starts_with(a))))
}

/// Which tip a waiver covers.
///
/// The recorded delivery is the parked anchor, else the open (or latest)
/// round's tip, else a commit Cassy recorded for the delivery. `worktree_merge`
/// checks the branch's *current* tip, so after a rebase a waiver on the
/// recorded tip never let the merge through. The waiver binds to the current
/// branch tip when that tip is still this delivery:
/// - the open QA round is already bound to it (GH #1078), or
/// - it is a rebased copy of the recorded tip: the recorded tip is no longer
///   on the branch and both carry the same change (GH #1048).
///
/// A tip that only adds commits on top of the recorded one is new, unreviewed
/// work: the waiver stays on the recorded tip and the merge still refuses.
pub fn waiver_head(
    task: &Task,
    passes: &[QaPass],
    branch_tip: Option<&str>,
    rebased_copy: impl Fn(&str, &str) -> bool,
) -> Option<WaiverHead> {
    let recorded = task
        .deliverables
        .factory_branch_anchor
        .clone()
        .or_else(|| {
            passes
                .iter()
                .find(|pass| !pass.is_withdrawn())
                .map(|pass| pass.bound_head.clone())
        })
        .or_else(|| task.deliverables.delivery_pr_merge_commit.clone())
        .or_else(|| task.deliverables.merge_commit.clone())
        .or_else(|| task.deliverables.commit_hash.clone())
        .filter(|head| !head.trim().is_empty());
    if let Some(tip) = branch_tip.map(str::trim).filter(|tip| !tip.is_empty()) {
        if recorded.as_deref().is_some_and(|recorded| same_sha(recorded, tip)) {
            return Some(WaiverHead { head: tip.to_string(), advanced_from: None, why: None });
        }
        let open_round_at_tip = passes
            .iter()
            .any(|pass| pass.state.is_active() && same_sha(&pass.bound_head, tip));
        if open_round_at_tip {
            return Some(WaiverHead {
                head: tip.to_string(),
                advanced_from: recorded,
                why: Some("the open QA round is bound to it"),
            });
        }
        if let Some(recorded) = recorded.as_deref()
            && rebased_copy(recorded, tip)
        {
            return Some(WaiverHead {
                head: tip.to_string(),
                advanced_from: Some(recorded.to_string()),
                why: Some("it is a rebased copy of the recorded tip (same change, which is no longer on the branch)"),
            });
        }
    }
    recorded.map(|head| WaiverHead { head, advanced_from: None, why: None })
}

/// The current tip of `branch` in `repo`, as a full SHA.
pub fn branch_tip(repo: &Path, branch: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", &format!("{branch}^{{commit}}")])
        .current_dir(repo)
        .output()
        .ok()?;
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !sha.is_empty()).then_some(sha)
}

/// Read the exact pushed head, rather than trusting a stale tracking ref.
pub(crate) fn pushed_branch_tip(repo: &Path, branch: &str) -> Option<String> {
    let remote_ref = format!("refs/heads/{branch}");
    let timeout = std::time::Duration::from_secs(10);
    let mut command = Command::new("git");
    command
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["ls-remote", "--heads", "--refs", "origin", &remote_ref]);
    let output = crate::bounded_process::run_command(
        &mut command,
        crate::bounded_process::Deadline::after(timeout),
        timeout,
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .find_map(|(sha, name)| (name == remote_ref && !sha.is_empty()).then(|| sha.to_string()))
}

/// Whether `tip` is a rebased copy of `recorded` against `target`: `recorded`
/// is no longer on `tip`'s history, and the change each brings over its
/// merge-base with `target` has the same `git patch-id --stable`.
pub fn is_rebased_copy(repo: &Path, recorded: &str, tip: &str, target: &str) -> bool {
    let on_branch = Command::new("git")
        .args(["merge-base", "--is-ancestor", recorded, tip])
        .current_dir(repo)
        .status()
        .is_ok_and(|status| status.success());
    if on_branch {
        return false;
    }
    match (change_patch_id(repo, recorded, target), change_patch_id(repo, tip, target)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

/// `git patch-id --stable` of everything `head` brings over its merge-base
/// with `target`. `None` when it cannot be computed or the change is empty.
fn change_patch_id(repo: &Path, head: &str, target: &str) -> Option<String> {
    use std::io::Write;
    let base = Command::new("git").args(["merge-base", head, target]).current_dir(repo).output().ok()?;
    if !base.status.success() {
        return None;
    }
    let base = String::from_utf8_lossy(&base.stdout).trim().to_string();
    let diff = Command::new("git").args(["diff", "--no-color", &base, head]).current_dir(repo).output().ok()?;
    if !diff.status.success() || diff.stdout.is_empty() {
        return None;
    }
    let mut child = Command::new("git")
        .args(["patch-id", "--stable"])
        .current_dir(repo)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(&diff.stdout).ok()?;
    let out = child.wait_with_output().ok()?;
    let id = String::from_utf8_lossy(&out.stdout).split_whitespace().next()?.to_string();
    (out.status.success() && !id.is_empty()).then_some(id)
}

/// cas-0c988: committed build output that a merge regenerates instead of
/// merging. A delivery's reviewed content never includes it.
pub const GENERATED_ARTIFACT_PATHS: &[&str] = &["hub-web/dist"];

/// `git patch-id --stable` of the change `head` brings over its merge-base
/// with `target`, leaving out [`GENERATED_ARTIFACT_PATHS`]. Two tips with the
/// same id carry the same reviewed source, whatever their build output.
pub fn source_patch_id(repo: &Path, head: &str, target: &str) -> Option<String> {
    let _ = (repo, head, target);
    None
}

/// cas-0c988: carry a passed or waived verdict from an earlier tip of this
/// delivery to `head` when both carry the same source patch against
/// `target`. Records the carried round and logs it; returns it.
pub fn carry_verdict(
    cas_root: &Path,
    repo: &Path,
    task: &Task,
    passes: &[QaPass],
    head: &str,
    target: &str,
) -> Option<QaPass> {
    let _ = (cas_root, repo, task, passes, head, target);
    None
}

/// First changed path matching a configured glob, with the glob it matched.
pub fn first_user_facing_path<'a>(
    changed_paths: &'a [String],
    globs: &'a [String],
) -> Option<(&'a str, &'a str)> {
    let options = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };
    let patterns: Vec<(glob::Pattern, &str)> = globs
        .iter()
        .filter_map(|raw| {
            let raw = raw.trim();
            glob::Pattern::new(raw).ok().map(|pattern| (pattern, raw))
        })
        .collect();
    changed_paths.iter().find_map(|path| {
        patterns.iter().find_map(|(pattern, raw)| {
            // `**/x` must also match `x` at the repository root.
            let root_match = raw
                .strip_prefix("**/")
                .and_then(|rest| glob::Pattern::new(rest).ok())
                .is_some_and(|rest| rest.matches_with(path, options));
            (pattern.matches_with(path, options) || root_match).then_some((path.as_str(), *raw))
        })
    })
}

/// Factory branches a shell command would `git merge` (cas-619f pre-tool
/// guard). `--abort`/`--continue`/`--quit` statements merge nothing new.
pub fn factory_branches_merged_by(command: &str) -> Vec<String> {
    let mut branches = Vec::new();
    for statement in command.split(['\n', ';', '|', '&']) {
        let tokens: Vec<&str> = statement
            .split_whitespace()
            .map(|token| token.trim_matches(|ch: char| matches!(ch, '\'' | '"' | '`' | '(' | ')')))
            .collect();
        let Some(git) = tokens.iter().position(|token| *token == "git" || token.ends_with("/git"))
        else {
            continue;
        };
        let Some(merge) = tokens[git + 1..].iter().position(|token| *token == "merge") else {
            continue;
        };
        let args = &tokens[git + 1 + merge + 1..];
        if args
            .iter()
            .any(|arg| matches!(*arg, "--abort" | "--continue" | "--quit"))
        {
            continue;
        }
        for arg in args {
            let name = arg
                .trim_start_matches("refs/remotes/")
                .trim_start_matches("refs/heads/")
                .trim_start_matches("origin/");
            if let Some(worker) = name.strip_prefix("factory/")
                && !worker.is_empty()
                && !branches.iter().any(|branch: &String| branch == name)
            {
                branches.push(name.to_string());
            }
        }
    }
    branches
}

/// Supervisor pre-tool guard: the refusal when a `git merge` would integrate
/// a user-facing delivery with a recorded round whose current tip has no passed or waived
/// independent QA round. Fails open (None) when Cassy state is unreadable —
/// the close backstop still refuses such a task later.
///
/// cas-2ee2: raw GitHub merges (`gh pr merge`, the merge API) are checked
/// too; [`github_merge_refusal`] also runs for non-supervisor roles.
pub fn supervisor_merge_refusal(cas_root: &Path, cwd: &Path, command: &str) -> Option<String> {
    let branches = factory_branches_merged_by(command);
    branches
        .into_iter()
        .find_map(|branch| branch_merge_refusal(cas_root, cwd, &branch))
        .or_else(|| github_merge_refusal(cas_root, cwd, command))
}

/// Check the branch selected by `worktree_merge`, including calls without a
/// task_id. The raw-git hook uses the same lookup so both merge paths agree.
pub fn branch_merge_refusal(cas_root: &Path, cwd: &Path, branch: &str) -> Option<String> {
    let config = crate::config::Config::load(cas_root).ok()?;
    let qa = config.qa();
    if !qa.independent_pass {
        return None;
    }
    let task_store = crate::store::open_task_store(cas_root).ok()?;
    // A close can dispatch a round from the post-merge backstop while the
    // delivery remains InProgress. The recorded round, not the park status,
    // determines whether this merge needs an independent verdict.
    let deliveries = task_store.list(None).ok()?;
    let worker = branch.trim_start_matches("factory/");
    for task in deliveries
        .iter()
        .filter(|task| task.status != cas_types::TaskStatus::Closed)
    {
        let passes = cas_store::list_qa_passes(cas_root, &task.id).unwrap_or_default();
        if !branch_binds_task(task, &qa, &passes, branch, worker) {
            continue;
        }
        let head = Command::new("git")
            .args(["rev-parse", "--verify", branch])
            .current_dir(cwd)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .filter(|sha| !sha.is_empty());
        let refusal = match head {
            Some(head) => merge_gate(task, &qa, &passes, &head).err(),
            None => Some(format!(
                "INDEPENDENT QA REQUIRED before {} merges, and {branch} does not resolve here.",
                task.id
            )),
        };
        if let Some(refusal) = refusal {
            return Some(format!("🚫 {refusal}"));
        }
    }
    None
}

fn branch_binds_task(
    task: &Task,
    qa: &QaConfig,
    passes: &[QaPass],
    branch: &str,
    worker: &str,
) -> bool {
    gate_applies(task, qa, passes)
        && (task.deliverables.parked_branch.as_deref() == Some(branch)
            || task.assignee.as_deref() == Some(worker)
            || passes.iter().any(|pass| pass.branch == branch))
}

/// Reviewable paths from the delivery's first-parent content commits after
/// `merge-base(parent, branch)`.
pub fn changed_paths_for_delivery(
    repo: &Path,
    parent_branch: &str,
    branch: &str,
) -> Result<Vec<String>, String> {
    let base = Command::new("git")
        .args(["merge-base", parent_branch, branch])
        .current_dir(repo)
        .output()
        .map_err(|error| format!("git merge-base failed to start: {error}"))?;
    if !base.status.success() {
        return Err(format!(
            "git merge-base {parent_branch} {branch} failed: {}",
            String::from_utf8_lossy(&base.stderr).trim()
        ));
    }
    let base = String::from_utf8_lossy(&base.stdout).trim().to_string();
    delivery_content_paths(repo, &base, branch)
}

/// Paths changed by first-parent content commits in a delivery. Merge commits
/// carry incoming history, not this task's own edits; `-w` and the diff filter
/// exclude formatting-only changes and deleted surfaces (GH #1027/#1037).
pub fn delivery_content_paths(repo: &Path, base: &str, head: &str) -> Result<Vec<String>, String> {
    let commits = Command::new("git")
        .args([
            "rev-list",
            "--first-parent",
            "--no-merges",
            "--reverse",
            &format!("{base}..{head}"),
        ])
        .current_dir(repo)
        .output()
        .map_err(|error| format!("git rev-list failed to start: {error}"))?;
    if !commits.status.success() {
        return Err(format!(
            "git rev-list {base}..{head} failed: {}",
            String::from_utf8_lossy(&commits.stderr).trim()
        ));
    }
    let mut paths = Vec::new();
    for commit in String::from_utf8_lossy(&commits.stdout).lines() {
        let diff = Command::new("git")
            .args([
                "diff",
                "-w",
                "--diff-filter=ACMRT",
                "--name-only",
                &format!("{commit}^"),
                commit,
                "--",
            ])
            .current_dir(repo)
            .output()
            .map_err(|error| format!("git diff failed to start: {error}"))?;
        if !diff.status.success() {
            return Err(format!(
                "git diff {commit} failed: {}",
                String::from_utf8_lossy(&diff.stderr).trim()
            ));
        }
        paths.extend(
            String::from_utf8_lossy(&diff.stdout)
                .lines()
                .map(str::to_string),
        );
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Reviewable paths a delivery brought into `target`: its own content commits
/// before the first merge on the ancestry path, or its unmerged content
/// commits. `None` when that merge boundary cannot be found.
pub fn integrated_paths(repo: &Path, head: &str, target: &str) -> Option<Vec<String>> {
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git").args(args).current_dir(repo).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let merged = Command::new("git")
        .args(["merge-base", "--is-ancestor", head, target])
        .current_dir(repo)
        .status()
        .is_ok_and(|status| status.success());
    let (from, to) = if merged {
        let merges = git(&[
            "rev-list",
            "--ancestry-path",
            "--merges",
            "--reverse",
            &format!("{head}..{target}"),
        ])?;
        let merge = merges.lines().next()?.trim().to_string();
        (format!("{merge}^1"), head.to_string())
    } else {
        (git(&["merge-base", target, head])?, head.to_string())
    };
    let base = git(&["merge-base", &from, &to])?;
    delivery_content_paths(repo, &base, &to).ok()
}

/// `epic_status` section listing each child's latest independent QA round
/// (cas-619f). Waivers show who waived and why. Empty when no child has one.
pub fn render_epic_qa_section(cas_root: &Path, children: &[Task]) -> String {
    let mut lines = Vec::new();
    for child in children {
        let Ok(passes) = cas_store::list_qa_passes(cas_root, &child.id) else {
            continue;
        };
        let Some(latest) = passes.first() else {
            continue;
        };
        let detail = match latest.state {
            cas_types::QaPassState::Waived => format!(
                "WAIVED by {} — {}",
                latest.issuer_agent_id.as_deref().unwrap_or("supervisor"),
                latest.summary.as_deref().unwrap_or("(no reason recorded)")
            ),
            cas_types::QaPassState::Passed | cas_types::QaPassState::Failed => format!(
                "{} by {}{}",
                latest.state,
                latest.reviewer_agent_id.as_deref().unwrap_or("-"),
                latest
                    .ledger_path
                    .as_deref()
                    .map(|ledger| format!(" — {ledger}"))
                    .unwrap_or_default()
            ),
            state => format!(
                "{state}{} — QA task {}",
                latest
                    .reviewer_agent_id
                    .as_deref()
                    .map(|reviewer| format!(" by {reviewer}"))
                    .unwrap_or_default(),
                latest.qa_task_id.as_deref().unwrap_or("-")
            ),
        };
        lines.push(format!(
            "- {} round {} @{}: {detail}",
            child.id,
            latest.round,
            latest.head8()
        ));
    }
    if lines.is_empty() {
        return String::new();
    }
    format!("\n\nIndependent QA:\n{}\n", lines.join("\n"))
}

/// Check the independent round's evidence bundle (cas-c3b8 contract v1)
/// beside its ledger: `bundle.json` with `producer: "independent-qa"`, the
/// delivery's task id, and `head_sha` equal to the reviewed tip. Returns the
/// bundle path, or why it cannot back a verdict.
///
/// cas-5488 (GH #1152): the visual-QA pass claim backs an approval only. A
/// rejection asserts no pass, so the bounds comparison never blocks it; a
/// reviewer who found a real defect can always record it.
pub fn validate_round_bundle(
    ledger_path: &Path,
    pass: &QaPass,
    verdict: cas_types::QaVerdict,
) -> Result<std::path::PathBuf, String> {
    let dir = ledger_path
        .parent()
        .ok_or_else(|| "ledger_path has no parent directory".to_string())?;
    let bundle = dir.join("bundle.json");
    let raw = std::fs::read_to_string(&bundle).map_err(|_| {
        format!(
            "no evidence bundle at {} — write the cas-qa-craft bundle (producer \"independent-qa\") beside LEDGER.md",
            bundle.display()
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| format!("{} is not valid JSON: {error}", bundle.display()))?;
    let field = |name: &str| value.get(name).and_then(|v| v.as_str()).unwrap_or("");
    if field("producer") != "independent-qa" {
        return Err(format!(
            "{}: producer must be \"independent-qa\" (found {:?})",
            bundle.display(),
            field("producer")
        ));
    }
    if field("task_id") != pass.task_id {
        return Err(format!(
            "{}: task_id must be {} (found {:?})",
            bundle.display(),
            pass.task_id,
            field("task_id")
        ));
    }
    if field("head_sha") != pass.bound_head {
        return Err(format!(
            "{}: head_sha must be the reviewed tip {} (found {:?})",
            bundle.display(),
            pass.bound_head,
            field("head_sha")
        ));
    }
    // cas-a6a3 (GH #1007): a round that claims a visual-QA pass backs it with
    // the strict run's own report, generated after the round opened, against
    // a local build of the reviewed tip, never the production URL. cas-e371
    // (GH #1023 finding 1): a "scoped" round backs it with that run plus the
    // base build's run over the same pages, and passes when the tip added no
    // finding the base does not have.
    let status = field("visual_qa_status");
    if verdict == cas_types::QaVerdict::Approved && matches!(status, "pass" | "scoped") {
        let inside = |key: &str, default: &str| -> Result<std::path::PathBuf, String> {
            let relative = value
                .pointer(&format!("/files/{key}"))
                .and_then(|v| v.as_str())
                .unwrap_or(default);
            if relative.trim().is_empty()
                || std::path::Path::new(relative).components().any(|part| {
                    !matches!(
                        part,
                        std::path::Component::Normal(_) | std::path::Component::CurDir
                    )
                })
            {
                return Err(format!(
                    "{}: files.{key} {relative:?} must name a file inside the round directory",
                    bundle.display()
                ));
            }
            Ok(dir.join(relative))
        };
        let tip = inside("visual_qa_json", "visual-qa/visual-qa.json")?;
        let opened = format!(
            "round {} opened at {}",
            pass.round,
            pass.requested_at.to_rfc3339()
        );
        if status == "pass" {
            crate::qa_evidence::check_visual_qa_run(&tip, pass.requested_at.timestamp(), &opened)
                .map_err(|problem| format!("{}: {problem}", bundle.display()))?;
        } else {
            let baseline = inside("visual_qa_baseline_json", "")?;
            crate::qa_evidence::check_visual_qa_scoped(
                &tip,
                &baseline,
                pass.requested_at.timestamp(),
                &opened,
            )
            .map_err(|problem| format!("{}: {problem}", bundle.display()))?;
        }
    }
    Ok(bundle)
}

/// A `qa_record` issue the reviewer marked `"scope": "pre-existing"`: the base
/// build has it too, so it is not the delivery's defect (cas-e371, GH #1023
/// finding 1). Cassy files each one as a follow-up task linked to the
/// delivery instead of letting it reject a correct, narrow change.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PreExistingIssue {
    /// The ledger's finding id ("F10"), when the reviewer gave one.
    pub id: String,
    /// A short title, when the reviewer gave one separately from the problem.
    pub title: String,
    pub severity: String,
    pub problem: String,
    pub suggestion: String,
    /// The screenshot or file the reviewer cited, if any.
    pub evidence: String,
}

/// One finding line in a ledger's "## Pre-existing" section (cas-2849).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerFinding {
    pub id: String,
    pub severity: String,
    pub text: String,
}

/// The "F10 NORMAL: <text>" lines of a ledger's pre-existing section
/// (cas-2849). The section is the first "## " heading naming pre-existing
/// findings; a finding line is a finding id, one severity word, a colon and
/// its text, optionally bulleted. Prose lines are skipped.
pub fn ledger_pre_existing_findings(ledger: &str) -> Vec<LedgerFinding> {
    let mut findings = Vec::new();
    let mut in_section = false;
    for line in ledger.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            let heading = heading.to_ascii_lowercase();
            in_section = heading.contains("pre-existing") || heading.contains("preexisting");
            continue;
        }
        if !in_section {
            continue;
        }
        let line = line.trim().trim_start_matches(['-', '*']).trim();
        let Some((head, text)) = line.split_once(':') else {
            continue;
        };
        let words: Vec<&str> = head.split_whitespace().collect();
        let [id, severity] = words.as_slice() else {
            continue;
        };
        let is_id = id.len() > 1
            && id.starts_with(['F', 'f'])
            && id[1..].bytes().all(|byte| byte.is_ascii_digit());
        let text = text.trim();
        if is_id && severity.chars().all(char::is_alphabetic) && !text.is_empty() {
            findings.push(LedgerFinding {
                id: id.to_ascii_uppercase(),
                severity: severity.to_ascii_lowercase(),
                text: text.to_string(),
            });
        }
    }
    findings
}

/// Fill each pre-existing issue's missing problem from the ledger's
/// "F10 NORMAL: <text>" line for its id, and refuse one that still has no
/// text: a follow-up titled "defect recorded by independent QA" with
/// "Problem: (not described)" is unactionable (cas-2849).
pub fn complete_pre_existing_issues(
    issues: &mut [PreExistingIssue],
    ledger: &str,
) -> Result<(), String> {
    let findings = ledger_pre_existing_findings(ledger);
    for (index, issue) in issues.iter_mut().enumerate() {
        if issue.problem.is_empty()
            && let Some(finding) = findings
                .iter()
                .find(|finding| !issue.id.is_empty() && finding.id.eq_ignore_ascii_case(&issue.id))
        {
            issue.problem = finding.text.clone();
            if issue.severity.is_empty() {
                issue.severity = finding.severity.clone();
            }
        }
        if issue.problem.is_empty() {
            let named = if issue.id.is_empty() {
                format!("pre-existing issue {}", index + 1)
            } else {
                format!("pre-existing issue {} ({})", index + 1, issue.id)
            };
            return Err(format!(
                "{named} has no problem text, so its follow-up task would say nothing. Give it a \
                 \"problem\" (or \"title\"/\"description\") in issues, or give it an \"id\" and write \
                 the line \"{id} <SEVERITY>: <what is wrong>\" in the ledger's ## Pre-existing \
                 section, then record the verdict again.",
                id = if issue.id.is_empty() { "F<n>" } else { issue.id.as_str() },
            ));
        }
    }
    Ok(())
}

/// The existing task a pre-existing issue already names as its follow-up
/// ("existing follow-up cas-2a33"), so qa_record does not file a duplicate
/// (cas-2849). The delivery's own id never counts.
pub fn tracked_follow_up(
    issue: &PreExistingIssue,
    delivery_id: &str,
    exists: impl Fn(&str) -> bool,
) -> Option<String> {
    let text = format!("{} {} {}", issue.title, issue.problem, issue.suggestion).to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut offset = 0;
    while let Some(found) = text[offset..].find("cas-") {
        let start = offset + found;
        let digits = bytes[start + 4..]
            .iter()
            .take_while(|byte| byte.is_ascii_hexdigit())
            .count();
        let end = start + 4 + digits;
        offset = start + 4;
        let boundary = bytes.get(end).is_none_or(|byte| !byte.is_ascii_alphanumeric());
        if !(4..=8).contains(&digits) || !boundary {
            continue;
        }
        let id = &text[start..end];
        if !id.eq_ignore_ascii_case(delivery_id) && exists(id) {
            return Some(id.to_string());
        }
    }
    None
}

/// Split `qa_record`'s `issues` JSON array into the pre-existing issues and
/// the number of issues the delivery owns. An issue without a `scope`, or
/// with any scope other than pre-existing, belongs to the delivery.
pub fn split_qa_issues(issues_json: Option<&str>) -> Result<(Vec<PreExistingIssue>, usize), String> {
    let Some(raw) = issues_json.filter(|raw| !raw.trim().is_empty()) else {
        return Ok((Vec::new(), 0));
    };
    let parsed: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| format!("issues is not valid JSON ({error})"))?;
    let items = parsed
        .as_array()
        .ok_or_else(|| "issues must be a JSON array".to_string())?;
    let mut pre_existing = Vec::new();
    let mut delivery = 0;
    for item in items {
        let text = |key: &str| {
            item.get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        let scope = text("scope").to_ascii_lowercase().replace(['_', ' '], "-");
        if matches!(scope.as_str(), "pre-existing" | "preexisting") {
            // cas-2849: reviewers also write id/title/description/evidence,
            // as cas-d1fa round 1 did; read those rather than dropping them.
            let first = |keys: &[&str]| {
                keys.iter()
                    .map(|key| text(key))
                    .find(|value| !value.is_empty())
                    .unwrap_or_default()
            };
            let title = text("title");
            let mut problem = first(&["problem", "description", "summary"]);
            if problem.is_empty() {
                problem = title.clone();
            }
            pre_existing.push(PreExistingIssue {
                id: text("id"),
                title,
                severity: text("severity"),
                problem,
                suggestion: first(&["suggestion", "fix"]),
                evidence: first(&["file", "evidence"]),
            });
        } else {
            delivery += 1;
        }
    }
    Ok((pre_existing, delivery))
}

/// Why a rejection is refused: every issue it names is pre-existing, so it
/// rejects the delivery for defects it did not cause.
pub fn rejection_scope_refusal(
    verdict: cas_types::QaVerdict,
    pre_existing: usize,
    delivery: usize,
) -> Option<String> {
    (verdict == cas_types::QaVerdict::Rejected && pre_existing > 0 && delivery == 0).then(|| {
        format!(
            "every issue ({pre_existing}) is marked \"scope\": \"pre-existing\", and a pre-existing \
             defect never rejects a delivery. Reject only for a regression the delivery introduced \
             or an unmet acceptance criterion, and name that issue without the pre-existing scope; \
             otherwise record status=approved and Cassy files the pre-existing issues as follow-ups."
        )
    })
}

/// Priority of a follow-up for a pre-existing issue, from its severity.
pub fn follow_up_priority(severity: &str) -> cas_types::Priority {
    match severity.trim().to_ascii_lowercase().as_str() {
        "blocking" | "critical" => cas_types::Priority::HIGH,
        "high" | "normal" => cas_types::Priority::MEDIUM,
        _ => cas_types::Priority::LOW,
    }
}

/// Title of the follow-up task for one pre-existing issue.
pub fn follow_up_title(delivery: &Task, issue: &PreExistingIssue) -> String {
    // The first sentence of the first line: "Footer contrast 3.1:1. Base too."
    // titles as "Footer contrast 3.1:1".
    let source = if issue.title.is_empty() { &issue.problem } else { &issue.title };
    let first_line = source.lines().next().unwrap_or_default();
    let mut problem = first_line
        .split(". ")
        .next()
        .unwrap_or_default()
        .trim()
        .trim_end_matches('.')
        .to_string();
    if problem.is_empty() {
        problem = "defect recorded by independent QA".to_string();
    }
    if problem.chars().count() > 80 {
        problem = problem.chars().take(77).collect::<String>() + "...";
    }
    format!("Pre-existing: {problem} (found in QA of {})", delivery.id)
}

/// Body of the follow-up task for one pre-existing issue.
pub fn follow_up_description(
    delivery: &Task,
    pass: &QaPass,
    issue: &PreExistingIssue,
    ledger_path: &str,
) -> String {
    let mut body = format!(
        "Independent QA round {round} of {task} ({title}) @{head} found this defect on the base \
         build as well, so it was recorded as pre-existing and did not count against the delivery \
         (cas-e371).\n\n- Severity: {severity}\n- Problem: {problem}\n",
        round = pass.round,
        task = delivery.id,
        title = delivery.title.trim(),
        head = pass.head8(),
        severity = if issue.severity.is_empty() { "unrated" } else { issue.severity.as_str() },
        problem = if issue.problem.is_empty() { "(not described)" } else { issue.problem.as_str() },
    );
    if !issue.suggestion.is_empty() {
        body.push_str(&format!("- Suggested fix: {}\n", issue.suggestion));
    }
    if !issue.evidence.is_empty() {
        body.push_str(&format!("- Evidence: {}\n", issue.evidence));
    }
    body.push_str(&format!("- Ledger: {ledger_path}\n"));
    body
}

/// The epic a QA follow-up joins: the reviewed delivery's parent epic.
pub fn follow_up_epic(
    store: &dyn cas_store::TaskStore,
    delivery_id: &str,
) -> cas_store::Result<Option<Task>> {
    Ok(store
        .get_parent_epic(delivery_id)?
        .filter(|epic| !epic.is_terminal()))
}

/// The follow-up task for one pre-existing issue, before it is stored.
pub fn follow_up_task(
    id: &str,
    delivery: &Task,
    pass: &QaPass,
    issue: &PreExistingIssue,
    ledger_path: &str,
    epic: Option<&Task>,
) -> Task {
    let mut task = Task::new(id.to_string(), follow_up_title(delivery, issue));
    task.task_type = TaskType::Bug;
    task.scope = crate::types::Scope::Project;
    task.origin_project = delivery.origin_project.clone();
    task.description = follow_up_description(delivery, pass, issue, ledger_path);
    task.priority = follow_up_priority(&issue.severity);
    task.risk = vec![cas_types::TaskRisk::None];
    task.labels = vec!["qa-follow-up".to_string(), "pre-existing".to_string()];
    task.external_ref = Some(ledger_path.to_string());
    task.deliverables.work_target = delivery.deliverables.work_target.clone();
    task.delivery_mode = delivery.delivery_mode;
    if let Some(epic) = epic {
        if let Some(target) =
            crate::mcp::tools::core::task::repo_context::default_child_work_target_from_epic(
                delivery, epic,
            )
        {
            task.deliverables.work_target = Some(target);
        }
        task.delivery_mode = epic.delivery_mode;
    }
    task
}

/// Title of the QA work item for one round.
pub fn qa_task_title(delivery: &Task, pass: &QaPass) -> String {
    let mut title = delivery.title.trim().to_string();
    if title.chars().count() > 80 {
        title = title.chars().take(77).collect::<String>() + "...";
    }
    format!(
        "QA pass (round {}): {title} @{}",
        pass.round,
        pass.head8()
    )
}

/// Ledger directory for one round, under the delivery's artifacts dir.
///
/// Deliberately beside, not inside, the implementer's own cas-qa-craft
/// bundle (`<task>/qa/`): the close-time evidence gate reads that directory
/// as the implementer's evidence, and an independent round must never be
/// mistaken for it (or vice versa).
pub fn round_dir(artifacts_root: &Path, pass: &QaPass) -> std::path::PathBuf {
    artifacts_root
        .join(&pass.task_id)
        .join("independent-qa")
        .join(format!("round-{}", pass.round))
}

/// The decision rule a reviewer applies (cas-6eb1). It lives in the generated
/// task text, not only in the skill reference, because a reviewer recorded
/// "approved" twice before a supervisor's stricter brief arrived, and an
/// installed skill may predate `references/independent-pass.md`.
pub const QA_REJECTION_BAR: &str = "Rejection bar. Decide by this, not by impression. Judge what the \
delivery changed (the pages, controls and journeys its diff touches) and its acceptance criteria, not the \
page's older backlog.\n\
- Reject on a regression the delivery introduced: a Blocking or High journey finding it causes, or a \
visual-qa.mjs --strict finding the base build does not have.\n\
- Reject when a cas-ui-craft critique dimension for what the delivery changed scores below 3, or \
distinctiveness, fit or hierarchy scores below 4 on a public surface.\n\
- Reject when an acceptance criterion or the demo statement is not met on the running build. A defect the \
delivery claims to fix that still reproduces is an unmet criterion.\n\
- Forced colors, reduced motion and more contrast count only when the capture proves the mode \
with matchMedia (for example matchMedia('(forced-colors: active)').matches is true).\n\
- Keyboard-only must reach and complete the demo's primary action.\n\
- A required mode you did not run is NOT EXERCISED, never PASS, and rejects.\n\
- A defect the base build already has never rejects, even on a touched page, unless the delivery claims \
to fix it. Record it under Pre-existing in LEDGER.md and pass it to qa_record with \"scope\": \
\"pre-existing\"; Cassy files it as a follow-up task linked to the delivery.\n\
Otherwise approve, and list Normal and Note findings in the summary.";

/// Brief the reviewer reads when it starts the QA work item.
pub fn qa_task_description(
    delivery: &Task,
    pass: &QaPass,
    reasons: &[String],
    ledger_dir: &Path,
    parent_branch: &str,
) -> String {
    let demo = if delivery.demo_statement.trim().is_empty() {
        "(none — derive the user goal from the delivery title and diff)".to_string()
    } else {
        delivery.demo_statement.trim().to_string()
    };
    format!(
        "Independent QA and polish pass for delivery {task} — round {round}.\n\n\
         You are NOT the implementer ({implementer}); Cassy refuses the implementer here. \
         Review the running product, never edit the delivery. Follow the cas-qa-craft skill, \
         section \"Independent pass\".\n\n\
         - Delivery: {task} — {title}\n\
         - Demo statement: {demo}\n\
         - Branch: {branch} at {head} (review exactly this tip; base: {parent})\n\
         - Why this pass: {reasons}\n\
         - Ledger: {ledger}/LEDGER.md (evidence beside it)\n\
         - Deadline: {deadline} (pass {pass_id})\n\n\
         Steps: build and serve {head}; walk the journeys the diff touches \
         (scripts/journeys-for-diff.py {parent} {head}) plus the demo statement. \
         For hub-web/src or hub-web/dist changes, cover every source-impact-selected journey \
         at {head}; never substitute a hand-picked subset. Include journey_receipt in bundle.json \
         (schema 1, producer journey-eval, exact base/head, selection_ids, per-ID PASS with native \
         passed/failed/skipped counts, tool_version and suite_exit). You may cite the implementer's \
         receipt for the same tip and selection, then spend the round on independent cells and pixels. \
         Workers and QA run affected journeys at --workers=4; the supervisor owns the one full suite \
         per epic assembly, and the merge queue runs it. Preserve any failing run and rerun only \
         its failing spec at --workers=1 to distinguish a flake. Then \
         walk the adjacent paths (empty, loading, error, long content, phone 390px, dark, \
         keyboard-only, reduced motion); run visual-qa.mjs --strict and score the \
         cas-ui-craft rubric for what the delivery changed, with desktop+phone, light+dark \
         screenshots. When a check fails, repeat it on the base build \
         (git merge-base {parent} {head}) before you count it: a failure the base shares is \
         pre-existing. For visual QA, run the base build over the same pages into \
         visual-qa-baseline/ and set visual_qa_status \"scoped\" with \
         files.visual_qa_baseline_json; only findings the base lacks count. Every finding \
         cites a trace action and a screenshot.\n\n\
         {bar}\n\n\
         Evidence: {ledger}/bundle.json with producer \"independent-qa\", task_id {task} and \
         head_sha {head}, listing trace.zip, trace-actions.txt, receipt.webm, final.aria.yml/json, \
         one F0N.png per finding, visual-qa/ and visual-qa.stdout, critique.md, and one \
         journeys/<ID>/ folder per journey. Run visual-qa.mjs --strict against your own local \
         serve of {head}, never the production URL: qa_record refuses a claimed visual-QA pass \
         without that local run.\n\n\
         {tool_naming}\n\n\
         Replace {{prefix}} with your harness's prefix above. Load verification.\n\
         Record the verdict with: {{prefix}}verification action=qa_record task_id={task} \
         status=approved|rejected summary=\"...\" issues='[...]' ledger_path={ledger}/LEDGER.md \
         — a rejection sends {task} back to its implementer with your ledger. Recording closes \
         this QA task and cannot be revised. If you change your mind after recording, do not \
         record again: message the supervisor (blocker=true) asking for request_changes on \
         {task}, and name the finding.",
        tool_naming = crate::builtins::TOOL_NAMING_LINE,
        bar = QA_REJECTION_BAR,
        task = delivery.id,
        round = pass.round,
        implementer = pass.implementer_agent_id,
        title = delivery.title.trim(),
        branch = pass.branch,
        head = pass.bound_head,
        parent = parent_branch,
        reasons = reasons.join(", "),
        ledger = ledger_dir.display(),
        deadline = pass.deadline_at.to_rfc3339(),
        pass_id = pass.id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> Task {
        Task::new("cas-ui1".to_string(), "Reply composer".to_string())
    }

    fn paths(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    /// cas-e86b: the cas-0d16 delivery (checker, two fixture pages, tests)
    /// is not user-facing; the same diff plus a product page still is.
    #[test]
    fn fixture_directory_html_is_not_user_facing_but_product_html_still_is_cas_e86b() {
        for fixture in [
            "scripts/visual-qa-fixtures/clip-box.html",
            "scripts/visual-qa-fixtures/nested/page.css",
            "web/src/__fixtures__/card.html",
            "golden_fixtures/report.html",
            "hub-web/src/fixture/layout.css",
            "src/__mocks__/view.tsx",
        ] {
            assert!(is_non_surface_path(fixture), "{fixture}");
        }
        for product in [
            "hub-web/src/styles.css",
            "hub-web/dist/index.html",
            "web/fixtures-page.html",
            "web/src/fixtures-panel/view.tsx",
            "scripts/visual-qa.mjs",
        ] {
            assert!(!is_non_surface_path(product), "{product}");
        }

        let qa = QaConfig::default();
        let cas_0d16 = paths(&[
            "scripts/visual-qa.mjs",
            "scripts/visual-qa-fixtures/clip-box.html",
            "scripts/visual-qa-fixtures/clip-overflow.html",
            "scripts/test-visual-qa.mjs",
        ]);
        let reasons = user_facing_reasons(&task(), &qa, Some(&cas_0d16), &[]);
        assert!(!reasons.reasons.iter().any(|reason| reason.starts_with("path:")), "{reasons:?}");
        assert!(!delivery_eligibility(&task(), &qa, Some(&cas_0d16), &[]).is_eligible());

        let mut mixed = cas_0d16.clone();
        mixed.push("hub-web/src/styles.css".to_string());
        let reasons = user_facing_reasons(&task(), &qa, Some(&mixed), &[]);
        assert_eq!(reasons.reasons, vec!["path:hub-web/src/styles.css (**/*.css)".to_string()]);
        assert!(delivery_eligibility(&task(), &qa, Some(&mixed), &[]).is_eligible());
    }

    #[test]
    fn eligibility_from_demo_journeys_or_surface_path() {
        let qa = QaConfig::default();
        let backend = paths(&["cas-cli/src/lib.rs"]);
        assert!(!delivery_eligibility(&task(), &qa, Some(&backend), &[]).is_eligible());

        let mut demo = task();
        demo.demo_statement = "Type a reply and see it land".to_string();
        assert_eq!(
            delivery_eligibility(&demo, &qa, Some(&backend), &[]).reasons,
            vec!["demo_statement".to_string()]
        );
        // Unknown diff: the demo statement alone still qualifies.
        assert!(delivery_eligibility(&demo, &qa, None, &[]).is_eligible());

        let css = paths(&["docs/x.md", "hub-web/src/a.css"]);
        assert_eq!(
            delivery_eligibility(&task(), &qa, Some(&css), &[]).reasons,
            vec!["path:hub-web/src/a.css (**/*.css)".to_string()]
        );
        let ts = paths(&["hub-web/src/composer-markup.ts"]);
        assert_eq!(
            delivery_eligibility(&task(), &qa, Some(&ts), &["HUB-J5".to_string()]).reasons,
            vec!["journeys:HUB-J5".to_string()]
        );
    }

    #[test]
    fn labels_alone_do_not_gate() {
        let qa = QaConfig::default();
        let mut labelled = task();
        labelled.labels = vec!["ui".to_string(), "hub".to_string()];
        let backend = paths(&["cas-cli/src/lib.rs"]);
        assert!(!delivery_eligibility(&labelled, &qa, Some(&backend), &[]).is_eligible());
    }

    #[test]
    fn docs_test_and_ci_only_deliveries_are_never_gated() {
        let qa = QaConfig::default();
        let mut demo = task();
        demo.demo_statement = "Reply lands".to_string();
        for only in [
            vec!["docs/qa/journeys.md", "README.md"],
            vec!["hub-web/e2e/journeys/reply.journey.spec.ts", "hub-web/playwright.config.ts"],
            vec!["cas-cli/tests/cli_test.rs", "crates/x/src/foo_tests.rs"],
            vec![".github/workflows/ci.yml", "scripts/test-ci-test-tiers.sh"],
            vec!["hub-web/src/composer.test.ts", "fixtures/hub/a.json", "hub-web/DESIGN.md"],
        ] {
            let changed = paths(&only);
            assert!(
                !delivery_eligibility(&demo, &qa, Some(&changed), &["HUB-J1".to_string()])
                    .is_eligible(),
                "{only:?} must never be gated"
            );
        }
        // One real surface file among them is enough.
        let mixed = paths(&["docs/a.md", "hub-web/src/styles.css"]);
        assert!(delivery_eligibility(&task(), &qa, Some(&mixed), &[]).is_eligible());
    }

    #[test]
    fn deleted_builtin_and_merge_only_or_whitespace_vue_do_not_trigger_qa_gh_1027_1037() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(["-c", "user.name=QA", "-c", "user.email=qa@example.invalid"])
                .args(args)
                .current_dir(repo)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}: {output:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("old.html"), "<main>old</main>\n").unwrap();
        std::fs::write(repo.join("old.css"), "body { color: red; }\n").unwrap();
        std::fs::write(repo.join("app.vue"), "<template><p>Hello</p></template>\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        git(&["switch", "-q", "-c", "factory/worker"]);
        std::fs::remove_file(repo.join("old.html")).unwrap();
        std::fs::remove_file(repo.join("old.css")).unwrap();
        std::fs::write(repo.join("app.vue"), "<template> <p>Hello</p> </template>\n").unwrap();
        std::fs::create_dir_all(repo.join("cas-cli/src/builtins/examples")).unwrap();
        std::fs::write(repo.join("cas-cli/src/builtins/examples/tokens.css"), "body { color: blue; }\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "remove examples and format"]);
        let paths = changed_paths_for_delivery(repo, "main", "HEAD").unwrap();
        assert_eq!(paths, vec!["cas-cli/src/builtins/examples/tokens.css"]);
        assert!(!user_facing_reasons(&task(), &QaConfig::default(), Some(&paths), &[]).is_eligible());

        git(&["switch", "-q", "main"]);
        std::fs::write(repo.join("incoming.vue"), "<template><p>Incoming</p></template>\n").unwrap();
        git(&["add", "incoming.vue"]);
        git(&["commit", "-q", "-m", "staging UI"]);
        git(&["switch", "-q", "factory/worker"]);
        git(&["merge", "-q", "--no-ff", "-m", "sync staging", "main"]);
        let paths = changed_paths_for_delivery(repo, "main~1", "HEAD").unwrap();
        assert!(!paths.iter().any(|path| path == "incoming.vue"), "{paths:?}");
        assert!(!paths.iter().any(|path| path == "app.vue"), "{paths:?}");

        std::fs::write(repo.join("app.vue"), "<template><p>Changed</p></template>\n").unwrap();
        git(&["add", "app.vue"]);
        git(&["commit", "-q", "-m", "real UI edit"]);
        let paths = changed_paths_for_delivery(repo, "main", "HEAD").unwrap();
        assert!(paths.iter().any(|path| path == "app.vue"), "{paths:?}");
        assert!(user_facing_reasons(&task(), &QaConfig::default(), Some(&paths), &[]).is_eligible());

        let delivery_head = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap();
        let delivery_head = String::from_utf8(delivery_head.stdout).unwrap();
        git(&["switch", "-q", "main"]);
        git(&["merge", "-q", "--no-ff", "-m", "land worker", "factory/worker"]);
        let integrated = integrated_paths(repo, delivery_head.trim(), "main").unwrap();
        assert!(integrated.iter().any(|path| path == "app.vue"), "{integrated:?}");
        assert!(!integrated.iter().any(|path| path == "incoming.vue"), "{integrated:?}");
    }

    /// A git repo for the cas-6c75 regression: `main` with a base commit and
    /// `factory/worker` carrying one UI change.
    fn rebase_repo() -> (tempfile::TempDir, impl Fn(&[&str]) -> String) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().to_path_buf();
        let git = move |args: &[&str]| -> String {
            let output = Command::new("git")
                .args(["-c", "user.name=QA", "-c", "user.email=qa@example.invalid"])
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}: {output:?}");
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("app.css"), "body { color: red; }\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        git(&["switch", "-q", "-c", "factory/worker"]);
        std::fs::write(dir.path().join("app.css"), "body { color: blue; }\n").unwrap();
        git(&["commit", "-q", "-am", "the delivery"]);
        (dir, git)
    }

    fn parked(anchor: &str) -> Task {
        let mut task = task();
        task.deliverables.factory_branch_anchor = Some(anchor.to_string());
        task.deliverables.parked_branch = Some("factory/worker".to_string());
        task
    }

    fn dispatch(cas: &Path, head: &str) {
        let new = cas_store::NewQaPass {
            task_id: "cas-ui1",
            implementer_agent_id: "worker",
            branch: "factory/worker",
            bound_head: head,
            deadline_at: chrono::Utc::now() + chrono::Duration::hours(1),
            max_rounds: 3,
        };
        cas_store::open_qa_pass(cas, &new, chrono::Utc::now()).unwrap();
    }

    /// cas-6c75 (GH #1048): park at A, a round dispatched for A, a sibling
    /// lands, the worker rebases to T (same change). The waiver used to record
    /// @A, and worktree_merge, which checks the branch tip T, refused it every
    /// time. It now binds to T, and the merge gate accepts it.
    #[test]
    fn waiver_after_a_rebase_binds_to_the_current_tip_and_the_merge_accepts_it_gh_1048() {
        let (dir, git) = rebase_repo();
        let repo = dir.path();
        let cas = tempfile::tempdir().unwrap();
        let anchor = git(&["rev-parse", "HEAD"]);
        let task = parked(&anchor);
        dispatch(cas.path(), &anchor);

        // A sibling lands on main; the worker rebases onto it.
        git(&["switch", "-q", "main"]);
        std::fs::write(repo.join("other.rs"), "fn sibling() {}\n").unwrap();
        git(&["add", "other.rs"]);
        git(&["commit", "-q", "-m", "sibling"]);
        git(&["switch", "-q", "factory/worker"]);
        git(&["rebase", "-q", "main"]);
        let tip = branch_tip(repo, "factory/worker").unwrap();
        assert_ne!(tip, anchor, "the rebase moved the tip");
        assert!(is_rebased_copy(repo, &anchor, &tip, "main"));

        let passes = cas_store::list_qa_passes(cas.path(), "cas-ui1").unwrap();
        let chosen = waiver_head(&task, &passes, Some(&tip), |recorded, tip| {
            is_rebased_copy(repo, recorded, tip, "main")
        })
        .unwrap();
        assert_eq!(chosen.head, tip);
        assert_eq!(chosen.advanced_from.as_deref(), Some(anchor.as_str()));

        cas_store::waive_qa_pass(
            cas.path(),
            "cas-ui1",
            "supervisor",
            "worker",
            "factory/worker",
            &chosen.head,
            "rebased; reviewed content unchanged",
            chrono::Utc::now(),
        )
        .unwrap();
        let passes = cas_store::list_qa_passes(cas.path(), "cas-ui1").unwrap();
        let qa = QaConfig::default();
        assert_eq!(
            merge_gate(&task, &qa, &passes, &tip),
            Ok(()),
            "worktree_merge accepts the waiver"
        );

        // The old binding: a waiver on the pre-rebase anchor never covers the tip.
        let mut stale = passes.clone();
        for pass in &mut stale {
            if pass.state == cas_types::QaPassState::Waived {
                pass.bound_head = anchor.clone();
            }
        }
        let refusal = merge_gate(&task, &qa, &stale, &tip).unwrap_err();
        assert!(refusal.contains(&format!("covers @{}", &tip[..8])), "{refusal}");
    }

    /// cas-6c75 (GH #1078): the branch and the open dispatch are both at the
    /// rebased tip while the parked anchor is the pre-rebase copy. The waiver
    /// binds to the dispatch's tip.
    #[test]
    fn waiver_binds_to_the_open_dispatch_tip_over_a_stale_anchor_gh_1078() {
        let (dir, git) = rebase_repo();
        let repo = dir.path();
        let cas = tempfile::tempdir().unwrap();
        let stale = git(&["rev-parse", "HEAD"]);
        git(&["commit", "-q", "--amend", "-m", "the delivery, reworded"]);
        let tip = branch_tip(repo, "factory/worker").unwrap();
        dispatch(cas.path(), &tip);
        let passes = cas_store::list_qa_passes(cas.path(), "cas-ui1").unwrap();
        let chosen = waiver_head(&parked(&stale), &passes, Some(&tip), |_, _| false).unwrap();
        assert_eq!(chosen.head, tip);
        assert_eq!(chosen.why, Some("the open QA round is bound to it"));
    }

    /// New commits on top of the parked tip are unreviewed work: the waiver
    /// stays on the parked tip and the merge of the new tip still refuses.
    #[test]
    fn waiver_never_jumps_to_a_tip_that_adds_unreviewed_work() {
        let (dir, git) = rebase_repo();
        let repo = dir.path();
        let cas = tempfile::tempdir().unwrap();
        let anchor = git(&["rev-parse", "HEAD"]);
        dispatch(cas.path(), &anchor);
        std::fs::write(repo.join("app.css"), "body { color: green; }\n").unwrap();
        git(&["commit", "-q", "-am", "more work after park"]);
        let tip = branch_tip(repo, "factory/worker").unwrap();
        assert!(!is_rebased_copy(repo, &anchor, &tip, "main"));
        let task = parked(&anchor);
        let passes = cas_store::list_qa_passes(cas.path(), "cas-ui1").unwrap();
        let chosen = waiver_head(&task, &passes, Some(&tip), |recorded, tip| {
            is_rebased_copy(repo, recorded, tip, "main")
        })
        .unwrap();
        assert_eq!(chosen.head, anchor);
        assert_eq!(chosen.advanced_from, None);
        // A rebased copy with a different change is not the same delivery either.
        git(&["switch", "-q", "-c", "factory/other", "main"]);
        std::fs::write(repo.join("app.css"), "body { color: purple; }\n").unwrap();
        git(&["commit", "-q", "-am", "something else"]);
        let other = branch_tip(repo, "factory/other").unwrap();
        assert!(!is_rebased_copy(repo, &anchor, &other, "main"));
    }

    #[test]
    fn epics_qa_items_and_disabled_config_are_never_eligible() {
        let mut qa = QaConfig::default();
        let css = paths(&["a.css"]);
        let mut epic = task();
        epic.task_type = TaskType::Epic;
        epic.demo_statement = "demo".to_string();
        assert!(!delivery_eligibility(&epic, &qa, Some(&css), &[]).is_eligible());

        let mut qa_item = task();
        qa_item.labels = vec![QA_PASS_LABEL.to_string()];
        assert!(!delivery_eligibility(&qa_item, &qa, Some(&css), &[]).is_eligible());

        qa.independent_pass = false;
        assert!(!delivery_eligibility(&task(), &qa, Some(&css), &[]).is_eligible());
        // The shared predicate ignores the gate flag.
        assert!(user_facing_reasons(&task(), &qa, Some(&css), &[]).is_eligible());
    }

    fn pass(head: &str, state: cas_types::QaPassState) -> QaPass {
        let now = chrono::Utc::now();
        QaPass {
            id: format!("qapass-{head}"),
            task_id: "cas-ui1".to_string(),
            round: 1,
            implementer_agent_id: "impl".to_string(),
            branch: "factory/impl".to_string(),
            bound_head: head.to_string(),
            qa_task_id: Some("cas-qa1".to_string()),
            reviewer_agent_id: None,
            state,
            summary: None,
            issues_json: None,
            ledger_path: None,
            issuer_agent_id: None,
            requested_at: now,
            deadline_at: now,
            resolved_at: None,
        }
    }

    #[test]
    fn merge_gate_requires_a_satisfying_round_for_the_exact_head() {
        use cas_types::QaPassState::*;
        let qa = QaConfig::default();
        let mut demo = task();
        demo.demo_statement = "Reply lands".to_string();

        // No recorded round: the park judged it not user-facing (or it
        // predates the gate), so merge is not blocked here.
        assert!(merge_gate(&demo, &qa, &[], "aaaa1111bbbb").is_ok());

        let pending = merge_gate(&demo, &qa, &[pass("aaaa1111bbbb", Pending)], "aaaa1111bbbb")
            .unwrap_err();
        assert!(pending.contains("is pending"), "{pending}");
        assert!(pending.contains("cas-qa1"), "{pending}");

        assert!(merge_gate(&demo, &qa, &[pass("aaaa1111bbbb", Passed)], "aaaa1111bbbb").is_ok());
        assert!(merge_gate(&demo, &qa, &[pass("aaaa1111bbbb", Waived)], "aaaa1111bbbb").is_ok());
        // A pass for an older tip does not cover new commits.
        assert!(merge_gate(&demo, &qa, &[pass("aaaa1111bbbb", Passed)], "cccc2222").is_err());
        assert!(merge_gate(&demo, &qa, &[pass("aaaa1111bbbb", Failed)], "aaaa1111bbbb").is_err());
    }

    #[test]
    fn branch_lookup_binds_the_recorded_round_after_reassignment() {
        use cas_types::QaPassState::*;
        let qa = QaConfig::default();
        let mut delivery = task();
        delivery.assignee = Some("replacement".to_string());
        let pending = pass("aaaa1111", Pending);
        assert!(branch_binds_task(
            &delivery,
            &qa,
            &[pending.clone()],
            "factory/impl",
            "impl"
        ));
        assert!(!branch_binds_task(
            &delivery,
            &qa,
            &[pending.clone()],
            "factory/unrelated",
            "unrelated"
        ));
        assert!(merge_gate(&delivery, &qa, &[pending], "aaaa1111").is_err());
        assert!(merge_gate(&delivery, &qa, &[pass("aaaa1111", Waived)], "aaaa1111").is_ok());
    }

    #[test]
    fn no_code_tasks_and_withdrawn_rounds_never_bind_the_gate_cas_5c38() {
        use cas_types::QaPassState::*;
        let qa = QaConfig::default();
        let mut no_code = task();
        no_code.demo_statement = "The dashboard shows the tenant".to_string();
        no_code.execution_note = Some("no-code".to_string());
        // Nothing user-facing in the diff (none at all, or docs/tests only).
        assert!(!delivery_eligibility(&no_code, &qa, None, &[]).is_eligible());
        assert!(!delivery_eligibility(&no_code, &qa, Some(&[]), &[]).is_eligible());
        let docs = vec!["docs/runbook.md".to_string()];
        assert!(!delivery_eligibility(&no_code, &qa, Some(&docs), &[]).is_eligible());
        // cas-2387: a no-code declaration never hides user-facing code.
        let css = vec!["web/app.css".to_string()];
        assert!(delivery_eligibility(&no_code, &qa, Some(&css), &[]).is_eligible());
        assert!(merge_gate(&no_code, &qa, &[pass("aaaa1111", Pending)], "aaaa1111").is_err());

        let mut withdrawn = pass("aaaa1111", Superseded);
        withdrawn.summary = Some(format!("{}demo_statement cleared", cas_types::QA_PASS_WITHDRAWN_PREFIX));
        assert!(withdrawn.is_withdrawn());
        assert!(!gate_applies(&task(), &qa, &[withdrawn.clone()]));
        // A tip that merely moved still binds: superseded is not withdrawn.
        assert!(gate_applies(&task(), &qa, &[pass("aaaa1111", Superseded)]));
        assert!(gate_applies(&task(), &qa, &[withdrawn, pass("bbbb2222", Failed)]));
    }

    #[test]
    fn merge_gate_binds_only_recorded_rounds() {
        let qa = QaConfig::default();
        assert!(merge_gate(&task(), &qa, &[], "aaaa1111").is_ok());
        // A path-eligible delivery is bound once the park recorded a round.
        assert!(
            merge_gate(
                &task(),
                &qa,
                &[pass("aaaa1111", cas_types::QaPassState::Pending)],
                "aaaa1111"
            )
            .is_err()
        );
        let mut disabled = QaConfig::default();
        disabled.independent_pass = false;
        assert!(
            merge_gate(
                &task(),
                &disabled,
                &[pass("aaaa1111", cas_types::QaPassState::Pending)],
                "aaaa1111"
            )
            .is_ok()
        );
    }

    #[test]
    fn merge_commands_name_the_factory_branches_they_integrate() {
        assert_eq!(
            factory_branches_merged_by("git merge --no-ff factory/zealous-cheetah-52"),
            vec!["factory/zealous-cheetah-52".to_string()]
        );
        assert_eq!(
            factory_branches_merged_by(
                "cd /repo && git fetch && git merge --no-edit origin/factory/a-1 factory/b-2"
            ),
            vec!["factory/a-1".to_string(), "factory/b-2".to_string()]
        );
        assert!(factory_branches_merged_by("git merge --abort").is_empty());
        assert!(factory_branches_merged_by("git merge epic/x").is_empty());
        assert!(factory_branches_merged_by("git log factory/a-1").is_empty());
        assert!(factory_branches_merged_by("echo factory/a-1 merge").is_empty());
    }

    #[test]
    fn round_bundle_must_name_the_reviewed_tip_and_producer() {
        let dir = tempfile::TempDir::new().unwrap();
        let ledger = dir.path().join("LEDGER.md");
        std::fs::write(&ledger, "# ledger").unwrap();
        let round = pass("aaaa1111", cas_types::QaPassState::Claimed);
        assert!(validate_round_bundle(&ledger, &round, cas_types::QaVerdict::Approved).unwrap_err().contains("no evidence bundle"));
        let write = |producer: &str, head: &str| {
            std::fs::write(
                dir.path().join("bundle.json"),
                serde_json::json!({"schema":1,"task_id":"cas-ui1","producer":producer,"head_sha":head})
                    .to_string(),
            )
            .unwrap();
        };
        write("cas-qa-craft", "aaaa1111");
        assert!(validate_round_bundle(&ledger, &round, cas_types::QaVerdict::Approved).unwrap_err().contains("producer"));
        write("independent-qa", "bbbb2222");
        assert!(validate_round_bundle(&ledger, &round, cas_types::QaVerdict::Approved).unwrap_err().contains("head_sha"));
        write("independent-qa", "aaaa1111");
        assert!(validate_round_bundle(&ledger, &round, cas_types::QaVerdict::Approved).is_ok());
    }

    /// cas-6eb1: the generated QA task states the rejection bar, the evidence
    /// contract and how to reopen after recording, so the reviewer needs no
    /// supervisor brief and no installed skill reference to decide.
    #[test]
    fn qa_task_description_states_the_rejection_bar_and_evidence_path() {
        let round = pass("aaaa1111", cas_types::QaPassState::Pending);
        let text = qa_task_description(
            &task(),
            &round,
            &["demo_statement".to_string()],
            Path::new("/artifacts/cas-ui1/independent-qa/round-1"),
            "epic/x",
        );
        assert!(text.contains(QA_REJECTION_BAR), "{text}");
        for pinned in [
            "Rejection bar. Decide by this, not by impression",
            "scores below 3",
            "below 4 on a public surface",
            "page's older backlog",
            "a regression the delivery introduced",
            "finding the base build does not have",
            "A defect the delivery claims to fix that still reproduces is an unmet criterion",
            "never rejects, even on a touched page",
            "\"scope\": \"pre-existing\"",
            "visual_qa_status \"scoped\"",
            "files.visual_qa_baseline_json",
            "matchMedia('(forced-colors: active)').matches",
            "Keyboard-only must reach and complete the demo's primary action",
            "NOT EXERCISED, never PASS, and rejects",
            "/artifacts/cas-ui1/independent-qa/round-1/bundle.json with producer \"independent-qa\"",
            "head_sha aaaa1111",
            "never the production URL",
            "Recording closes this QA task and cannot be revised",
            "asking for request_changes on cas-ui1",
        ] {
            assert!(text.contains(pinned), "missing {pinned:?} in:\n{text}");
        }
    }

    #[test]
    fn cas_bb7e_qa_task_brief_uses_the_reviewers_harness_prefix() {
        let text = qa_task_description(
            &task(),
            &pass("aaaa1111", cas_types::QaPassState::Pending),
            &["demo_statement".to_string()],
            Path::new("/artifacts/cas-ui1/independent-qa/round-1"),
            "epic/x",
        );
        assert!(text.contains(crate::builtins::TOOL_NAMING_LINE), "{text}");
        assert!(text.contains("{prefix}verification action=qa_record task_id=cas-ui1"), "{text}");
        assert!(!text.contains("mcp__cas__verification action=qa_record"), "{text}");
    }

    /// cas-a6a3 (GH #1007): a round claiming `visual_qa_status: pass` needs
    /// the strict run's own report, from after the round opened, of a local
    /// build. A claim with no run, or a run against production, is refused.
    #[test]
    fn round_bundle_visual_qa_pass_claim_needs_a_local_run_after_the_round_opened() {
        let dir = tempfile::TempDir::new().unwrap();
        let ledger = dir.path().join("LEDGER.md");
        std::fs::write(&ledger, "# ledger").unwrap();
        let round = pass("aaaa1111", cas_types::QaPassState::Claimed);
        std::fs::write(
            dir.path().join("bundle.json"),
            serde_json::json!({
                "schema": 1, "task_id": "cas-ui1", "producer": "independent-qa",
                "head_sha": "aaaa1111", "visual_qa_status": "pass",
                "files": {"visual_qa_json": "visual-qa/visual-qa.json"}
            })
            .to_string(),
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("visual-qa")).unwrap();
        let report = |status: &str, generated: chrono::DateTime<chrono::Utc>, url: &str| {
            std::fs::write(
                dir.path().join("visual-qa/visual-qa.json"),
                serde_json::json!({
                    "status": status, "strict": true,
                    "generatedAt": generated.to_rfc3339(), "urls": [url]
                })
                .to_string(),
            )
            .unwrap();
        };
        // Claimed, never run.
        let refused = validate_round_bundle(&ledger, &round, cas_types::QaVerdict::Approved).unwrap_err();
        assert!(refused.contains("missing or unreadable"), "{refused}");
        let later = round.requested_at + chrono::Duration::seconds(60);
        // Run against the production origin.
        report("PASS", later, "https://hub.petrastella.io/commander/");
        let refused = validate_round_bundle(&ledger, &round, cas_types::QaVerdict::Approved).unwrap_err();
        assert!(refused.contains("not a local build"), "{refused}");
        // A local run from before the round opened.
        let earlier = round.requested_at - chrono::Duration::hours(1);
        report("PASS", earlier, "http://127.0.0.1:28511/commander/");
        let refused = validate_round_bundle(&ledger, &round, cas_types::QaVerdict::Approved).unwrap_err();
        assert!(refused.contains("before round 1 opened"), "{refused}");
        // A failed local run.
        report("FAIL", later, "http://127.0.0.1:28511/commander/");
        let refused = validate_round_bundle(&ledger, &round, cas_types::QaVerdict::Approved).unwrap_err();
        assert!(refused.contains("status \"FAIL\""), "{refused}");
        // A passing local run after the round opened backs the claim.
        report("PASS", later, "http://127.0.0.1:28511/commander/");
        assert!(validate_round_bundle(&ledger, &round, cas_types::QaVerdict::Approved).is_ok());
    }

    /// cas-e371 (GH #1023 finding 1): the rejection bar no longer turns a
    /// pre-existing defect on a touched page into a rejection.
    #[test]
    fn rejection_bar_scopes_rejection_to_what_the_delivery_changed() {
        assert!(!QA_REJECTION_BAR.contains("easy-to-spot bug on the touched path"));
        assert!(!QA_REJECTION_BAR.contains("including a pre-existing one"));
        assert!(!QA_REJECTION_BAR.contains("when visual-qa.mjs --strict fails"));
    }

    #[test]
    fn qa_issues_split_into_pre_existing_follow_ups_and_delivery_issues() {
        let (pre, delivery) = split_qa_issues(Some(
            r#"[
                {"severity":"high","problem":"no focus ring on Send"},
                {"severity":"normal","scope":"pre-existing","problem":"Footer contrast 3.1:1. Base too.","suggestion":"darken","file":"F02.png"},
                {"severity":"note","scope":"Pre_Existing","problem":"old overflow"},
                {"severity":"note","scope":"delivery","problem":"new clip"}
            ]"#,
        ))
        .unwrap();
        assert_eq!(delivery, 2);
        assert_eq!(pre.len(), 2);
        assert_eq!(pre[0].problem, "Footer contrast 3.1:1. Base too.");
        assert_eq!(pre[0].suggestion, "darken");
        assert_eq!(pre[0].evidence, "F02.png");
        assert_eq!(split_qa_issues(None).unwrap(), (Vec::new(), 0));
        assert_eq!(split_qa_issues(Some("  ")).unwrap(), (Vec::new(), 0));
        assert!(split_qa_issues(Some("{}")).unwrap_err().contains("JSON array"));
        assert!(split_qa_issues(Some("[")).unwrap_err().contains("not valid JSON"));
    }

    /// cas-1980: a delivery under epic E that once also hung under a closed
    /// epic C (cas-9ebd's double parent). Returns the store, E and the delivery.
    fn delivery_under_epic() -> (
        tempfile::TempDir,
        std::sync::Arc<dyn cas_store::TaskStore>,
        Task,
        Task,
    ) {
        let dir = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
        let store = crate::store::open_task_store(&cas_dir).unwrap();
        let mut closed = Task::new("cas-c10d".to_string(), "Finished epic".to_string());
        closed.task_type = TaskType::Epic;
        closed.status = cas_types::TaskStatus::Closed;
        closed.branch = Some("epic/finished".to_string());
        store.add(&closed).unwrap();
        let mut epic = Task::new("cas-e9e1".to_string(), "Live epic".to_string());
        epic.task_type = TaskType::Epic;
        epic.branch = Some("epic/live".to_string());
        epic.delivery_mode = cas_types::DeliveryMode::LocalMerge;
        epic.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:cas-src".to_string(),
            target_branch: "main".to_string(),
        });
        store.add(&epic).unwrap();
        let delivery = task();
        store.add(&delivery).unwrap();
        for parent in [&closed.id, &epic.id] {
            store
                .add_dependency(&cas_types::Dependency::new(
                    delivery.id.clone(),
                    parent.clone(),
                    cas_types::DependencyType::ParentChild,
                ))
                .unwrap();
        }
        (dir, store, epic, delivery)
    }

    /// cas-1980: a QA follow-up joins the delivery's open epic and targets
    /// its branch, so its close never targets main.
    #[test]
    fn a_qa_follow_up_joins_the_deliverys_open_epic_cas_1980() {
        let (_dir, store, epic, delivery) = delivery_under_epic();
        let parent = follow_up_epic(store.as_ref(), &delivery.id)
            .unwrap()
            .expect("the delivery has an open epic");
        assert_eq!(
            parent.id, epic.id,
            "the closed parent is never the follow-up's epic"
        );

        let issue = PreExistingIssue {
            severity: "normal".to_string(),
            problem: "Footer contrast 3.1:1".to_string(),
            suggestion: String::new(),
            evidence: String::new(),
            ..Default::default()
        };
        let round = pass("aaaa1111", cas_types::QaPassState::Passed);
        let follow_up = follow_up_task(
            "cas-f011",
            &delivery,
            &round,
            &issue,
            "/a/LEDGER.md",
            Some(&parent),
        );
        assert_eq!(
            follow_up
                .deliverables
                .work_target
                .as_ref()
                .map(|target| target.target_branch.as_str()),
            Some("epic/live"),
            "its close targets the epic's branch"
        );
        assert_eq!(follow_up.delivery_mode, cas_types::DeliveryMode::LocalMerge);
        store
            .create_atomic(&follow_up, &[], Some(&parent.id), Some("cas-qa-record"))
            .unwrap();
        assert_eq!(
            store
                .get_parent_epic("cas-f011")
                .unwrap()
                .map(|epic| epic.id),
            Some(epic.id.clone())
        );

        // Without a parent epic the follow-up keeps the old shape.
        let loose = follow_up_task("cas-f012", &delivery, &round, &issue, "/a/LEDGER.md", None);
        assert!(loose.deliverables.work_target.is_none());
    }

    #[test]
    fn follow_up_preserves_explicit_targets_and_skips_terminal_parent_cas_1980() {
        let (_dir, store, epic, mut delivery) = delivery_under_epic();
        let issue = PreExistingIssue {
            severity: "normal".to_string(),
            problem: "contrast".to_string(),
            suggestion: String::new(),
            evidence: String::new(),
            ..Default::default()
        };
        let round = pass("aaaa1111", cas_types::QaPassState::Passed);
        // A child still targeted to the epic's base follows its live lane.
        delivery.deliverables.work_target = epic.deliverables.work_target.clone();
        let inherited = follow_up_task(
            "cas-f013",
            &delivery,
            &round,
            &issue,
            "/a/LEDGER.md",
            Some(&epic),
        );
        assert_eq!(
            inherited.deliverables.work_target.unwrap().target_branch,
            "epic/live"
        );
        // A distinct explicit repository/branch remains authoritative.
        for target in [
            cas_types::WorkTarget {
                repo_selector: "project:cas-src".into(),
                target_branch: "release/custom".into(),
            },
            cas_types::WorkTarget {
                repo_selector: "project:other".into(),
                target_branch: "main".into(),
            },
        ] {
            delivery.deliverables.work_target = Some(target.clone());
            let follow_up = follow_up_task(
                "cas-f014",
                &delivery,
                &round,
                &issue,
                "/a/LEDGER.md",
                Some(&epic),
            );
            assert_eq!(follow_up.deliverables.work_target, Some(target));
            assert!(follow_up.assignee.is_none());
        }
        store.remove_dependency(&delivery.id, &epic.id).unwrap();
        assert!(
            store
                .get_parent_epic(&delivery.id)
                .unwrap()
                .unwrap()
                .is_terminal()
        );
        assert!(
            follow_up_epic(store.as_ref(), &delivery.id)
                .unwrap()
                .is_none()
        );
        store.remove_dependency(&delivery.id, "cas-c10d").unwrap();
        assert!(
            follow_up_epic(store.as_ref(), &delivery.id)
                .unwrap()
                .is_none()
        );
    }

    const D1FA_LEDGER: &str = include_str!("../tests/data/qa-ledgers/cas-d1fa-round-1-LEDGER.ledger.txt");
    const D1FA_ISSUES: &str = include_str!("../tests/data/qa-ledgers/cas-d1fa-round-1-issues.json");

    /// cas-2849, the real cas-d1fa round 1: its pre-existing section is free
    /// text, "F10 NORMAL: <text>", not a table.
    #[test]
    fn free_text_pre_existing_lines_are_parsed_cas_2849() {
        let findings = ledger_pre_existing_findings(D1FA_LEDGER);
        let ids: Vec<_> = findings.iter().map(|finding| finding.id.as_str()).collect();
        assert_eq!(ids, ["F10", "F11"], "{findings:?}");
        assert!(findings.iter().all(|finding| finding.severity == "normal"));
        assert!(findings[0].text.starts_with("nativeMac CtrlK fixture mismatch"), "{findings:?}");
        assert!(findings[1].text.starts_with("strict contrast captureinstability"), "{findings:?}");
    }

    /// cas-2849: the issues qa_record received for cas-d1fa used id, title
    /// and description; the follow-ups are titled from that text and carry it
    /// as the problem, never "(not described)".
    #[test]
    fn titled_issues_fill_the_follow_up_cas_2849() {
        let (mut pre, delivery_issues) = split_qa_issues(Some(D1FA_ISSUES)).unwrap();
        assert_eq!(delivery_issues, 2);
        complete_pre_existing_issues(&mut pre, D1FA_LEDGER).unwrap();
        let delivery = task();
        let round = pass("bd3d3afd", cas_types::QaPassState::Failed);
        assert_eq!(pre[0].id, "F10");
        assert_eq!(
            follow_up_title(&delivery, &pre[1]),
            "Pre-existing: Strict contrast captures race existing color transitions (found in QA of cas-ui1)"
        );
        let body = follow_up_description(&delivery, &round, &pre[1], "/a/LEDGER.md");
        assert!(body.contains("visual-qa.mjs940 waits50ms"), "{body}");
        assert!(!body.contains("(not described)"), "{body}");
    }

    /// cas-2849: an issue that gives only its id takes its text from the
    /// ledger's "F10 NORMAL:" line.
    #[test]
    fn an_id_only_issue_takes_its_text_from_the_ledger_cas_2849() {
        let (mut pre, _) = split_qa_issues(Some(
            r#"[{"id":"F11","scope":"pre-existing","severity":"normal"}]"#,
        ))
        .unwrap();
        complete_pre_existing_issues(&mut pre, D1FA_LEDGER).unwrap();
        assert!(pre[0].problem.starts_with("strict contrast captureinstability"), "{pre:?}");
        assert!(
            follow_up_title(&task(), &pre[0]).starts_with("Pre-existing: strict contrast captureinstability"),
            "{}",
            follow_up_title(&task(), &pre[0])
        );
    }

    /// cas-2849: F10 names its existing follow-up cas-2a33, so no new task.
    #[test]
    fn a_finding_naming_an_existing_task_is_not_refiled_cas_2849() {
        let (mut pre, _) = split_qa_issues(Some(D1FA_ISSUES)).unwrap();
        complete_pre_existing_issues(&mut pre, D1FA_LEDGER).unwrap();
        let exists = |id: &str| id == "cas-2a33";
        assert_eq!(tracked_follow_up(&pre[0], "cas-d1fa", exists).as_deref(), Some("cas-2a33"));
        assert_eq!(tracked_follow_up(&pre[1], "cas-d1fa", exists), None);
        // The delivery itself is never its own follow-up.
        let own = PreExistingIssue {
            problem: "seen while reviewing cas-d1fa".to_string(),
            ..Default::default()
        };
        assert_eq!(tracked_follow_up(&own, "cas-d1fa", |_| true), None);
    }

    /// cas-2849: a pre-existing issue with no text anywhere is refused with
    /// what to add.
    #[test]
    fn a_pre_existing_issue_without_text_is_refused_cas_2849() {
        let (mut pre, _) =
            split_qa_issues(Some(r#"[{"scope":"pre-existing","severity":"normal"}]"#)).unwrap();
        let refusal = complete_pre_existing_issues(&mut pre, "# ledger\n").unwrap_err();
        assert!(refusal.contains("problem"), "{refusal}");
        assert!(refusal.contains("## Pre-existing"), "{refusal}");
        let (mut unknown, _) =
            split_qa_issues(Some(r#"[{"id":"F99","scope":"pre-existing"}]"#)).unwrap();
        assert!(complete_pre_existing_issues(&mut unknown, D1FA_LEDGER).unwrap_err().contains("F99"));
    }

    #[test]
    fn a_rejection_needs_an_issue_the_delivery_owns() {
        use cas_types::QaVerdict::*;
        let refusal = rejection_scope_refusal(Rejected, 2, 0).expect("only pre-existing issues");
        assert!(refusal.contains("never rejects a delivery"), "{refusal}");
        assert!(rejection_scope_refusal(Rejected, 2, 1).is_none());
        assert!(rejection_scope_refusal(Rejected, 0, 0).is_none());
        assert!(rejection_scope_refusal(Approved, 3, 0).is_none());
    }

    #[test]
    fn follow_up_task_names_the_defect_the_delivery_and_the_evidence() {
        let delivery = task();
        let round = pass("aaaa1111", cas_types::QaPassState::Passed);
        let issue = PreExistingIssue {
            severity: "normal".to_string(),
            problem: "Footer links fail contrast at 3.1:1. The base build too.".to_string(),
            suggestion: "use --ink-mid".to_string(),
            evidence: "F02.png".to_string(),
            ..Default::default()
        };
        assert_eq!(
            follow_up_title(&delivery, &issue),
            "Pre-existing: Footer links fail contrast at 3.1:1 (found in QA of cas-ui1)"
        );
        let body = follow_up_description(&delivery, &round, &issue, "/a/LEDGER.md");
        for pinned in [
            "Independent QA round 1 of cas-ui1 (Reply composer) @aaaa1111",
            "base build as well",
            "- Severity: normal",
            "- Suggested fix: use --ink-mid",
            "- Evidence: F02.png",
            "- Ledger: /a/LEDGER.md",
        ] {
            assert!(body.contains(pinned), "missing {pinned:?} in:\n{body}");
        }
        let long = PreExistingIssue {
            problem: "x".repeat(200),
            ..issue.clone()
        };
        assert!(follow_up_title(&delivery, &long).contains(&format!("{}...", "x".repeat(77))));
        let blank = PreExistingIssue {
            problem: String::new(),
            ..issue
        };
        assert!(follow_up_title(&delivery, &blank).contains("defect recorded by independent QA"));
        assert_eq!(follow_up_priority("Blocking"), cas_types::Priority::HIGH);
        assert_eq!(follow_up_priority("high"), cas_types::Priority::MEDIUM);
        assert_eq!(follow_up_priority("note"), cas_types::Priority::LOW);
    }

    /// cas-e371: a round whose tip shares every visual-QA finding with the
    /// base build backs its verdict with the scoped pair.
    #[test]
    fn round_bundle_accepts_a_scoped_visual_qa_pair() {
        let dir = tempfile::TempDir::new().unwrap();
        let ledger = dir.path().join("LEDGER.md");
        std::fs::write(&ledger, "# ledger").unwrap();
        let round = pass("aaaa1111", cas_types::QaPassState::Claimed);
        let later = round.requested_at + chrono::Duration::seconds(60);
        std::fs::create_dir_all(dir.path().join("visual-qa")).unwrap();
        std::fs::create_dir_all(dir.path().join("visual-qa-baseline")).unwrap();
        let finding = |port: u16| {
            serde_json::json!({
                "type": "insufficient-contrast", "selector": "footer a",
                "url": format!("http://127.0.0.1:{port}/?fixture=home"),
                "scheme": "light", "viewport": {"name": "phone", "width": 390, "height": 844}
            })
        };
        let report = |name: &str, port: u16, findings: serde_json::Value| {
            std::fs::write(
                dir.path().join(name),
                serde_json::json!({
                    "status": "FAIL", "strict": true, "generatedAt": later.to_rfc3339(),
                    "urls": [format!("http://127.0.0.1:{port}/?fixture=home")],
                    "findings": findings
                })
                .to_string(),
            )
            .unwrap();
        };
        report("visual-qa/visual-qa.json", 28511, serde_json::json!([finding(28511)]));
        report("visual-qa-baseline/visual-qa.json", 28512, serde_json::json!([finding(28512)]));
        let bundle = |files: serde_json::Value| {
            std::fs::write(
                dir.path().join("bundle.json"),
                serde_json::json!({
                    "schema": 1, "task_id": "cas-ui1", "producer": "independent-qa",
                    "head_sha": "aaaa1111", "visual_qa_status": "scoped", "files": files
                })
                .to_string(),
            )
            .unwrap();
        };
        bundle(serde_json::json!({"visual_qa_json": "visual-qa/visual-qa.json"}));
        let refused = validate_round_bundle(&ledger, &round, cas_types::QaVerdict::Approved).unwrap_err();
        assert!(refused.contains("visual_qa_baseline_json"), "{refused}");
        bundle(serde_json::json!({
            "visual_qa_json": "visual-qa/visual-qa.json",
            "visual_qa_baseline_json": "visual-qa-baseline/visual-qa.json"
        }));
        validate_round_bundle(&ledger, &round, cas_types::QaVerdict::Approved).expect("every tip finding is on the base build");
        // A finding the base build does not have is the delivery's.
        let mut introduced = finding(28511);
        introduced["selector"] = serde_json::json!("#send");
        report(
            "visual-qa/visual-qa.json",
            28511,
            serde_json::json!([finding(28511), introduced]),
        );
        let refused = validate_round_bundle(&ledger, &round, cas_types::QaVerdict::Approved).unwrap_err();
        assert!(refused.contains("introduced 1 visual-QA finding"), "{refused}");
        assert!(refused.contains("#send"), "{refused}");
    }

    #[test]
    fn globs_match_nested_and_root_paths_only() {
        let globs = paths(&["**/*.css", "hub-web/**"]);
        assert_eq!(
            first_user_facing_path(&paths(&["styles.css"]), &globs),
            Some(("styles.css", "**/*.css"))
        );
        assert_eq!(
            first_user_facing_path(&paths(&["hub-web/src/main.ts"]), &globs),
            Some(("hub-web/src/main.ts", "hub-web/**"))
        );
        assert_eq!(first_user_facing_path(&paths(&["cas-cli/src/lib.rs"]), &globs), None);
        assert_eq!(first_user_facing_path(&paths(&["a.css"]), &[]), None);
    }

    /// A repo where `factory/worker` changes the composer source and the
    /// committed build output beside it.
    fn dist_repo() -> (tempfile::TempDir, impl Fn(&[&str]) -> String) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().to_path_buf();
        let git = move |args: &[&str]| -> String {
            let output = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("GIT_AUTHOR_NAME", "CAS Test")
                .env("GIT_AUTHOR_EMAIL", "cas@example.test")
                .env("GIT_COMMITTER_NAME", "CAS Test")
                .env("GIT_COMMITTER_EMAIL", "cas@example.test")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}: {output:?}");
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        let write = |dir: &Path, path: &str, body: &str| {
            let full = dir.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, body).unwrap();
        };
        git(&["init", "-q", "-b", "main"]);
        write(dir.path(), "hub-web/src/composer.ts", "export const gap = 4;\n");
        write(dir.path(), "hub-web/dist/app.js", "gap=4\n");
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        git(&["switch", "-q", "-c", "factory/worker"]);
        write(dir.path(), "hub-web/src/composer.ts", "export const gap = 8;\n");
        write(dir.path(), "hub-web/dist/app.js", "gap=8\n");
        git(&["commit", "-q", "-am", "composer spacing"]);
        (dir, git)
    }

    /// cas-0c988: the reviewer passed T1. A sibling UI lane lands; the worker
    /// rebases and regenerates the build output (the dist now differs), the
    /// reviewed source does not. The verdict carries to T2, logged, and the
    /// merge gate accepts T2 with no new review.
    #[test]
    fn a_rebased_tip_with_the_same_source_patch_keeps_its_qa_verdict_cas_0c988() {
        let (dir, git) = dist_repo();
        let repo = dir.path();
        let cas = tempfile::tempdir().unwrap();
        let reviewed = git(&["rev-parse", "HEAD"]);
        let task = parked(&reviewed);
        dispatch(cas.path(), &reviewed);
        let now = chrono::Utc::now();
        cas_store::claim_qa_pass(cas.path(), "cas-ui1", "reviewer", now).unwrap();
        cas_store::resolve_qa_pass(
            cas.path(),
            "cas-ui1",
            "reviewer",
            cas_types::QaVerdict::Approved,
            "approved at 390 and 1280",
            None,
            "/ledger/LEDGER.md",
            now,
        )
        .unwrap();

        // A sibling lane lands on main: other source, other build output.
        git(&["switch", "-q", "main"]);
        std::fs::write(repo.join("hub-web/src/list.ts"), "export const rows = 5;\n").unwrap();
        std::fs::write(repo.join("hub-web/dist/app.js"), "gap=4 rows=5\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "sibling list"]);
        // The worker rebases; the dist conflicts and is regenerated.
        git(&["switch", "-q", "factory/worker"]);
        git(&["reset", "-q", "--hard", "main"]);
        std::fs::write(repo.join("hub-web/src/composer.ts"), "export const gap = 8;\n").unwrap();
        std::fs::write(repo.join("hub-web/dist/app.js"), "gap=8 rows=5\n").unwrap();
        git(&["commit", "-q", "-am", "composer spacing"]);
        let rebased = git(&["rev-parse", "HEAD"]);
        assert_ne!(rebased, reviewed);
        assert!(!is_rebased_copy(repo, &reviewed, &rebased, "main"), "the whole patch differs (dist)");
        assert_eq!(
            source_patch_id(repo, &reviewed, "main"),
            source_patch_id(repo, &rebased, "main"),
            "the reviewed source patch is byte-identical"
        );
        assert!(source_patch_id(repo, &rebased, "main").is_some());

        let passes = cas_store::list_qa_passes(cas.path(), "cas-ui1").unwrap();
        let qa = QaConfig::default();
        assert!(merge_gate(&task, &qa, &passes, &rebased).is_err(), "not yet carried");
        let carried = carry_verdict(cas.path(), repo, &task, &passes, &rebased, "main")
            .expect("the verdict carries to the rebased tip");
        assert_eq!(carried.bound_head, rebased);
        assert_eq!(carried.state, cas_types::QaPassState::Passed);
        assert_eq!(carried.reviewer_agent_id.as_deref(), Some("reviewer"));
        assert_eq!(carried.ledger_path.as_deref(), Some("/ledger/LEDGER.md"));
        let summary = carried.summary.clone().unwrap_or_default();
        assert!(summary.contains(&format!("carried over from @{}", &reviewed[..8])), "{summary}");
        let passes = cas_store::list_qa_passes(cas.path(), "cas-ui1").unwrap();
        assert_eq!(merge_gate(&task, &qa, &passes, &rebased), Ok(()));
        // Idempotent: the tip is covered now, nothing more is recorded.
        assert!(carry_verdict(cas.path(), repo, &task, &passes, &rebased, "main").is_none());
    }

    /// cas-0c988: a rebase that also changed the reviewed source keeps no
    /// verdict; a failed round never carries.
    #[test]
    fn a_changed_source_patch_or_a_failed_round_does_not_carry_cas_0c988() {
        let (dir, git) = dist_repo();
        let repo = dir.path();
        let cas = tempfile::tempdir().unwrap();
        let reviewed = git(&["rev-parse", "HEAD"]);
        let task = parked(&reviewed);
        dispatch(cas.path(), &reviewed);
        let now = chrono::Utc::now();
        cas_store::claim_qa_pass(cas.path(), "cas-ui1", "reviewer", now).unwrap();
        cas_store::resolve_qa_pass(cas.path(), "cas-ui1", "reviewer",
            cas_types::QaVerdict::Approved, "ok", None, "/l/LEDGER.md", now).unwrap();
        std::fs::write(repo.join("hub-web/src/composer.ts"), "export const gap = 12;\n").unwrap();
        git(&["commit", "-q", "-a", "--amend", "-m", "composer spacing, retuned"]);
        let changed = git(&["rev-parse", "HEAD"]);
        assert_ne!(source_patch_id(repo, &reviewed, "main"), source_patch_id(repo, &changed, "main"));
        let passes = cas_store::list_qa_passes(cas.path(), "cas-ui1").unwrap();
        assert!(carry_verdict(cas.path(), repo, &task, &passes, &changed, "main").is_none());

        let mut failed = passes.clone();
        for pass in &mut failed {
            pass.state = cas_types::QaPassState::Failed;
        }
        // Same source as the reviewed tip again, under a new sha.
        std::fs::write(repo.join("hub-web/src/composer.ts"), "export const gap = 8;\n").unwrap();
        git(&["commit", "-q", "-a", "--amend", "-m", "composer spacing, again"]);
        let same_source = git(&["rev-parse", "HEAD"]);
        assert_ne!(same_source, reviewed);
        assert_eq!(source_patch_id(repo, &reviewed, "main"), source_patch_id(repo, &same_source, "main"));
        assert!(carry_verdict(cas.path(), repo, &task, &failed, &same_source, "main").is_none());
    }
}
