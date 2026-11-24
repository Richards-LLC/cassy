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
//!    the same [`merge_gate`] at the PR's head commit. Raw `gh pr merge`
//!    commands additionally refuse observed pending/failed PR-head checks,
//!    independently of QA settings and GitHub branch protection.
//! 2. **Repository side (opt-in, `qa.github_status`).** Cassy publishes the
//!    commit status [`QA_STATUS_CONTEXT`] on every delivered head: pending
//!    while a round is open, success once it passes or is waived (or when the
//!    delivery needs no independent QA), failure on a rejection or timeout.
//!    With that context required by branch protection, a merge from the web UI
//!    or any other client is held to the same verdict. Setup:
//!    `docs/qa/independent-qa-pass.md` ("GitHub required check").

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use cas_types::{QaPass, QaPassState, Task, TaskStatus};

use super::{branch_binds_task, gate_applies, merge_gate};
use crate::bounded_process::{BoundedCommandError, Deadline, run_command};
use crate::config::QaConfig;

mod verifier;

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
    PullRequestNodes { ids: Vec<String>, host: Option<String> },
    Unresolvable(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeTransport { PrCommand, Api }

/// One raw GitHub merge found in a shell command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubMerge {
    transport: MergeTransport,
    pub selector: MergeSelector,
    /// `[HOST/]OWNER/REPO` when the command names one.
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

/// Programs whose here-document is data, not code: text in their bodies is
/// never executed, so it cannot be a merge. Shells, `ssh`, `sudo`,
/// interpreters (`python3`, `node`, … run their body as a program) and
/// anything unknown keep their bodies (fail closed).
const HEREDOC_DATA_CONSUMERS: [&str; 13] = [
    "cat", "tee", "git", "jq", "sed", "awk", "grep", "wc", "diff", "patch", "psql", "sqlite3",
    "rustfmt",
];

/// The command with here-document bodies removed when the program reading
/// them is a known data consumer (cas-0169: a file written through a heredoc
/// whose body quoted a merge command was refused as a merge). A body whose
/// terminator never appears is kept, because the shell may not read it as a
/// heredoc.
fn strip_data_heredoc_bodies(command: &str) -> String {
    let lines: Vec<&str> = command.lines().collect();
    let mut kept = Vec::with_capacity(lines.len());
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        kept.push(line);
        index += 1;
        for (delimiter, strip_tabs, data) in heredocs_opened_by(line) {
            let terminator = lines[index..].iter().position(|body| {
                let body = if strip_tabs {
                    body.trim_start_matches('\t')
                } else {
                    body
                };
                body.trim_end() == delimiter
            });
            match terminator {
                Some(offset) if data => {
                    // Drop the body; keep the terminator line.
                    index += offset;
                }
                Some(offset) => {
                    kept.extend(&lines[index..index + offset]);
                    index += offset;
                }
                None => break,
            }
            if index < lines.len() {
                kept.push(lines[index]);
                index += 1;
            }
        }
    }
    kept.join("\n")
}

/// `(delimiter, <<- strips tabs, body is data)` for each `<<WORD` on a line,
/// in order. `<<<` here-strings open nothing.
fn heredocs_opened_by(line: &str) -> Vec<(String, bool, bool)> {
    let mut opened = Vec::new();
    let mut rest = line;
    let mut consumed = 0;
    while let Some(at) = rest.find("<<") {
        let before = &line[..consumed + at];
        let after = &rest[at + 2..];
        consumed += at + 2;
        rest = after;
        if after.starts_with('<') {
            rest = &after[1..];
            consumed += 1;
            continue;
        }
        let (strip_tabs, after) = match after.strip_prefix('-') {
            Some(after) => (true, after),
            None => (false, after),
        };
        let word: String = after
            .trim_start()
            .chars()
            .take_while(|ch| {
                !ch.is_whitespace() && !matches!(ch, ';' | '|' | '&' | '<' | '>' | ')')
            })
            .filter(|ch| !matches!(ch, '\'' | '"' | '\\'))
            .collect();
        if word.is_empty() {
            continue;
        }
        let program = before
            .rsplit([';', '|', '&', '('])
            .next()
            .unwrap_or_default()
            .split_whitespace()
            .find(|token| !token.contains('='))
            .map(|token| token.rsplit('/').next().unwrap_or(token))
            .unwrap_or_default();
        opened.push((word, strip_tabs, HEREDOC_DATA_CONSUMERS.contains(&program)));
    }
    opened
}

