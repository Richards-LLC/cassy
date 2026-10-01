//! cas-2ee2 (GH #1023 finding 3): the independent QA gate on the GitHub side.
//!
//! `worktree_merge` and a supervisor's raw `git merge factory/<w>` already
//! wait for a passed or waived round. A pull request merged on GitHub skipped
//! both: `gh pr merge 2546` names a PR number, not a factory branch, so the
//! pre-tool guard never recognised it (cas-a6cf merged to staging before its
//! QA dispatch was read). This module closes that path twice:
//!
//! 1. **Agent side (always on).** [`github_merge_refusal`] recognises
//!    `gh pr merge`, a `PUT …/pulls/<n>/merge` through `gh api` or `curl`,
//!    and the GraphQL merge mutations. It maps the PR to its delivery through
//!    the recorded `delivery_pr_number` or a bounded `gh pr view`, then applies
//!    the same [`merge_gate`] at the PR's head commit.
//! 2. **Repository side (opt-in, `qa.github_status`).** Cassy publishes the
//!    commit status [`QA_STATUS_CONTEXT`] on every delivered head: pending
//!    while a round is open, success once it passes or is waived (or when the
//!    delivery needs no independent QA), failure on a rejection or timeout.
//!    With that context required by branch protection, a merge from the web UI
//!    or any other client is held to the same verdict. Setup:
//!    `docs/qa/independent-qa-pass.md` ("GitHub required check").

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cas_types::{QaPass, QaPassState, Task, TaskStatus};

use super::{branch_binds_task, gate_applies, merge_gate};
use crate::config::QaConfig;

/// Commit status context branch protection requires.
pub const QA_STATUS_CONTEXT: &str = "cassy/independent-qa";

/// Overrides the `gh` executable (tests, or a pinned install).
pub const GH_BIN_ENV: &str = "CAS_QA_GH";

/// Bound on one `gh pr view` made from the pre-tool hook.
const GH_LOOKUP_TIMEOUT: Duration = Duration::from_secs(8);
/// Bound on one background status publication.
const GH_PUBLISH_TIMEOUT: Duration = Duration::from_secs(20);
/// GitHub rejects commit status descriptions longer than this.
const STATUS_DESCRIPTION_MAX: usize = 140;

/// How a raw GitHub merge names its pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeSelector {
    /// `gh pr merge` with no argument: the PR of the checked-out branch.
    CurrentBranch,
    Number(u64),
    Branch(String),
    /// A merge the command alone cannot map to a PR, such as a GraphQL
    /// `mergePullRequest` mutation keyed by node id.
    Unresolvable(&'static str),
}

/// One raw GitHub merge found in a shell command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubMerge {
    pub selector: MergeSelector,
    /// `OWNER/REPO` when the command names one.
    pub repo: Option<String>,
    /// `--match-head-commit`: the exact head the caller insists on.
    pub match_head: Option<String>,
    /// The statement, for the refusal text.
    pub statement: String,
}

fn gh_bin() -> std::ffi::OsString {
    std::env::var_os(GH_BIN_ENV)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "gh".into())
}

fn is_gh(token: &str) -> bool {
    token == "gh" || token.ends_with("/gh")
}

/// Raw GitHub merges a shell command would perform (cas-2ee2 pre-tool guard).
/// `gh pr merge --disable-auto` merges nothing and is not reported.
pub fn github_merges_in(command: &str) -> Vec<GithubMerge> {
    let mut merges = Vec::new();
    for statement in command.split(['\n', ';', '|', '&']) {
        let tokens: Vec<&str> = statement
            .split_whitespace()
            .map(|token| token.trim_matches(|ch: char| matches!(ch, '\'' | '"' | '`' | '(' | ')')))
            .filter(|token| !token.is_empty())
            .collect();
        let statement_text = statement.trim().to_string();
        if let Some(gh) = tokens.iter().position(|token| is_gh(token))
            && tokens.get(gh + 1) == Some(&"pr")
            && tokens.get(gh + 2) == Some(&"merge")
        {
            if let Some(merge) = parse_pr_merge(&tokens[gh + 3..], &statement_text) {
                merges.push(merge);
            }
            continue;
        }
        if let Some((number, repo)) = put_pull_merge(&tokens) {
            merges.push(GithubMerge {
                selector: MergeSelector::Number(number),
                repo,
                match_head: None,
                statement: statement_text,
            });
            continue;
        }
        if ["mergePullRequest", "enablePullRequestAutoMerge"]
            .iter()
            .any(|mutation| statement.contains(mutation))
        {
            merges.push(GithubMerge {
                selector: MergeSelector::Unresolvable("a GraphQL merge mutation names no branch"),
                repo: None,
                match_head: None,
                statement: statement_text,
            });
        }
    }
    merges
}

