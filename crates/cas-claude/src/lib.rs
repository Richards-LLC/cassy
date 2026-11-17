//! Headless client for the Claude Code CLI.
//!
//! Runs `claude --print --output-format stream-json --verbose` and reads the
//! newline-delimited JSON events it writes to stdout. [`prompt`] returns the
//! final `result` event of a one-shot run; [`session`] keeps stdin open with
//! `--input-format stream-json` for multi-turn conversations.
//!
//! A `result` event ends every run the CLI completes, including failed ones
//! (`is_error`, `error_max_turns`), so those come back as `Ok` with the
//! result's fields set. `Err` means no result arrived: the CLI could not start,
//! wrote an unreadable event or exited early.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::task::JoinHandle;

pub type Result<T> = std::result::Result<T, Error>;

/// Bytes of CLI stderr kept for a failure report.
const STDERR_TAIL_BYTES: usize = 4096;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Claude CLI not found; install it or set CLAUDE_EXECUTABLE")]
    ExecutableNotFound,
    #[error("Claude CLI I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Claude CLI wrote an unreadable event ({source}): {line}")]
    Parse {
        line: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("Claude CLI exited without a result: {stderr}")]
    StreamEnded { stderr: String },
    #[error("Claude CLI sent nothing for {0:?}")]
    Timeout(Duration),
    #[error("invalid option: {0}")]
    InvalidOption(String),
}

/// Options for one CLI run. Unset options leave the CLI's own defaults.
#[derive(Debug, Clone, Default)]
pub struct QueryOptions {
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub max_turns: Option<u32>,
    pub max_thinking_tokens: Option<u32>,
    pub allowed_tools: Vec<String>,
    pub mcp_config: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    /// Variables added to (or overriding) the inherited environment.
    pub env: Option<HashMap<String, String>>,
    /// Arguments passed through verbatim after the options above.
    pub extra_args: Vec<String>,
    /// CLI to run instead of the `claude` found by [`default_executable`].
    pub executable: Option<PathBuf>,
}

impl QueryOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn max_turns(mut self, turns: u32) -> Self {
        self.max_turns = Some(turns);
        self
    }

    pub fn max_thinking_tokens(mut self, tokens: u32) -> Self {
        self.max_thinking_tokens = Some(tokens);
        self
    }

    pub fn allow_tool(mut self, tool: impl Into<String>) -> Self {
        self.allowed_tools.push(tool.into());
        self
    }

    pub fn mcp_config(mut self, path: impl AsRef<Path>) -> Self {
        self.mcp_config = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn cwd(mut self, dir: impl AsRef<Path>) -> Self {
        self.cwd = Some(dir.as_ref().to_path_buf());
        self
    }

    pub fn env_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env
            .get_or_insert_with(HashMap::new)
            .insert(key.into(), value.into());
        self
    }

    pub fn extra_arg(mut self, arg: impl Into<String>) -> Self {
        self.extra_args.push(arg.into());
        self
    }

    pub fn executable(mut self, path: impl AsRef<Path>) -> Self {
        self.executable = Some(path.as_ref().to_path_buf());
        self
    }

    /// CLI arguments for a one-shot `prompt`, or for a stream-json session
    /// when `prompt` is `None`. The prompt follows `--` so variadic options
    /// such as `--allowed-tools` cannot absorb it.
    pub fn args(&self, prompt: Option<&str>) -> Vec<String> {
        let mut args: Vec<String> = ["--print", "--output-format", "stream-json", "--verbose"]
            .map(String::from)
            .into();
        if prompt.is_none() {
            args.extend(["--input-format", "stream-json"].map(String::from));
        }
        let mut option = |flag: &str, value: String| {
            args.push(flag.to_string());
            args.push(value);
        };
        if let Some(model) = &self.model {
            option("--model", model.clone());
        }
        if let Some(system_prompt) = &self.system_prompt {
            option("--system-prompt", system_prompt.clone());
        }
        if let Some(turns) = self.max_turns {
            option("--max-turns", turns.to_string());
        }
        if let Some(tokens) = self.max_thinking_tokens {
            option("--max-thinking-tokens", tokens.to_string());
        }
        if let Some(config) = &self.mcp_config {
            option("--mcp-config", config.to_string_lossy().into_owned());
        }
        for tool in &self.allowed_tools {
            option("--allowed-tools", tool.clone());
        }
        args.extend(self.extra_args.iter().cloned());
        if let Some(prompt) = prompt {
            args.push("--".to_string());
            args.push(prompt.to_string());
        }
        args
    }

    fn validate(&self) -> Result<()> {
        if self.max_turns == Some(0) {
            return Err(Error::InvalidOption(
                "max_turns must be at least 1".to_string(),
            ));
        }
        Ok(())
    }
}

