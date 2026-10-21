//! Diff-scoped registry upkeep at park (G1–G3). All content comes from Git snapshots.
use super::*;
use crate::config::{QaConfig, RegistryEnforcement};
use crate::qa_impact::io::git;
use crate::qa_registry::{Problem, Registry};
use std::path::Path;

/// One close-path entry point, sharing the QA gate's delivery head and attribution window.
#[allow(clippy::too_many_arguments)]
pub(super) fn at_park(
    store: &dyn cas_store::TaskStore,
    task: &mut Task,
    cas_root: &Path,
    repo: &Path,
    target: &str,
    window: Option<&TaskCommitReceiptWindow>,
    receipt: Option<&str>,
    override_reason: Option<&str>,
) -> Result<Vec<String>, String> {
    let config = match crate::config::Config::load(cas_root) {
        Ok(config) => config,
        Err(error) => {
            return warning(
                store,
                task,
                format!("UPKEEP WARNING: config unreadable (not blocking): {error}"),
            );
        }
    };
    let qa = config.qa();
    if qa.registry.enforce == RegistryEnforcement::Off {
        return Ok(vec![]);
    }
    let Some(head) = super::super::qa_evidence_gate::delivered_head(task, repo, target, receipt)
    else {
        return warning(
            store,
            task,
            "UPKEEP WARNING: delivery head unavailable (not blocking)".into(),
        );
    };
    let paths =
        window.and_then(|window| task_attribution::paths(repo, target, window, Some(&head)));
    let base = if let Some(window) = window {
        task_attribution::delivery_base(repo, target, window, Some(&head))
    } else {
        crate::qa_evidence::delivery_range(repo, &head, &freshest_target_ref(repo, target))
            .map(|(base, _)| base)
    };
    let Some(base) = base else {
        return warning(
            store,
            task,
            "UPKEEP WARNING: attributed delivery base unavailable (not blocking)".into(),
        );
    };
    // Do not substitute a broad diff when attribution could not be established.
    if window.is_some() && paths.is_none() {
        return warning(
            store,
            task,
            "UPKEEP WARNING: attributed delivery paths unavailable (not blocking)".into(),
        );
    }
    check(
        store,
        task,
        repo,
        &qa,
        &base,
        &head,
        paths.as_deref(),
        override_reason,
    )
}

fn warning(
    store: &dyn cas_store::TaskStore,
    task: &mut Task,
    text: String,
) -> Result<Vec<String>, String> {
    append_close_decision_note(store, task, &text);
    Ok(vec![text])
}

/// Counts recorded gate uses, including nested epics; notes/retries cannot inflate it.
pub(super) fn epic_escape_summary(store: &dyn cas_store::TaskStore, epic: &Task) -> String {
    let mut pending = vec![epic.id.clone()];
    let mut visited = std::collections::BTreeSet::new();
    let mut escapes = std::collections::BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !visited.insert(id.clone()) {
            continue;
        }
        let children = match store.get_subtasks(&id) {
            Ok(children) => children,
            Err(error) => return format!("Upkeep escapes in this epic: unavailable ({error})"),
        };
        for child in children {
            pending.push(child.id.clone());
            for line in child.notes.lines() {
                let Some((_, body)) = line.split_once("] ") else {
                    continue;
                };
                let body = body.strip_prefix("✅ DECISION ").unwrap_or(body);
                if let Some(escape) = body.strip_prefix("UPKEEP ESCAPE ") {
                    escapes.insert(format!(
                        "{}: {}",
                        child.id,
                        escape.split(". Waived refusal:").next().unwrap_or(escape)
                    ));
                }
            }
        }
    }
    format!(
        "Upkeep escapes in this epic: {} ({})",
        escapes.len(),
        escapes.into_iter().collect::<Vec<_>>().join("; ")
    )
}

struct Finding {
    gate: &'static str,
    token: &'static str,
    title: &'static str,
    detail: String,
}
impl Finding {
    fn broken(detail: String) -> Self {
        Self {
            gate: "G1",
            token: "registry unchanged:",
            title: "JOURNEY REGISTRY BROKEN BY THIS DELIVERY",
            detail,
        }
    }
}

fn decision_reason<'a>(notes: &'a str, token: &str) -> Option<&'a str> {
    notes.lines().rev().find_map(|line| {
        let (stamp, decision) = line.strip_prefix('[')?.split_once("] ✅ DECISION ")?;
        chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M").ok()?;
        let reason = decision.strip_prefix(token)?.trim();
        (!reason.is_empty()).then_some(reason)
    })
}