fn parse_pr_merge(args: &[&str], statement: &str) -> Option<GithubMerge> {
    const VALUE_FLAGS: [&str; 11] = [
        "-R",
        "--repo",
        "-b",
        "--body",
        "-F",
        "--body-file",
        "-t",
        "--subject",
        "-A",
        "--author-email",
        "--match-head-commit",
    ];
    let mut selector = None;
    let mut repo = None;
    let mut match_head = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index];
        if arg == "--disable-auto" {
            return None;
        }
        if let Some(value) = arg.strip_prefix("--repo=") {
            repo = Some(value.to_string());
        } else if let Some(value) = arg.strip_prefix("--match-head-commit=") {
            match_head = Some(value.to_string());
        } else if VALUE_FLAGS.contains(&arg) {
            let value = args.get(index + 1).copied();
            match arg {
                "-R" | "--repo" => repo = value.map(str::to_string),
                "--match-head-commit" => match_head = value.map(str::to_string),
                _ => {}
            }
            index += 2;
            continue;
        } else if !arg.starts_with('-') && selector.is_none() {
            let (parsed, url_repo) = classify_pr_argument(arg);
            selector = Some(parsed);
            if repo.is_none() {
                repo = url_repo;
            }
        }
        index += 1;
    }
    Some(GithubMerge {
        selector: selector.unwrap_or(MergeSelector::CurrentBranch),
        repo,
        match_head,
        statement: statement.to_string(),
    })
}

/// `gh pr merge` accepts a number, `#number`, a PR URL, or a head branch
/// (optionally `OWNER:branch`).
fn classify_pr_argument(arg: &str) -> (MergeSelector, Option<String>) {
    let bare = arg.trim_start_matches('#');
    if let Ok(number) = bare.parse::<u64>() {
        return (MergeSelector::Number(number), None);
    }
    if let Some((prefix, rest)) = arg.split_once("/pull/") {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(number) = digits.parse::<u64>() {
            let repo = prefix
                .rsplit('/')
                .take(2)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("/");
            return (
                MergeSelector::Number(number),
                repo.contains('/').then_some(repo),
            );
        }
    }
    let branch = arg.rsplit_once(':').map_or(arg, |(_, branch)| branch);
    (MergeSelector::Branch(branch.to_string()), None)
}

/// A `PUT …/pulls/<n>/merge` REST call (`gh api`, `curl`, …).
fn put_pull_merge(tokens: &[&str]) -> Option<(u64, Option<String>)> {
    let is_put = tokens.iter().enumerate().any(|(index, token)| {
        let upper = token.to_ascii_uppercase();
        upper == "-XPUT"
            || upper == "--METHOD=PUT"
            || upper == "--REQUEST=PUT"
            || (matches!(upper.as_str(), "-X" | "--METHOD" | "--REQUEST")
                && tokens
                    .get(index + 1)
                    .is_some_and(|next| next.eq_ignore_ascii_case("PUT")))
    });
    if !is_put {
        return None;
    }
    tokens.iter().find_map(|token| {
        let (before, after) = token.split_once("pulls/")?;
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        let rest = &after[digits.len()..];
        if digits.is_empty()
            || !(rest == "/merge" || rest.starts_with("/merge?") || rest.starts_with("/merge/"))
        {
            return None;
        }
        let repo = before
            .split_once("repos/")
            .map(|(_, repo)| repo.trim_end_matches('/').to_string())
            .filter(|repo| repo.split('/').count() == 2 && !repo.contains('{'));
        Some((digits.parse().ok()?, repo))
    })
}

