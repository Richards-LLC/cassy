//! Quantitative model proof uses the phase receipt shape: measurements plus
//! an actual evaluation artifact. Both gates bind that artifact and its run log.
use crate::phase_outcome::{Bar, Outcome, Receipt};
use anyhow::{Context, Result, ensure};
use cas_types::Task;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

type Metrics = BTreeMap<String, f64>;
type Thresholds = BTreeMap<String, Bar>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    dataset: String,
    thresholds: Thresholds,
}

/// The evaluator's saved JSON artifact, shared with ordinary phase receipts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EvalArtifact {
    dataset: String,
    model: String,
    command: String,
    run_log: String,
    thresholds: Thresholds,
    measurements: Metrics,
}

impl EvalArtifact {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.dataset.trim().is_empty(),
            "model eval artifact requires a dataset id or hash"
        );
        ensure!(
            !self.model.trim().is_empty(),
            "model eval artifact requires a model id"
        );
        ensure!(
            !self.command.trim().is_empty(),
            "model eval artifact requires the evaluation command"
        );
        ensure!(
            Path::new(&self.run_log).is_absolute(),
            "model eval artifact requires an absolute run log path"
        );
        Ok(())
    }
}

fn task_artifacts_dir(root: &Path, id: &str) -> Result<PathBuf> {
    ensure!(
        !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "invalid task id for eval evidence: {id}"
    );
    let config = crate::config::Config::load(root)?;
    let base =
        crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
    Ok(crate::config::project_factory_artifacts_root(root, &base).join(id))
}

fn scoped_file(root: &Path, id: &str, path: &Path, kind: &str) -> Result<PathBuf> {
    ensure!(path.is_absolute(), "eval {kind} must name an absolute path");
    let recovery = || {
        format!(
            "eval {kind} missing or unreadable for {id}: {}; re-record the receipt",
            path.display()
        )
    };
    let resolved = path.canonicalize().with_context(recovery)?;
    let task_dir = task_artifacts_dir(root, id)?
        .canonicalize()
        .with_context(recovery)?;
    ensure!(
        resolved.starts_with(&task_dir),
        "eval {kind} for {id} must live under the task artifacts directory {}; re-record the receipt",
        task_dir.display()
    );
    ensure!(
        resolved.is_file(),
        "eval {kind} is not a file for {id}; re-record the receipt"
    );
    Ok(resolved)
}

/// The evaluator emits one result line; unrelated numbers in prose are not proof.
fn log_digest(path: &Path, measurements: &Metrics) -> Result<String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RunResult {
        measurements: Metrics,
    }
    let bytes = std::fs::read(path).context("cannot read eval run log; re-record the receipt")?;
    let text = std::str::from_utf8(&bytes).unwrap_or_default();
    let mut results = text
        .lines()
        .filter_map(|line| line.strip_prefix("MODEL_EVAL_RESULT "));
    let result = results
        .next()
        .and_then(|line| serde_json::from_str::<RunResult>(line).ok());
    let ambiguous = results.next().is_some();
    let unbacked: Vec<_> = measurements
        .iter()
        .filter_map(|(metric, value)| {
            (ambiguous || result.as_ref().and_then(|r| r.measurements.get(metric)) != Some(value))
                .then(|| format!("{metric}={value}"))
        })
        .collect();
    ensure!(
        unbacked.is_empty(),
        "MODEL_EVAL_UNBACKED: {}; run log {} must contain exactly one MODEL_EVAL_RESULT JSON line with matching measurements; re-record the receipt",
        unbacked.join(", "),
        path.display()
    );
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

fn load_artifact(
    root: &Path,
    id: &str,
    evidence: &Receipt,
    thresholds: &Thresholds,
) -> Result<(EvalArtifact, String, String)> {
    let path = Path::new(&evidence.receipt);
    ensure!(
        path.is_absolute(),
        "eval receipt must name an absolute path to its saved JSON artifact; fetch a remote artifact first"
    );
    let path = scoped_file(root, id, path, "receipt artifact")?;
    let bytes =
        std::fs::read(path).context("cannot read eval receipt artifact; re-record the receipt")?;
    let artifact: EvalArtifact =
        serde_json::from_slice(&bytes).context("invalid eval receipt artifact")?;
    artifact.validate()?;
    ensure!(
        &artifact.thresholds == thresholds,
        "eval artifact thresholds do not match the declared bar"
    );
    ensure!(
        artifact.measurements == evidence.measurements,
        "eval artifact measurements do not match the receipt"
    );
    let log = scoped_file(root, id, Path::new(&artifact.run_log), "run log")?;
    let run_log_sha = log_digest(&log, &evidence.measurements)?;
    Ok((
        artifact,
        format!("{:x}", Sha256::digest(&bytes)),
        run_log_sha,
    ))
}

