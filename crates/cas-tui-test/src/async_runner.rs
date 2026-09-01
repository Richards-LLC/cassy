//! Async PTY Runner - Tokio-compatible wrapper for PTY operations
//!
//! Provides async methods for PTY interaction with timeout and wait capabilities.
//! All methods are designed to work seamlessly with tokio runtime.
//!
//! # Example
//!
//! ```rust,no_run
//! use cas_tui_test::async_runner::AsyncPtyRunner;
//! use cas_tui_test::pty_runner::{PtyRunnerConfig, Key};
//! use std::time::Duration;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let mut runner = AsyncPtyRunner::new();
//!     runner.spawn("bash", &["-c", "echo hello"]).await?;
//!
//!     // Wait for text with timeout
//!     runner.wait_for_text("hello", Duration::from_secs(5)).await?;
//!
//!     // Send input
//!     runner.send_input("exit\n").await?;
//!
//!     Ok(())
//! }
//! ```

use crate::pty_runner::{Key, OutputBuffer, PtyRunnerConfig, PtyRunnerError};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::time::{Instant, interval};

/// Errors specific to async PTY operations
#[derive(Debug, Error)]
pub enum AsyncPtyError {
    #[error("PTY error: {0}")]
    Pty(#[from] PtyRunnerError),

    #[error("Timeout after {0:?} waiting for: {1}")]
    Timeout(Duration, String),

    #[error("Process exited unexpectedly")]
    ProcessExited,

    #[error("Invalid regex pattern: {0}")]
    InvalidRegex(String),

    #[error("Not spawned yet")]
    NotSpawned,

    #[error("PTY creation failed: {0}")]
    PtyCreation(String),

    #[error("Spawn failed: {0}")]
    SpawnFailed(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Shared state between the async runner and background reader
struct SharedState {
    output: OutputBuffer,
    running: bool,
}

/// Async PTY Runner - Tokio-compatible wrapper for PTY operations
///
/// This wrapper provides async methods for interacting with a pseudo-terminal,
/// including timeout-aware waiting and text matching capabilities.
///
/// Uses a background thread for reading PTY output, allowing wait operations
/// to timeout properly even when no data is available.
pub struct AsyncPtyRunner {
    config: PtyRunnerConfig,
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    state: Arc<Mutex<SharedState>>,
    reader_handle: Option<std::thread::JoinHandle<()>>,
    default_timeout: Duration,
    poll_interval: Duration,
    headful_fifo: Option<std::path::PathBuf>,
    headful_cleanup: bool,
}

impl AsyncPtyRunner {
    /// Create a new async PTY runner with default configuration
    pub fn new() -> Self {
        Self::with_config(PtyRunnerConfig::default())
    }

    /// Create a new async PTY runner with custom configuration
    pub fn with_config(config: PtyRunnerConfig) -> Self {
        Self {
            config,
            master: None,
            writer: None,
            child: None,
            state: Arc::new(Mutex::new(SharedState {
                output: OutputBuffer::default(),
                running: false,
            })),
            reader_handle: None,
            default_timeout: Duration::from_secs(30),
            poll_interval: Duration::from_millis(50),
            headful_fifo: None,
            headful_cleanup: false,
        }
    }

    /// Set the default timeout for wait operations
    pub fn timeout(mut self, duration: Duration) -> Self {
        self.default_timeout = duration;
        self
    }

    /// Set the poll interval for wait operations
    pub fn poll_interval(mut self, duration: Duration) -> Self {
        self.poll_interval = duration;
        self
    }

    /// Spawn a command in the PTY
    ///
    /// This starts a background thread that continuously reads PTY output.
    pub async fn spawn(&mut self, program: &str, args: &[&str]) -> Result<(), AsyncPtyError> {
        let pty_system = native_pty_system();

        let pair = pty_system
            .openpty(PtySize {
                rows: self.config.rows,
                cols: self.config.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| AsyncPtyError::PtyCreation(e.to_string()))?;

        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);

        // Set working directory if specified
        if let Some(ref cwd) = self.config.cwd {
            cmd.cwd(cwd);
        }

        // Handle environment
        if self.config.clear_env {
            cmd.env_clear();
        }
        for (key, value) in &self.config.env {
            cmd.env(key, value);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| AsyncPtyError::SpawnFailed(e.to_string()))?;

        // Get reader and writer from the master
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| AsyncPtyError::PtyCreation(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| AsyncPtyError::PtyCreation(e.to_string()))?;

        self.master = Some(pair.master);
        self.writer = Some(writer);
        self.child = Some(child);

        // Mark as running
        {
            let mut state = self.state.lock().unwrap();
            state.running = true;
        }

        // Start background reader thread
        let state = Arc::clone(&self.state);
        let max_output_bytes = self.config.max_output_bytes;
        let mut headful_path = None;
        let mut headful_cleanup = false;
        let headful = self
            .config
            .headful
            .as_ref()
            .and_then(|config| config.open_sink())
            .map(|(file, path, cleanup)| {
                headful_path = Some(path);
                headful_cleanup = cleanup;
                file
            });
        let handle = std::thread::spawn(move || {
            Self::reader_loop(reader, state, max_output_bytes, headful);
        });
        self.reader_handle = Some(handle);
        self.headful_fifo = headful_path;
        self.headful_cleanup = headful_cleanup;

        Ok(())
    }

    /// Background reader loop that continuously reads PTY output
    fn reader_loop(
        mut reader: Box<dyn Read + Send>,
        state: Arc<Mutex<SharedState>>,
        max_output_bytes: usize,
        headful: Option<std::sync::Arc<std::sync::Mutex<std::fs::File>>>,
    ) {
        let mut buf = [0u8; 4096];

        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    // EOF - process exited
                    let mut state = state.lock().unwrap();
                    state.running = false;
                    break;
                }
                Ok(n) => {
                    let mut state = state.lock().unwrap();
                    state.output.append_bounded(&buf[..n], max_output_bytes);
                    if let Some(ref file) = headful {
                        let mut file = file.lock().unwrap();
                        let _ = file.write(&buf[..n]);
                    }
                }
                Err(e) => {
                    if e.kind() != std::io::ErrorKind::WouldBlock {
                        // Real error - mark as not running
                        let mut state = state.lock().unwrap();
                        state.running = false;
                        break;
                    }
                }
            }
        }
    }

