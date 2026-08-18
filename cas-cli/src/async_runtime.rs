//! Async runtime utilities for Cassy
//!
//! Provides helpers for running async operations from sync code,
//! progress reporting, and cancellation support.
//!
//! # Usage
//!
//! ```rust,ignore
//! use cas::async_runtime::{run_async, AsyncOperation, Progress};
//!
//! // Run an async operation from sync code
//! let result = run_async(async {
//!     // async work here
//!     Ok("done")
//! })?;
//!
//! // With progress reporting
//! let op = AsyncOperation::new("Reindexing", 100);
//! run_async(async {
//!     for i in 0..100 {
//!
//! # Integration Status
//!
//! Ready for use in CLI commands that need progress reporting.

// #![allow(dead_code)] // Check unused

// NOTE: Original doc comment continues below - run_async(async { for i in 0..100 {
//!         op.set_progress(i);
//!         // do work
//!     }
//!     op.complete();
//!     Ok(())
//! })?;
//! ```

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::error::CasError;

/// Run an async operation from synchronous code
///
/// Creates a new tokio runtime if one doesn't exist.
/// For operations that are already async, prefer calling directly.
pub fn run_async<F, T>(future: F) -> Result<T, CasError>
where
    F: Future<Output = Result<T, CasError>>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CasError::Other(format!("Failed to create async runtime: {e}")))?;

    runtime.block_on(future)
}

/// Progress tracking for long-running operations
#[derive(Debug, Clone)]
pub struct Progress {
    /// Current progress (0-100)
    current: Arc<AtomicU64>,
    /// Total items to process
    total: Arc<AtomicU64>,
    /// Whether the operation is complete
    completed: Arc<AtomicBool>,
    /// Whether the operation was cancelled
    cancelled: Arc<AtomicBool>,
    /// Start time
    start_time: Instant,
}

impl Default for Progress {
    fn default() -> Self {
        Self::new(100)
    }
}

impl Progress {
    /// Create a new progress tracker with the given total
    pub fn new(total: u64) -> Self {
        Self {
            current: Arc::new(AtomicU64::new(0)),
            total: Arc::new(AtomicU64::new(total)),
            completed: Arc::new(AtomicBool::new(false)),
            cancelled: Arc::new(AtomicBool::new(false)),
            start_time: Instant::now(),
        }
    }

    /// Get current progress value
    pub fn current(&self) -> u64 {
        self.current.load(Ordering::SeqCst)
    }

    /// Get total value
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::SeqCst)
    }

    /// Get progress as percentage (0-100)
    pub fn percentage(&self) -> f64 {
        let total = self.total();
        if total == 0 {
            return 100.0;
        }
        (self.current() as f64 / total as f64) * 100.0
    }

    /// Set current progress
    pub fn set(&self, value: u64) {
        self.current.store(value, Ordering::SeqCst);
    }

    /// Increment progress by 1
    pub fn increment(&self) {
        self.current.fetch_add(1, Ordering::SeqCst);
    }

    /// Increment progress by a value
    pub fn increment_by(&self, value: u64) {
        self.current.fetch_add(value, Ordering::SeqCst);
    }

    /// Update total (for dynamic progress)
    pub fn set_total(&self, total: u64) {
        self.total.store(total, Ordering::SeqCst);
    }

    /// Mark as complete
    pub fn complete(&self) {
        self.completed.store(true, Ordering::SeqCst);
        self.current.store(self.total(), Ordering::SeqCst);
    }

    /// Check if complete
    pub fn is_complete(&self) -> bool {
        self.completed.load(Ordering::SeqCst)
    }

    /// Request cancellation
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Check if cancelled
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Get elapsed time
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Estimate remaining time based on current progress
    pub fn estimated_remaining(&self) -> Option<Duration> {
        let current = self.current();
        let total = self.total();
        let elapsed = self.elapsed();

        if current == 0 || current >= total {
            return None;
        }

        let rate = current as f64 / elapsed.as_secs_f64();
        let remaining = total - current;
        let remaining_secs = remaining as f64 / rate;

        Some(Duration::from_secs_f64(remaining_secs))
    }

    /// Format progress as a string (e.g., "Processing: 50/100 (50%) - ETA: 30s")
    pub fn format(&self, label: &str) -> String {
        let current = self.current();
        let total = self.total();
        let pct = self.percentage();

        let eta = self
            .estimated_remaining()
            .map(|d| format!(" - ETA: {:.0}s", d.as_secs_f64()))
            .unwrap_or_default();

        format!("{label}: {current}/{total} ({pct:.0}%){eta}")
    }
}

/// A long-running async operation with progress tracking
pub struct AsyncOperation {
    /// Operation name/label
    pub name: String,
    /// Progress tracker
    pub progress: Progress,
}

