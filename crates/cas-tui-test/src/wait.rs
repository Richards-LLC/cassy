//! Wait DSL - Methods for waiting on terminal output conditions
//!
//! Provides configurable wait methods with timeouts for:
//! - Text appearance
//! - Regex pattern matching
//! - Output stability (no changes for a duration)

use crate::pty_runner::{PtyRunner, PtyRunnerError};
use regex::Regex;
use std::time::{Duration, Instant};
use thiserror::Error;

/// Default timeout for wait operations
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Default poll interval for wait operations
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Default stability duration for wait_stable
pub const DEFAULT_STABLE_DURATION: Duration = Duration::from_millis(200);

/// Errors that can occur during wait operations
#[derive(Debug, Error)]
pub enum WaitError {
    #[error("Timeout waiting for text '{0}' after {1:?}")]
    TextTimeout(String, Duration),

    #[error("Timeout waiting for pattern '{0}' after {1:?}")]
    PatternTimeout(String, Duration),

    #[error("Timeout waiting for stable output after {0:?}")]
    StableTimeout(Duration),

    #[error("Invalid regex pattern: {0}")]
    InvalidPattern(#[from] regex::Error),

    #[error("PTY error: {0}")]
    PtyError(#[from] PtyRunnerError),
}

/// Configuration for wait operations
#[derive(Debug, Clone)]
pub struct WaitConfig {
    /// Maximum time to wait
    pub timeout: Duration,
    /// How often to poll for changes
    pub poll_interval: Duration,
    /// For wait_stable: how long output must be unchanged
    pub stable_duration: Duration,
}

impl Default for WaitConfig {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            poll_interval: DEFAULT_POLL_INTERVAL,
            stable_duration: DEFAULT_STABLE_DURATION,
        }
    }
}

impl WaitConfig {
    /// Create config with custom timeout
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout,
            ..Default::default()
        }
    }

    /// Create config with timeout in milliseconds
    pub fn with_timeout_ms(ms: u64) -> Self {
        Self::with_timeout(Duration::from_millis(ms))
    }

    /// Set poll interval
    pub fn poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Set stable duration for wait_stable
    pub fn stable_duration(mut self, duration: Duration) -> Self {
        self.stable_duration = duration;
        self
    }
}

/// Wait for text to appear in output
pub fn wait_for_text(
    runner: &mut PtyRunner,
    text: &str,
    config: &WaitConfig,
) -> Result<String, WaitError> {
    let start = Instant::now();

    while start.elapsed() < config.timeout {
        let maybe_match = runner.with_output(|output| output.as_str_lossy().contains(text));
        if maybe_match {
            return Ok(runner.with_output(|output| output.as_str()));
        }

        std::thread::sleep(config.poll_interval);
    }

    Err(WaitError::TextTimeout(text.to_string(), config.timeout))
}

/// Wait for a regex pattern to match in output
pub fn wait_for_regex(
    runner: &mut PtyRunner,
    pattern: &str,
    config: &WaitConfig,
) -> Result<String, WaitError> {
    let regex = Regex::new(pattern)?;
    let start = Instant::now();

    while start.elapsed() < config.timeout {
        let maybe_match =
            runner.with_output(|output| regex.is_match(output.as_str_lossy().as_ref()));
        if maybe_match {
            return Ok(runner.with_output(|output| output.as_str()));
        }

        std::thread::sleep(config.poll_interval);
    }

    Err(WaitError::PatternTimeout(
        pattern.to_string(),
        config.timeout,
    ))
}

/// Wait for output to stabilize (no changes for stable_duration)
pub fn wait_stable(runner: &mut PtyRunner, config: &WaitConfig) -> Result<String, WaitError> {
    let start = Instant::now();
    let mut last_total = runner.with_output(|output| output.total_bytes());
    let mut stable_since = Instant::now();

    while start.elapsed() < config.timeout {
        let current_total = runner.with_output(|output| output.total_bytes());
        if current_total != last_total {
            last_total = current_total;
            stable_since = Instant::now();
        } else if stable_since.elapsed() >= config.stable_duration {
            return Ok(runner.with_output(|output| output.as_str()));
        }

        std::thread::sleep(config.poll_interval);
    }

    Err(WaitError::StableTimeout(config.timeout))
}

/// Extension trait for PtyRunner to add wait methods
pub trait WaitExt {
    /// Wait for text with default config
    fn wait_for_text(&mut self, text: &str) -> Result<String, WaitError>;

    /// Wait for text with custom timeout
    fn wait_for_text_timeout(&mut self, text: &str, timeout: Duration)
    -> Result<String, WaitError>;

    /// Wait for regex pattern with default config
    fn wait_for_regex(&mut self, pattern: &str) -> Result<String, WaitError>;

