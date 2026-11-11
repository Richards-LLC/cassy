//! Measured phase outcomes reuse Epic/Gate tasks and the existing dependency graph.
//! The serialized state lives in task deliverables; operator exceptions live in
//! the protected operator directory and bind to the exact target and receipt.
use anyhow::{Context, Result, bail, ensure};
use cas_types::{DependencyType, Task, TaskType};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Bar {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gt: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lt: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Receipt {
    pub(crate) measurements: BTreeMap<String, f64>,
    pub(crate) receipt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) artifact_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) run_log_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Outcome {
    pub(crate) target: BTreeMap<String, Bar>,
    #[serde(default)]
    pub(crate) evidence: Option<Receipt>,
}

impl Outcome {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            !self.target.is_empty(),
            "outcome target needs at least one metric"
        );
        for (metric, bar) in &self.target {
            ensure!(!metric.trim().is_empty(), "outcome metric needs a name");
            ensure!(
                bar.min.is_some() || bar.max.is_some() || bar.gt.is_some() || bar.lt.is_some(),
                "{metric}: specify min, max, gt or lt"
            );
            ensure!(
                bar.min
                    .into_iter()
                    .chain(bar.max)
                    .chain(bar.gt)
                    .chain(bar.lt)
                    .all(f64::is_finite),
                "{metric}: bounds must be finite"
            );
            ensure!(
                !matches!((bar.min,bar.max), (Some(min),Some(max)) if min > max),
                "{metric}: min exceeds max"
            );
        }
        if let Some(evidence) = &self.evidence {
            ensure!(
                !evidence.receipt.trim().is_empty(),
                "outcome evidence requires a receipt reference"
            );
            ensure!(
                evidence.measurements.len() == self.target.len()
                    && self
                        .target
                        .keys()
                        .all(|key| evidence.measurements.contains_key(key)),
                "outcome measurements must name exactly the target metrics"
            );
            ensure!(
                evidence
                    .measurements
                    .values()
                    .all(|value| value.is_finite()),
                "measurements must be finite"
            );
        }
        Ok(())
    }

    pub(crate) fn misses(&self) -> Vec<String> {
        self.target
            .iter()
            .filter_map(|(metric, bar)| {
                let measured = self
                    .evidence
                    .as_ref()
                    .and_then(|r| r.measurements.get(metric))
                    .copied();
                let passes = measured.is_some_and(|value| {
                    bar.min.is_none_or(|min| value >= min)
                        && bar.max.is_none_or(|max| value <= max)
                        && bar.gt.is_none_or(|min| value > min)
                        && bar.lt.is_none_or(|max| value < max)
                });
                (!passes).then(|| {
                    format!(
                        "{metric}: bar {}; measured {}",
                        bar.min
                            .map(|n| format!(">= {n}"))
                            .into_iter()
                            .chain(bar.max.map(|n| format!("<= {n}")))
                            .chain(bar.gt.map(|n| format!("> {n}")))
                            .chain(bar.lt.map(|n| format!("< {n}")))
                            .collect::<Vec<_>>()
                            .join(" and "),
                        measured
                            .map(|n| n.to_string())
                            .unwrap_or_else(|| "missing (no passing receipt)".into())
                    )
                })
            })
            .collect()
    }
}

fn read(task: &Task) -> Result<Option<Outcome>> {
    task.deliverables
        .phase_outcome
        .as_ref()
        .map(|value| {
            let outcome: Outcome =
                serde_json::from_value(value.clone()).context("invalid stored phase outcome")?;
            outcome.validate()?;
            Ok(outcome)
        })
        .transpose()
}