/// Raw GitHub merges a shell command would perform (cas-2ee2 pre-tool guard).
/// `gh pr merge --disable-auto` merges nothing and is not reported.
pub fn github_merges_in(command: &str) -> Vec<GithubMerge> {
    github_merges_in_depth(command, 0)
}

fn github_merges_in_depth(command: &str, depth: usize) -> Vec<GithubMerge> {
    let mut merges = Vec::new();
    let command = strip_data_heredoc_bodies(command);
    let mut quote = None;
    let mut escaped = false;
    for statement in command.split(|character| {
        if std::mem::take(&mut escaped) {
            return false;
        }
        if character == '\\' && quote != Some('\'') {
            escaped = true;
        } else if quote == Some(character) {
            quote = None;
        } else if quote.is_none() && matches!(character, '\'' | '"' | '`') {
            quote = Some(character);
        } else if quote.is_none() {
            return matches!(character, '\n' | ';' | '|' | '&');
        }
        false
    }) {
        let words = shell_words::split(statement).ok();
        let words = words.as_ref().map(|words| words.iter().map(String::as_str).collect::<Vec<_>>());
        let invocation = words.as_deref().map(strip_command_prefixes);
        if depth < 3 && let Some(script) = invocation.and_then(shell_script) {
            merges.extend(github_merges_in_depth(&script, depth + 1));
            continue;
        }
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
                transport: MergeTransport::Api,
                selector: MergeSelector::Number(number),
                repo,
                match_head: None,
                statement: statement_text,
            });
            continue;
        }
        if ["mergePullRequest", "enablePullRequestAutoMerge", "enqueuePullRequest"]
            .iter()
            .any(|mutation| statement.contains(mutation))
        {
            let (ids, complete) = invocation.map(graphql_merge_ids).unwrap_or_default();
            let unresolved = !complete || ids.is_empty();
            let mut push = |selector| merges.push(GithubMerge {
                transport: MergeTransport::Api, selector, repo: None, match_head: None,
                statement: statement_text.clone(),
            });
            if !ids.is_empty() {
                push(MergeSelector::PullRequestNodes {
                    ids, host: invocation.and_then(hostname_flag).map(str::to_owned),
                });
            }
            if unresolved {
                push(MergeSelector::Unresolvable("a GraphQL merge mutation has an unresolved pull request node"));
            }
        }
    }
    merges
}

fn program_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

fn strip_command_prefixes<'a, 'b>(mut words: &'a [&'b str]) -> &'a [&'b str] {
    while let Some((program, rest)) = words.split_first() {
        if program.split_once('=').is_some_and(|(name, _)| {
            let mut bytes = name.bytes();
            bytes.next().is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
                && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        }) {
            words = rest;
            continue;
        }
        let wrapper = program_name(program);
        if !matches!(wrapper, "env" | "sudo" | "command" | "exec" | "nohup" | "time" | "timeout") {
            break;
        }
        words = rest;
        while let Some((flag, after)) = words.split_first() {
            if !flag.starts_with('-') || *flag == "-" { break; }
            words = after;
            if *flag == "--" { break; }
            if matches!((wrapper, *flag),
                ("env", "-u" | "--unset" | "-C" | "--chdir")
                | ("sudo", "-u" | "--user")
                | ("timeout", "-k" | "--kill-after" | "-s" | "--signal")) {
                words = words.get(1..).unwrap_or_default();
            }
        }
        if wrapper == "timeout" { words = words.get(1..).unwrap_or_default(); }
    }
    words
}

fn shell_script(words: &[&str]) -> Option<String> {
    let (program, args) = words.split_first()?;
    match program_name(program) {
        "sh" | "bash" | "zsh" | "dash" | "ksh" => args.iter().enumerate()
            .take_while(|(_, flag)| flag.starts_with('-') && **flag != "--")
            .find(|(_, flag)| !flag.starts_with("--") && flag.contains('c'))
            .and_then(|(index, _)| args.get(index + 1)).map(|script| (*script).to_owned()),
        "eval" => Some(args.join(" ")),
        _ => None,
    }
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
    let mut url_repo = None;
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
            let (parsed, repository) = classify_pr_argument(arg);
            selector = Some(parsed);
            url_repo = repository;
        }
        index += 1;
    }
    Some(GithubMerge {
        transport: MergeTransport::PrCommand,
        selector: selector.unwrap_or(MergeSelector::CurrentBranch),
        repo: url_repo.or(repo),
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
    if let Some((prefix, rest)) = arg.split_once("/pull/")
        && let Some(repo) = repo_identity(prefix)
    {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(number) = digits.parse::<u64>() {
            return (MergeSelector::Number(number), Some(repo));
        }
    }
    (MergeSelector::Branch(arg.to_string()), None)
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
        let repo = before.split_once("repos/").and_then(|(endpoint, repo)| {
            let repo = repo.trim_end_matches('/');
            if repo.split('/').count() != 2 || repo.contains('{') {
                return None;
            }
            let url = url::Url::parse(endpoint).ok();
            let host = url.as_ref().and_then(url::Url::host_str)
                .or_else(|| hostname_flag(tokens));
            Some(match host {
                Some("api.github.com") => format!("github.com/{repo}"),
                Some(host) => format!("{host}/{repo}"),
                None => repo.to_string(),
            })
        });
        Some((digits.parse().ok()?, repo))
    })
}

