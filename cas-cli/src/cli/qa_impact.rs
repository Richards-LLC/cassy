use super::Cli;
use crate::qa_impact::{
    self, Change, Impact,
    io::{changes_for_diff, git, resolve_ref, untracked},
};
use anyhow::{Result, anyhow};
use clap::Args;
use std::{io::Read, path::Path};

#[derive(Debug, Args)]
pub struct ImpactArgs {
    /// Compare base to HEAD, or base..head (committed snapshots)
    #[arg(conflicts_with_all = ["files", "stdin", "task"])]
    diff: Option<String>,
    /// Explicit repository-relative paths
    #[arg(long, num_args = 1.., conflicts_with_all = ["stdin", "task"])]
    files: Vec<String>,
    /// Read newline- or NUL-separated paths from stdin
    #[arg(long, conflicts_with = "task")]
    stdin: bool,
    /// Inspect the same task-attributed delivery paths as the close gates
    #[arg(long)]
    task: Option<String>,
    /// Render a Markdown impact table
    #[arg(long, conflicts_with = "json")]
    markdown: bool,
}

pub fn execute(args: &ImpactArgs, cli: &Cli, cas_root: Option<&Path>) -> Result<()> {
    let report = report(args, cas_root).map_err(|error| {
        if error.is::<qa_impact::UsageError>() {
            error
        } else {
            anyhow!(qa_impact::ReadError(format!("{error:#}")))
        }
    })?;
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render(&report, args.markdown));
    }
    Ok(())
}

fn report(args: &ImpactArgs, cas_root: Option<&Path>) -> Result<Impact> {
    let root = crate::store::detect::find_git_toplevel(&std::env::current_dir()?)?;
    let local_cas = root.join(".cas");
    let cas_root = cas_root.unwrap_or(&local_cas);
    let config = crate::config::Config::load(cas_root)?;
    if let Some(id) = &args.task {
        return task_report(cas_root, id, &config.qa());
    }
    if !args.files.is_empty() || args.stdin {
        let paths = if args.stdin {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            text.split(['\n', '\0'])
                .map(|p| p.trim_end_matches('\r').to_owned())
                .filter(|p| !p.is_empty())
                .collect()
        } else {
            args.files.clone()
        };
        let mut changes = Vec::new();
        for path in paths {
            if Path::new(&path).is_absolute()
                || Path::new(&path)
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir))
            {
                return Err(qa_impact::UsageError(format!(
                    "impact paths must be repository-relative: {path}"
                ))
                .into());
            }
            changes.push(Change::modified(path.trim_start_matches("./")));
        }
        return qa_impact::analyze(&root, &config.qa(), &changes, None, None);
    }
    if let Some(diff) = &args.diff {
        let (base, head) = diff.split_once("..").unwrap_or((diff, "HEAD"));
        if base.is_empty() || head.is_empty() || head.starts_with('.') {
            return Err(qa_impact::UsageError("use <base> or <base>..<head>".into()).into());
        }
        let base = resolve_ref(&root, base)?;
        let head = resolve_ref(&root, head)?;
        let changes = changes_for_diff(&root, &base, Some(&head))?;
        return qa_impact::analyze(&root, &config.qa(), &changes, Some(&base), Some(&head));
    }
    let target = crate::worktree::git::GitOperations::new(root.clone()).detect_default_branch();
    let target =
        crate::mcp::tools::core::task::lifecycle::close_ops::freshest_target_ref(&root, &target);
    let base = git(&root, &["merge-base", &target, "HEAD"])?;
    let base = base.trim();
    // The default includes committed changes, index/worktree edits and untracked files.
    let mut changes = changes_for_diff(&root, base, None)?;
    changes.extend(untracked(&root)?);
    qa_impact::analyze(&root, &config.qa(), &changes, Some(base), None)
}