fn finish(
    store: &dyn cas_store::TaskStore,
    task: &mut Task,
    qa: &QaConfig,
    findings: Vec<Finding>,
    warnings: &[String],
    override_reason: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut notices = warnings.to_vec();
    for warning in warnings {
        append_close_decision_note(store, task, warning);
    }
    let mut refusals = Vec::new();
    for finding in findings {
        let text = gate_text::registry_upkeep_message(
            &task.id,
            &qa.registry.path,
            finding.title,
            &finding.detail,
            finding.token,
            warnings,
            crate::mcp::tools::core::guidance::caller_prefix(),
        );
        if let Some(reason) = override_reason.filter(|reason| !reason.trim().is_empty()) {
            append_close_decision_note(
                store,
                task,
                &format!(
                    "✅ DECISION UPKEEP ESCAPE {}: supervisor override: {}. Waived refusal: {}",
                    finding.gate,
                    reason.trim(),
                    text
                ),
            );
        } else if qa.registry.enforce == RegistryEnforcement::Observe {
            notices.push(format!(
                "UPKEEP OBSERVE {}: {} (see task notes)",
                finding.gate, finding.title
            ));
            append_close_decision_note(
                store,
                task,
                &format!("UPKEEP OBSERVE {}: {text}", finding.gate),
            );
        } else if let Some(reason) = decision_reason(&task.notes, finding.token).map(str::to_owned)
        {
            append_close_decision_note(
                store,
                task,
                &format!("UPKEEP ESCAPE {}: {} {reason}", finding.gate, finding.token),
            );
        } else {
            refusals.push(text);
        }
    }
    if refusals.is_empty() {
        Ok(notices)
    } else {
        Err(refusals.join("\n\n"))
    }
}

fn tracked(repo: &Path, revision: &str) -> anyhow::Result<Vec<String>> {
    Ok(git(repo, &["ls-tree", "-rz", "--name-only", revision])?
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect())
}
fn read(
    repo: &Path,
    revision: &str,
    path: &str,
    paths: &[String],
) -> anyhow::Result<Option<String>> {
    if !paths.iter().any(|p| p == path) {
        return Ok(None);
    }
    git(repo, &["show", &format!("{revision}:{path}")]).map(Some)
}
fn problem_lines(problems: &[Problem]) -> String {
    problems
        .iter()
        .map(|p| format!("- {}: {} ({})", p.subject, p.message, p.code))
        .collect::<Vec<_>>()
        .join("\n")
}
fn changed<T: serde::Serialize>(current: &T, before: Option<&T>) -> bool {
    before.is_none_or(|before| {
        serde_yaml::to_value(current).ok() != serde_yaml::to_value(before).ok()
    })
}
fn has_changed_case(registry: &Registry, before: Option<&Registry>, id: &str) -> bool {
    registry.bug_families.iter().any(|(key, family)| {
        family.tasks.iter().any(|task| task == id)
            && changed(family, before.and_then(|r| r.bug_families.get(key)))
    }) || registry.invariants.iter().any(|(key, invariant)| {
        let names_task = invariant.bug.as_ref().is_some_and(|bug| {
            bug.get("task").and_then(serde_yaml::Value::as_str) == Some(id)
                || bug
                    .get("tasks")
                    .and_then(serde_yaml::Value::as_sequence)
                    .is_some_and(|tasks| tasks.iter().any(|task| task.as_str() == Some(id)))
        });
        names_task && changed(invariant, before.and_then(|r| r.invariants.get(key)))
    })
}

#[allow(clippy::too_many_arguments)]
fn check(
    store: &dyn cas_store::TaskStore,
    task: &mut Task,
    repo: &Path,
    qa: &QaConfig,
    base: &str,
    head: &str,
    paths: Option<&[String]>,
    override_reason: Option<&str>,
) -> Result<Vec<String>, String> {
    if qa.registry.enforce == RegistryEnforcement::Off {
        return Ok(vec![]);
    }
    let result = inspect(task, repo, qa, base, head, paths);
    match result {
        Ok((findings, warnings)) => finish(store, task, qa, findings, &warnings, override_reason),
        // Git failures cannot establish causality. Never brick unrelated deliveries.
        Err(error) => warning(
            store,
            task,
            format!("UPKEEP WARNING: registry inputs unavailable (not blocking): {error:#}"),
        ),
    }
}

