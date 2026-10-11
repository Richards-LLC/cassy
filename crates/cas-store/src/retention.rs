//! Bounded retention for the prompt, prompt-queue and supervisor-queue tables
//! (cas-f207).
//!
//! Each store exposes a single-batch step that runs in its own short
//! IMMEDIATE transaction of at most [`RETENTION_MAX_BATCH`] rows. The
//! [`run_retention_batches`] driver repeats a step with the connection lock
//! released and a pause between batches, up to a per-run batch budget, so a
//! large backlog never holds the write lock for long and drains across runs.

use std::time::Duration;

use crate::Result;

/// Most rows one retention transaction touches.
pub const RETENTION_MAX_BATCH: usize = 1_000;

/// Outcome of one bounded retention run over a single table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetentionReport {
    /// Rows deleted or trimmed across all batches.
    pub affected: usize,
    /// Write transactions committed.
    pub batches: usize,
    /// Largest single transaction (never above [`RETENTION_MAX_BATCH`]).
    pub largest_batch: usize,
    /// False when the run stopped at its batch budget with aged rows left.
    pub complete: bool,
}

/// Run `step` in batches of at most `batch_size` rows (clamped to
/// `1..=RETENTION_MAX_BATCH`) until it reports a short batch or `max_batches`
/// batches have committed. `step` receives the batch size and returns how many
/// rows its transaction affected; it must commit before returning, so the
/// pause between batches runs without the write lock.
pub fn run_retention_batches<F>(
    batch_size: usize,
    max_batches: usize,
    pause: Duration,
    mut step: F,
) -> Result<RetentionReport>
where
    F: FnMut(usize) -> Result<usize>,
{
    let mut report = RetentionReport {
        complete: true,
        ..Default::default()
    };
    let batch = batch_size.clamp(1, RETENTION_MAX_BATCH);
    loop {
        if report.batches >= max_batches {
            report.complete = false;
            return Ok(report);
        }
        let affected = step(batch)?;
        if affected == 0 {
            return Ok(report);
        }
        report.affected += affected;
        report.batches += 1;
        report.largest_batch = report.largest_batch.max(affected);
        if affected < batch {
            return Ok(report);
        }
        if !pause.is_zero() {
            std::thread::sleep(pause);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cas-f207: the driver never hands a step more than the bound, stops on a
    /// short batch, and reports an unfinished backlog at the batch budget.
    #[test]
    fn cas_f207_retention_driver_bounds_batches_and_reports_backlog() {
        let mut remaining = 2_500usize;
        let mut seen = Vec::new();
        let report = run_retention_batches(5_000, 10, Duration::ZERO, |batch| {
            seen.push(batch);
            let n = remaining.min(batch);
            remaining -= n;
            Ok(n)
        })
        .unwrap();
        assert!(seen.iter().all(|batch| *batch == RETENTION_MAX_BATCH), "{seen:?}");
        assert_eq!(
            report,
            RetentionReport {
                affected: 2_500,
                batches: 3,
                largest_batch: 1_000,
                complete: true
            }
        );

        let mut calls = 0;
        let capped = run_retention_batches(10, 2, Duration::ZERO, |batch| {
            calls += 1;
            Ok(batch)
        })
        .unwrap();
        assert_eq!(calls, 2, "the batch budget caps the run");
        assert_eq!(capped.affected, 20);
        assert!(!capped.complete, "a full final batch leaves a backlog");

        let empty = run_retention_batches(0, 5, Duration::ZERO, |batch| {
            assert_eq!(batch, 1, "a zero batch size is clamped up to one row");
            Ok(0)
        })
        .unwrap();
        assert_eq!(empty, RetentionReport { complete: true, ..Default::default() });
    }
}
