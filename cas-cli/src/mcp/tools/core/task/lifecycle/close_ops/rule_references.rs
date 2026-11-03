//! A cited rule needs source references, not a blanket citation on every edit.
//! Workers declare causal sites; verification checks for omitted causal sites.
use super::*;
use crate::rule_refs::{task_context as context, task_rule_ids as cited_ids};
use serde::Deserialize;
use std::path::{Component, Path};

const PREFIX: &str = "RULE REFERENCES REQUIRED";
const DECLARATION: &str = "Rule-driven sites:";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Site {
    rule_id: String,
    path: String,
    /// Exact, unique trimmed source line immediately after its marker block.
    anchor: String,
}

fn declarations(text: &str) -> Result<Vec<Site>, String> {
    let mut sites = Vec::new();
    for line in text.lines() {
        if let Some(json) = line.trim().strip_prefix(DECLARATION) {
            let values: Vec<Site> = serde_json::from_str(json.trim()).map_err(|error| {
                format!("{DECLARATION} needs a JSON array of rule_id, path and anchor: {error}")
            })?;
            sites.extend(values);
        }
    }
    Ok(sites)
}

fn adjacent(repo: &Path, head: &str, site: &Site) -> anyhow::Result<bool> {
    if site.anchor.trim().is_empty()
        || site.path.is_empty()
        || Path::new(&site.path)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        anyhow::bail!("site needs a repository-relative path and a nonempty anchor");
    }
    let text = crate::qa_impact::io::git(repo, &["show", &format!("{head}:{}", site.path)])?;
    let lines: Vec<_> = text.lines().collect();
    let anchors: Vec<_> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (line.trim() == site.anchor.trim()).then_some(index))
        .collect();
    if anchors.len() != 1 {
        anyhow::bail!(
            "anchor must match exactly one trimmed source line (found {})",
            anchors.len()
        );
    }
    for line in lines[..anchors[0]].iter().rev() {
        if line.trim().is_empty() {
            continue;
        }
        match crate::rule_refs::marker(line) {
            Some((id, scope)) if id == site.rule_id => return Ok(scope.is_ok()),
            Some(_) => continue,
            None => break,
        }
    }
    Ok(false)
}

fn validate(
    repo: &Path,
    head: Option<&str>,
    task: &Task,
    receipt: &str,
    rules: &[Rule],
) -> Result<(), String> {
    let task_context = context(task);
    // A post-merge close keeps the worker's parked receipt if the new reason
    // has no declaration. Progress/history notes never become task scope.
    let receipt = if receipt
        .lines()
        .any(|line| line.trim().starts_with(DECLARATION))
    {
        receipt
    } else {
        task.deliverables
            .parked_close_reason
            .as_deref()
            .unwrap_or(receipt)
    };
    let mut sites = declarations(&task_context)?;
    sites.extend(declarations(receipt)?);
    let mut ids = cited_ids(task, rules);
    for site in &sites {
        if !rules.iter().any(|rule| rule.id == site.rule_id) {
            return Err(format!(
                "declared site {} cites unknown rule {}",
                site.path, site.rule_id
            ));
        }
        ids.insert(site.rule_id.clone());
    }
    if ids.is_empty() {
        return Ok(());
    }
    let head = head.ok_or("cannot resolve the delivered tip for cited rules")?;
    let refs = crate::rule_refs::scan(repo, Some(head), &ids).map_err(|error| error.to_string())?;
    let mut problems = Vec::new();
    for id in &ids {
        if !refs.iter().any(|site| site.rule_id == *id) {
            problems.push(format!("{id}: no marked site at the delivered tip"));
        }
    }
    for reference in &refs {
        let rule = rules
            .iter()
            .find(|rule| rule.id == reference.rule_id)
            .expect("filtered rule");
        if !crate::rule_refs::scope_matches(rule, &reference.scope) {
            problems.push(format!(
                "{}:{}: {} scope differs from stakeholder.scope",
                reference.path, reference.line, reference.rule_id
            ));
        }
    }
    for site in &sites {
        match adjacent(repo, head, site) {
            Ok(true) => {}
            Ok(false) => problems.push(format!(
                "{}: {} needs its adjacent {} marker",
                site.path, site.anchor, site.rule_id
            )),
            Err(error) => problems.push(format!("{}: {error:#}", site.path)),
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{}\nDeclare causal sites in the brief or close receipt as one JSON line:\nRule-driven sites: [{{\"rule_id\":\"<id>\",\"path\":\"src/file.rs\",\"anchor\":\"<exact source line>\"}}]\nPlace `cas-rule: <id> scope: <JSON-quoted verbatim scope>` in an adjacent comment. The verifier checks undeclared causal changes.",
            problems.join("\n")
        ))
    }
}

impl CasCore {
    pub(super) fn check_rule_reference_delivery(
        &self,
        task: &Task,
        repo: &Path,
        head: Option<&str>,
        receipt: &str,
    ) -> Result<(), String> {
        if !context(task).contains("rule-")
            && !receipt.contains(DECLARATION)
            && !task
                .deliverables
                .parked_close_reason
                .as_deref()
                .unwrap_or_default()
                .contains(DECLARATION)
        {
            return Ok(());
        }
        let rules = self
            .open_rule_store()
            .map_err(|error| format!("{PREFIX}: {error}"))?
            .list()
            .map_err(|error| format!("{PREFIX}: cannot read cited rules: {error}"))?;
        validate(repo, head, task, receipt, &rules)
            .map_err(|message| format!("{PREFIX}:\n{message}"))
    }
}
