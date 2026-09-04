//! Bounded injected-relevance evaluation for daemon maintenance.

use std::path::Path;

use cas_store::{RelevanceSamplingReport, RetrievalSample, SqliteRetrievalStore};

use crate::error::CasError;

/// The embedded daemon currently has no receiving-agent or scheduled model
/// adapter. Keep diagnostics tied to this implementation fact so they cannot
/// claim that a missing label is a measured negative.
pub const SCHEDULED_RELEVANCE_JUDGE_CONFIGURED: bool = false;

/// Run one injected-relevance sampling pass with a caller-supplied judge.
///
/// The daemon owns cadence, cool-down, and enablement; this function owns the
/// store boundary. Keeping the judge as a callback makes the job deterministic
/// in tests and allows a receiving-agent or scheduled model runner to be
/// plugged in without coupling the SQLite crate to a model client.
pub fn run_injected_relevance_sampling<F>(
    cas_root: &Path,
    sample_size: usize,
    cooldown_secs: u64,
    judge: F,
) -> Result<RelevanceSamplingReport, CasError>
where
    F: FnMut(&RetrievalSample) -> std::result::Result<Option<bool>, String>,
{
    let store = SqliteRetrievalStore::open(cas_root)?;
    Ok(store.sample_injected_relevance(sample_size, cooldown_secs, judge)?)
}

/// Scheduled daemon runs have no live receiving-agent callback by default.
/// Returning `None` keeps the pass fail-open and leaves the operator an honest
/// `null` precision until a judge is attached.
pub fn run_unconfigured_injected_relevance_sampling(
    cas_root: &Path,
    sample_size: usize,
    cooldown_secs: u64,
) -> Result<RelevanceSamplingReport, CasError> {
    if SCHEDULED_RELEVANCE_JUDGE_CONFIGURED {
        return Err(CasError::Other(
            "scheduled relevance judge is marked configured but has no runtime adapter".to_string(),
        ));
    }
    run_injected_relevance_sampling(cas_root, sample_size, cooldown_secs, |_sample| Ok(None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_store::{DEFAULT_RETRIEVAL_POLICY, RetrievalHitIdentity, RetrievalStore};
    use tempfile::TempDir;

    #[test]
    fn daemon_sampling_job_writes_judge_labels_from_stub() {
        let project = TempDir::new().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let store = SqliteRetrievalStore::open(&cas_root).unwrap();
        store
            .record_query(
                "qry-sampling",
                "repair parser cache",
                "context_session_start",
                DEFAULT_RETRIEVAL_POLICY,
                Some("session"),
                &[RetrievalHitIdentity {
                    result_id: "entry-1".to_string(),
                    document_type: "entry".to_string(),
                    rank: 0,
                }],
            )
            .unwrap();

        let report = run_injected_relevance_sampling(&cas_root, 1, 604_800, |sample| {
            assert_eq!(sample.result_id, "entry-1");
            Ok(Some(true))
        })
        .unwrap();
        assert_eq!(report.sampled, 1);
        assert_eq!(report.labels_recorded, 1);
        assert_eq!(
            store.rolling_injected_precision(30).unwrap().precision,
            Some(1.0)
        );
    }

    #[test]
    fn scheduled_sampling_consumes_the_reported_unconfigured_capability() {
        let project = TempDir::new().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();

        assert!(!SCHEDULED_RELEVANCE_JUDGE_CONFIGURED);
        let report = run_unconfigured_injected_relevance_sampling(&cas_root, 1, 604_800).unwrap();
        assert_eq!(report.labels_recorded, 0);
    }
}
