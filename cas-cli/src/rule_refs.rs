//! Source references to rules. Git limits scans to tracked, regular text files;
//! immutable close checks and the working-tree CLI use the same comment parser.
use std::{collections::BTreeSet, path::Path, process::Command};

use anyhow::{Context, bail};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RuleReference {
    pub rule_id: String,
    pub path: String,
    pub line: usize,
    pub scope: String,
}

pub(crate) fn comment_body(line: &str) -> Option<&str> {
    let line = line.trim();
    for (start, end) in [
        ("//", ""),
        ("#", ""),
        ("--", ""),
        ("/*", "*/"),
        ("<!--", "-->"),
    ] {
        if let Some(body) = line.strip_prefix(start) {
            return Some(
                if end.is_empty() {
                    body
                } else {
                    body.strip_suffix(end)?
                }
                .trim(),
            );
        }
    }
    None
}

pub(crate) fn marker(line: &str) -> Option<(&str, anyhow::Result<String>)> {
    let body = comment_body(line)?.strip_prefix("cas-rule:")?.trim_start();
    let id = body.split_whitespace().next()?;
    let scope = body[id.len()..].trim_start().strip_prefix("scope:");
    let parsed = (|| {
        let scope: String = serde_json::from_str(scope.context("missing scope:")?.trim())
            .context("scope must be a JSON-quoted string")?;
        if scope.trim().is_empty() {
            bail!("scope must not be empty");
        }
        Ok(scope)
    })();
    Some((id, parsed))
}

/// `revision=None` scans tracked working-tree files, including unstaged edits.
/// A revision scans Git blobs; neither mode follows symlinks out of the repo.
pub(crate) fn scan(
    repo: &Path,
    revision: Option<&str>,
    ids: &BTreeSet<String>,
) -> anyhow::Result<Vec<RuleReference>> {
    let mut command = Command::new("git");
    command.current_dir(repo).args([
        "grep",
        "--no-color",
        "--no-column",
        "--no-heading",
        "--no-break",
        "-H",
        "--fixed-strings",
        "--no-textconv",
        "--no-recurse-submodules",
        "--full-name",
        "-n",
        "-z",
        "-I",
        "-e",
        "cas-rule:",
    ]);
    if let Some(revision) = revision {
        command.arg(revision);
    }
    let output = command
        .arg("--")
        .output()
        .context("cannot scan rule references")?;
    if output.status.code() == Some(1) {
        return Ok(vec![]);
    }
    if !output.status.success() {
        bail!(
            "cannot scan rule references: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = String::from_utf8(output.stdout).context("rule references must be UTF-8")?;
    let mut rest = text.as_str();
    let mut sites = Vec::new();
    let mut problems = Vec::new();
    while !rest.is_empty() {
        // NUL separates path and line number, so filenames containing colons or
        // newlines remain unambiguous. The matching source line ends at LF.
        let (path, tail) = rest
            .split_once('\0')
            .context("invalid Git reference path")?;
        let (number, tail) = tail
            .split_once('\0')
            .context("invalid Git reference line")?;
        let (line, tail) = tail.split_once('\n').unwrap_or((tail, ""));
        rest = tail;
        let path = match revision {
            Some(revision) => path
                .strip_prefix(&format!("{revision}:"))
                .context("invalid Git revision prefix")?,
            None => path,
        };
        let number: usize = number.parse().context("invalid reference line number")?;
        if let Some((id, scope)) = marker(line) {
            if !ids.contains(id) {
                continue;
            }
            match scope {
                Ok(scope) => sites.push(RuleReference {
                    rule_id: id.into(),
                    path: path.into(),
                    line: number,
                    scope,
                }),
                Err(error) => problems.push(format!("{path}:{number}: {id}: {error:#}")),
            }
        }
    }
    if !problems.is_empty() {
        bail!(
            "Invalid rule markers:\n{}\nUse a comment: cas-rule: <id> scope: <JSON string>",
            problems.join("\n")
        );
    }
    sites.sort_by(|a, b| (&a.path, a.line, &a.rule_id).cmp(&(&b.path, b.line, &b.rule_id)));
    Ok(sites)
}

/// Provenance enrichment seam: reuse cas-adeb's optional attribution directly,
/// without duplicating its type, fields or migration.
pub(crate) fn stakeholder_scope(rule: &cas_types::Rule) -> Option<&str> {
    rule.stakeholder
        .as_ref()
        .map(|stakeholder| stakeholder.scope.as_str())
}

pub(crate) fn scope_matches(rule: &cas_types::Rule, scope: &str) -> bool {
    stakeholder_scope(rule).is_none_or(|expected| expected == scope)
}

pub(crate) fn task_context(task: &cas_types::Task) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}",
        task.title,
        task.description,
        task.design,
        task.acceptance_criteria,
        task_brief(task)
    )
}