/// Run a command with a hard deadline; stdout on success.
fn run_bounded(command: Command, timeout: Duration) -> Option<Vec<u8>> {
    run_bounded_detailed(command, timeout).ok()
}

/// [`run_bounded`] that says why it produced nothing: the program could not
/// start, it timed out, or it exited non-zero (with its first stderr line).
fn run_bounded_detailed(mut command: Command, timeout: Duration) -> Result<Vec<u8>, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("`{program}` could not be started: {error}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    let mut stderr = String::new();
                    if let Some(mut pipe) = child.stderr.take() {
                        let _ = pipe.read_to_string(&mut stderr);
                    }
                    let detail: String = stderr
                        .lines()
                        .map(str::trim)
                        .find(|line| !line.is_empty())
                        .unwrap_or("no error output")
                        .chars()
                        .take(200)
                        .collect();
                    return Err(format!("`{program}` exited with {status}: {detail}"));
                }
                let mut stdout = Vec::new();
                child
                    .stdout
                    .take()
                    .ok_or_else(|| format!("`{program}` produced no output"))?
                    .read_to_end(&mut stdout)
                    .map_err(|error| format!("reading `{program}` output failed: {error}"))?;
                return Ok(stdout);
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "`{program}` did not answer within {}s",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("waiting for `{program}` failed: {error}"));
            }
        }
    }
}

/// The PR's head branch and commit, from GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PrHead {
    branch: String,
    sha: String,
}

/// Look the PR up with `gh pr view`; the error says why it could not be.
fn gh_pr_head(cwd: &Path, repo: Option<&str>, selector: Option<&str>) -> Result<PrHead, String> {
    let mut command = Command::new(gh_bin());
    command.args(["pr", "view"]);
    if let Some(selector) = selector {
        command.arg(selector);
    }
    if let Some(repo) = repo {
        command.args(["--repo", repo]);
    }
    command
        .args(["--json", "headRefName,headRefOid"])
        .current_dir(cwd);
    let stdout = run_bounded_detailed(command, GH_LOOKUP_TIMEOUT)?;
    let value: serde_json::Value = serde_json::from_slice(&stdout)
        .map_err(|error| format!("`gh pr view` returned unreadable JSON: {error}"))?;
    let field = |name: &str| {
        value
            .get(name)
            .and_then(|field| field.as_str())
            .map(|field| field.trim().to_string())
            .filter(|field| !field.is_empty())
    };
    match (field("headRefName"), field("headRefOid")) {
        (Some(branch), Some(sha)) => Ok(PrHead { branch, sha }),
        _ => Err("`gh pr view` returned no headRefName/headRefOid".to_string()),
    }
}

/// `owner/repo`, lowercased, from `OWNER/REPO`, `HOST/OWNER/REPO`, or a remote
/// URL (`https://…`, `git@host:…`). Local-path remotes have no slug.
fn repo_slug(value: &str) -> Option<String> {
    let normalized =
        crate::cloud::normalize_git_remote_url(value).unwrap_or_else(|| value.trim().to_string());
    let normalized = normalized.trim_end_matches('/');
    let normalized = normalized.strip_suffix(".git").unwrap_or(normalized);
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    match parts.as_slice() {
        [.., owner, repo] if !owner.contains(':') => {
            Some(format!("{owner}/{repo}").to_ascii_lowercase())
        }
        _ => None,
    }
}

/// The GitHub `owner/repo` of a checkout's `origin`, when it has one.
fn origin_slug(dir: &Path) -> Option<String> {
    repo_slug(&git_output(dir, &["remote", "get-url", "origin"])?)
}

fn git_output(cwd: &Path, args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|value| !value.is_empty() && value != "HEAD")
}