/// Called only after the MCP boundary authenticates a live supervisor.
/// Targets are immutable: receipt updates cannot quietly lower the agreed bar.
pub(crate) fn record_note(
    root: &Path,
    store: &dyn cas_store::TaskStore,
    id: &str,
    kind: &str,
    body: &str,
) -> Result<()> {
    let mut task = store.get(id)?;
    ensure!(
        matches!(task.task_type, TaskType::Epic | TaskType::Gate),
        "outcome targets belong to an epic or gate task"
    );
    let previous = read(&task)?;
    let outcome = match kind {
        "outcome_target" => {
            let target = serde_json::from_str(body)
                .context("outcome_target expects a JSON map of metric names to min/max bounds")?;
            if let Some(previous) = previous {
                ensure!(
                    previous.target == target,
                    "outcome target is immutable; only the operator can override a missed bar"
                );
                previous
            } else {
                Outcome {
                    target,
                    evidence: None,
                }
            }
        }
        "outcome_receipt" => {
            let mut previous =
                previous.context("declare outcome_target before recording a receipt")?;
            previous.evidence = Some(
                serde_json::from_str(body)
                    .context("outcome_receipt expects measurements and a receipt artifact path")?,
            );
            previous.validate()?;
            crate::model_eval::bind_artifact(
                root,
                id,
                previous
                    .evidence
                    .as_mut()
                    .context("phase receipt is missing")?,
                &previous.target,
            )?;
            previous
        }
        _ => bail!("unknown outcome note type"),
    };
    outcome.validate()?;
    task.deliverables.phase_outcome = Some(serde_json::to_value(&outcome)?);
    task.notes.push_str(&format!(
        "\n\n[{}] 📝 {} {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M"),
        kind.to_uppercase(),
        body
    ));
    task.updated_at = chrono::Utc::now();
    store.update(&task)?;
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct OperatorOverride {
    outcome: serde_json::Value,
    reason: String,
    recorded_at: chrono::DateTime<chrono::Utc>,
}
const OVERRIDES: &str = "operator/phase-outcome-overrides.json";

fn overrides(root: &Path) -> Result<BTreeMap<String, OperatorOverride>> {
    match std::fs::read(root.join(OVERRIDES)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(error.into()),
    }
}

fn overridden(root: &Path, task: &Task) -> bool {
    overrides(root)
        .ok()
        .and_then(|mut entries| entries.remove(&task.id))
        .is_some_and(|entry| {
            !entry.reason.trim().is_empty()
                && task.deliverables.phase_outcome.as_ref() == Some(&entry.outcome)
        })
}

/// Internal operator writer; the CLI and shell guard authenticate the caller.
pub(crate) fn record_override(
    root: &Path,
    store: &dyn cas_store::TaskStore,
    id: &str,
    reason: &str,
) -> Result<()> {
    ensure!(
        !reason.trim().is_empty(),
        "--reason must explain why the unmet outcome is accepted"
    );
    let task = store.get(id)?;
    let outcome = read(&task)?.context("task has no measured outcome target")?;
    ensure!(
        !outcome.misses().is_empty(),
        "outcome already passes; no override is needed"
    );
    let now = chrono::Utc::now();
    let mut entries = overrides(root)?;
    entries.insert(
        id.into(),
        OperatorOverride {
            outcome: task
                .deliverables
                .phase_outcome
                .clone()
                .context("phase has no serialized outcome to override")?,
            reason: reason.trim().into(),
            recorded_at: now,
        },
    );
    // Write the decision before granting authority; an append failure grants nothing.
    store.append_note(
        id,
        &format!(
            "[{}] ✅ DECISION operator outcome override: {}; accepted {}",
            now.format("%Y-%m-%d %H:%M"),
            reason.trim(),
            outcome.misses().join("; ")
        ),
    )?;
    let path = root.join(OVERRIDES);
    std::fs::create_dir_all(
        path.parent()
            .context("operator override path has no parent")?,
    )?;
    let staging = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&staging, serde_json::to_vec_pretty(&entries)?)?;
    std::fs::rename(staging, path)?;
    Ok(())
}