fn hostname_flag<'a>(tokens: &[&'a str]) -> Option<&'a str> {
    tokens.iter().enumerate().find_map(|(index, token)| match *token {
        "--hostname" => tokens.get(index + 1).copied(),
        _ => token.strip_prefix("--hostname="),
    })
}

#[track_caller]
fn gh_run(command: &mut Command, timeout: Duration) -> Result<Output, String> {
    run_command(command, Deadline::after(timeout), timeout).map_err(|error| {
        let program = command.get_program().to_string_lossy();
        match error {
            BoundedCommandError::TimedOut => format!("`{program}` did not answer within {}s", timeout.as_secs()),
            BoundedCommandError::Io => format!("`{program}` could not be started or read"),
        }
    })
}

fn gh_exit_error(command: &Command, output: &Output) -> String {
    let detail: String = String::from_utf8_lossy(&output.stderr).lines()
        .map(str::trim).find(|line| !line.is_empty()).unwrap_or("no error output")
        .chars().take(200).collect();
    format!("`{}` exited with {}: {detail}", command.get_program().to_string_lossy(), output.status)
}

#[track_caller]
fn gh_output(command: &mut Command, timeout: Duration) -> Result<Vec<u8>, String> {
    let output = gh_run(command, timeout)?;
    if output.status.success() { Ok(output.stdout) } else { Err(gh_exit_error(command, &output)) }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrMetadata {
    number: Option<u64>,
    url: Option<String>,
    head_ref_name: Option<String>,
    head_ref_oid: Option<String>,
    head_repository: Option<HeadRepo>,
    head_repository_owner: Option<HeadOwner>,
    status_check_rollup: Option<Vec<serde_json::Value>>,
}

#[derive(serde::Deserialize)]
struct HeadRepo { name: String }

#[derive(serde::Deserialize)]
struct HeadOwner { login: String }

impl PrMetadata {
    fn repository(&self) -> Option<String> {
        let (repo, _) = self.url.as_deref()?.split_once("/pull/")?;
        repo_identity(repo)
    }

    fn branch(&self) -> Option<&str> {
        self.head_ref_name.as_deref().map(str::trim).filter(|value| !value.is_empty())
    }

    fn head_repository_identity(&self) -> Option<String> {
        let url = url::Url::parse(self.url.as_deref()?).ok()?;
        let owner = &self.head_repository_owner.as_ref()?.login;
        let name = &self.head_repository.as_ref()?.name;
        repo_identity(&format!("{}/{owner}/{name}", url.host_str()?))
    }

    fn head(&self) -> Option<&str> {
        self.head_ref_oid.as_deref().map(str::trim).filter(|value| !value.is_empty())
    }
}

/// The PR number a raw merge targets, resolved the way the QA merge guard
/// resolves its head: the explicit number, else `gh pr view` of the branch or
/// of the checked-out branch (cas-bddf9).
pub fn merge_pr_number(cwd: &Path, merge: &GithubMerge) -> Result<u64, String> {
    let selector = match &merge.selector {
        MergeSelector::Number(number) => return Ok(*number),
        MergeSelector::PullRequestNodes { .. } => return Err("a GraphQL merge names pull requests by node id".into()),
        MergeSelector::Unresolvable(why) => return Err((*why).to_string()),
        MergeSelector::CurrentBranch => None,
        MergeSelector::Branch(branch) => Some(branch.as_str()),
    };
    let mut command = Command::new(gh_bin());
    command.args(["pr", "view"]);
    if let Some(selector) = selector {
        command.arg(selector);
    }
    if let Some(repo) = merge.repo.as_deref() {
        command.args(["--repo", repo]);
    }
    command.args(["--json", "number"]).current_dir(cwd);
    let stdout = gh_output(&mut command, GH_LOOKUP_TIMEOUT)?;
    serde_json::from_slice::<serde_json::Value>(&stdout)
        .ok()
        .and_then(|value| value.get("number").and_then(serde_json::Value::as_u64))
        .ok_or_else(|| "`gh pr view` returned no PR number".to_string())
}

fn resolve_pr(cwd: &Path, merge: &GithubMerge) -> Result<PrMetadata, String> {
    let selector = match &merge.selector {
        MergeSelector::Number(number) => Some(number.to_string()),
        MergeSelector::Branch(branch) => Some(branch.clone()),
        MergeSelector::CurrentBranch => None,
        MergeSelector::Unresolvable(why) => return Err((*why).into()),
        MergeSelector::PullRequestNodes { .. } => return Err("a GraphQL merge names pull requests by node id".into()),
    };
    let mut command = Command::new(gh_bin());
    command.args(["pr", "view"]);
    if let Some(selector) = selector {
        command.arg(selector);
    }
    if let Some(repo) = &merge.repo {
        command.args(["--repo", repo]);
    }
    let fields = if merge.transport == MergeTransport::PrCommand {
        "number,url,headRefName,headRefOid,headRepository,headRepositoryOwner,statusCheckRollup"
    } else {
        "number,url,headRefName,headRefOid,headRepository,headRepositoryOwner"
    };
    command.args(["--json", fields]).current_dir(cwd);
    let stdout = gh_output(&mut command, GH_LOOKUP_TIMEOUT)?;
    serde_json::from_slice(&stdout)
        .map_err(|error| format!("`gh pr view` returned unreadable JSON: {error}"))
}

fn resolve_prs(cwd: &Path, merge: &GithubMerge) -> Vec<Result<PrMetadata, String>> {
    match &merge.selector {
        MergeSelector::PullRequestNodes { ids, host } => resolve_nodes(cwd, ids, host.as_deref()),
        _ => vec![resolve_pr(cwd, merge)],
    }
}

fn resolve_nodes(cwd: &Path, ids: &[String], host: Option<&str>) -> Vec<Result<PrMetadata, String>> {
    let variables = (0..ids.len()).map(|index| format!("$id{index}: ID!")).collect::<Vec<_>>().join(", ");
    let selections = (0..ids.len()).map(|index| format!("n{index}: node(id: $id{index}) {{ ...pr }}")).collect::<Vec<_>>().join(" ");
    let mut command = Command::new(gh_bin());
    command.args(["api", "graphql", "-f"]).arg(format!(
        "query=query({variables}) {{ {selections} }} fragment pr on PullRequest {{ number url headRefName headRefOid headRepository {{ name }} headRepositoryOwner {{ login }} }}"
    )).current_dir(cwd);
    for (index, id) in ids.iter().enumerate() {
        command.arg("-f").arg(format!("id{index}={id}"));
    }
    if let Some(host) = host { command.args(["--hostname", host]); }
    let fail_all = |error: String| ids.iter().map(|_| Err(error.clone())).collect();
    let output = match gh_run(&mut command, GH_LOOKUP_TIMEOUT) {
        Ok(output) => output,
        Err(error) => return fail_all(error),
    };
    let body = serde_json::from_slice::<serde_json::Value>(&output.stdout);
    let Some(data) = body.as_ref().ok().and_then(|body| body["data"].as_object()) else {
        let error = if output.status.success() {
            "`gh api graphql` returned no pull request data".into()
        } else { gh_exit_error(&command, &output) };
        return fail_all(error);
    };
    let errors = body.as_ref().ok().and_then(|body| body["errors"].as_array());
    (0..ids.len()).map(|index| {
        let alias = format!("n{index}");
        if let Some(value) = data.get(&alias).filter(|value| value.is_object()) {
            return serde_json::from_value(value.clone()).map_err(|error| format!("Unreadable pull request data: {error}"));
        }
        let detail = errors.into_iter().flatten()
            .find(|error| error["path"][0].as_str() == Some(alias.as_str()))
            .and_then(|error| error["message"].as_str())
            .unwrap_or("GitHub did not resolve the pull request node");
        Err(detail.to_owned())
    }).collect()
}

fn graphql_merge_ids(tokens: &[&str]) -> (Vec<String>, bool) {
    let fields: std::collections::HashMap<&str, &str> = tokens.iter().enumerate()
        .filter_map(|(index, token)| {
            let field = match *token {
                "-f" | "-F" | "--raw-field" | "--field" => tokens.get(index + 1).copied(),
                _ => ["--field=", "--raw-field=", "-f", "-F"].into_iter()
                    .find_map(|prefix| token.strip_prefix(prefix)),
            }?;
            field.split_once('=')
        }).collect();
    let body: Option<serde_json::Value> = tokens.iter().enumerate().find_map(|(index, token)| {
        let value = match *token {
            "<<<" | "-d" | "--data" | "--data-raw" | "--data-binary" | "--json" =>
                tokens.get(index + 1).copied(),
            _ => ["<<<", "--data=", "--data-raw=", "--data-binary=", "--json=", "-d"].into_iter()
                .find_map(|prefix| token.strip_prefix(prefix)),
        }?;
        serde_json::from_str(value).ok()
    });
    let query = match &body {
        Some(body) => body.get("query").and_then(serde_json::Value::as_str),
        None => fields.get("query").copied(),
    };
    let Some(query) = query else { return (Vec::new(), false); };
    static MERGE_INPUT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(concat!(
            r"\b(?:mergePull", r"Request|enablePull", r"RequestAutoMerge|enqueuePull", r"Request)\s*\(",
            r#"(?:\s*input\s*:\s*(?:\$(\w+)|\{[^}]*?\bpullRequestId\s*:\s*(?:\$(\w+)|"([^"]+)")))?"#,
        )).expect("valid GraphQL input pattern")
    });
    let mut ids = Vec::new();
    let mut complete = true;
    for captures in MERGE_INPUT.captures_iter(query) {
        let id = if let Some(name) = captures.get(1).map(|name| name.as_str()) {
            match &body {
                Some(body) => body["variables"][name]["pullRequestId"].as_str(),
                None => fields.get(format!("{name}[pullRequestId]").as_str()).copied(),
            }
        } else if let Some(name) = captures.get(2).map(|name| name.as_str()) {
            match &body {
                Some(body) => body["variables"][name].as_str(),
                None => fields.get(name).copied(),
            }
        } else {
            captures.get(3).map(|id| id.as_str())
        };
        match id.filter(|id| !id.is_empty() && !id.starts_with('@')) {
            Some(id) => ids.push(id.to_owned()),
            None => complete = false,
        }
    }
    (ids, complete)
}