/// Pre-tool guard for raw GitHub merges, for every role: the refusal when a
/// merge would integrate a QA-bound delivery whose PR head has no passed or
/// waived round. The PR is looked up only while some unclosed delivery has a
/// recorded round, so ordinary merges never reach GitHub from the hook.
/// Unreadable Cassy state fails open to the close backstop, like the git
/// guard; a PR that cannot be mapped while a round is open fails closed.
pub fn github_merge_refusal(cas_root: &Path, cwd: &Path, command: &str) -> Option<String> {
    let merges = github_merges_in(command);
    if merges.is_empty() {
        return None;
    }
    let config = crate::config::Config::load(cas_root).ok()?;
    let qa = config.qa();
    if !qa.independent_pass {
        return None;
    }
    let tasks = crate::store::open_task_store(cas_root)
        .ok()?
        .list(None)
        .ok()?;
    let gated: Vec<(Task, Vec<QaPass>)> = tasks
        .into_iter()
        .filter(|task| task.status != TaskStatus::Closed)
        .filter_map(|task| {
            let passes = cas_store::list_qa_passes(cas_root, &task.id).unwrap_or_default();
            gate_applies(&task, &qa, &passes).then_some((task, passes))
        })
        .collect();
    if gated.is_empty() {
        return None;
    }
    // cas-0169: a round belongs to the repository its delivery lives in. A
    // task whose repository has no GitHub origin stays in scope (fail closed).
    let task_slugs: Vec<Option<String>> = gated
        .iter()
        .map(|(task, _)| origin_slug(&task_repo(cas_root, task)))
        .collect();
    merges
        .iter()
        .find_map(|merge| {
            // `gh` merges in `--repo`, else in the repository it runs in.
            let target = merge
                .repo
                .as_deref()
                .and_then(repo_slug)
                .or_else(|| origin_slug(cwd));
            let scoped: Vec<(Task, Vec<QaPass>)> = gated
                .iter()
                .zip(&task_slugs)
                .filter(|(_, slug)| match (&target, slug) {
                    (Some(target), Some(slug)) => target == slug,
                    _ => true,
                })
                .map(|(entry, _)| entry.clone())
                .collect();
            if scoped.is_empty() {
                return None;
            }
            refusal_for_merge(cwd, &qa, &scoped, merge)
        })
        .map(|refusal| format!("🚫 {refusal}"))
}