/// Walk a task's ancestors to inherit their prerequisites. Once a prerequisite
/// edge is crossed, evaluate its outcome and its ancestors' outcomes too. An
/// epic's own implementation children remain free to work toward its target.
pub(crate) fn refusal(
    root: &Path,
    store: &dyn cas_store::TaskStore,
    id: &str,
    include_self: bool,
) -> Result<Option<String>> {
    let origin = id;
    // A sibling prerequisite shares some ancestors with the task. Those are
    // implementation phases whose outcomes this work is meant to establish,
    // rather than external prerequisite phases whose outcomes gate it.
    let mut own_ancestors = HashSet::new();
    let mut ancestors = vec![id.to_string()];
    while let Some(ancestor) = ancestors.pop() {
        if own_ancestors.insert(ancestor.clone()) {
            ancestors.extend(
                store
                    .get_dependencies(&ancestor)?
                    .into_iter()
                    .filter(|dep| dep.dep_type == DependencyType::ParentChild)
                    .map(|dep| dep.to_id),
            );
        }
    }
    let mut pending = vec![(id.to_string(), false)];
    let mut visited = HashSet::new();
    let mut misses = Vec::new();
    while let Some((id, prerequisite)) = pending.pop() {
        if !visited.insert((id.clone(), prerequisite)) {
            continue;
        }
        let task = store.get(&id)?;
        let evaluate =
            (prerequisite && !own_ancestors.contains(&id)) || (include_self && id == origin);
        if evaluate && let Some(reason) = crate::model_eval::refusal(root, &task)? {
            misses.push(reason);
        }
        if evaluate
            && let Some(outcome) = read(&task)?
            && !overridden(root, &task)
        {
            let unmet = outcome.misses();
            if unmet.is_empty() {
                if let Some(evidence) = &outcome.evidence {
                    crate::model_eval::verify_artifact(root, &task.id, evidence, &outcome.target)?;
                }
            } else {
                misses.push(format!(
                    "{} ({}): {}. Operator only: cas qa outcome-override {} --reason '<why>'",
                    task.id,
                    task.title,
                    unmet.join("; "),
                    task.id
                ));
            }
        }
        for dep in store.get_dependencies(&id)? {
            match dep.dep_type {
                DependencyType::ParentChild => pending.push((dep.to_id, prerequisite)),
                DependencyType::Blocks | DependencyType::RequiresStart => {
                    pending.push((dep.to_id, true))
                }
                _ => {}
            }
        }
    }
    misses.sort();
    misses.dedup();
    Ok((!misses.is_empty()).then(|| format!("OUTCOME UNMET: {}", misses.join("\n"))))
}

