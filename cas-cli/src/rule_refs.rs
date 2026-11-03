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

/// Provenance enrichment seam for cas-adeb: consume its optional serialized
/// `stakeholder.scope`, without duplicating its type, fields or migration.
pub(crate) fn stakeholder_scope(rule_json: &serde_json::Value) -> Option<&str> {
    rule_json.get("stakeholder")?.get("scope")?.as_str()
}

pub(crate) fn scope_matches(rule_json: &serde_json::Value, scope: &str) -> bool {
    stakeholder_scope(rule_json).is_none_or(|expected| expected == scope)
}

pub(crate) fn task_context(task: &cas_types::Task) -> String {
    let brief =
        crate::ui::factory::daemon::runtime::queue_and_events::latest_spawn_brief(&task.notes)
            .unwrap_or_default();
    format!(
        "{}\n{}\n{}\n{}\n{brief}",
        task.title, task.description, task.design, task.acceptance_criteria
    )
}

pub(crate) fn task_rule_ids(task: &cas_types::Task, rules: &[cas_types::Rule]) -> BTreeSet<String> {
    let text = task_context(task);
    let words: BTreeSet<_> = text
        .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))
        .collect();
    rules
        .iter()
        .filter(|rule| words.contains(rule.id.as_str()))
        .map(|rule| rule.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn stakeholder_scope_is_verbatim_and_optional_cas_4f87() {
        let provenance = serde_json::json!({"stakeholder": {
            "name":"Ben", "quote":"Please limit SMS sending", "scope":"Only SMS sends"
        }});
        assert!(super::scope_matches(&provenance, "Only SMS sends"));
        for broader in ["All sending", "Only SMS sends ", "only SMS sends"] {
            assert!(!super::scope_matches(&provenance, broader));
        }
        assert!(super::scope_matches(
            &serde_json::json!({}),
            "Only SMS sends"
        ));
    }
}