/// Recording computes these bindings from the actual files, never caller hashes.
pub(crate) fn bind_artifact(
    root: &Path,
    id: &str,
    evidence: &mut Receipt,
    thresholds: &Thresholds,
) -> Result<EvalArtifact> {
    let (artifact, artifact_sha, log_sha) = load_artifact(root, id, evidence, thresholds)?;
    evidence.artifact_sha256 = Some(artifact_sha);
    evidence.run_log_sha256 = Some(log_sha);
    Ok(artifact)
}

pub(crate) fn verify_artifact(
    root: &Path,
    id: &str,
    evidence: &Receipt,
    thresholds: &Thresholds,
) -> Result<EvalArtifact> {
    let (artifact, artifact_sha, log_sha) = load_artifact(root, id, evidence, thresholds)?;
    ensure!(
        Some(artifact_sha) == evidence.artifact_sha256,
        "MODEL EVAL INVALID: receipt artifact changed or is not bound; record a fresh receipt"
    );
    ensure!(
        Some(log_sha) == evidence.run_log_sha256,
        "MODEL EVAL INVALID: run log changed since receipt recording; record a fresh receipt"
    );
    Ok(artifact)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Evaluation {
    target: Target,
    receipt: Option<Receipt>,
    artifact: Option<EvalArtifact>,
}

impl Evaluation {
    fn numeric(&self) -> Outcome {
        Outcome {
            target: self.target.thresholds.clone(),
            evidence: self.receipt.clone(),
        }
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            !self.target.dataset.trim().is_empty(),
            "model eval target requires a dataset id or hash"
        );
        self.numeric().validate()?;
        if let Some(receipt) = &self.receipt {
            let artifact = self
                .artifact
                .as_ref()
                .context("model eval receipt has no artifact metadata")?;
            artifact.validate()?;
            ensure!(
                artifact.dataset == self.target.dataset,
                "model eval artifact dataset does not match declared dataset {}",
                self.target.dataset
            );
            ensure!(
                artifact.thresholds == self.target.thresholds
                    && artifact.measurements == receipt.measurements,
                "model eval artifact does not match the target and receipt"
            );
        }
        Ok(())
    }
}

fn read(task: &Task) -> Result<Option<Evaluation>> {
    task.deliverables
        .model_eval
        .as_ref()
        .map(|value| {
            let state: Evaluation = serde_json::from_value(value.clone())
                .context("invalid stored model eval receipt")?;
            state.validate()?;
            Ok(state)
        })
        .transpose()
}

/// Targets are supervisor-authenticated; receipts are assignee/supervisor proof.
/// A below-bar result is saved as a miss and cannot authorize close.
pub(crate) fn record_note(
    root: &Path,
    store: &dyn cas_store::TaskStore,
    id: &str,
    kind: &str,
    body: &str,
) -> Result<&'static str> {
    let mut task = store.get(id)?;
    let previous = read(&task)?;
    let state = if kind == "model_eval_target" {
        let target: Target = serde_json::from_str(body)
            .context("model_eval_target expects dataset and thresholds")?;
        match previous {
            Some(previous) => {
                ensure!(
                    previous.target == target,
                    "model eval target is immutable; a receipt cannot lower its bar or replace its dataset"
                );
                previous
            }
            None => Evaluation {
                target,
                receipt: None,
                artifact: None,
            },
        }
    } else {
        ensure!(kind == "model_eval_receipt", "unknown model eval note type");
        let mut previous =
            previous.context("declare model_eval_target before recording a model eval receipt")?;
        let mut receipt: Receipt = serde_json::from_str(body)
            .context("model_eval_receipt expects measurements and a receipt artifact path")?;
        let artifact = bind_artifact(root, id, &mut receipt, &previous.target.thresholds)?;
        previous.receipt = Some(receipt);
        previous.artifact = Some(artifact);
        previous
    };
    state.validate()?;
    let verdict = if kind == "model_eval_target" {
        "MODEL_EVAL_TARGET"
    } else if state.numeric().misses().is_empty() {
        "MODEL_EVAL_PASS"
    } else {
        "MODEL_EVAL_MISS"
    };
    task.deliverables.model_eval = Some(serde_json::to_value(&state)?);
    task.notes.push_str(&format!(
        "\n\n[{}] 🧪 {} {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M"),
        verdict,
        body
    ));
    task.updated_at = chrono::Utc::now();
    store.update(&task)?;
    Ok(verdict)
}