impl AsyncOperation {
    /// Create a new operation with the given name and total items
    pub fn new(name: &str, total: u64) -> Self {
        Self {
            name: name.to_string(),
            progress: Progress::new(total),
        }
    }

    /// Set progress value
    pub fn set_progress(&self, value: u64) {
        self.progress.set(value);
    }

    /// Increment progress
    pub fn increment(&self) {
        self.progress.increment();
    }

    /// Mark as complete
    pub fn complete(&self) {
        self.progress.complete();
    }

    /// Check if cancelled
    pub fn is_cancelled(&self) -> bool {
        self.progress.is_cancelled()
    }

    /// Cancel the operation
    pub fn cancel(&self) {
        self.progress.cancel();
    }

    /// Get formatted status string
    pub fn status(&self) -> String {
        self.progress.format(&self.name)
    }
}

/// Batch processor for running operations on many items
pub struct BatchProcessor<T> {
    items: Vec<T>,
    progress: Progress,
    batch_size: usize,
}

impl<T> BatchProcessor<T> {
    /// Create a new batch processor
    pub fn new(items: Vec<T>, batch_size: usize) -> Self {
        let total = items.len() as u64;
        Self {
            items,
            progress: Progress::new(total),
            batch_size,
        }
    }

    /// Get progress tracker
    pub fn progress(&self) -> &Progress {
        &self.progress
    }

    /// Process items in batches with a sync callback
    pub fn process_sync<F, R, E>(&self, mut callback: F) -> Result<Vec<R>, E>
    where
        F: FnMut(&T) -> Result<R, E>,
    {
        let mut results = Vec::with_capacity(self.items.len());

        for item in &self.items {
            if self.progress.is_cancelled() {
                break;
            }

            results.push(callback(item)?);
            self.progress.increment();
        }

        self.progress.complete();
        Ok(results)
    }

    /// Get batch iterator
    pub fn batches(&self) -> impl Iterator<Item = &[T]> {
        self.items.chunks(self.batch_size)
    }

    /// Total number of items
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Run a cancellable async operation with timeout
pub async fn with_timeout<F, T>(future: F, timeout: Duration) -> Result<T, CasError>
where
    F: Future<Output = Result<T, CasError>>,
{
    tokio::time::timeout(timeout, future)
        .await
        .map_err(|_| CasError::Other("Operation timed out".to_string()))?
}

#[cfg(test)]
mod tests {
    use crate::async_runtime::*;

    #[test]
    fn test_progress_basic() {
        let progress = Progress::new(100);
        assert_eq!(progress.current(), 0);
        assert_eq!(progress.total(), 100);
        assert_eq!(progress.percentage(), 0.0);

        progress.set(50);
        assert_eq!(progress.current(), 50);
        assert_eq!(progress.percentage(), 50.0);

        progress.increment();
        assert_eq!(progress.current(), 51);

        progress.increment_by(9);
        assert_eq!(progress.current(), 60);
    }

    #[test]
    fn test_progress_complete() {
        let progress = Progress::new(100);
        assert!(!progress.is_complete());

        progress.complete();
        assert!(progress.is_complete());
        assert_eq!(progress.current(), 100);
    }

    #[test]
    fn test_progress_cancel() {
        let progress = Progress::new(100);
        assert!(!progress.is_cancelled());

        progress.cancel();
        assert!(progress.is_cancelled());
    }

    #[test]
    fn test_progress_format() {
        let progress = Progress::new(100);
        progress.set(50);

        let status = progress.format("Processing");
        assert!(status.contains("50/100"));
        assert!(status.contains("50%"));
    }

    #[test]
    fn test_async_operation() {
        let op = AsyncOperation::new("Test Operation", 10);
        assert_eq!(op.name, "Test Operation");
        assert_eq!(op.progress.total(), 10);

        op.increment();
        assert_eq!(op.progress.current(), 1);

        op.set_progress(5);
        assert_eq!(op.progress.current(), 5);

        op.complete();
        assert!(op.progress.is_complete());
    }

    #[test]
    fn test_batch_processor() {
        let items: Vec<i32> = (1..=10).collect();
        let processor = BatchProcessor::new(items, 3);

        assert_eq!(processor.len(), 10);

        let batches: Vec<_> = processor.batches().collect();
        assert_eq!(batches.len(), 4); // 10 items / 3 per batch = 4 batches
        assert_eq!(batches[0].len(), 3);
        assert_eq!(batches[3].len(), 1); // last batch has 1 item
    }

    #[test]
    fn test_batch_processor_sync() {
        let items: Vec<i32> = (1..=5).collect();
        let processor = BatchProcessor::new(items, 2);

        let results: Result<Vec<_>, &str> = processor.process_sync(|x| Ok(x * 2));

        assert!(results.is_ok());
        assert_eq!(results.unwrap(), vec![2, 4, 6, 8, 10]);
        assert!(processor.progress.is_complete());
    }
}
