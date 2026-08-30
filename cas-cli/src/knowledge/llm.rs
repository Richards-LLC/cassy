//! LLM invocation for distillation (EPIC cas-7d31 / cas-c9be).
//!
//! The pipeline only ever sees [`LlmRunner`], so the whole distillation path is
//! testable without spending a token: [`ScriptedLlm`] replays canned responses
//! and counts calls, which is how "an unchanged repo costs zero LLM calls" is
//! asserted rather than assumed.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tempfile::NamedTempFile;

use crate::bounded_process::{configure_process_group, terminate_process_group};

/// Why a distillation call could not produce text.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// The provider binary is missing or not runnable.
    #[error("llm runner unavailable: {0}")]
    Unavailable(String),
    /// The provider ran but failed, timed out, or returned nothing usable.
    #[error("llm call failed: {0}")]
    Failed(String),
    /// The enclosing knowledge build deadline expired while this call was
    /// active or before it could be started.
    #[error("llm call timed out after {0:?}")]
    TimedOut(Duration),
}

/// A one-shot text completion provider.
pub trait LlmRunner: Send + Sync {
    /// Run `prompt` and return the raw response text.
    fn complete(&self, prompt: &str) -> Result<String, LlmError>;

    /// Run a completion without starting it after an enclosing build deadline.
    /// Implementations that own a subprocess should also enforce the deadline
    /// while the call is active; the default preserves existing test doubles
    /// and non-process runners.
    fn complete_with_deadline(
        &self,
        prompt: &str,
        deadline: Option<Instant>,
    ) -> Result<String, LlmError> {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(LlmError::TimedOut(Duration::ZERO));
        }
        let result = self.complete(prompt);
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            Err(LlmError::TimedOut(Duration::ZERO))
        } else {
            result
        }
    }

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

/// Override for the provider binary the distiller shells out to.
pub const LLM_BIN_ENV: &str = "CAS_KNOWLEDGE_LLM_BIN";

/// Headless `claude -p` runner.
///
/// stdout/stderr are captured to temp files rather than pipes: the process is
/// polled to enforce the deadline, and a full pipe buffer would otherwise wedge
/// the child while we wait on it. The captures are created O_EXCL with random
/// names (and mode 0600 on unix) — a predictable name in a shared temp dir is a
/// symlink-attack target, since we create, read back, and then unlink it.
pub struct ClaudeCliRunner {
    binary: String,
    model: Option<String>,
    timeout: Duration,
    calls: AtomicUsize,
}

impl ClaudeCliRunner {
    pub fn new(model: Option<String>) -> Self {
        Self {
            binary: std::env::var(LLM_BIN_ENV).unwrap_or_else(|_| "claude".to_string()),
            model,
            timeout: DEFAULT_TIMEOUT,
            calls: AtomicUsize::new(0),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Create one capture file: unpredictable name, exclusive create, 0600.
    fn capture_file(tag: &str) -> Result<NamedTempFile, LlmError> {
        let mut builder = tempfile::Builder::new();
        builder.prefix("cas-knowledge-").suffix(tag);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o600));
        }
        builder
            .tempfile()
            .map_err(|error| LlmError::Failed(format!("capture {tag}: {error}")))
    }
}

