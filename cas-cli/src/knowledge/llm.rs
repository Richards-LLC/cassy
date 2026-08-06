//! LLM invocation for distillation (EPIC cas-7d31 / cas-c9be).
//!
//! The pipeline only ever sees [`LlmRunner`], so the whole distillation path is
//! testable without spending a token: [`ScriptedLlm`] replays canned responses
//! and counts calls, which is how "an unchanged repo costs zero LLM calls" is
//! asserted rather than assumed.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Why a distillation call could not produce text.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// The provider binary is missing or not runnable.
    #[error("llm runner unavailable: {0}")]
    Unavailable(String),
    /// The provider ran but failed, timed out, or returned nothing usable.
    #[error("llm call failed: {0}")]
    Failed(String),
}

/// A one-shot text completion provider.
pub trait LlmRunner: Send + Sync {
    /// Run `prompt` and return the raw response text.
    fn complete(&self, prompt: &str) -> Result<String, LlmError>;

    /// How many completions this runner has performed. Used by the pipeline
    /// report (and by tests asserting the zero-call short-circuit).
    fn calls(&self) -> usize;

    /// Human-readable provider name for reports.
    fn label(&self) -> String {
        "llm".to_string()
    }
}

/// Default wall-clock budget for one distillation call.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);

/// Headless `claude -p` runner.
///
/// stdout/stderr are captured to temp files rather than pipes: the process is
/// polled to enforce the deadline, and a full pipe buffer would otherwise wedge
/// the child while we wait on it.
pub struct ClaudeCliRunner {
    binary: String,
    model: Option<String>,
    timeout: Duration,
    calls: AtomicUsize,
}

impl ClaudeCliRunner {
    pub fn new(model: Option<String>) -> Self {
        Self {
            binary: std::env::var("CAS_KNOWLEDGE_LLM_BIN").unwrap_or_else(|_| "claude".to_string()),
            model,
            timeout: DEFAULT_TIMEOUT,
            calls: AtomicUsize::new(0),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    fn capture_path(tag: &str) -> PathBuf {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "cas-knowledge-{}-{}-{tag}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }
}

impl LlmRunner for ClaudeCliRunner {
    fn complete(&self, prompt: &str) -> Result<String, LlmError> {
        let stdout_path = Self::capture_path("out");
        let stderr_path = Self::capture_path("err");
        let stdout_file = std::fs::File::create(&stdout_path)
            .map_err(|error| LlmError::Failed(format!("capture stdout: {error}")))?;
        let stderr_file = std::fs::File::create(&stderr_path)
            .map_err(|error| LlmError::Failed(format!("capture stderr: {error}")))?;

        let mut command = Command::new(&self.binary);
        command
            .arg("-p")
            .arg("--output-format")
            .arg("text")
            .stdin(Stdio::piped())
            .stdout(Stdio::from(stdout_file))
            .stderr(Stdio::from(stderr_file));
        if let Some(model) = &self.model {
            command.arg("--model").arg(model);
        }

        let mut child = command.spawn().map_err(|error| {
            let _ = std::fs::remove_file(&stdout_path);
            let _ = std::fs::remove_file(&stderr_path);
            LlmError::Unavailable(format!("{}: {error}", self.binary))
        })?;
        self.calls.fetch_add(1, Ordering::Relaxed);

        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(prompt.as_bytes());
        }
        drop(child.stdin.take());

        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => {
                    let _ = std::fs::remove_file(&stdout_path);
                    let _ = std::fs::remove_file(&stderr_path);
                    return Err(LlmError::Failed(format!("wait failed: {error}")));
                }
            }
        };

        let stdout = std::fs::read_to_string(&stdout_path).unwrap_or_default();
        let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
        let _ = std::fs::remove_file(&stdout_path);
        let _ = std::fs::remove_file(&stderr_path);

        let Some(status) = status else {
            return Err(LlmError::Failed(format!(
                "timed out after {}s",
                self.timeout.as_secs()
            )));
        };
        if !status.success() {
            let detail = stderr.trim().chars().take(400).collect::<String>();
            return Err(LlmError::Failed(format!(
                "{} exited with {status}: {detail}",
                self.binary
            )));
        }
        if stdout.trim().is_empty() {
            return Err(LlmError::Failed("empty response".to_string()));
        }
        Ok(stdout)
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }

    fn label(&self) -> String {
        match &self.model {
            Some(model) => format!("{} ({model})", self.binary),
            None => self.binary.clone(),
        }
    }
}

/// Test/dry-run runner: replays scripted responses in order and records the
/// prompts it saw. Exposed (not `#[cfg(test)]`) so integration tests and
/// `cas knowledge build --dry-run` can drive the real pipeline offline.
pub struct ScriptedLlm {
    responses: Mutex<std::collections::VecDeque<String>>,
    prompts: Mutex<Vec<String>>,
    /// Reply used once the script runs out. `None` → error instead.
    fallback: Option<String>,
    calls: AtomicUsize,
}

impl ScriptedLlm {
    /// Replay `responses` in order; error once exhausted.
    pub fn new(responses: impl IntoIterator<Item = String>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            prompts: Mutex::new(Vec::new()),
            fallback: None,
            calls: AtomicUsize::new(0),
        }
    }

    /// Replay `responses`, then answer everything else with `fallback`.
    pub fn with_fallback(
        responses: impl IntoIterator<Item = String>,
        fallback: impl Into<String>,
    ) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            prompts: Mutex::new(Vec::new()),
            fallback: Some(fallback.into()),
            calls: AtomicUsize::new(0),
        }
    }

    /// Answer every call with the same response.
    pub fn always(response: impl Into<String>) -> Self {
        Self::with_fallback(Vec::new(), response)
    }

    /// Every prompt this runner has been handed, in order.
    pub fn prompts(&self) -> Vec<String> {
        self.prompts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

impl LlmRunner for ScriptedLlm {
    fn complete(&self, prompt: &str) -> Result<String, LlmError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.prompts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(prompt.to_string());
        let next = self
            .responses
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop_front();
        match next.or_else(|| self.fallback.clone()) {
            Some(response) => Ok(response),
            None => Err(LlmError::Failed("scripted runner exhausted".to_string())),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }

    fn label(&self) -> String {
        "scripted".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_runner_replays_in_order_and_counts_calls() {
        let runner = ScriptedLlm::new(vec!["one".to_string(), "two".to_string()]);
        assert_eq!(runner.calls(), 0);
        assert_eq!(runner.complete("a").unwrap(), "one");
        assert_eq!(runner.complete("b").unwrap(), "two");
        assert_eq!(runner.calls(), 2);
        assert!(runner.complete("c").is_err());
        assert_eq!(runner.prompts(), vec!["a", "b", "c"]);
    }

    #[test]
    fn fallback_answers_after_the_script_runs_out() {
        let runner = ScriptedLlm::with_fallback(vec!["first".to_string()], "rest");
        assert_eq!(runner.complete("x").unwrap(), "first");
        assert_eq!(runner.complete("y").unwrap(), "rest");
        assert_eq!(runner.complete("z").unwrap(), "rest");
        assert_eq!(runner.calls(), 3);
    }

    #[test]
    fn missing_binary_is_reported_as_unavailable_not_a_panic() {
        let runner = ClaudeCliRunner {
            binary: "cas-no-such-llm-binary".to_string(),
            model: None,
            timeout: Duration::from_secs(1),
            calls: AtomicUsize::new(0),
        };
        match runner.complete("hi") {
            Err(LlmError::Unavailable(_)) => {}
            other => panic!("expected Unavailable, got {other:?}"),
        }
        assert_eq!(runner.calls(), 0, "a failed spawn is not a billed call");
    }
}