fn repo_identity(value: &str) -> Option<String> {
    let value = value.trim();
    let normalized = url::Url::parse(value).ok()
        .and_then(|url| Some(format!("{}{}", url.host_str()?, url.path())))
        .or_else(|| crate::cloud::normalize_git_remote_url(value))
        .unwrap_or_else(|| value.to_string());
    let normalized = normalized.trim_end_matches('/');
    let normalized = normalized.strip_suffix(".git").unwrap_or(normalized);
    let parts: Vec<&str> = normalized.split('/').collect();
    let (host, owner, name) = match parts.as_slice() {
        [owner, name] => ("github.com", *owner, *name),
        [host, owner, name] => (*host, *owner, *name),
        _ => return None,
    };
    [host, owner, name].iter().all(|part| !matches!(*part, "" | "." | "..") &&
        part.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte)))
        .then(|| format!("{host}/{owner}/{name}").to_ascii_lowercase())
}

fn origin_identity(dir: &Path) -> Option<String> {
    repo_identity(&git_output(dir, &["config", "--get", "remote.origin.url"])?)
}

fn same_repository(configured: &str, actual: &str, cwd: &Path) -> bool {
    if configured == actual {
        return true;
    }
    let (Some(origin), Some(actual)) = (
        configured.strip_prefix("github.com/"),
        actual.strip_prefix("github.com/"),
    ) else { return false };
    crate::github_repo::resolve_slug(origin, cwd, Path::new(&gh_bin()), GH_LOOKUP_TIMEOUT)
        .is_ok_and(|repo| repo.canonical.eq_ignore_ascii_case(actual))
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

/// GH #2260: --auto is not a CI gate on an unprotected target. Read the
/// current PR head and its own rollup together, independently of QA settings.
/// Only observed pending/failed checks refuse; missing evidence retains the
/// existing admission policy, and API merges retain their existing QA path.
fn pr_ci_refusal(pr: &PrMetadata, merge: &GithubMerge) -> Option<String> {
    if merge.transport != MergeTransport::PrCommand { return None; }
    let head = pr.head()?;
    let checks = pr.status_check_rollup.as_ref()?;
    let (mut pending, mut failed) = (Vec::new(), Vec::new());
    for check in checks {
        let name = check.get("name").or_else(|| check.get("context"))
            .and_then(serde_json::Value::as_str).unwrap_or("<unnamed check>");
        let state = check.get("status").or_else(|| check.get("state"))
            .and_then(serde_json::Value::as_str).unwrap_or_default().to_ascii_uppercase();
        let conclusion = check.get("conclusion").and_then(serde_json::Value::as_str)
            .unwrap_or_default().to_ascii_uppercase();
        if matches!(state.as_str(), "PENDING" | "EXPECTED" | "QUEUED" | "IN_PROGRESS" | "WAITING" | "REQUESTED") {
            pending.push(name);
        } else if matches!(state.as_str(), "FAILURE" | "ERROR")
            || (state == "COMPLETED" && matches!(conclusion.as_str(),
                "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE" | "STALE"))
        {
            failed.push(name);
        }
    }
    if pending.is_empty() && failed.is_empty() { return None; }
    let mut reasons = Vec::new();
    if !pending.is_empty() { reasons.push(format!("PR_CI_PENDING: {}", pending.join(", "))); }
    if !failed.is_empty() { reasons.push(format!("PR_CI_FAILED: {}", failed.join(", "))); }
    Some(crate::gates::tag("G-108", format!(
        "{} (PR head {head}). Wait for this head's checks to succeed, then retry the same merge command. GitHub --auto does not guarantee waiting on an unprotected target.",
        reasons.join("; "),
    )))
}

/// Pre-tool guard for raw GitHub merges, for every role: the refusal when a
/// merge would integrate a QA-bound delivery whose PR head has no passed or
/// waived round. Raw PR merge commands also read current-head CI, even when
/// independent QA is disabled; API merges retain their existing QA policy.
/// Unreadable Cassy state fails open to the close backstop, like the git
/// guard; a PR that cannot be mapped while a round is open fails closed.
pub fn github_merge_refusal(cas_root: &Path, cwd: &Path, command: &str) -> Option<String> {
    github_merge_refusal_with_supervisor(cas_root, cwd, command, None)
}

pub(crate) fn github_merge_refusal_with_supervisor(
    cas_root: &Path,
    cwd: &Path,
    command: &str,
    supervisor: Option<&str>,
) -> Option<String> {
    let mut lookups = Vec::new();
    for merge in github_merges_in(command) {
        for lookup in resolve_prs(cwd, &merge) {
            if let Ok(pr) = &lookup && let Some(refusal) = pr_ci_refusal(pr, &merge) {
                return Some(refusal);
            }
            lookups.push((merge.clone(), lookup));
        }
    }
    if lookups.is_empty() { return None; }
    for (_, lookup) in &lookups {
        match lookup {
            Ok(pr) => if let Some(refusal) = verifier::merge_refusal(cas_root, cwd, pr, supervisor) {
                return Some(refusal);
            },
            Err(error) => tracing::warn!(code = "PR_VERIFIER_LOOKUP_FAILED", %error, "Raw GitHub merge: verifier could not resolve PR head"),
        }
    }
    let qa = crate::config::Config::load(cas_root).ok()?.qa();
    if !qa.independent_pass {
        return None;
    }
    let tasks = match crate::store::open_task_store(cas_root).and_then(|store| Ok(store.list(None)?)) {
        Ok(tasks) => tasks,
        Err(error) => {
            tracing::warn!(%error, "Raw GitHub merge: delivery state unreadable");
            return None;
        }
    };
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
    let task_repositories: Vec<_> = gated
        .iter()
        .map(|(task, _)| {
            let directory = task_repo(cas_root, task);
            origin_identity(&directory).map(|identity| (directory, identity))
        })
        .collect();
    lookups
        .iter()
        .find_map(|(merge, lookup)| {
            // `gh` merges in `--repo`, else in the repository it runs in.
            let target = lookup.as_ref().ok().and_then(PrMetadata::repository)
                .or_else(|| merge.repo.as_deref().and_then(repo_identity))
                .or_else(|| origin_identity(cwd));
            let scoped: Vec<(Task, Vec<QaPass>)> = gated
                .iter()
                .zip(&task_repositories)
                .filter(|(_, repository)| match (&target, repository) {
                    (Some(target), Some((directory, identity))) => same_repository(identity, target, directory),
                    _ => true,
                })
                .map(|(entry, _)| entry.clone())
                .collect();
            if scoped.is_empty() {
                return None;
            }
            refusal_for_merge(cas_root, cwd, &qa, &scoped, merge, lookup.as_ref())
        })
        .map(|refusal| format!("🚫 {refusal}"))
}

fn refusal_for_merge(
    cas_root: &Path,
    cwd: &Path,
    qa: &QaConfig,
    gated: &[(Task, Vec<QaPass>)],
    merge: &GithubMerge,
    lookup: Result<&PrMetadata, &String>,
) -> Option<String> {
    let resolved = lookup.ok().filter(|pr| pr.branch().is_some() && pr.head().is_some());
    let pr_number = lookup.ok().and_then(|pr| pr.number).or_else(|| match &merge.selector {
        MergeSelector::Number(number) => Some(*number),
        _ => None,
    });
    let head_branch = resolved.and_then(PrMetadata::branch).map(str::to_string)
        .or_else(|| match &merge.selector {
            MergeSelector::Branch(branch) => Some(branch.split_once(':').map_or(branch.as_str(), |(_, bare)| bare).to_string()),
            MergeSelector::CurrentBranch => git_output(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]),
            _ => None,
        });
    let head_sha = merge
        .match_head
        .clone()
        .or_else(|| resolved.and_then(PrMetadata::head).map(str::to_string));
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
                        &super::delivery_implementer(cas_root, task),
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
        let failure = lookup.err().map(String::as_str)
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
                    .and_then(|branch| {
                        super::pushed_branch_tip(cwd, branch)
                            .or_else(|| git_output(cwd, &["rev-parse", "--verify", branch]))
                    })
            })
            .or_else(|| {
                let implementer = super::delivery_implementer(cas_root, task);
                let branch = super::delivery_branch(task, passes, &implementer, |candidate| {
                    super::branch_tip(cwd, candidate).is_some()
                        || super::pushed_branch_tip(cwd, candidate).is_some()
                });
                super::pushed_branch_tip(cwd, &branch)
                    .or_else(|| git_output(cwd, &["rev-parse", "--verify", &branch]))
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

/// The checkout a task's delivery lives in, for explicit origin resolution.
fn delivery_repo(cas_root: &Path, task_id: &str) -> PathBuf {
    crate::store::open_task_store(cas_root)
        .ok()
        .and_then(|store| store.get(task_id).ok())
        .map(|task| task_repo(cas_root, &task))
        .unwrap_or_else(|| cas_root.parent().unwrap_or(cas_root).to_path_buf())
}

/// [`delivery_repo`] for a task already in hand.
pub(crate) fn task_repo(cas_root: &Path, task: &Task) -> PathBuf {
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

/// The `gh api` arguments that publish one status on an explicit repo.
pub fn status_publish_args(
    repo: &str,
    sha: &str,
    state: QaCommitState,
    description: &str,
) -> Vec<String> {
    vec![
        "api".to_string(),
        "--method".to_string(),
        "POST".to_string(),
        format!("repos/{repo}/statuses/{sha}"),
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
            let timeout = Duration::from_secs(2);
            let origin = match crate::github_repo::origin_slug(&repo, timeout) {
                Ok(Some(origin)) => origin,
                result => {
                    tracing::warn!(target: "cas::qa", ?result, "QA STATUS PUBLISH FAILED: GitHub origin unavailable; required check stays unmet");
                    return;
                }
            };
            let publish_repo = match crate::github_repo::resolve_slug(&origin, &repo, Path::new(&gh), timeout) {
                Ok(repo) => repo.canonical,
                Err(error) => {
                    // Try the explicit origin endpoint when metadata is unavailable;
                    // upstream/default selection must never suppress the status.
                    tracing::warn!(target: "cas::qa", %error, %origin, "independent QA canonical repository lookup failed; publishing against explicit origin");
                    origin
                }
            };
            let mut command = Command::new(gh);
            command
                .env("GH_HOST", "github.com")
                .args(status_publish_args(&publish_repo, &sha, state, &description))
                .current_dir(&repo);
            match gh_output(&mut command, GH_PUBLISH_TIMEOUT) {
                Ok(_) => tracing::info!(
                    target: "cas::qa",
                    sha = %sha,
                    state = state.as_str(),
                    context = QA_STATUS_CONTEXT,
                    "cas-2ee2: published independent QA commit status"
                ),
                Err(error) => tracing::warn!(
                    target: "cas::qa",
                    %error,
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
            MergeSelector::Branch("acme:factory/w-1".to_string())
        );
        let url = only("gh pr merge https://github.com/acme/gabber/pull/2546 --squash");
        assert_eq!(url.selector, MergeSelector::Number(2546));
        assert_eq!(url.repo.as_deref(), Some("github.com/acme/gabber"));
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
        assert_eq!(
            only("gh api graphql -f query='mutation { mergePullRequest(input: {pullRequestId: \"PR_x\"}) { clientMutationId } }'").selector,
            MergeSelector::PullRequestNodes { ids: vec!["PR_x".into()], host: None },
        );
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
        let args = status_publish_args(
            "Richards-LLC/cassy",
            "abc123",
            QaCommitState::Pending,
            "round open",
        );
        assert_eq!(
            args,
            vec![
                "api",
                "--method",
                "POST",
                "repos/Richards-LLC/cassy/statuses/abc123",
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
    fn data_heredoc_bodies_are_not_merges_but_shell_heredocs_are() {
        // cas-0169: a file written through a data heredoc whose body quotes a
        // merge command.
        let edit =
            "cat > fixture.rs <<'EOF'\nrefusal = run(\"gh pr merge 77 --squash\")\nEOF\necho done";
        assert!(github_merges_in(edit).is_empty(), "{edit}");
        let tabbed = "cat > notes.md <<-EOF\n\tgh pr merge 12 --squash\n\tEOF\n";
        assert!(github_merges_in(tabbed).is_empty());
        let two = "git commit -F - <<A && cat <<B\ngh pr merge 1\nA\ngh pr merge 2\nB\n";
        assert!(github_merges_in(two).is_empty());

        // A merge after the body, a shell-fed body, an interpreter's body
        // (code, not data), an unknown consumer, and an unterminated body are
        // still merges (fail closed).
        let after = "cat > notes.md <<'EOF'\nnotes\nEOF\ngh pr merge 5 --squash";
        assert_eq!(only(after).selector, MergeSelector::Number(5));
        for command in [
            "bash <<'EOF'\ngh pr merge 6 --squash\nEOF",
            "ssh host <<EOF\ngh pr merge 6 --squash\nEOF",
            "sudo tee x <<EOF\ngh pr merge 6 --squash\nEOF",
            "python3 - <<'EOF'\nimport subprocess\nsubprocess.run([\"sh\", \"-c\", \"gh pr merge 6 --squash\"])\nEOF",
            "python3 <<EOF\ncmd = \"gh pr merge 6 --squash\"\nEOF",
            "node <<'EOF'\nconst merge = \"gh pr merge 6 --squash\";\nEOF",
            "cat <<'EOF'\ngh pr merge 6 --squash\n",
        ] {
            assert_eq!(
                only(command).selector,
                MergeSelector::Number(6),
                "{command}"
            );
        }
        // A here-string opens no body.
        assert_eq!(
            only("cat <<< x\ngh pr merge 7").selector,
            MergeSelector::Number(7)
        );
    }

    #[test]
    fn repository_identity_compares_host_owner_and_name() {
        for value in [
            "Richards-LLC/cassy",
            "github.com/richards-llc/cassy",
            "https://github.com/Richards-LLC/cassy.git",
            "git@github.com:Richards-LLC/cassy.git",
            "ssh://git@github.com/Richards-LLC/cassy/",
        ] {
            assert_eq!(
                repo_identity(value).as_deref(),
                Some("github.com/richards-llc/cassy"),
                "{value}"
            );
        }
        assert_eq!(repo_identity("cassy"), None);
        assert_eq!(repo_identity(""), None);
        assert_ne!(
            repo_identity("github.example.com/Richards-LLC/cassy"),
            repo_identity("Richards-LLC/cassy")
        );
    }

    #[cfg(unix)]
    #[test]
    fn bounded_lookup_failures_say_why() {
        let mut failing = Command::new("sh");
        failing.args(["-c", "echo 'HTTP 401: Bad credentials' >&2; exit 4"]);
        assert!(gh_output(&mut failing, Duration::from_secs(5)).is_err());

        let mut missing = Command::new("/nonexistent/cas-0169-gh");
        assert!(gh_output(&mut missing, Duration::from_secs(5)).is_err());

        let mut slow = Command::new("sh");
        slow.args(["-c", "sleep 5"]);
        assert!(gh_output(&mut slow, Duration::from_millis(100)).is_err());

        let mut ok = Command::new("sh");
        ok.args(["-c", "printf done"]);
        assert_eq!(
            gh_output(&mut ok, Duration::from_secs(5)).unwrap(),
            b"done"
        );
    }
}