fn refusal_for_merge(
    cwd: &Path,
    qa: &QaConfig,
    gated: &[(Task, Vec<QaPass>)],
    merge: &GithubMerge,
) -> Option<String> {
    let repo = merge.repo.as_deref();
    let (pr_number, lookup) = match &merge.selector {
        MergeSelector::Unresolvable(why) => (None, Err((*why).to_string())),
        MergeSelector::Number(number) => (
            Some(*number),
            gh_pr_head(cwd, repo, Some(&number.to_string())),
        ),
        MergeSelector::Branch(branch) => (None, gh_pr_head(cwd, repo, Some(branch))),
        MergeSelector::CurrentBranch => (None, gh_pr_head(cwd, repo, None)),
    };
    let (resolved, lookup_error) = match lookup {
        Ok(head) => (Some(head), None),
        Err(error) => (None, Some(error)),
    };
    let head_branch = resolved
        .as_ref()
        .map(|head| head.branch.clone())
        .or_else(|| match &merge.selector {
            MergeSelector::Branch(branch) => Some(branch.clone()),
            MergeSelector::CurrentBranch => git_output(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]),
            _ => None,
        });
    let head_sha = merge
        .match_head
        .clone()
        .or_else(|| resolved.as_ref().map(|head| head.sha.clone()));
    let bound: Vec<&(Task, Vec<QaPass>)> = gated
        .iter()
        .filter(|(task, passes)| {
            pr_number.is_some_and(|number| task.deliverables.delivery_pr_number == Some(number))
                || head_branch.as_deref().is_some_and(|branch| {
                    branch_binds_task(
                        task,
                        qa,
                        passes,
                        branch,
                        branch.trim_start_matches("factory/"),
                    )
                })
        })
        .collect();

    if bound.is_empty() {
        if head_branch.is_some() {
            // The PR is known and no recorded QA round binds its branch.
            return None;
        }
        let open: Vec<String> = gated
            .iter()
            .filter_map(|(task, passes)| {
                let latest = passes.iter().find(|pass| !pass.is_withdrawn())?;
                (!latest.state.satisfies_gate()).then(|| {
                    format!(
                        "{} (round {} {}, pass {})",
                        task.id, latest.round, latest.state, latest.id
                    )
                })
            })
            .collect();
        if open.is_empty() {
            return None;
        }
        if let MergeSelector::Unresolvable(why) = &merge.selector {
            return Some(format!(
                "INDEPENDENT QA REQUIRED: Cassy cannot tell which delivery `{}` merges ({why}), \
                 and independent QA is still open for {}. Wait for the verdict, name the PR by number \
                 or branch, or check with `{prefix}verification action=qa_status task_id=<task>`.",
                merge.statement,
                open.join(", "),
                prefix = crate::mcp::tools::core::guidance::supervisor_prefix(),
            ));
        }
        // cas-0169: the lookup failed, so nothing ties this PR to any open
        // round. Name the failure; listing the open rounds would blame tasks
        // the PR may have nothing to do with.
        let failure = lookup_error
            .as_deref()
            .unwrap_or("no head branch was returned");
        return Some(format!(
            "INDEPENDENT QA REQUIRED: Cassy could not look up the head of the PR `{}` merges, \
             so it cannot rule out a delivery whose independent QA is still open \
             ({} open round(s) in this repository). Lookup failure: {failure}. \
             Fix the lookup and retry, or name the PR by its head branch. The hook runs `gh` \
             in its own environment: a login that exists only inside `bash -ic` is invisible \
             to it, so export GH_TOKEN where the hook can see it.",
            merge.statement,
            open.len(),
        ));
    }

    for (task, passes) in bound {
        let head = head_sha
            .clone()
            .or_else(|| {
                head_branch
                    .as_deref()
                    .and_then(|branch| git_output(cwd, &["rev-parse", "--verify", branch]))
            })
            .or_else(|| {
                task.deliverables
                    .parked_branch
                    .as_deref()
                    .or_else(|| {
                        passes
                            .iter()
                            .find(|pass| !pass.is_withdrawn())
                            .map(|pass| pass.branch.as_str())
                    })
                    .and_then(|branch| git_output(cwd, &["rev-parse", "--verify", branch]))
            });
        let refusal = match head {
            Some(head) => merge_gate(task, qa, passes, &head).err(),
            None => Some(format!(
                "INDEPENDENT QA REQUIRED before {} merges, and the head of `{}` could not be resolved here.",
                task.id, merge.statement
            )),
        };
        if let Some(refusal) = refusal {
            return Some(format!(
                "{refusal} A raw GitHub merge (`gh pr merge`, the REST or GraphQL merge API) \
                 is held to the same gate as `worktree_merge`."
            ));
        }
    }
    None
}

/// GitHub commit status states Cassy publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QaCommitState {
    Pending,
    Success,
    Failure,
}

impl QaCommitState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Success => "success",
            Self::Failure => "failure",
        }
    }
}

/// The status a round puts on its `bound_head`. A superseded round's head is
/// no longer the delivery, so it publishes nothing.
pub fn qa_commit_status(pass: &QaPass) -> Option<(QaCommitState, String)> {
    let (state, description) = match pass.state {
        QaPassState::Pending | QaPassState::Claimed => (
            QaCommitState::Pending,
            format!(
                "Independent QA round {} is {} (pass {}); merge waits for its verdict",
                pass.round, pass.state, pass.id
            ),
        ),
        QaPassState::Passed => (
            QaCommitState::Success,
            format!(
                "Independent QA round {} passed (pass {})",
                pass.round, pass.id
            ),
        ),
        QaPassState::Waived => (
            QaCommitState::Success,
            format!(
                "Independent QA waived by supervisor: {}",
                pass.summary.as_deref().unwrap_or("no reason recorded")
            ),
        ),
        QaPassState::Failed => (
            QaCommitState::Failure,
            format!(
                "Independent QA round {} rejected (pass {})",
                pass.round, pass.id
            ),
        ),
        QaPassState::TimedOut => (
            QaCommitState::Failure,
            format!(
                "Independent QA round {} timed out; redispatch or waive (pass {})",
                pass.round, pass.id
            ),
        ),
        QaPassState::Superseded => return None,
    };
    Some((state, truncate_description(&description)))
}