/// `CLAUDE_EXECUTABLE`, else `claude` on `PATH`, else the per-user install
/// at `~/.claude/local/claude`.
pub fn default_executable() -> PathBuf {
    if let Some(path) = std::env::var_os("CLAUDE_EXECUTABLE") {
        return PathBuf::from(path);
    }
    let on_path = std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("claude").is_file()));
    if !on_path && let Some(home) = std::env::var_os("HOME") {
        let local = Path::new(&home).join(".claude/local/claude");
        if local.is_file() {
            return local;
        }
    }
    PathBuf::from("claude")
}

/// One event from the CLI's stream-json output.
#[derive(Debug, Clone)]
pub enum Message {
    System(Value),
    Assistant(AssistantMessage),
    User(Value),
    Result(ResultMessage),
    /// Any other event type, such as `rate_limit_event`.
    Other(Value),
}

/// An `assistant` event; `raw` is the event as written by the CLI.
#[derive(Debug, Clone)]
pub struct AssistantMessage {
    pub raw: Value,
}

impl AssistantMessage {
    /// The concatenated text blocks of the message.
    pub fn text(&self) -> String {
        self.raw["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str())
            .collect()
    }
}

/// The final `result` event of a run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResultMessage {
    /// `success`, `error_max_turns`, `error_during_execution`, ...
    pub subtype: Option<String>,
    pub is_error: Option<bool>,
    pub result: Option<String>,
    pub session_id: Option<String>,
    pub num_turns: Option<u32>,
    pub duration_ms: Option<u64>,
    pub total_cost_usd: Option<f64>,
    pub usage: Option<Usage>,
    pub errors: Option<Vec<Value>>,
}

impl ResultMessage {
    /// The final answer text, empty when the run produced none.
    pub fn text(&self) -> &str {
        self.result.as_deref().unwrap_or("")
    }

    pub fn is_success(&self) -> bool {
        self.subtype.as_deref() == Some("success") && self.is_error != Some(true)
    }

    pub fn is_max_turns(&self) -> bool {
        self.subtype.as_deref() == Some("error_max_turns")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

/// Parse one stdout line; blank lines yield `None`.
pub fn parse_line(line: &str) -> Result<Option<Message>> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    let parse_error = |source| Error::Parse {
        line: line.chars().take(200).collect(),
        source,
    };
    let value: Value = serde_json::from_str(line).map_err(parse_error)?;
    let message = match value["type"].as_str() {
        Some("result") => Message::Result(serde_json::from_value(value).map_err(parse_error)?),
        Some("assistant") => Message::Assistant(AssistantMessage { raw: value }),
        Some("system") => Message::System(value),
        Some("user") => Message::User(value),
        _ => Message::Other(value),
    };
    Ok(Some(message))
}

/// A running CLI process and its event stream.
struct Run {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Lines<BufReader<ChildStdout>>,
    stderr: Option<JoinHandle<String>>,
}

impl Run {
    fn spawn(options: &QueryOptions, prompt: Option<&str>) -> Result<Self> {
        options.validate()?;
        let executable = options
            .executable
            .clone()
            .unwrap_or_else(default_executable);
        let mut command = Command::new(executable);
        command
            .args(options.args(prompt))
            // A CLI started from inside Claude Code is a separate session.
            .env_remove("CLAUDECODE")
            .stdin(if prompt.is_some() {
                Stdio::null()
            } else {
                Stdio::piped()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &options.cwd {
            command.current_dir(cwd);
        }
        if let Some(env) = &options.env {
            command.envs(env);
        }
        let mut child = command.spawn().map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => Error::ExecutableNotFound,
            _ => Error::Io(error),
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Io(std::io::Error::other("stdout not captured")))?;
        let stderr = child.stderr.take().map(|mut stderr| {
            // Drained concurrently so a chatty CLI never blocks on a full pipe.
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let _ = stderr.read_to_end(&mut bytes).await;
                let start = bytes.len().saturating_sub(STDERR_TAIL_BYTES);
                String::from_utf8_lossy(&bytes[start..]).trim().to_string()
            })
        });
        Ok(Self {
            stdin: child.stdin.take(),
            stdout: BufReader::new(stdout).lines(),
            stderr,
            child,
        })
    }