fn task_report(cas_root: &Path, id: &str, qa: &crate::config::QaConfig) -> Result<Impact> {
    use crate::mcp::tools::core::task::{lifecycle::close_ops, repo_context};
    let store = crate::store::open_task_store(cas_root)?;
    let task = store.get(id)?;
    let parent = store.get_parent_epic(id)?;
    let target = close_ops::effective_close_work_target(&task, parent.as_ref())
        .ok_or_else(|| anyhow!("task {id} has no work target"))?;
    let context = repo_context::resolve_repo_context(cas_root, &target).map_err(|e| anyhow!(e))?;
    let repo = &context.repo_root;
    let latest =
        cas_store::get_latest_worker_delivery(cas_root, id)?.map(|(receipt, _)| receipt.commit_sha);
    let history = crate::store::open_agent_store(cas_root)?.get_lease_history(id, None)?;
    let window = close_ops::resolve_task_commit_receipt_window(
        task.created_at,
        &history,
        close_ops::task_commit_identity(&task, latest.clone()),
    );
    let implementer = crate::qa_pass::delivery_implementer(cas_root, &task);
    let branch = crate::qa_pass::delivery_branch(&task, &[], &implementer, |branch| {
        crate::qa_pass::branch_tip(repo, branch).is_some()
    });
    let head = task
        .deliverables
        .factory_branch_anchor
        .as_deref()
        .or(latest.as_deref())
        .map(str::to_owned)
        .or_else(|| crate::qa_pass::branch_tip(repo, &branch))
        .ok_or_else(|| anyhow!("task {id} has no readable delivery head"))?;
    let target = close_ops::freshest_target_ref(repo, &context.target_branch);
    let paths = close_ops::task_attribution::paths(repo, &target, &window, Some(&head))
        .ok_or_else(|| anyhow!("task {id} delivery paths could not be attributed"))?;
    let base = close_ops::task_attribution::delivery_base(repo, &target, &window, Some(&head));
    let mut changes = match &base {
        Some(base) => changes_for_diff(repo, base, Some(&head))?,
        None => Vec::new(),
    };
    changes.retain(|change| {
        paths.contains(&change.file) || change.old_file.as_ref().is_some_and(|p| paths.contains(p))
    });
    for path in paths {
        if !changes
            .iter()
            .any(|c| c.file == path || c.old_file.as_deref() == Some(path.as_str()))
        {
            changes.push(Change::modified(path));
        }
    }
    qa_impact::analyze(repo, qa, &changes, base.as_deref(), Some(&head))
}

pub(crate) fn render(report: &Impact, markdown: bool) -> String {
    let clean = |text: &str| {
        text.replace('\n', " ")
            .replace('\r', " ")
            .replace('|', "\\|")
    };
    let mut out = String::new();
    for warning in &report.warnings {
        out.push_str(&format!(
            "{}{}\n",
            if markdown { "> WARNING: " } else { "WARNING: " },
            clean(warning)
        ));
    }
    if let Some(safety) = &report.safety {
        out.push_str(&format!(
            "{}{}\n",
            if markdown { "> " } else { "" },
            clean(&safety.brief())
        ));
    }
    if !report.warnings.is_empty() || report.safety.is_some() {
        out.push('\n');
    }
    out.push_str(&format!(
        "{} journey(s), {} unowned file(s) [{}]\n",
        report.journeys.len(),
        report.unowned.len(),
        report.source
    ));
    if markdown {
        out.push_str(
            "\n| Journey | Status | Share | Evidence required |\n| --- | --- | --- | --- |\n",
        );
    }
    for journey in &report.journeys {
        let share = journey
            .share_pct
            .map(|share| format!("{share}%"))
            .unwrap_or_else(|| "unknown".into());
        if markdown {
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                clean(&journey.id),
                clean(&journey.status),
                share,
                clean(&journey.evidence_required)
            ));
        } else {
            out.push_str(&format!(
                "{}: {} ({}, {})\n  {}\n",
                clean(&journey.id),
                clean(&journey.name),
                clean(&journey.status),
                share,
                clean(&journey.evidence_required)
            ));
        }
    }
    if !report.areas.is_empty() {
        out.push_str("\nCODEMAP areas:\n");
    }
    for area in &report.areas {
        out.push_str(&format!(
            "- {}: {} ({} files)\n",
            clean(&area.prefix),
            clean(&area.summary),
            area.files.len()
        ));
    }
    if !report.unowned.is_empty() {
        out.push_str("\nUnowned files:\n");
    }
    for path in &report.unowned {
        out.push_str(&format!(
            "- {}{}: {}\n",
            clean(&path.file),
            if path.surface { " [surface]" } else { "" },
            path.codemap
                .as_ref()
                .map(|label| clean(&label.summary))
                .unwrap_or_else(|| "unmapped".into())
        ));
    }
    if !report.ignored.is_empty() {
        out.push_str(&format!("\n{} ignored path(s).\n", report.ignored.len()));
    }
    for (name, item) in [
        ("CODEMAP", &report.upkeep.codemap),
        ("PRODUCT_OVERVIEW", &report.upkeep.product_overview),
        ("registry", &report.upkeep.registry),
    ] {
        if item.required {
            out.push_str(&format!("\nRequired upkeep ({name}):\n"));
            for reason in &item.reasons {
                out.push_str(&format!("- {}\n", clean(reason)));
            }
        }
    }
    out
}