/// The newest supervisor brief, where supervisors declare causal sites.
pub(crate) fn task_brief(task: &cas_types::Task) -> &str {
    crate::ui::factory::daemon::runtime::queue_and_events::latest_spawn_brief(&task.notes)
        .unwrap_or_default()
}

/// Stakeholder rules the task cites. A citation of a process rule (one
/// without stakeholder provenance) explains the work; it does not make code
/// rule-driven, so it needs no marker.
pub(crate) fn task_rule_ids(task: &cas_types::Task, rules: &[cas_types::Rule]) -> BTreeSet<String> {
    let text = task_context(task);
    let words: BTreeSet<_> = text
        .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))
        .collect();
    rules
        .iter()
        .filter(|rule| rule.stakeholder.is_some() && words.contains(rule.id.as_str()))
        .map(|rule| rule.id.clone())
        .collect()
}

/// A rule change after which its cited code and tests need review. Listing a
/// site never edits or deletes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuleChange {
    Retired,
    Superseded,
    StakeholderCleared,
    ReviewExpired,
    /// The stakeholder scope changed; only markers recording another scope are listed.
    ScopeChanged(String),
}

impl RuleChange {
    fn label(&self) -> &'static str {
        match self {
            Self::Retired => "retired",
            Self::Superseded => "superseded",
            Self::StakeholderCleared => "stakeholder attribution cleared",
            Self::ReviewExpired => "review_after expired",
            Self::ScopeChanged(_) => "stakeholder scope changed",
        }
    }

    fn lists(&self, site: &RuleReference) -> bool {
        match self {
            Self::ScopeChanged(scope) => &site.scope != scope,
            _ => true,
        }
    }
}

fn superseded(rule: &cas_types::Rule) -> bool {
    rule.tags
        .iter()
        .any(|tag| tag == "superseded" || tag.starts_with("superseded-by:"))
}

/// The cleanup obligations one stored rule update created.
pub(crate) fn update_changes(old: &cas_types::Rule, new: &cas_types::Rule) -> Vec<RuleChange> {
    let mut changes = Vec::new();
    if new.status == cas_types::RuleStatus::Retired && old.status != new.status {
        changes.push(RuleChange::Retired);
    }
    if superseded(new) && !superseded(old) {
        changes.push(RuleChange::Superseded);
    }
    match (stakeholder_scope(old), stakeholder_scope(new)) {
        (Some(_), None) => changes.push(RuleChange::StakeholderCleared),
        (before, Some(after)) if before != Some(after) => {
            changes.push(RuleChange::ScopeChanged(after.into()))
        }
        _ => {}
    }
    changes
}