fn truncate_description(description: &str) -> String {
    if description.chars().count() <= STATUS_DESCRIPTION_MAX {
        return description.to_string();
    }
    let mut short: String = description
        .chars()
        .take(STATUS_DESCRIPTION_MAX - 1)
        .collect();
    short.push('…');
    short
}

fn github_status_enabled(cas_root: &Path) -> bool {
    crate::config::Config::load(cas_root)
        .map(|config| config.qa().github_status)
        .unwrap_or(false)
}

/// The checkout a task's delivery lives in, for `gh`'s `{owner}/{repo}`.
fn delivery_repo(cas_root: &Path, task_id: &str) -> PathBuf {
    crate::store::open_task_store(cas_root)
        .ok()
        .and_then(|store| store.get(task_id).ok())
        .map(|task| task_repo(cas_root, &task))
        .unwrap_or_else(|| cas_root.parent().unwrap_or(cas_root).to_path_buf())
}

/// [`delivery_repo`] for a task already in hand.
fn task_repo(cas_root: &Path, task: &Task) -> PathBuf {
    task.deliverables
        .work_target
        .as_ref()
        .and_then(|target| {
            crate::mcp::tools::core::task::repo_context::resolve_repo_context(cas_root, target).ok()
        })
        .map(|repo| repo.repo_root)
        .unwrap_or_else(|| cas_root.parent().unwrap_or(cas_root).to_path_buf())
}

/// Publish `pass`'s status on its head when `qa.github_status` is on.
/// Background and bounded: it never delays the park, verdict or waiver that
/// triggered it. A lost publication leaves the required check missing or
/// pending, which GitHub treats as "not mergeable", so failure is safe.
pub fn publish_pass_status(cas_root: &Path, pass: &QaPass) {
    if !github_status_enabled(cas_root) {
        return;
    }
    let Some((state, description)) = qa_commit_status(pass) else {
        return;
    };
    let repo = delivery_repo(cas_root, &pass.task_id);
    spawn_publish(repo, pass.bound_head.clone(), state, description);
}

/// A delivery that needs no independent QA still needs the required check,
/// or its PR could never merge.
pub fn publish_not_required(cas_root: &Path, repo: &Path, head: &str) {
    if !github_status_enabled(cas_root) {
        return;
    }
    spawn_publish(
        repo.to_path_buf(),
        head.to_string(),
        QaCommitState::Success,
        "Independent QA not required for this delivery".to_string(),
    );
}

/// The `gh api` arguments that publish one status on the current repo.
pub fn status_publish_args(sha: &str, state: QaCommitState, description: &str) -> Vec<String> {
    vec![
        "api".to_string(),
        "--method".to_string(),
        "POST".to_string(),
        format!("repos/{{owner}}/{{repo}}/statuses/{sha}"),
        "-f".to_string(),
        format!("state={}", state.as_str()),
        "-f".to_string(),
        format!("context={QA_STATUS_CONTEXT}"),
        "-f".to_string(),
        format!("description={description}"),
    ]
}