    /// The next event, or `None` once stdout closes.
    async fn next(&mut self, timeout: Option<Duration>) -> Result<Option<Message>> {
        loop {
            let line = match timeout {
                Some(limit) => tokio::time::timeout(limit, self.stdout.next_line())
                    .await
                    .map_err(|_| Error::Timeout(limit))??,
                None => self.stdout.next_line().await?,
            };
            let Some(line) = line else { return Ok(None) };
            if let Some(message) = parse_line(&line)? {
                return Ok(Some(message));
            }
        }
    }

    async fn ended(&mut self) -> Error {
        let _ = self.child.wait().await;
        let stderr = match self.stderr.take() {
            Some(handle) => handle.await.unwrap_or_default(),
            None => String::new(),
        };
        Error::StreamEnded { stderr }
    }
}

/// Run `prompt` once and return the CLI's final result.
pub async fn prompt(prompt: &str, options: QueryOptions) -> Result<ResultMessage> {
    let mut run = Run::spawn(&options, Some(prompt))?;
    let mut result = None;
    while let Some(message) = run.next(None).await? {
        if let Message::Result(message) = message {
            result = Some(message);
        }
    }
    match result {
        Some(result) => {
            let _ = run.child.wait().await;
            Ok(result)
        }
        None => Err(run.ended().await),
    }
}

/// A multi-turn conversation over stream-json stdin.
pub struct Session {
    run: Run,
}

impl Session {
    /// Send one user message.
    pub async fn send(&mut self, text: &str) -> Result<()> {
        let event = serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": text},
        });
        let stdin = self
            .run
            .stdin
            .as_mut()
            .ok_or_else(|| Error::Io(std::io::Error::other("session stdin is closed")))?;
        stdin.write_all(format!("{event}\n").as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    /// The next event; each turn ends with a [`Message::Result`].
    pub async fn await_response(&mut self, timeout: Option<Duration>) -> Result<Message> {
        match self.run.next(timeout).await? {
            Some(message) => Ok(message),
            None => Err(self.run.ended().await),
        }
    }
}

