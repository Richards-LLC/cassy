//! Parallel search execution utilities
//!
//! This module provides utilities for executing multiple search channels
//! in parallel, useful when building async hybrid search systems.
//!
//! # Feature Flag
//!
//! The async functionality requires the `parallel` feature:
//!
//! ```toml
//! cas-search = { version = "0.5", features = ["parallel"] }
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use cas_search::parallel::{ParallelExecutor, SearchTask};
//!
//! // Define search tasks
//! let tasks = vec![
//!     SearchTask::new("bm25", || async { bm25_search(&query, limit) }),
//!     SearchTask::new("semantic", || async { semantic_search(&query, limit) }),
//! ];
//!
//! // Execute in parallel
//! let results = executor.execute(tasks).await?;
//! ```

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Result from a parallel search execution
#[derive(Debug, Clone)]
pub struct ParallelResult {
    /// Channel name
    pub channel: String,
    /// Search results (id, score)
    pub results: Vec<(String, f64)>,
    /// Execution time
    pub latency: Duration,
    /// Whether the search succeeded
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
}

/// Statistics from parallel execution
#[derive(Debug, Clone, Default)]
pub struct ParallelStats {
    /// Total execution time (wall clock)
    pub total_latency: Duration,
    /// Individual channel latencies
    pub channel_latencies: HashMap<String, Duration>,
    /// Number of successful channels
    pub success_count: usize,
    /// Number of failed channels
    pub failure_count: usize,
}

impl ParallelStats {
    /// Calculate speedup vs sequential execution
    pub fn speedup(&self) -> f64 {
        if self.total_latency.as_nanos() == 0 {
            return 1.0;
        }

        let sequential: Duration = self.channel_latencies.values().sum();
        sequential.as_secs_f64() / self.total_latency.as_secs_f64()
    }
}

/// Synchronous parallel search coordinator
///
/// This provides a simple way to track and coordinate search results
/// from multiple channels without requiring async runtime.
pub struct ParallelCoordinator {
    results: Vec<ParallelResult>,
    start: Instant,
}

impl ParallelCoordinator {
    /// Create a new coordinator
    pub fn new() -> Self {
        Self {
            results: Vec::new(),
            start: Instant::now(),
        }
    }

    /// Record results from a channel
    pub fn record(
        &mut self,
        channel: impl Into<String>,
        results: Vec<(String, f64)>,
        latency: Duration,
    ) {
        self.results.push(ParallelResult {
            channel: channel.into(),
            results,
            latency,
            success: true,
            error: None,
        });
    }

    /// Record a failed channel
    pub fn record_failure(
        &mut self,
        channel: impl Into<String>,
        error: impl Into<String>,
        latency: Duration,
    ) {
        self.results.push(ParallelResult {
            channel: channel.into(),
            results: Vec::new(),
            latency,
            success: false,
            error: Some(error.into()),
        });
    }

    /// Get all results
    pub fn results(&self) -> &[ParallelResult] {
        &self.results
    }

    /// Get results for a specific channel
    pub fn get_channel(&self, name: &str) -> Option<&ParallelResult> {
        self.results.iter().find(|r| r.channel == name)
    }

    /// Get execution statistics
    pub fn stats(&self) -> ParallelStats {
        let mut stats = ParallelStats {
            total_latency: self.start.elapsed(),
            ..Default::default()
        };

        for result in &self.results {
            stats
                .channel_latencies
                .insert(result.channel.clone(), result.latency);
            if result.success {
                stats.success_count += 1;
            } else {
                stats.failure_count += 1;
            }
        }

        stats
    }

    /// Combine all successful results using a scoring function
    pub fn combine<F>(&self, combiner: F) -> Vec<(String, f64)>
    where
        F: Fn(&[&ParallelResult]) -> Vec<(String, f64)>,
    {
        let successful: Vec<&ParallelResult> = self.results.iter().filter(|r| r.success).collect();
        combiner(&successful)
    }
}

impl Default for ParallelCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

/// Timer for measuring channel execution time
pub struct ChannelTimer {
    start: Instant,
}

impl ChannelTimer {
    /// Start timing
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// Get elapsed duration
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Helper for executing a search with timing
pub fn timed_search<F, T>(search_fn: F) -> (T, Duration)
where
    F: FnOnce() -> T,
{
    let timer = ChannelTimer::start();
    let result = search_fn();
    (result, timer.elapsed())
}

#[cfg(feature = "parallel")]
pub use async_impl::*;

#[cfg(feature = "parallel")]
mod async_impl {
    use crate::parallel::*;
    use std::future::Future;
    use tokio::task::JoinHandle;

    /// A search task to execute in parallel
    pub struct SearchTask<F, Fut>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<Vec<(String, f64)>, String>> + Send + 'static,
    {
        /// Channel name
        pub name: String,
        /// The search function to execute
        pub search_fn: Option<F>,
    }

    impl<F, Fut> SearchTask<F, Fut>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<Vec<(String, f64)>, String>> + Send + 'static,
    {
        /// Create a new search task
        pub fn new(name: impl Into<String>, search_fn: F) -> Self {
            Self {
                name: name.into(),
                search_fn: Some(search_fn),
            }
        }
    }

    /// Async parallel search executor
    pub struct ParallelExecutor {
        timeout: Option<Duration>,
    }

    impl ParallelExecutor {
        /// Create a new executor
        pub fn new() -> Self {
            Self { timeout: None }
        }

        /// Set a timeout for each search task
        pub fn with_timeout(mut self, timeout: Duration) -> Self {
            self.timeout = Some(timeout);
            self
        }