    /// Send string input to the PTY
    pub async fn send_input(&mut self, input: &str) -> Result<(), AsyncPtyError> {
        let writer = self.writer.as_mut().ok_or(AsyncPtyError::NotSpawned)?;
        writer.write_all(input.as_bytes())?;
        writer.flush()?;
        Ok(())
    }

    /// Send raw bytes to the PTY
    pub async fn send_bytes(&mut self, bytes: &[u8]) -> Result<(), AsyncPtyError> {
        let writer = self.writer.as_mut().ok_or(AsyncPtyError::NotSpawned)?;
        writer.write_all(bytes)?;
        writer.flush()?;
        Ok(())
    }

    /// Send a key press to the PTY
    pub async fn send_key(&mut self, key: Key) -> Result<(), AsyncPtyError> {
        self.send_bytes(key.as_bytes()).await
    }

    /// Get all captured output so far (non-blocking)
    pub fn get_output(&self) -> OutputBuffer {
        self.state.lock().unwrap().output.clone()
    }

    /// Clear the internal output buffer
    pub fn clear_output(&self) {
        self.state.lock().unwrap().output.clear();
    }

    pub fn with_output<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&OutputBuffer) -> R,
    {
        let state = self.state.lock().unwrap();
        f(&state.output)
    }

    /// Check if the process is still running
    pub fn is_running(&self) -> bool {
        // Check the reader thread state (updated when EOF or error occurs)
        self.state.lock().unwrap().running
    }

    /// Get the current terminal size
    pub fn size(&self) -> (u16, u16) {
        (self.config.cols, self.config.rows)
    }

    /// Resize the terminal
    pub async fn resize(&mut self, cols: u16, rows: u16) -> Result<(), AsyncPtyError> {
        if let Some(ref master) = self.master {
            master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| AsyncPtyError::PtyCreation(e.to_string()))?;
            self.config.cols = cols;
            self.config.rows = rows;
        }
        Ok(())
    }

    /// Kill the running process
    pub async fn kill(&mut self) -> Result<(), AsyncPtyError> {
        if let Some(ref mut child) = self.child {
            child.kill()?;
        }
        {
            let mut state = self.state.lock().unwrap();
            state.running = false;
        }
        Ok(())
    }

    // === Wait Methods ===

    /// Wait for a fixed duration
    pub async fn wait(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }

    /// Wait for a condition to be true, checking the output buffer
    ///
    /// Polls at the configured interval until the condition returns true
    /// or the timeout is reached.
    pub async fn wait_for<F>(
        &self,
        condition: F,
        wait_timeout: Duration,
    ) -> Result<(), AsyncPtyError>
    where
        F: Fn(&str) -> bool,
    {
        let deadline = Instant::now() + wait_timeout;
        let mut interval = interval(self.poll_interval);

        loop {
            // Check condition against accumulated output
            let found = self.with_output(|output| condition(output.as_str_lossy().as_ref()));
            if found {
                return Ok(());
            }

            // Check if process is still running
            if !self.is_running() {
                return Err(AsyncPtyError::ProcessExited);
            }

            // Check timeout
            if Instant::now() >= deadline {
                return Err(AsyncPtyError::Timeout(
                    wait_timeout,
                    "condition not met".to_string(),
                ));
            }

            interval.tick().await;
        }
    }

    /// Wait for text to appear in the output
    ///
    /// Returns when the specified text is found anywhere in the accumulated output.
    pub async fn wait_for_text(
        &self,
        text: &str,
        wait_timeout: Duration,
    ) -> Result<(), AsyncPtyError> {
        let text_for_closure = text.to_string();
        let text_for_error = text.to_string();
        self.wait_for(
            move |output| output.contains(&text_for_closure),
            wait_timeout,
        )
        .await
        .map_err(|e| match e {
            AsyncPtyError::Timeout(d, _) => {
                AsyncPtyError::Timeout(d, format!("text '{text_for_error}'"))
            }
            other => other,
        })
    }

    /// Wait for a regex pattern to match in the output
    ///
    /// Returns when the pattern matches anywhere in the accumulated output.
    pub async fn wait_for_regex(
        &self,
        pattern: &str,
        wait_timeout: Duration,
    ) -> Result<(), AsyncPtyError> {
        let regex =
            regex::Regex::new(pattern).map_err(|e| AsyncPtyError::InvalidRegex(e.to_string()))?;

        let pattern_for_error = pattern.to_string();
        self.wait_for(move |output| regex.is_match(output), wait_timeout)
            .await
            .map_err(|e| match e {
                AsyncPtyError::Timeout(d, _) => {
                    AsyncPtyError::Timeout(d, format!("regex '{pattern_for_error}'"))
                }
                other => other,
            })
    }

    /// Wait for the output to stabilize (no new output for the specified duration)
    ///
    /// Useful for waiting until a TUI has finished rendering.
    pub async fn wait_stable(&self, stable_duration: Duration) -> Result<(), AsyncPtyError> {
        let mut last_total = self.with_output(|output| output.total_bytes());
        let mut stable_since = Instant::now();
        let mut interval = interval(self.poll_interval);

        loop {
            let current_total = self.with_output(|output| output.total_bytes());
            if current_total != last_total {
                // Output changed, reset stability timer
                last_total = current_total;
                stable_since = Instant::now();
            } else if stable_since.elapsed() >= stable_duration {
                // Output stable for required duration
                return Ok(());
            }

            // Check if process is still running
            if !self.is_running() {
                // Process exited - consider output stable
                return Ok(());
            }

            interval.tick().await;
        }
    }

    /// Wait for text with the default timeout
    pub async fn wait_for_text_default(&self, text: &str) -> Result<(), AsyncPtyError> {
        self.wait_for_text(text, self.default_timeout).await
    }

    /// Wait for regex with the default timeout
    pub async fn wait_for_regex_default(&self, pattern: &str) -> Result<(), AsyncPtyError> {
        self.wait_for_regex(pattern, self.default_timeout).await
    }
}