fn inspect(
    task: &Task,
    repo: &Path,
    qa: &QaConfig,
    base: &str,
    head: &str,
    paths: Option<&[String]>,
) -> anyhow::Result<(Vec<Finding>, Vec<String>)> {
    let mut changes = crate::qa_impact::io::changes_for_diff(repo, base, Some(head))?;
    if let Some(paths) = paths {
        changes.retain(|c| {
            paths.contains(&c.file) || c.old_file.as_ref().is_some_and(|old| paths.contains(old))
        });
    }
    let registry_touched = changes
        .iter()
        .any(|c| c.file == qa.registry.path || c.old_file.as_deref() == Some(&qa.registry.path));
    let after_paths = tracked(repo, head)?;
    let after = match read(repo, head, &qa.registry.path, &after_paths) {
        Ok(None) => return Ok((vec![], vec![])), // no input file: gate off
        Ok(Some(contents)) => crate::qa_registry::parse(&contents),
        Err(error) => Err(error),
    };
    let registry = match after {
        Ok(registry) => registry,
        Err(error) if registry_touched => {
            return Ok((
                vec![Finding::broken(format!(
                    "{}: cannot read or parse delivered registry: {error:#}",
                    qa.registry.path
                ))],
                vec![],
            ));
        }
        Err(error) => {
            return Ok((
                vec![],
                vec![format!(
                    "UPKEEP WARNING: {} unreadable or malformed (not blocking; untouched by this delivery): {error:#}",
                    qa.registry.path
                )],
            ));
        }
    };
    let before_paths = tracked(repo, base)?;
    let before = read(repo, base, &qa.registry.path, &before_paths)?
        .and_then(|s| crate::qa_registry::parse(&s).ok());
    let old_problems = before
        .as_ref()
        .map(|r| r.validate(&before_paths).problems)
        .unwrap_or_default();
    let problems = registry.validate(&after_paths).problems;
    // Restore only paths removed by this task. A disappearing glob/spec must have
    // matched at base, and restoring the task's deletion must resolve its problem.
    let removed: Vec<_> = changes
        .iter()
        .filter_map(|c| {
            if c.status.starts_with('D') {
                Some(c.file.clone())
            } else if c.status.starts_with('R') {
                c.old_file.clone()
            } else {
                None
            }
        })
        .collect();
    let mut restored_paths = after_paths.clone();
    restored_paths.extend(removed);
    let restored_problems = registry.validate(&restored_paths).problems;
    let caused: Vec<_> = problems
        .iter()
        .filter(|p| {
            !old_problems.contains(p)
                && if matches!(p.code.as_str(), "orphan_glob" | "untracked_spec") {
                    !restored_problems.contains(p)
                } else {
                    registry_touched
                }
        })
        .cloned()
        .collect();
    let debt: Vec<_> = problems
        .iter()
        .filter(|p| !caused.contains(p))
        .cloned()
        .collect();
    let warnings = if debt.is_empty() {
        vec![]
    } else {
        vec![format!(
            "UPKEEP WARNING: pre-existing or unrelated registry problems (not blocking):\n{}",
            problem_lines(&debt)
        )]
    };
    let mut findings = Vec::new();
    if !caused.is_empty() {
        findings.push(Finding::broken(format!(
            "{}: {} problem(s) this diff caused:\n{}",
            qa.registry.path,
            caused.len(),
            problem_lines(&caused)
        )));
    }
    let codemap = read(repo, head, ".claude/CODEMAP.md", &after_paths)
        .ok()
        .flatten()
        .unwrap_or_default();
    let impact = crate::qa_impact::impact_for_changes(Some(&registry), qa, &changes, &codemap);
    let unowned: Vec<_> = impact
        .unowned
        .iter()
        .filter(|file| {
            file.surface
                && changes.iter().any(|c| {
                    c.file == file.file
                        && (c.status.starts_with(['A', 'R'])
                            || (qa.registry.enforce == RegistryEnforcement::Strict
                                && c.status.starts_with('M')))
                })
        })
        .map(|file| {
            format!(
                "- {} — {}",
                file.file,
                file.codemap
                    .as_ref()
                    .map(|c| c.summary.as_str())
                    .unwrap_or("unmapped CODEMAP area")
            )
        })
        .collect();
    if !unowned.is_empty() {
        findings.push(Finding {gate: "G2", token: "no journey impact:", title: "UNOWNED USER-FACING FILE",
            detail: format!("{} {}surface file(s) no journey owns:\n{}\nOwn each file with an owner glob, an owner group, or ignore in {}.", unowned.len(), if qa.registry.enforce == RegistryEnforcement::Strict {"new or modified "} else {"new "}, unowned.join("\n"), qa.registry.path)});
    }
    let live: Vec<_> = impact
        .journeys
        .iter()
        .filter(|j| j.status == "live")
        .map(|j| j.id.as_str())
        .collect();
    if task.task_type == TaskType::Bug
        && !live.is_empty()
        && !(registry_touched && has_changed_case(&registry, before.as_ref(), &task.id))
    {
        findings.push(Finding {gate: "G3", token: "no case:", title: "BUG FIX WITHOUT A CASE",
            detail: format!("Task {} is a bug and touches live journeys {}. Add or extend a bug_families entry whose tasks lists {} (with the invariant that would have caught it), or an invariant with bug.task naming this task. The case must change in this delivery.", task.id, live.join(", "), task.id)});
    }
    Ok((findings, warnings))
}

#[cfg(test)]
#[path = "registry_upkeep_tests.rs"]
mod tests;
