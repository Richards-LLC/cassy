//! Shared park-gate helpers: delivery snapshots, scoped decision escapes and
//! the epic escape count. Journey registry upkeep (G-49) runs as the lane lint
//! (`scripts/doc-fragments.py check`, cas-4b33).
use super::*;
use crate::config::RegistryEnforcement;
use crate::qa_impact::io::git;
use sha2::{Digest, Sha256};
use std::path::Path;

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
// gate-audit[override_reason]: bypass G-51 Authorizes a report-quality finding waiver; final close retains its successful notice.
// gate-audit[decision_reason]: bypass G-51 Accepts the same measured report finding under a delivery/finding-bound operator decision note.
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
            &format!("{} ({})", finding.title, finding.gate),
            &finding.detail,
            &token,
            warnings,
            crate::mcp::tools::core::guidance::caller_prefix(),
        );
        if let Some(reason) = override_reason.filter(|reason| !reason.trim().is_empty()) {
            notices.push(crate::gates::override_tag("G-51", ""));
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
            notices.push(crate::gates::override_tag("G-51", ""));
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
