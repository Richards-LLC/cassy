//! Diff-scoped registry upkeep at park (G1–G3). All content comes from Git snapshots.
use super::*;
use crate::config::{QaConfig, RegistryEnforcement};
use crate::qa_impact::io::git;
use crate::qa_registry::{BugFamily, DuplicateBugFamily, Problem, Registry};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

/// One close-path entry point, sharing the QA gate's delivery head and attribution window.
#[allow(clippy::too_many_arguments)]
pub(super) fn at_park(
    notes: &mut CloseNoteTracker<'_>,
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
                notes,
                task,
                format!("UPKEEP WARNING: config unreadable (not blocking): {error}"),
            );
        }
    };
    let qa = config.qa();
    if qa.registry.enforce == RegistryEnforcement::Off {
        // No input at the delivery base means no gate to run. Avoid path
        // attribution and its warnings for projects with local QA disabled.
        let base = snapshot_range(task, repo, target, window, receipt)
            .map(|(base, _)| base)
            .unwrap_or_else(|_| freshest_target_ref(repo, target));
        if git(
            repo,
            &["cat-file", "-e", &format!("{base}:{}", qa.registry.path)],
        )
        .is_err()
        {
            return Ok(vec![]);
        }
    }
    let (base, head, paths) = match delivery_snapshot(task, repo, target, window, receipt) {
        Ok(snapshot) => snapshot,
        Err(message) => return warning(notes, task, message),
    };
    let Some(delivery_head) =
        super::super::qa_evidence_gate::delivered_head(task, repo, target, receipt)
    else {
        return warning(
            notes,
            task,
            "UPKEEP WARNING: delivery head unavailable (not blocking)".into(),
        );
    };
    let target_tip = freshest_target_ref(repo, target);
    // A landed ownership repair can satisfy an older receipt. An unmerged
    // delivery still answers to its own registry, including newly added owners.
    let ownership_head = if git(repo, &["merge-base", "--is-ancestor", &head, &target_tip]).is_ok()
    {
        &target_tip
    } else {
        &head
    };
    check(
        notes,
        task,
        repo,
        &qa,
        &base,
        &head,
        ownership_head,
        paths.as_deref(),
        &delivery_head,
        override_reason,
    )
}

pub(super) fn delivery_snapshot(
    task: &Task,
    repo: &Path,
    target: &str,
    window: Option<&TaskCommitReceiptWindow>,
    receipt: Option<&str>,
) -> Result<(String, String, Option<Vec<String>>), String> {
    // A published delivery owns its integration delta. Re-selecting commits
    // from the now-merged lane can drop unnamed upkeep commits before a target
    // sync. The recorded publication may itself be a generated-only follow-up;
    // find the integration of the immutable parked anchor beneath it instead.
    // Its first-parent delta excludes incoming target work and later repairs.
    if task.status == TaskStatus::AwaitingMerge
        && let Some(anchor) = task.deliverables.factory_branch_anchor.as_deref()
        && let Some(published) = task.deliverables.green_merge_commit.as_deref()
        && let Some(head) =
            super::super::qa_evidence_gate::delivered_head(task, repo, target, receipt)
        && resolve_branch_sha(repo, anchor).as_deref() == Some(head.as_str())
        && git_commit_is_ancestor(repo, &head, published)
        && git_commit_is_ancestor(repo, published, &freshest_target_ref(repo, target))
        && let Some((base, integrated)) =
            crate::qa_evidence::delivery_range(repo, &head, published, None)
        && integrated != head
        && crate::git_evidence::git_commit_parent_count(repo, &integrated) > 1
    {
        // A reused lane can carry a successor beyond this task's parked tip.
        // Only an integration of the exact anchor owns its entire merge delta.
        let merged_parent = resolve_branch_sha(repo, &format!("{integrated}^2"));
        if merged_parent.as_deref() == Some(head.as_str()) {
            return Ok((base, integrated, None));
        }
        tracing::warn!(
            task_id = %task.id,
            parked_tip = %head,
            integration = %integrated,
            ?merged_parent,
            "UPKEEP INTEGRATION PARENT MISMATCH: using task-only delivery attribution"
        );
    }
    let (base, head) = snapshot_range(task, repo, target, window, receipt)?;
    let paths = window.and_then(|w| task_attribution::paths(repo, target, w, Some(&head)));
    if window.is_some() && paths.is_none() {
        return Err("UPKEEP WARNING: attributed delivery paths unavailable (not blocking)".into());
    }
    Ok((base, head, paths))
}