/// File one cleanup task for this project's checkout, listing every cited site
/// the change affects. `change_key` identifies the rule change, so a retry or
/// a repeated sweep returns the existing task. Returns `None` when the project
/// is not a Git checkout or no site needs review.
pub(crate) fn file_cleanup(
    cas_root: &Path,
    tasks: &dyn cas_store::TaskStore,
    rule: &cas_types::Rule,
    changes: &[RuleChange],
    change_key: &str,
) -> anyhow::Result<Option<String>> {
    if changes.is_empty() {
        return Ok(None);
    }
    let project = cas_root.parent().unwrap_or(cas_root);
    let toplevel = Command::new("git")
        .current_dir(project)
        .args(["rev-parse", "--show-toplevel"])
        .output();
    let Some(repo) = toplevel
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    else {
        return Ok(None);
    };
    let external_ref = format!("rule-cleanup:{}:{change_key}", rule.id);
    if let Some(existing) = tasks
        .list(None)?
        .into_iter()
        .find(|task| task.external_ref.as_deref() == Some(external_ref.as_str()))
    {
        return Ok(Some(existing.id));
    }
    let sites: Vec<_> = scan(Path::new(&repo), None, &BTreeSet::from([rule.id.clone()]))?
        .into_iter()
        .filter(|site| changes.iter().any(|change| change.lists(site)))
        .collect();
    if sites.is_empty() {
        return Ok(None);
    }
    let labels: Vec<_> = changes.iter().map(RuleChange::label).collect();
    let repo_name = Path::new(&repo)
        .file_name()
        .map_or(repo.clone(), |name| name.to_string_lossy().into_owned());
    let listed: String = sites
        .iter()
        .map(|site| {
            let scope = serde_json::to_string(&site.scope).unwrap_or_default();
            format!("- {}:{} scope: {scope}\n", site.path, site.line)
        })
        .collect();
    let current = match stakeholder_scope(rule) {
        Some(scope) => format!(
            "Its current stakeholder scope is {}.",
            serde_json::to_string(scope).unwrap_or_default()
        ),
        None => "It has no stakeholder scope.".into(),
    };
    let id = tasks.generate_id()?;
    let mut task = cas_types::Task::new(
        id.clone(),
        format!(
            "Review {} {} citation(s) in {repo_name}: rule {}",
            sites.len(),
            rule.id,
            labels.join(", ")
        ),
    );
    task.task_type = cas_types::TaskType::Chore;
    task.origin_project = tasks.project_id().map(str::to_owned);
    task.external_ref = Some(external_ref);
    task.description = format!(
        "Rule {} was {} (change {change_key}). {current} These tracked sites in {repo} cite it:\n{listed}\
         For each site, remove the marker together with the code or test written for the rule, \
         or re-scope it to the rule's current stakeholder scope. Cassy deleted nothing. \
         Re-list with `cas rules refs {}`.",
        rule.id,
        labels.join(", "),
        rule.id
    );
    task.acceptance_criteria = format!(
        "Every listed site is removed or deliberately kept, and `cas rules refs {}` lists only sites the rule still governs.",
        rule.id
    );
    tasks.create_atomic(&task, &[], None, Some("rule-cleanup"))?;
    Ok(Some(id))
}

/// Daemon sweep: a stakeholder rule whose review_after has passed gets one
/// cleanup task per expiry. Unattributed rules carry only a default review hint.
pub(crate) fn file_expired_review_cleanups(cas_root: &Path) -> anyhow::Result<usize> {
    let mut expired = crate::store::open_rule_store(cas_root)?.list()?;
    expired.retain(|rule| rule.is_active() && rule.stakeholder.is_some() && rule.needs_review());
    if expired.is_empty() {
        return Ok(0);
    }
    let tasks = crate::store::open_task_store(cas_root)?;
    let mut filed = 0;
    for rule in &expired {
        let key = format!(
            "review-after-{}",
            rule.review_after.map(|at| at.timestamp()).unwrap_or_default()
        );
        if file_cleanup(cas_root, tasks.as_ref(), rule, &[RuleChange::ReviewExpired], &key)?.is_some() {
            filed += 1;
        }
    }
    Ok(filed)
}

#[cfg(test)]
mod tests {
    #[test]
    fn stakeholder_scope_is_verbatim_and_optional_cas_4f87() {
        let mut provenance = cas_types::Rule::new("rule-177".into(), "SMS send constraint".into());
        provenance.stakeholder = Some(cas_types::StakeholderAttribution {
            name: "Ben".into(),
            quote: "Please limit SMS sending".into(),
            scope: "Only SMS sends".into(),
        });
        assert!(super::scope_matches(&provenance, "Only SMS sends"));
        for broader in ["All sending", "Only SMS sends ", "only SMS sends"] {
            assert!(!super::scope_matches(&provenance, broader));
        }
        provenance.stakeholder = None;
        assert!(super::scope_matches(&provenance, "Only SMS sends"));
    }
}
