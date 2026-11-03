//! Model evaluations are quantitative close proof, separate from deterministic
//! test receipts. The shared outcome graph supplies close and dependency holds.
use crate::phase_outcome::{Bar, Outcome, Receipt};
use anyhow::{Context, Result, ensure};
use cas_types::Task;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    dataset: String,
    thresholds: BTreeMap<String, Bar>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvalReceipt {
    dataset: String,
    model: String,
    run_log: String,
    measurements: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Evaluation {
    target: Target,
    receipt: Option<EvalReceipt>,
    run_log_sha256: Option<String>,
}

impl Evaluation {
    fn numeric(&self) -> Outcome {
        Outcome {
            target: self.target.thresholds.clone(),
            evidence: self.receipt.as_ref().map(|receipt| Receipt {
                measurements: receipt.measurements.clone(),
                receipt: receipt.run_log.clone(),
            }),
        }
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            !self.target.dataset.trim().is_empty(),
            "model eval target requires a dataset id or hash"
        );
        self.numeric().validate()?;
        if let Some(receipt) = &self.receipt {
            ensure!(
                receipt.dataset == self.target.dataset,
                "model eval receipt dataset does not match declared dataset {}",
                self.target.dataset
            );
            ensure!(
                !receipt.model.trim().is_empty(),
                "model eval receipt requires a model id"
            );
            ensure!(
                Path::new(&receipt.run_log).is_absolute(),
                "model eval receipt requires an absolute run log path"
            );
        }
        Ok(())
    }
}

fn log_digest(path: &str) -> Result<String> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("cannot read model eval run log {path}"))?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() > 0,
        "model eval run log must be a non-empty file: {path}"
    );
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
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

/// The MCP boundary authenticates target declarations and receipt ownership.
/// A below-bar receipt is valid evidence and is saved; it never authorizes close.
pub(crate) fn record_note(
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
                    "model eval target is immutable; a receipt cannot lower its declared bar or replace its dataset"
                );
                previous
            }
            None => Evaluation {
                target,
                receipt: None,
                run_log_sha256: None,
            },
        }
    } else {
        ensure!(kind == "model_eval_receipt", "unknown model eval note type");
        let mut previous =
            previous.context("declare model_eval_target before recording a model eval receipt")?;
        previous.receipt = Some(
            serde_json::from_str(body)
                .context("model_eval_receipt expects dataset, model, run_log and measurements")?,
        );
        previous.validate()?;
        previous.run_log_sha256 = Some(log_digest(&previous.receipt.as_ref().unwrap().run_log)?);
        previous
    };
    state.validate()?;
    task.deliverables.model_eval = Some(serde_json::to_value(&state)?);
    let verdict = if kind == "model_eval_target" {
        "MODEL_EVAL_TARGET"
    } else if state.numeric().misses().is_empty() {
        "MODEL_EVAL_PASS"
    } else {
        "MODEL_EVAL_MISS"
    };
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

/// A phase outcome exception never waives a failed model evaluation.
pub(crate) fn refusal(task: &Task) -> Result<Option<String>> {
    let Some(state) = read(task)? else {
        return Ok(None);
    };
    let missed = state.numeric().misses();
    if !missed.is_empty() {
        return Ok(Some(format!(
            "MODEL EVAL UNMET: {} dataset={} model={}: {}",
            task.id,
            state.target.dataset,
            state
                .receipt
                .as_ref()
                .map(|r| r.model.as_str())
                .unwrap_or("missing"),
            missed.join("; ")
        )));
    }
    let receipt = state
        .receipt
        .as_ref()
        .context("model eval receipt is missing")?;
    ensure!(
        Some(log_digest(&receipt.run_log)?) == state.run_log_sha256,
        "MODEL EVAL INVALID: {} run log changed since the receipt was recorded: {}",
        task.id,
        receipt.run_log
    );
    Ok(None)
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
        let log = project.join("model.log");
        std::fs::write(&log, "reader evaluation\n").unwrap();
        record_note(
            store.as_ref(),
            "eval",
            "model_eval_target",
            r#"{"dataset":"reader-v1","thresholds":{"accuracy":{"min":95}}}"#,
        )
        .unwrap();
        assert!(
            refusal(&store.get("eval").unwrap())
                .unwrap()
                .unwrap()
                .contains("measured missing")
        );
        let receipt = serde_json::json!({"dataset":"reader-v1", "model":"provider/model", "run_log":log, "measurements":{"accuracy":96}});
        for (field, value) in [
            ("dataset", serde_json::json!("other-dataset")),
            ("model", serde_json::json!("")),
            ("run_log", serde_json::json!("relative.log")),
            ("measurements", serde_json::json!({"other":100})),
        ] {
            let mut invalid = receipt.clone();
            invalid[field] = value;
            assert!(
                record_note(
                    store.as_ref(),
                    "eval",
                    "model_eval_receipt",
                    &invalid.to_string()
                )
                .is_err(),
                "{field}"
            );
        }
        assert!(
            record_note(
                store.as_ref(),
                "eval",
                "model_eval_target",
                r#"{"dataset":"reader-v1","thresholds":{"accuracy":{"min":80}}}"#
            )
            .is_err()
        );
        crate::phase_outcome::record_note(
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
        let mut failed = receipt.clone();
        failed["measurements"]["accuracy"] = serde_json::json!(87.9);
        record_note(
            store.as_ref(),
            "eval",
            "model_eval_receipt",
            &failed.to_string(),
        )
        .unwrap();
        assert!(store.get("eval").unwrap().notes.contains("MODEL_EVAL_MISS"));
        assert!(
            crate::phase_outcome::check(&root, store.as_ref(), "eval", true)
                .unwrap_err()
                .contains("MODEL EVAL UNMET")
        );
        record_note(
            store.as_ref(),
            "eval",
            "model_eval_receipt",
            &receipt.to_string(),
        )
        .unwrap();
        assert!(refusal(&store.get("eval").unwrap()).unwrap().is_none());
        std::fs::write(&log, "different evaluation\n").unwrap();
        assert!(
            refusal(&store.get("eval").unwrap())
                .unwrap_err()
                .to_string()
                .contains("run log changed")
        );
    }
}