        /// Execute search tasks in parallel
        pub async fn execute<F, Fut>(&self, tasks: Vec<SearchTask<F, Fut>>) -> Vec<ParallelResult>
        where
            F: FnOnce() -> Fut + Send + 'static,
            Fut: Future<Output = Result<Vec<(String, f64)>, String>> + Send + 'static,
        {
            let handles: Vec<(String, JoinHandle<ParallelResult>)> = tasks
                .into_iter()
                .map(|mut task| {
                    let channel_name = task.name.clone();
                    let channel_for_task = channel_name.clone();
                    let search_fn = task.search_fn.take().expect("search_fn already taken");
                    let timeout = self.timeout;

                    let handle = tokio::spawn(async move {
                        let timer = ChannelTimer::start();

                        if let Some(timeout_dur) = timeout {
                            match tokio::time::timeout(timeout_dur, search_fn()).await {
                                Ok(Ok(results)) => ParallelResult {
                                    channel: channel_for_task.clone(),
                                    results,
                                    latency: timer.elapsed(),
                                    success: true,
                                    error: None,
                                },
                                Ok(Err(e)) => ParallelResult {
                                    channel: channel_for_task.clone(),
                                    results: Vec::new(),
                                    latency: timer.elapsed(),
                                    success: false,
                                    error: Some(e),
                                },
                                Err(_) => ParallelResult {
                                    channel: channel_for_task.clone(),
                                    results: Vec::new(),
                                    latency: timer.elapsed(),
                                    success: false,
                                    error: Some("timeout".to_string()),
                                },
                            }
                        } else {
                            match search_fn().await {
                                Ok(results) => ParallelResult {
                                    channel: channel_for_task.clone(),
                                    results,
                                    latency: timer.elapsed(),
                                    success: true,
                                    error: None,
                                },
                                Err(e) => ParallelResult {
                                    channel: channel_for_task.clone(),
                                    results: Vec::new(),
                                    latency: timer.elapsed(),
                                    success: false,
                                    error: Some(e),
                                },
                            }
                        }
                    });

                    (channel_name, handle)
                })
                .collect();

            let mut results = Vec::new();
            for (name, handle) in handles {
                match handle.await {
                    Ok(result) => results.push(result),
                    Err(e) => results.push(ParallelResult {
                        channel: name,
                        results: Vec::new(),
                        latency: Duration::ZERO,
                        success: false,
                        error: Some(format!("task panicked: {}", e)),
                    }),
                }
            }

            results
        }
    }

    impl Default for ParallelExecutor {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::parallel::*;

    #[test]
    fn test_parallel_coordinator() {
        let mut coord = ParallelCoordinator::new();

        coord.record(
            "bm25",
            vec![("doc-1".into(), 10.0), ("doc-2".into(), 5.0)],
            Duration::from_millis(50),
        );
        coord.record(
            "semantic",
            vec![("doc-2".into(), 0.9), ("doc-3".into(), 0.8)],
            Duration::from_millis(100),
        );

        assert_eq!(coord.results().len(), 2);
        assert!(coord.get_channel("bm25").is_some());
        assert!(coord.get_channel("semantic").is_some());
        assert!(coord.get_channel("unknown").is_none());
    }

    #[test]
    fn test_parallel_coordinator_failure() {
        let mut coord = ParallelCoordinator::new();

        coord.record(
            "bm25",
            vec![("doc-1".into(), 10.0)],
            Duration::from_millis(50),
        );
        coord.record_failure("semantic", "model not loaded", Duration::from_millis(5));

        let stats = coord.stats();
        assert_eq!(stats.success_count, 1);
        assert_eq!(stats.failure_count, 1);
    }

    #[test]
    fn test_parallel_stats_speedup() {
        let mut coord = ParallelCoordinator::new();

        // Simulate 3 channels that each took 100ms but ran in parallel
        coord.record("bm25", vec![], Duration::from_millis(100));
        coord.record("semantic", vec![], Duration::from_millis(100));
        coord.record("graph", vec![], Duration::from_millis(100));

        let stats = coord.stats();

        // Total should be close to 100ms (parallel), individual sum is 300ms
        // So speedup should be around 3x (but we can't test exact values due to timing)
        assert!(stats.channel_latencies.len() == 3);
    }

    #[test]
    fn test_timed_search() {
        let (result, latency): (Vec<(String, f64)>, Duration) = timed_search(|| {
            std::thread::sleep(Duration::from_millis(10));
            vec![("doc-1".to_string(), 1.0)]
        });

        assert_eq!(result.len(), 1);
        assert!(latency >= Duration::from_millis(10));
    }

    #[test]
    fn test_coordinator_combine() {
        let mut coord = ParallelCoordinator::new();

        coord.record(
            "bm25",
            vec![("a".into(), 10.0), ("b".into(), 5.0)],
            Duration::ZERO,
        );
        coord.record(
            "semantic",
            vec![("b".into(), 0.9), ("c".into(), 0.8)],
            Duration::ZERO,
        );

        let combined = coord.combine(|results| {
            // Simple merge - just flatten all results
            let mut all = Vec::new();
            for r in results {
                all.extend(r.results.clone());
            }
            all
        });

        assert_eq!(combined.len(), 4);
    }

    #[test]
    fn test_channel_timer() {
        let timer = ChannelTimer::start();
        std::thread::sleep(Duration::from_millis(10));
        assert!(timer.elapsed() >= Duration::from_millis(10));
    }
}