fn spawn_publish(repo: PathBuf, sha: String, state: QaCommitState, description: String) {
    // Resolve the executable on the caller's thread, from the caller's env.
    let gh = gh_bin();
    let spawned = std::thread::Builder::new()
        .name("cas-qa-github-status".to_string())
        .spawn(move || {
            let mut command = Command::new(gh);
            command
                .args(status_publish_args(&sha, state, &description))
                .current_dir(&repo);
            match run_bounded(command, GH_PUBLISH_TIMEOUT) {
                Some(_) => tracing::info!(
                    target: "cas::qa",
                    sha = %sha,
                    state = state.as_str(),
                    context = QA_STATUS_CONTEXT,
                    "cas-2ee2: published independent QA commit status"
                ),
                None => tracing::warn!(
                    target: "cas::qa",
                    sha = %sha,
                    state = state.as_str(),
                    repo = %repo.display(),
                    "cas-2ee2: independent QA commit status not published; the required check stays unmet"
                ),
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(target: "cas::qa", %error, "cas-2ee2: status publisher thread not started");
    }
}

/// cas-2ee2: the hold a merge request carries while its tip has no passed or
/// waived round, placed ahead of the worker's text so the supervisor reads
/// the QA state before any merge guidance.
pub fn merge_request_qa_hold(cas_root: &Path, task: &Task, head: &str) -> Option<String> {
    let config = crate::config::Config::load(cas_root).ok()?;
    let qa = config.qa();
    let passes = cas_store::list_qa_passes(cas_root, &task.id).ok()?;
    let refusal = merge_gate(task, &qa, &passes, head).err()?;
    Some(format!(
        "⏸ QA HOLD — not mergeable yet. {refusal} Do not merge this delivery by any path \
         (`worktree_merge`, `git merge`, `gh pr merge`, the GitHub UI or API) until \
         qa_status shows the round passed or waived."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(command: &str) -> GithubMerge {
        let merges = github_merges_in(command);
        assert_eq!(merges.len(), 1, "{command}: {merges:?}");
        merges.into_iter().next().unwrap()
    }

    #[test]
    fn gh_pr_merge_selectors_are_recognised() {
        assert_eq!(
            only("gh pr merge 2546 --squash").selector,
            MergeSelector::Number(2546)
        );
        assert_eq!(
            only("gh pr merge '#2546' -s -d").selector,
            MergeSelector::Number(2546)
        );
        assert_eq!(
            only("gh pr merge --auto --merge").selector,
            MergeSelector::CurrentBranch
        );
        assert_eq!(
            only("gh pr merge factory/zealous-cheetah-52 --rebase").selector,
            MergeSelector::Branch("factory/zealous-cheetah-52".to_string())
        );
        assert_eq!(
            only("gh pr merge acme:factory/w-1").selector,
            MergeSelector::Branch("factory/w-1".to_string())
        );
        let url = only("gh pr merge https://github.com/acme/gabber/pull/2546 --squash");
        assert_eq!(url.selector, MergeSelector::Number(2546));
        assert_eq!(url.repo.as_deref(), Some("acme/gabber"));
        let flagged = only(
            "cd /repo && /usr/bin/gh pr merge -R acme/gabber --body 'x' --match-head-commit abc123 77",
        );
        assert_eq!(flagged.selector, MergeSelector::Number(77));
        assert_eq!(flagged.repo.as_deref(), Some("acme/gabber"));
        assert_eq!(flagged.match_head.as_deref(), Some("abc123"));
    }

    #[test]
    fn rest_and_graphql_merges_are_recognised() {
        let rest = only("gh api -X PUT repos/acme/gabber/pulls/2546/merge -f merge_method=squash");
        assert_eq!(rest.selector, MergeSelector::Number(2546));
        assert_eq!(rest.repo.as_deref(), Some("acme/gabber"));
        let templated = only("gh api --method PUT repos/{owner}/{repo}/pulls/12/merge");
        assert_eq!(templated.selector, MergeSelector::Number(12));
        assert_eq!(templated.repo, None);
        let curl = only(
            "curl -sS -X PUT -H 'Authorization: token t' https://api.github.com/repos/acme/gabber/pulls/9/merge",
        );
        assert_eq!(curl.selector, MergeSelector::Number(9));
        assert!(matches!(
            only("gh api graphql -f query='mutation { mergePullRequest(input: {pullRequestId: \"PR_x\"}) { clientMutationId } }'").selector,
            MergeSelector::Unresolvable(_)
        ));
    }

    #[test]
    fn non_merges_are_ignored() {
        for command in [
            "gh pr merge 12 --disable-auto",
            "gh pr view 2546",
            "gh pr checks 2546",
            "gh api repos/acme/gabber/pulls/2546/merge",
            "gh api repos/acme/gabber/pulls/2546",
            "gh api -X PUT repos/acme/gabber/pulls/2546/requested_reviewers",
            "git merge factory/w",
        ] {
            let merges = github_merges_in(command);
            assert!(merges.is_empty(), "{command}: {merges:?}");
        }
    }

    fn pass(state: QaPassState) -> QaPass {
        QaPass {
            id: "qap-1".to_string(),
            task_id: "cas-ui1".to_string(),
            round: 2,
            implementer_agent_id: "impl".to_string(),
            branch: "factory/impl".to_string(),
            bound_head: "aaaa1111bbbb".to_string(),
            qa_task_id: Some("cas-qa1".to_string()),
            reviewer_agent_id: None,
            state,
            summary: Some("hotfix; reviewed live".to_string()),
            issues_json: None,
            ledger_path: None,
            issuer_agent_id: None,
            requested_at: chrono::Utc::now(),
            deadline_at: chrono::Utc::now(),
            resolved_at: None,
        }
    }

    #[test]
    fn commit_status_follows_the_round() {
        use QaPassState::*;
        let state = |s| qa_commit_status(&pass(s)).map(|(state, _)| state);
        assert_eq!(state(Pending), Some(QaCommitState::Pending));
        assert_eq!(state(Claimed), Some(QaCommitState::Pending));
        assert_eq!(state(Passed), Some(QaCommitState::Success));
        assert_eq!(state(Waived), Some(QaCommitState::Success));
        assert_eq!(state(Failed), Some(QaCommitState::Failure));
        assert_eq!(state(TimedOut), Some(QaCommitState::Failure));
        assert_eq!(state(Superseded), None);
        let (_, waived) = qa_commit_status(&pass(Waived)).unwrap();
        assert!(
            waived.contains("waived by supervisor: hotfix; reviewed live"),
            "{waived}"
        );
        let mut long = pass(Waived);
        long.summary = Some("x".repeat(400));
        let (_, description) = qa_commit_status(&long).unwrap();
        assert_eq!(description.chars().count(), STATUS_DESCRIPTION_MAX);
    }

    #[test]
    fn status_publication_targets_the_required_context() {
        let args = status_publish_args("abc123", QaCommitState::Pending, "round open");
        assert_eq!(
            args,
            vec![
                "api",
                "--method",
                "POST",
                "repos/{owner}/{repo}/statuses/abc123",
                "-f",
                "state=pending",
                "-f",
                "context=cassy/independent-qa",
                "-f",
                "description=round open",
            ]
        );
    }

    #[test]
    fn repo_slugs_compare_owner_and_name_across_spellings() {
        for value in [
            "Richards-LLC/cassy",
            "github.com/richards-llc/cassy",
            "https://github.com/Richards-LLC/cassy.git",
            "git@github.com:Richards-LLC/cassy.git",
            "ssh://git@github.com/Richards-LLC/cassy/",
        ] {
            assert_eq!(
                repo_slug(value).as_deref(),
                Some("richards-llc/cassy"),
                "{value}"
            );
        }
        assert_eq!(repo_slug("cassy"), None);
        assert_eq!(repo_slug(""), None);
        assert_ne!(
            repo_slug("Richards-LLC/petra-stella-cloud"),
            repo_slug("Richards-LLC/cassy")
        );
    }

    #[cfg(unix)]
    #[test]
    fn bounded_lookup_failures_say_why() {
        let mut failing = Command::new("sh");
        failing.args(["-c", "echo 'HTTP 401: Bad credentials' >&2; exit 4"]);
        let error = run_bounded_detailed(failing, Duration::from_secs(5)).unwrap_err();
        assert!(
            error.contains("exited with") && error.contains("HTTP 401: Bad credentials"),
            "{error}"
        );

        let missing = Command::new("/nonexistent/cas-0169-gh");
        let error = run_bounded_detailed(missing, Duration::from_secs(5)).unwrap_err();
        assert!(error.contains("could not be started"), "{error}");

        let mut slow = Command::new("sh");
        slow.args(["-c", "sleep 5"]);
        let error = run_bounded_detailed(slow, Duration::from_millis(100)).unwrap_err();
        assert!(error.contains("did not answer within"), "{error}");

        let mut ok = Command::new("sh");
        ok.args(["-c", "printf done"]);
        assert_eq!(
            run_bounded_detailed(ok, Duration::from_secs(5)).unwrap(),
            b"done"
        );
    }
}