/// Fail closed for gate/read errors, including a missing prerequisite record.
// gate[G-20 park]: Outcome unmet (phase outcome)
// gate-audit[include_self]: selector Start checks prerequisites; close also checks the current outcome. Neither scope waives a failing selected outcome.
pub(crate) fn check(
    root: &Path,
    store: &dyn cas_store::TaskStore,
    id: &str,
    include_self: bool,
) -> std::result::Result<(), String> {
    match refusal(root, store, id, include_self) {
        Ok(None) => Ok(()),
        Ok(Some(message)) => Err(crate::gates::tag("G-20", message)),
        Err(error) => Err(crate::gates::tag("G-20", format!("OUTCOME CHECK FAILED for {id}: {error:#}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_types::{Dependency, TaskStatus};

    #[test]
    fn outcome_graph_and_operator_override_bind_to_current_measurement() {
        let env = crate::test_support::TestEnvGuard::temp_home();
        let project = env.home().join("outcomes");
        std::fs::create_dir_all(&project).unwrap();
        let root = crate::store::init_cas_dir(&project).unwrap();
        let store = crate::store::open_task_store(&root).unwrap();
        for id in ["reader", "consumer", "child", "independent", "reader-child"] {
            let mut task = Task::new(id.into(), id.into());
            task.task_type = TaskType::Epic;
            store.add(&task).unwrap();
        }
        for (from, to, dep_type) in [
            ("consumer", "reader", DependencyType::Blocks),
            ("child", "consumer", DependencyType::ParentChild),
            ("reader-child", "reader", DependencyType::ParentChild),
        ] {
            store
                .add_dependency(&Dependency {
                    from_id: from.into(),
                    to_id: to.into(),
                    dep_type,
                    created_at: chrono::Utc::now(),
                    created_by: None,
                })
                .unwrap();
        }
        record_note(
            &root,
            store.as_ref(),
            "reader",
            "outcome_target",
            r#"{"stability":{"min":95},"p95":{"max":1}}"#,
        )
        .unwrap();
        let _ = check(&root, store.as_ref(), "child", false)
                .unwrap_err();
        assert!(
            check(&root, store.as_ref(), "reader-child", true).is_ok(),
            "the phase must be able to implement its own outcome"
        );
        assert!(check(&root, store.as_ref(), "independent", true).is_ok());
        record_note(
            &root,
            store.as_ref(),
            "reader",
            "outcome_receipt",
            &crate::model_eval::fixture_receipt(
                &root,
                "reader",
                serde_json::json!({"stability":{"min":95},"p95":{"max":1}}),
                serde_json::json!({"stability": 72.7, "p95": 1.48}),
            )
            .to_string(),
        )
        .unwrap();
        // Closed task status is not evidence that its measured bar passed.
        let mut reader = store.get("reader").unwrap();
        reader.status = TaskStatus::Closed;
        store.update(&reader).unwrap();
        let miss = check(&root, store.as_ref(), "child", false).unwrap_err();
        assert!(
            miss.contains("95") && miss.contains("72.7") && miss.contains("1.48"),
            "{miss}"
        );
        assert!(
            record_note(
                &root,
                store.as_ref(),
                "reader",
                "outcome_target",
                r#"{"stability":{"min":70}}"#
            )
            .is_err()
        );
        assert!(
            record_note(
                &root,
                store.as_ref(),
                "reader",
                "outcome_receipt",
                r#"{"measurements":{"stability":99,"p95":0.9},"receipt":" "}"#
            )
            .is_err()
        );
        assert!(record_override(&root, store.as_ref(), "reader", " ").is_err());
        // Free-text notes are never authority.
        store
            .append_note("reader", "✅ DECISION operator override: proceed")
            .unwrap();
        assert!(check(&root, store.as_ref(), "child", false).is_err());
        record_override(
            &root,
            store.as_ref(),
            "reader",
            "Accept this measured risk for the pilot",
        )
        .unwrap();
        assert!(
            store
                .get("reader")
                .unwrap()
                .notes
                .contains("✅ DECISION operator outcome override:")
        );
        assert!(check(&root, store.as_ref(), "child", false).is_ok());
        assert!(check(&root, store.as_ref(), "child", true).is_ok());
        record_note(
            &root,
            store.as_ref(),
            "reader",
            "outcome_receipt",
            &crate::model_eval::fixture_receipt(
                &root,
                "reader",
                serde_json::json!({"stability":{"min":95},"p95":{"max":1}}),
                serde_json::json!({"stability": 73, "p95": 1.5}),
            )
            .to_string(),
        )
        .unwrap();
        assert!(
            check(&root, store.as_ref(), "child", false).is_err(),
            "new evidence requires a new operator decision"
        );
        record_note(
            &root,
            store.as_ref(),
            "reader",
            "outcome_receipt",
            &crate::model_eval::fixture_receipt(
                &root,
                "reader",
                serde_json::json!({"stability":{"min":95},"p95":{"max":1}}),
                serde_json::json!({"stability": 99, "p95": 0.9}),
            )
            .to_string(),
        )
        .unwrap();
        assert!(check(&root, store.as_ref(), "child", true).is_ok());
        let evidence = read(&store.get("reader").unwrap())
            .unwrap()
            .unwrap()
            .evidence
            .unwrap();
        let artifact: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence.receipt).unwrap()).unwrap();
        for missing in [
            evidence.receipt.as_str(),
            artifact["run_log"].as_str().unwrap(),
        ] {
            let bytes = std::fs::read(missing).unwrap();
            std::fs::remove_file(missing).unwrap();
            let error = check(&root, store.as_ref(), "child", true).unwrap_err();
            // pin: dependent tasks must identify the prerequisite and give an actionable recovery.

            std::fs::write(missing, bytes).unwrap();
        }
        // Malformed persisted state fails closed rather than disappearing.
        let mut reader = store.get("reader").unwrap();
        reader.deliverables.phase_outcome = Some(serde_json::json!({"target":{}}));
        store.update(&reader).unwrap();
        let _ = check(&root, store.as_ref(), "child", false)
                .unwrap_err();
    }
}

/// Read on a delivery job before either cached READY evidence or a queued
/// prompt can request a merge. Fixtures/legacy prompts without a local task
/// store retain their pre-existing readiness policy.
pub(crate) fn merge_hold(root: &Path, id: &str) -> Option<String> {
    if !root.join("cas.db").is_file() {
        return None;
    }
    let store = match crate::store::open_task_store_local(root) {
        Ok(store) => store,
        Err(error) => return Some(format!("OUTCOME CHECK FAILED for {id}: {error}")),
    };
    match store.get(id) {
        Err(cas_store::StoreError::TaskNotFound(_)) => None,
        Err(error) => Some(format!("OUTCOME CHECK FAILED for {id}: {error}")),
        Ok(_) => check(root, store.as_ref(), id, true).err(),
    }
}