fn snapshot_range(
    task: &Task,
    repo: &Path,
    target: &str,
    window: Option<&TaskCommitReceiptWindow>,
    receipt: Option<&str>,
) -> Result<(String, String), String> {
    let head = super::super::qa_evidence_gate::delivered_head(task, repo, target, receipt)
        .ok_or("UPKEEP WARNING: delivery head unavailable (not blocking)")?;
    let base = if let Some(w) = window {
        task_attribution::delivery_base(repo, target, w, Some(&head))
    } else {
        crate::qa_evidence::delivery_range(repo, &head, &freshest_target_ref(repo, target), None)
            .map(|(base, _)| base)
    }
    .ok_or("UPKEEP WARNING: attributed delivery base unavailable (not blocking)")?;
    Ok((base, head))
}

pub(super) fn warning(
    notes: &mut CloseNoteTracker<'_>,
    task: &mut Task,
    text: String,
) -> Result<Vec<String>, String> {
    notes.record(task, &text);
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

pub(super) struct Finding {
    pub(super) gate: &'static str,
    pub(super) token: &'static str,
    pub(super) title: &'static str,
    pub(super) detail: String,
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
    fn missing_case(detail: String) -> Self {
        Self {
            gate: "G3",
            token: "no case:",
            title: "BUG FIX WITHOUT A CASE",
            detail,
        }
    }
}

/// A G1 waiver cannot authorize a bug whose case coverage was never evaluated.
fn retain_case_gate(task: &Task, findings: &mut Vec<Finding>, reason: &str) {
    if task.task_type == TaskType::Bug && findings.iter().any(|finding| finding.gate == "G1") {
        findings.push(Finding::missing_case(format!(
            "Task {} is a bug, but {reason}; regression-case coverage could not be evaluated. Repair the registry and add or extend a case naming {}.",
            task.id, task.id
        )));
    }
}

impl Finding {
    fn scoped_token(&self, input_path: &str, head: &str) -> String {
        let mut digest = Sha256::new();
        for field in [self.gate, input_path, self.title, &self.detail] {
            digest.update(field.as_bytes());
            digest.update([0]);
        }
        let id = hex::encode(digest.finalize());
        format!(
            "{} delivery={head} finding={}-{}",
            self.token,
            self.gate,
            &id[..16]
        )
    }
}

fn decision_reason<'a>(
    notes: &'a str,
    token: &str,
    delivery_minute: Option<i64>,
) -> Option<&'a str> {
    let delivery_minute = delivery_minute?;
    notes.lines().rev().find_map(|line| {
        let (stamp, decision) = line.strip_prefix('[')?.split_once("] ✅ DECISION ")?;
        let stamp = chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M").ok()?;
        if stamp.and_utc().timestamp().div_euclid(60) < delivery_minute {
            return None;
        }
        let reason = decision.strip_prefix(token)?.strip_prefix(' ')?.trim();
        (!reason.is_empty()).then_some(reason)
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn finish(
    notes: &mut CloseNoteTracker<'_>,
    task: &mut Task,
    repo: &Path,
    delivery_head: &str,
    stage: RegistryEnforcement,
    input_path: &str,
    findings: Vec<Finding>,
    warnings: &[String],
    override_reason: Option<&str>,
) -> Result<Vec<String>, String> {
    // Notes have minute precision. Exact delivery/finding identity is primary;
    // the timestamp also rejects a pre-delivery note carrying copied scope.
    // The inspected snapshot can be an integration merge; waivers belong to
    // the parked delivery anchor and still require the identical finding.
    let delivery_minute = git(repo, &["show", "-s", "--format=%ct", delivery_head])
        .ok()
        .and_then(|stamp| stamp.trim().parse::<i64>().ok())
        .map(|stamp| stamp.div_euclid(60));
    let mut notices = warnings.to_vec();
    for warning in warnings {
        notes.record(task, warning);
    }
    let mut refusals = Vec::new();
    for finding in findings {
        let token = finding.scoped_token(input_path, delivery_head);
        let text = gate_text::upkeep_message(
            &task.id,
            input_path,
            finding.title,
            &finding.detail,
            &token,
            warnings,
            crate::mcp::tools::core::guidance::caller_prefix(),
        );
        if let Some(reason) = override_reason.filter(|reason| !reason.trim().is_empty()) {
            notes.record(
                task,
                &format!(
                    "✅ DECISION UPKEEP ESCAPE {}: supervisor override: {}. Waived refusal: {}",
                    finding.gate,
                    reason.trim(),
                    text
                ),
            );
        } else if stage == RegistryEnforcement::Observe {
            notices.push(format!(
                "UPKEEP OBSERVE {}: {} (see task notes)",
                finding.gate, finding.title
            ));
            notes.record(task, &format!("UPKEEP OBSERVE {}: {text}", finding.gate));
        } else if let Some(reason) =
            decision_reason(&task.notes, &token, delivery_minute).map(str::to_owned)
        {
            notes.record(
                task,
                &format!("UPKEEP ESCAPE {}: {token} {reason}", finding.gate),
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

pub(super) fn tracked(repo: &Path, revision: &str) -> anyhow::Result<Vec<String>> {
    Ok(git(repo, &["ls-tree", "-rz", "--name-only", revision])?
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect())
}
pub(super) fn read(
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
pub(super) fn read_registry(
    repo: &Path,
    revision: &str,
    path: &str,
    paths: &[String],
) -> anyhow::Result<Option<Registry>> {
    crate::qa_registry::load_with(path, paths, |path| read(repo, revision, path, paths))
}

#[derive(Default)]
struct RegistryBaseline {
    registry: Option<Registry>,
    alternatives: BTreeMap<String, Vec<BugFamily>>,
    notices: Vec<String>,
}

/// Recover comparison data only; the delivered registry still uses the strict loader.
/// Keep every duplicate variant so removing one cannot turn an old case into a new one.
fn read_registry_baseline(
    repo: &Path,
    revision: &str,
    path: &str,
    paths: &[String],
) -> anyhow::Result<RegistryBaseline> {
    let mut baseline = RegistryBaseline::default();
    let mut remaining = paths.to_vec();
    let mut first_duplicate: Option<anyhow::Error> = None;
    loop {
        match read_registry(repo, revision, path, &remaining) {
            Ok(registry) => {
                baseline.registry = registry;
                return Ok(baseline);
            }
            Err(error) => {
                if let Some(duplicate) = error.downcast_ref::<DuplicateBugFamily>() {
                    let (key, file) = (duplicate.key.clone(), duplicate.path.clone());
                    let notice = format!(
                        "UPKEEP NOTICE: base registry duplicate repaired by this delivery: {error:#}"
                    );
                    let Some(contents) = read(repo, revision, &file, &remaining)? else {
                        return Err(error.context(format!("BASE JOURNEY REGISTRY INVALID: {path}")));
                    };
                    let family = match serde_yaml::from_str(&contents) {
                        Ok(family) => family,
                        Err(cause) => {
                            return Err(error.context(format!(
                                "BASE JOURNEY REGISTRY INVALID: cannot compare {file}: {cause}"
                            )));
                        }
                    };
                    baseline.alternatives.entry(key).or_default().push(family);
                    baseline.notices.push(notice);
                    remaining.retain(|candidate| candidate != &file);
                    first_duplicate.get_or_insert(error);
                    continue;
                }
                if error.is::<serde_yaml::Error>() {
                    // Retain YAML debt, never discard an incomplete duplicate comparison.
                    return match first_duplicate {
                        Some(duplicate) => Err(duplicate.context(format!(
                            "BASE JOURNEY REGISTRY INVALID: cannot compare {path}: {error:#}"
                        ))),
                        None => Ok(baseline),
                    };
                }
                return Err(error);
            }
        }
    }
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
fn has_changed_case(registry: &Registry, before: &RegistryBaseline, id: &str) -> bool {
    registry.bug_families.iter().any(|(key, family)| {
        family.tasks.iter().any(|task| task == id)
            && changed(
                family,
                before
                    .registry
                    .as_ref()
                    .and_then(|r| r.bug_families.get(key)),
            )
            && before.alternatives.get(key).is_none_or(|variants| {
                variants
                    .iter()
                    .all(|variant| changed(family, Some(variant)))
            })
    }) || registry.invariants.iter().any(|(key, invariant)| {
        let names_task = invariant.bug.as_ref().is_some_and(|bug| {
            bug.get("task").and_then(serde_yaml::Value::as_str) == Some(id)
                || bug
                    .get("tasks")
                    .and_then(serde_yaml::Value::as_sequence)
                    .is_some_and(|tasks| tasks.iter().any(|task| task.as_str() == Some(id)))
        });
        names_task
            && changed(
                invariant,
                before.registry.as_ref().and_then(|r| r.invariants.get(key)),
            )
    })
}

#[allow(clippy::too_many_arguments)]
fn check(
    notes: &mut CloseNoteTracker<'_>,
    task: &mut Task,
    repo: &Path,
    qa: &QaConfig,
    base: &str,
    head: &str,
    ownership_head: &str,
    paths: Option<&[String]>,
    delivery_head: &str,
    override_reason: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut qa = qa.clone();
    // A delivery cannot relax its own gate. At each revision the tracked
    // header wins, with local configuration as fallback only when absent.
    // An unreadable base cannot authorize a downgrade: retain at least enforce
    // (or local strict). A broken head keeps the base/local policy's severity.
    if let Some(stage) = [base, head]
        .into_iter()
        .filter_map(|revision| {
            git(repo, &["show", &format!("{revision}:{}", qa.registry.path)])
                .ok()
                .map(|text| (revision, text))
        })
        .filter_map(|(revision, text)| match crate::qa_registry::parse(&text) {
            Ok(registry) => Some(registry.enforce.unwrap_or(qa.registry.enforce)),
            Err(_) if revision == base => {
                Some(if qa.registry.enforce == RegistryEnforcement::Strict {
                    RegistryEnforcement::Strict
                } else {
                    RegistryEnforcement::Enforce
                })
            }
            Err(_) => None,
        })
        .max_by_key(|stage| match stage {
            RegistryEnforcement::Off => 0,
            RegistryEnforcement::Observe => 1,
            RegistryEnforcement::Enforce => 2,
            RegistryEnforcement::Strict => 3,
        })
    {
        qa.registry.enforce = stage;
    }
    if qa.registry.enforce == RegistryEnforcement::Off {
        return Ok(vec![]);
    }
    let result = inspect(task, repo, &qa, base, head, ownership_head, paths);
    match result {
        Ok((findings, warnings)) => finish(
            notes,
            task,
            repo,
            delivery_head,
            qa.registry.enforce,
            &qa.registry.path,
            findings,
            &warnings,
            override_reason,
        ),
        // Semantic duplicates must not become unavailable-Git warnings or
        // escapable G1 findings that authorize an unexamined delivered registry.
        Err(error) if error.is::<DuplicateBugFamily>() => Err(format!("{error:#}")),
        // Git failures cannot establish causality. Never brick unrelated deliveries.
        Err(error) => warning(
            notes,
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
    ownership_head: &str,
    paths: Option<&[String]>,
) -> anyhow::Result<(Vec<Finding>, Vec<String>)> {
    let mut changes = crate::qa_impact::io::changes_for_diff(repo, base, Some(head))?;
    if let Some(paths) = paths {
        changes.retain(|c| {
            paths.contains(&c.file) || c.old_file.as_ref().is_some_and(|old| paths.contains(old))
        });
    }
    let registry_touched = changes.iter().any(|c| {
        crate::qa_registry::is_registry_input(&qa.registry.path, &c.file)
            || c.old_file
                .as_deref()
                .is_some_and(|old| crate::qa_registry::is_registry_input(&qa.registry.path, old))
    });
    let after_paths = tracked(repo, head)?;
    let after = read_registry(repo, head, &qa.registry.path, &after_paths);
    let registry = match after {
        Ok(None) => return Ok((vec![], vec![])), // no input file: gate off
        Ok(Some(registry)) => registry,
        Err(error) if error.is::<DuplicateBugFamily>() => {
            return Err(error.context(format!(
                "DELIVERED JOURNEY REGISTRY INVALID: {}; remove the duplicate key before retrying",
                qa.registry.path
            )));
        }
        Err(error) if registry_touched => {
            let mut findings = vec![Finding::broken(format!(
                "{}: cannot read or parse delivered registry: {error:#}",
                qa.registry.path
            ))];
            retain_case_gate(
                task,
                &mut findings,
                "the delivered registry cannot be read or parsed",
            );
            return Ok((findings, vec![]));
        }
        Err(error) => {
            let findings = if task.task_type == TaskType::Bug {
                vec![Finding::missing_case(format!(
                    "Task {} is a bug, but {} cannot be read or parsed; regression-case coverage cannot be evaluated. Repair the registry and add or extend a case naming {}.",
                    task.id, qa.registry.path, task.id
                ))]
            } else {
                vec![]
            };
            return Ok((
                findings,
                vec![format!(
                    "UPKEEP WARNING: {} unreadable or malformed (not blocking; untouched by this delivery): {error:#}",
                    qa.registry.path
                )],
            ));
        }
    };
    let before_paths = tracked(repo, base)?;
    let before = read_registry_baseline(repo, base, &qa.registry.path, &before_paths)?;
    let old_problems = before
        .registry
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
    let mut warnings = before.notices.clone();
    if !debt.is_empty() {
        warnings.push(format!(
            "UPKEEP WARNING: pre-existing or unrelated registry problems (not blocking):\n{}",
            problem_lines(&debt)
        ));
    }
    let mut findings = Vec::new();
    if !caused.is_empty() {
        findings.push(Finding::broken(format!(
            "{}: {} problem(s) this diff caused:\n{}",
            qa.registry.path,
            caused.len(),
            problem_lines(&caused)
        )));
    }
    let impact = match crate::qa_impact::analyze(repo, qa, &changes, Some(base), Some(head)) {
        Ok(impact) => impact,
        Err(error) => {
            if error.is::<crate::qa_impact::io::CatalogDrift>() {
                findings.push(Finding::broken(format!("{error:#}")));
            } else {
                warnings.push(format!(
                    "UPKEEP WARNING: journey impact unavailable (not blocking): {error:#}"
                ));
            }
            // An unavailable impact report must not erase proven G1 findings.
            retain_case_gate(task, &mut findings, "journey impact cannot be evaluated");
            return Ok((findings, warnings));
        }
    };
    // Only G2 uses current ownership. G1/G3 retain the delivered registry and
    // every gate retains the task's original changes, never the target's diff.
    // Ownership is independent of legacy catalog selection.
    let ownership_impact = if ownership_head != head {
        let current = (|| -> anyhow::Result<Option<crate::qa_impact::Impact>> {
            let paths = tracked(repo, ownership_head)?;
            let Some(registry) = read_registry(repo, ownership_head, &qa.registry.path, &paths)?
            else {
                return Ok(None);
            };
            let codemap = read(repo, head, ".claude/CODEMAP.md", &after_paths)?.unwrap_or_default();
            Ok(Some(crate::qa_impact::impact_for_changes(
                Some(&registry),
                qa,
                &changes,
                &codemap,
            )))
        })();
        match current {
            Ok(impact) => impact,
            Err(error) => {
                warnings.push(format!(
                    "UPKEEP WARNING: target ownership unavailable (using delivered registry): {error:#}"
                ));
                None
            }
        }
    } else {
        None
    };
    let unowned: Vec<_> = ownership_impact
        .as_ref()
        .unwrap_or(&impact)
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
        && !(registry_touched && has_changed_case(&registry, &before, &task.id))
    {
        findings.push(Finding::missing_case(format!("Task {} is a bug and touches live journeys {}. Add or extend a bug family in a sibling bug-families/<slug>.yaml file (or the inline bug_families map) whose tasks lists {} (with the invariant that would have caught it), or an invariant with bug.task naming this task. The case must change in this delivery.", task.id, live.join(", "), task.id)));
    }
    Ok((findings, warnings))
}

#[cfg(test)]
#[path = "registry_upkeep_tests.rs"]
mod tests;