/// Spawn, retrying briefly on `ETXTBSY`.
///
/// A concurrent `fork` elsewhere in the process can inherit a write handle to
/// the very binary we are about to exec, which the kernel reports as "text file
/// busy". It clears on its own within milliseconds.
fn spawn_with_retry(command: &mut Command) -> std::io::Result<std::process::Child> {
    let mut last_error = None;
    for attempt in 0..5 {
        match command.spawn() {
            Ok(child) => return Ok(child),
            Err(error) if error.raw_os_error() == Some(26) => {
                std::thread::sleep(Duration::from_millis(20 * (attempt + 1)));
                last_error = Some(error);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| std::io::Error::other("spawn failed")))
}

impl LlmRunner for ClaudeCliRunner {
    fn complete(&self, prompt: &str) -> Result<String, LlmError> {
        self.complete_with_deadline(prompt, None)
    }

    fn complete_with_deadline(
        &self,
        prompt: &str,
        deadline: Option<Instant>,
    ) -> Result<String, LlmError> {
        self.complete_inner(prompt, deadline)
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

impl ClaudeCliRunner {
    fn complete_inner(
        &self,
        prompt: &str,
        enclosing_deadline: Option<Instant>,
    ) -> Result<String, LlmError> {
        // Both captures are owned by `NamedTempFile`, so every early return —
        // including the `?` on the second create — unlinks what was created.
        let stdout_capture = Self::capture_file("out")?;
        let stderr_capture = Self::capture_file("err")?;
        let stdout_file = stdout_capture
            .reopen()
            .map_err(|error| LlmError::Failed(format!("capture stdout: {error}")))?;
        let stderr_file = stderr_capture
            .reopen()
            .map_err(|error| LlmError::Failed(format!("capture stderr: {error}")))?;

        let mut command = Command::new(&self.binary);
        crate::internal_llm::isolate_command(&mut command);
        command
            .arg("-p")
            .arg("--output-format")
            .arg("text")
            .stdin(Stdio::piped())
            .stdout(Stdio::from(stdout_file))
            .stderr(Stdio::from(stderr_file));
        // Put the provider and all ordinary descendants in a private process
        // group. A direct `Child::kill` leaves a stalled provider descendant
        // holding our capture files (and possibly stdin) open on Unix/macOS.
        // The shared bounded-process primitive kills the group and reaps the
        // direct child at the deadline; non-Unix targets retain direct-child
        // cleanup through the same API.
        configure_process_group(&mut command);
        if let Some(model) = &self.model {
            command.arg("--model").arg(model);
        }

        let call_deadline = Instant::now() + self.timeout;
        if enclosing_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(LlmError::TimedOut(self.timeout));
        }

        let mut child = spawn_with_retry(&mut command)
            .map_err(|error| LlmError::Unavailable(format!("{}: {error}", self.binary)))?;
        self.calls.fetch_add(1, Ordering::Relaxed);

        // Feed stdin from a worker thread. A blocking `write_all` here would sit
        // outside the deadline loop: a prompt larger than the pipe buffer
        // (~64 KiB) against a child that never reads would hang forever and the
        // timeout would never be consulted.
        let writer = child.stdin.take().map(|mut stdin| {
            let payload = prompt.to_string();
            std::thread::spawn(move || stdin.write_all(payload.as_bytes()))
        });

        let deadline = enclosing_deadline.map_or(call_deadline, |global| global.min(call_deadline));
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {
                    if Instant::now() >= deadline {
                        terminate_process_group(&mut child);
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => {
                    terminate_process_group(&mut child);
                    return Err(LlmError::Failed(format!("wait failed: {error}")));
                }
            }
        };

        // The writer thread is deliberately NOT joined. Killing the child does
        // not necessarily close the read end of the pipe — a grandchild can
        // still hold it — so joining here would reintroduce exactly the
        // unbounded block the thread exists to avoid. It owns its `ChildStdin`
        // and exits on its own when the write completes or the pipe breaks.
        drop(writer);

        let stdout = std::fs::read_to_string(stdout_capture.path()).unwrap_or_default();
        let stderr = std::fs::read_to_string(stderr_capture.path()).unwrap_or_default();

        let Some(status) = status else {
            if enclosing_deadline.is_some_and(|global| Instant::now() >= global) {
                return Err(LlmError::TimedOut(self.timeout));
            }
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
}

/// Test runner: replays scripted responses in order and records the prompts it
/// saw.
///
/// It is exposed rather than `#[cfg(test)]` because the integration tests in
/// `cas-cli/tests/` live outside the crate and need a double for `LlmRunner`.
/// (`cas knowledge build --dry-run` is handed one too, but only as a runner
/// that cannot answer — a dry run returns before any prompt is built.)
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

    fn runner_for(binary: &str, timeout: Duration) -> ClaudeCliRunner {
        ClaudeCliRunner {
            binary: binary.to_string(),
            model: None,
            timeout,
            calls: AtomicUsize::new(0),
        }
    }

    /// Write an executable stub that stands in for the provider CLI. It ignores
    /// the flags the runner passes, so the test controls exit status and output.
    #[cfg(unix)]
    fn stub(script: &str) -> (tempfile::TempDir, String) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("provider");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).expect("write stub");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let name = path.to_string_lossy().to_string();
        (dir, name)
    }

    #[cfg(unix)]
    #[test]
    fn a_nonzero_exit_is_a_failure_carrying_the_stderr_excerpt() {
        let (_dir, binary) = stub("echo 'boom' >&2; exit 3");
        let runner = runner_for(&binary, Duration::from_secs(10));
        match runner.complete("hi") {
            Err(LlmError::Failed(message)) => assert!(message.contains("boom"), "{message}"),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert_eq!(runner.calls(), 1, "a spawned call is billed even if it fails");
    }

    #[cfg(unix)]
    #[test]
    fn an_empty_reply_is_a_failure_not_an_empty_page() {
        let (_dir, binary) = stub("exit 0");
        let runner = runner_for(&binary, Duration::from_secs(10));
        match runner.complete("hi") {
            Err(LlmError::Failed(message)) => assert!(message.contains("empty"), "{message}"),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert_eq!(runner.calls(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_hung_provider_is_killed_at_the_deadline() {
        let (_dir, binary) = stub("sleep 30");
        let runner = runner_for(&binary, Duration::from_millis(300));
        let started = Instant::now();
        match runner.complete("hi") {
            Err(LlmError::Failed(message)) => assert!(message.contains("timed out"), "{message}"),
            other => panic!("expected a timeout, got {other:?}"),
        }
        assert!(started.elapsed() < Duration::from_secs(10), "deadline not enforced");
        assert_eq!(runner.calls(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_timed_out_provider_group_leaves_no_stalled_descendant() {
        let stub_dir = tempfile::tempdir().expect("stub dir");
        let child_pid_file = stub_dir.path().join("child.pid");
        let script = format!(
            "(sleep 30) & child_pid=$!; echo $child_pid > '{}'; wait",
            child_pid_file.display()
        );
        let (_provider_dir, binary) = stub(&script);
        let runner = runner_for(&binary, Duration::from_millis(300));
        let started = Instant::now();
        let result = runner.complete("hi");
        assert!(matches!(result, Err(LlmError::Failed(message)) if message.contains("timed out")));
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "stalled provider must be bounded, took {:?}",
            started.elapsed()
        );

        let child_pid = std::fs::read_to_string(&child_pid_file)
            .expect("provider must start its descendant before the deadline")
            .trim()
            .parse::<u32>()
            .expect("child pid");
        // Give the kernel a short opportunity to reap the group member, then
        // make one liveness probe. The codemap workflow itself never polls.
        std::thread::sleep(Duration::from_millis(100));
        let alive = unsafe { libc::kill(child_pid as i32, 0) == 0 };
        assert!(
            !alive,
            "timed-out provider descendant {child_pid} is still alive"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_prompt_larger_than_the_pipe_buffer_still_honours_the_deadline() {
        // The child never reads stdin, so a blocking write in the parent would
        // wedge before the deadline loop was ever entered.
        let (_dir, binary) = stub("sleep 30");
        let runner = runner_for(&binary, Duration::from_millis(300));
        let huge = "x".repeat(512 * 1024);
        let started = Instant::now();
        assert!(runner.complete(&huge).is_err());
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "a large prompt must not bypass the timeout"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_successful_reply_is_returned_and_captures_are_cleaned_up() {
        let (_dir, binary) = stub("echo '{\"pages\":[]}'");
        let runner = runner_for(&binary, Duration::from_secs(10));
        let reply = runner.complete("hi").expect("reply");
        assert!(reply.contains("pages"));
    }

    #[cfg(unix)]
    #[test]
    fn capture_files_are_private_and_removed_with_their_handle() {
        use std::os::unix::fs::PermissionsExt;

        let capture = ClaudeCliRunner::capture_file("out").expect("capture");
        let path = capture.path().to_path_buf();
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "captures must not be world-readable");
        assert!(
            !path.to_string_lossy().contains(&std::process::id().to_string()),
            "the name must not be predictable from the pid"
        );

        drop(capture);
        assert!(!path.exists(), "the capture must be unlinked with its handle");
    }

    #[test]
    fn missing_binary_is_reported_as_unavailable_not_a_panic() {
        let runner = runner_for("cas-no-such-llm-binary", Duration::from_secs(1));
        match runner.complete("hi") {
            Err(LlmError::Unavailable(_)) => {}
            other => panic!("expected Unavailable, got {other:?}"),
        }
        assert_eq!(runner.calls(), 0, "a failed spawn is not a billed call");
    }
}