    /// Wait for regex pattern with custom timeout
    fn wait_for_regex_timeout(
        &mut self,
        pattern: &str,
        timeout: Duration,
    ) -> Result<String, WaitError>;

    /// Wait for output to stabilize with default config
    fn wait_stable(&mut self) -> Result<String, WaitError>;

    /// Wait for output to stabilize with custom config
    fn wait_stable_config(&mut self, config: &WaitConfig) -> Result<String, WaitError>;
}

impl WaitExt for PtyRunner {
    fn wait_for_text(&mut self, text: &str) -> Result<String, WaitError> {
        wait_for_text(self, text, &WaitConfig::default())
    }

    fn wait_for_text_timeout(
        &mut self,
        text: &str,
        timeout: Duration,
    ) -> Result<String, WaitError> {
        wait_for_text(self, text, &WaitConfig::with_timeout(timeout))
    }

    fn wait_for_regex(&mut self, pattern: &str) -> Result<String, WaitError> {
        wait_for_regex(self, pattern, &WaitConfig::default())
    }

    fn wait_for_regex_timeout(
        &mut self,
        pattern: &str,
        timeout: Duration,
    ) -> Result<String, WaitError> {
        wait_for_regex(self, pattern, &WaitConfig::with_timeout(timeout))
    }

    fn wait_stable(&mut self) -> Result<String, WaitError> {
        wait_stable(self, &WaitConfig::default())
    }

    fn wait_stable_config(&mut self, config: &WaitConfig) -> Result<String, WaitError> {
        wait_stable(self, config)
    }
}

#[cfg(test)]
mod tests {
    use crate::wait::*;

    #[test]
    fn test_wait_config_default() {
        let config = WaitConfig::default();
        assert_eq!(config.timeout, DEFAULT_TIMEOUT);
        assert_eq!(config.poll_interval, DEFAULT_POLL_INTERVAL);
        assert_eq!(config.stable_duration, DEFAULT_STABLE_DURATION);
    }

    #[test]
    fn test_wait_config_builder() {
        let config = WaitConfig::with_timeout_ms(1000)
            .poll_interval(Duration::from_millis(10))
            .stable_duration(Duration::from_millis(100));

        assert_eq!(config.timeout, Duration::from_millis(1000));
        assert_eq!(config.poll_interval, Duration::from_millis(10));
        assert_eq!(config.stable_duration, Duration::from_millis(100));
    }

    #[test]
    fn test_wait_for_text_success() {
        let mut runner = PtyRunner::new();
        runner
            .spawn("echo", &["hello world"])
            .expect("spawn failed");

        let config = WaitConfig::with_timeout_ms(1000);
        let result = wait_for_text(&mut runner, "hello", &config);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("hello"));
    }

    #[test]
    fn test_wait_for_text_timeout() {
        let mut runner = PtyRunner::new();
        runner.spawn("echo", &["hello"]).expect("spawn failed");

        let config = WaitConfig::with_timeout_ms(100);
        let result = wait_for_text(&mut runner, "nonexistent", &config);
        assert!(matches!(result, Err(WaitError::TextTimeout(_, _))));
    }

    #[test]
    fn test_wait_for_regex_success() {
        let mut runner = PtyRunner::new();
        runner.spawn("echo", &["value: 42"]).expect("spawn failed");

        let config = WaitConfig::with_timeout_ms(1000);
        let result = wait_for_regex(&mut runner, r"value:\s+\d+", &config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_wait_for_regex_invalid() {
        let mut runner = PtyRunner::new();
        runner.spawn("echo", &["test"]).expect("spawn failed");

        let config = WaitConfig::with_timeout_ms(100);
        let result = wait_for_regex(&mut runner, r"[invalid", &config);
        assert!(matches!(result, Err(WaitError::InvalidPattern(_))));
    }

    #[test]
    fn test_wait_stable() {
        let mut runner = PtyRunner::new();
        runner
            .spawn("sh", &["-c", "echo 'stable output'; read -r release"])
            .expect("spawn failed");

        runner.wait_for_text("stable output").expect("output ready");
        let config = WaitConfig::with_timeout_ms(1000).stable_duration(Duration::from_millis(100));
        let output = wait_stable(&mut runner, &config).expect("stable output");
        assert!(output.contains("stable output"));
        assert!(runner.is_running(), "stability must not depend on child exit");
    }

    #[test]
    fn test_wait_ext_trait() {
        let mut runner = PtyRunner::new();
        runner
            .spawn("echo", &["test output"])
            .expect("spawn failed");

        let result = runner.wait_for_text_timeout("test", Duration::from_millis(1000));
        assert!(result.is_ok());
    }
}