impl Default for AsyncPtyRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for AsyncPtyRunner {
    fn drop(&mut self) {
        // Signal reader to stop and clean up
        {
            let mut state = self.state.lock().unwrap();
            state.running = false;
        }

        // Kill the child process if still running
        if let Some(ref mut child) = self.child {
            let _ = child.kill();
        }

        if self.headful_cleanup {
            if let Some(ref path) = self.headful_fifo {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::async_runner::*;

    #[tokio::test]
    async fn test_async_spawn_echo() {
        let mut runner = AsyncPtyRunner::new();
        runner
            .spawn("echo", &["hello"])
            .await
            .expect("spawn failed");

        // Wait for output
        runner
            .wait_for_text("hello", Duration::from_secs(5))
            .await
            .expect("wait failed");

        let output = runner.get_output();
        assert!(output.contains("hello"));
    }

    #[tokio::test]
    async fn test_async_send_input() {
        let mut runner = AsyncPtyRunner::new();
        runner.spawn("cat", &[]).await.expect("spawn failed");

        // Send input
        runner
            .send_input("test input\n")
            .await
            .expect("send failed");

        // Wait for echo
        runner
            .wait_for_text("test input", Duration::from_secs(5))
            .await
            .expect("wait failed");

        // Clean up
        runner.send_key(Key::CtrlD).await.expect("send key failed");
    }

    #[tokio::test]
    async fn test_async_wait_for_regex() {
        let mut runner = AsyncPtyRunner::new();
        runner
            .spawn("sh", &["-c", "echo 'count: 42'"])
            .await
            .expect("spawn failed");

        runner
            .wait_for_regex(r"count: \d+", Duration::from_secs(5))
            .await
            .expect("wait failed");
    }

    #[tokio::test]
    async fn test_async_timeout() {
        let mut runner = AsyncPtyRunner::new();
        runner.spawn("cat", &[]).await.expect("spawn failed");

        // This should timeout since we never send the expected text
        let result = runner
            .wait_for_text("never appears", Duration::from_millis(200))
            .await;

        assert!(matches!(result, Err(AsyncPtyError::Timeout(_, _))));

        // Clean up
        let _ = runner.kill().await;
    }

    #[tokio::test]
    async fn test_async_wait_stable() {
        let mut runner = AsyncPtyRunner::new();
        runner
            .spawn("echo", &["quick output"])
            .await
            .expect("spawn failed");

        // Wait for output to stabilize
        runner
            .wait_stable(Duration::from_millis(200))
            .await
            .expect("wait stable failed");

        let output = runner.get_output();
        assert!(output.contains("quick output"));
    }

    #[tokio::test]
    async fn test_async_custom_config() {
        let config = PtyRunnerConfig::with_size(120, 40).env("TEST_VAR", "async_test");

        let mut runner = AsyncPtyRunner::with_config(config);
        runner
            .spawn("sh", &["-c", "echo $TEST_VAR"])
            .await
            .expect("spawn failed");

        runner
            .wait_for_text("async_test", Duration::from_secs(5))
            .await
            .expect("wait failed");
    }

    #[tokio::test]
    async fn test_async_wait_for_condition() {
        let mut runner = AsyncPtyRunner::new();
        runner
            .spawn("sh", &["-c", "echo 'line1'; sleep 0.1; echo 'line2'"])
            .await
            .expect("spawn failed");

        // Wait for both lines
        runner
            .wait_for(
                |output| output.contains("line1") && output.contains("line2"),
                Duration::from_secs(5),
            )
            .await
            .expect("wait failed");
    }

    #[tokio::test]
    async fn test_async_resize() {
        let mut runner = AsyncPtyRunner::new();
        runner.spawn("cat", &[]).await.expect("spawn failed");

        assert_eq!(runner.size(), (80, 24));

        runner.resize(120, 40).await.expect("resize failed");

        assert_eq!(runner.size(), (120, 40));

        let _ = runner.kill().await;
    }

    #[tokio::test]
    async fn test_async_process_exit_detection() {
        let mut runner = AsyncPtyRunner::new();
        runner
            .spawn("sh", &["-c", "exit 0"])
            .await
            .expect("spawn failed");

        // Now waiting for text should fail with ProcessExited
        let result = runner
            .wait_for_text("will not appear", Duration::from_secs(1))
            .await;

        assert!(matches!(result, Err(AsyncPtyError::ProcessExited)));
    }
}