/// A phase exception never waives a failed model evaluation.
pub(crate) fn refusal(root: &Path, task: &Task) -> Result<Option<String>> {
    let Some(state) = read(task)? else {
        return Ok(None);
    };
    let misses = state.numeric().misses();
    if !misses.is_empty() {
        return Ok(Some(format!(
            "MODEL EVAL UNMET: {} dataset={} model={}: {}",
            task.id,
            state.target.dataset,
            state
                .artifact
                .as_ref()
                .map(|a| a.model.as_str())
                .unwrap_or("missing"),
            misses.join("; ")
        )));
    }
    let receipt = state
        .receipt
        .as_ref()
        .context("model eval receipt is missing")?;
    let actual = verify_artifact(root, &task.id, receipt, &state.target.thresholds)?;
    ensure!(
        Some(actual) == state.artifact,
        "model eval artifact metadata changed"
    );
    Ok(None)
}

#[cfg(test)]
pub(crate) fn fixture_receipt(
    root: &Path,
    id: &str,
    thresholds: serde_json::Value,
    measurements: serde_json::Value,
) -> serde_json::Value {
    let dir = task_artifacts_dir(root, id).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    let stem = uuid::Uuid::new_v4().to_string();
    let path = dir.join(format!("eval-{stem}.json"));
    let log = dir.join(format!("eval-{stem}.log"));
    std::fs::write(
        &log,
        format!(
            "MODEL_EVAL_RESULT {}\n",
            serde_json::json!({"measurements":measurements})
        ),
    )
    .unwrap();
    std::fs::write(&path, serde_json::json!({"dataset":"reader-v1","model":"provider/model", "command":"reader-eval --dataset reader-v1",
        "thresholds":thresholds,"measurements":measurements,"run_log":log}).to_string()).unwrap();
    serde_json::json!({"measurements":measurements,"receipt":path})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_eval_receipt_binds_dataset_model_measurements_and_run_log() {
        let env = crate::test_support::TestEnvGuard::temp_home();
        let project = env.home().join("eval");
        std::fs::create_dir_all(&project).unwrap();
        let root = crate::store::init_cas_dir(&project).unwrap();
        let store = crate::store::open_task_store(&root).unwrap();
        let mut task = Task::new("eval".into(), "Model evaluation".into());
        task.task_type = cas_types::TaskType::Epic;
        store.add(&task).unwrap();
        record_note(
            &root,
            store.as_ref(),
            "eval",
            "model_eval_target",
            r#"{"dataset":"reader-v1","thresholds":{"accuracy":{"min":95}}}"#,
        )
        .unwrap();
        let _ = refusal(&root, &store.get("eval").unwrap())
                .unwrap()
                .unwrap();
        let receipt = fixture_receipt(
            &root,
            "eval",
            serde_json::json!({"accuracy":{"min":95}}),
            serde_json::json!({"accuracy":96}),
        );
        let path = Path::new(receipt["receipt"].as_str().unwrap());
        let original = std::fs::read(path).unwrap();
        let artifact: serde_json::Value = serde_json::from_slice(&original).unwrap();
        let log = Path::new(artifact["run_log"].as_str().unwrap());
        assert!(
            record_note(
                &root,
                store.as_ref(),
                "eval",
                "model_eval_receipt",
                &artifact.to_string()
            )
            .is_err(),
            "inline claims are not artifact proof"
        );
        for (field, value) in [
            ("dataset", serde_json::json!("other")),
            ("model", serde_json::json!("")),
            ("command", serde_json::json!("")),
            ("run_log", serde_json::json!("relative.log")),
            ("measurements", serde_json::json!({"accuracy":100})),
            ("thresholds", serde_json::json!({"accuracy":{"min":80}})),
        ] {
            let mut invalid = artifact.clone();
            invalid[field] = value;
            std::fs::write(path, invalid.to_string()).unwrap();
            assert!(
                record_note(
                    &root,
                    store.as_ref(),
                    "eval",
                    "model_eval_receipt",
                    &receipt.to_string()
                )
                .is_err(),
                "{field}"
            );
        }
        std::fs::write(path, &original).unwrap();
        let original_log = std::fs::read(log).unwrap();
        for text in [
            "accuracy=96 appears in prose only\n",
            "MODEL_EVAL_RESULT {\"measurements\":{\"other_metric\":96}}\n",
            "MODEL_EVAL_RESULT {\"measurements\":{\"accuracy\":96}}\nMODEL_EVAL_RESULT {\"measurements\":{\"accuracy\":80.3}}\n",
        ] {
            std::fs::write(log, text).unwrap();
            let error = record_note(
                &root,
                store.as_ref(),
                "eval",
                "model_eval_receipt",
                &receipt.to_string(),
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains("MODEL_EVAL_UNBACKED"));
        }
        std::fs::write(log, original_log).unwrap();
        // Both direct paths and symlinks must stay within this task's evidence.
        #[cfg(unix)]
        for field in ["receipt", "run_log"] {
            let outside = project.join(format!("outside-{field}"));
            std::fs::copy(if field == "receipt" { path } else { log }, &outside).unwrap();
            let link = path.parent().unwrap().join(format!("escape-{field}"));
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            for location in [&outside, &link] {
                let mut candidate = receipt.clone();
                if field == "receipt" {
                    candidate["receipt"] = serde_json::json!(location);
                } else {
                    let mut changed = artifact.clone();
                    changed["run_log"] = serde_json::json!(location);
                    std::fs::write(path, changed.to_string()).unwrap();
                }
                let error = record_note(
                    &root,
                    store.as_ref(),
                    "eval",
                    "model_eval_receipt",
                    &candidate.to_string(),
                )
                .unwrap_err()
                .to_string();
                // pin: the refusal explains how to correct evidence stored outside the task.
            }
            std::fs::write(path, &original).unwrap();
        }
        assert!(
            record_note(
                &root,
                store.as_ref(),
                "eval",
                "model_eval_target",
                r#"{"dataset":"reader-v1","thresholds":{"accuracy":{"min":80}}}"#
            )
            .is_err()
        );
        crate::phase_outcome::record_note(
            &root,
            store.as_ref(),
            "eval",
            "outcome_target",
            r#"{"service":{"min":1}}"#,
        )
        .unwrap();
        crate::phase_outcome::record_override(
            &root,
            store.as_ref(),
            "eval",
            "Accept operational miss for pilot",
        )
        .unwrap();
        let failed = fixture_receipt(
            &root,
            "eval",
            serde_json::json!({"accuracy":{"min":95}}),
            serde_json::json!({"accuracy":87.9}),
        );
        record_note(
            &root,
            store.as_ref(),
            "eval",
            "model_eval_receipt",
            &failed.to_string(),
        )
        .unwrap();
        assert!(store.get("eval").unwrap().notes.contains("MODEL_EVAL_MISS"));
        let _ = crate::phase_outcome::check(&root, store.as_ref(), "eval", true)
                .unwrap_err();
        record_note(
            &root,
            store.as_ref(),
            "eval",
            "model_eval_receipt",
            &receipt.to_string(),
        )
        .unwrap();
        assert!(
            refusal(&root, &store.get("eval").unwrap())
                .unwrap()
                .is_none()
        );
        let mut changed = artifact.clone();
        changed["command"] = serde_json::json!("different-command");
        std::fs::write(path, changed.to_string()).unwrap();
        let _ = refusal(&root, &store.get("eval").unwrap())
                .unwrap_err()
                .to_string();
        std::fs::write(path, original).unwrap();
        std::fs::write(
            log,
            "different evaluation\nMODEL_EVAL_RESULT {\"measurements\":{\"accuracy\":96}}\n",
        )
        .unwrap();
        let _ = refusal(&root, &store.get("eval").unwrap())
                .unwrap_err()
                .to_string();
    }
}