/// Start a session, run `f` with it, and stop the CLI when `f` returns.
pub async fn session<F, Fut, T>(options: QueryOptions, f: F) -> Result<T>
where
    F: FnOnce(Session) -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let run = Run::spawn(&options, None)?;
    f(Session { run }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(name)
    }

    /// A stand-in CLI: records its argv, input and environment in its cwd,
    /// then replays a recorded `claude` stdout and exit status.
    fn fake_cli(dir: &Path) -> PathBuf {
        let path = dir.join("claude");
        std::fs::write(
            &path,
            "#!/bin/sh\n\
             printf '%s\\n' \"$@\" > argv.txt\n\
             printf '%s\\n' \"${CLAUDECODE:-unset}\" > env.txt\n\
             if [ \"$1 $2 $3 $4 $5\" = '--print --output-format stream-json --verbose --input-format' ]; then\n\
               IFS= read -r line; printf '%s\\n' \"$line\" > input.jsonl\n\
             fi\n\
             cat \"$FIXTURE\"\n\
             exit \"$EXIT\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn replay(dir: &Path, name: &str, exit: u8) -> QueryOptions {
        QueryOptions::new()
            .executable(fake_cli(dir))
            .cwd(dir)
            .env_var("FIXTURE", fixture(name).to_string_lossy())
            .env_var("EXIT", exit.to_string())
    }

    /// Recorded `claude` 2.1.326 runs: the CLI exits 1 after an error result,
    /// and the result still reaches the caller.
    #[tokio::test]
    async fn prompt_returns_the_result_of_recorded_cli_runs() {
        let dir = tempfile::tempdir().unwrap();

        let ok = prompt("hi", replay(dir.path(), "success.jsonl", 0))
            .await
            .unwrap();
        assert!(ok.is_success());
        assert_eq!(ok.text(), "OK");
        assert!(ok.usage.unwrap().output_tokens > 0);

        let model_error = prompt("hi", replay(dir.path(), "error.jsonl", 1))
            .await
            .unwrap();
        assert_eq!(model_error.is_error, Some(true));
        assert!(!model_error.is_success());

        let max_turns = prompt("hi", replay(dir.path(), "max_turns.jsonl", 1))
            .await
            .unwrap();
        assert!(max_turns.is_max_turns());
        assert!(!max_turns.errors.unwrap().is_empty());

        let missing = prompt("hi", replay(dir.path(), "absent.jsonl", 1)).await;
        assert!(
            matches!(missing, Err(Error::StreamEnded { .. })),
            "{missing:?}"
        );
    }

    #[tokio::test]
    async fn spawned_cli_receives_every_option() {
        let dir = tempfile::tempdir().unwrap();
        let options = replay(dir.path(), "success.jsonl", 0)
            .model("haiku")
            .system_prompt("be brief")
            .max_turns(2)
            .max_thinking_tokens(1024)
            .mcp_config(dir.path().join("mcp.json"))
            .allow_tool("Read")
            .allow_tool("mcp__cas__task")
            .extra_arg("--effort")
            .extra_arg("low");
        prompt("-starts with a dash", options).await.unwrap();

        let argv = std::fs::read_to_string(dir.path().join("argv.txt")).unwrap();
        let mcp = dir.path().join("mcp.json");
        let expected = [
            "--print",
            "--output-format",
            "stream-json",
            "--verbose",
            "--model",
            "haiku",
            "--system-prompt",
            "be brief",
            "--max-turns",
            "2",
            "--max-thinking-tokens",
            "1024",
            "--mcp-config",
            mcp.to_str().unwrap(),
            "--allowed-tools",
            "Read",
            "--allowed-tools",
            "mcp__cas__task",
            "--effort",
            "low",
            "--",
            "-starts with a dash",
        ];
        assert_eq!(argv.lines().collect::<Vec<_>>(), expected);
        let env = std::fs::read_to_string(dir.path().join("env.txt")).unwrap();
        assert_eq!(env.trim(), "unset", "CLAUDECODE must not reach the child");
    }

    #[tokio::test]
    async fn session_sends_stream_json_and_reads_until_the_result() {
        let dir = tempfile::tempdir().unwrap();
        let result = session(
            replay(dir.path(), "success.jsonl", 0),
            |mut session| async move {
                session.send("hello").await?;
                let mut assistant_text = String::new();
                loop {
                    match session
                        .await_response(Some(Duration::from_secs(30)))
                        .await?
                    {
                        Message::Assistant(message) => assistant_text.push_str(&message.text()),
                        Message::Result(result) => return Ok((assistant_text, result)),
                        _ => {}
                    }
                }
            },
        )
        .await
        .unwrap();

        assert_eq!(result.0, "OK");
        assert!(result.1.is_success());
        let input: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("input.jsonl")).unwrap())
                .unwrap();
        assert_eq!(input["type"], "user");
        assert_eq!(input["message"]["content"], "hello");
    }
}
