//! Stable client transport; only the private backend is replaced on an update.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::{BTreeSet, VecDeque};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout};

const FRAME_LIMIT: usize = 8 * 1024 * 1024;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(45);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

type Reader = Box<dyn AsyncRead + Unpin + Send>;
type Writer = Box<dyn AsyncWrite + Unpin + Send>;

struct Backend {
    input: Writer,
    output: Reader,
    control_input: Writer,
    control_output: Reader,
    child: Option<tokio::process::Child>,
    image: Option<crate::mcp::socket::ExeIdentity>,
}

#[derive(Debug)]
enum Event {
    Serving { version: String, pid: Option<u32>, image: Option<crate::mcp::socket::ExeIdentity> },
    Prepared,
    Deferred(String),
}

async fn relay<R, W>(
    input: R,
    mut output: W,
    backend: Backend,
    mut updates: mpsc::Receiver<Backend>,
    events: mpsc::Sender<Event>,
) -> Result<()>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    let mut input = Frames::new(input);
    let initialize = loop {
        let Some(frame) = input.read().await? else { return Ok(()) };
        match super::pre_initialize_guard::classify_pre_initialize(&frame) {
            super::pre_initialize_guard::PreInitialize::Drop => continue,
            super::pre_initialize_guard::PreInitialize::Reply(reply) => {
                write(&mut output, &reply).await?;
            }
            super::pre_initialize_guard::PreInitialize::Passthrough => break frame,
        }
    };
    let initialize: Value = serde_json::from_slice(&initialize)?;
    let mut serving = timeout(STARTUP_TIMEOUT, Connection::prepare(backend, &initialize)).await??;
    timeout(STARTUP_TIMEOUT, serving.activate()).await??;
    write_json(&mut output, &json!({"jsonrpc":"2.0", "id":initialize["id"], "result":serving.info})).await?;
    let _ = events.send(serving.event()).await;
    let mut initialized = false;
    let mut pending = Pending::default();
    let mut preparing: Option<tokio::task::JoinHandle<Result<Connection>>> = None;
    let mut prepared: Option<(Connection, Instant)> = None;
    let mut updates_open = true;

    loop {
        // Private probes can encounter real notifications/requests. They stay
        // in order and are forwarded before another attempt at the boundary.
        while let Some(frame) = serving.buffered.pop_front() {
            pending.server(&frame)?;
            write(&mut output, &frame).await?;
        }
        if prepared.as_ref().is_some_and(|(_, deadline)| Instant::now() >= *deadline) {
            prepared.take();
            let _ = events.send(Event::Deferred("drain timed out; old backend is still serving".into())).await;
        }
        if initialized && prepared.is_some() && pending.is_empty() && input.bytes.is_empty() {
            match input.try_read().await {
                std::task::Poll::Ready(Ok(Some(frame))) => {
                    pending.client(&frame)?;
                    write(&mut serving.input, &frame).await?;
                    continue;
                }
                std::task::Poll::Ready(Err(error)) => return Err(error),
                std::task::Poll::Ready(Ok(None)) => return Ok(()),
                std::task::Poll::Pending => {}
            }
        }
        if initialized && prepared.is_some() && pending.is_empty()
            && input.bytes.is_empty() && serving.output.bytes.is_empty()
        {
            let mut candidate = prepared.take().unwrap().0;
            let switch = async {
                // The ping orders earlier notifications ahead of the backend's
                // idle acknowledgement, which also waits for their handlers.
                serving.rpc("ping", None).await?;
                serving.control("idle").await?;
                serving.collect_ready().await?;
                if !serving.buffered.is_empty() || !serving.output.bytes.is_empty() {
                    bail!("backend produced more protocol work at the drain boundary");
                }
                candidate.activate().await?;
                serving.collect_ready().await?;
                if !serving.buffered.is_empty() || !serving.output.bytes.is_empty() {
                    bail!("old backend became busy before switch confirmation");
                }
                Ok::<_, anyhow::Error>(())
            };
            match timeout(STARTUP_TIMEOUT, switch).await {
                Ok(Ok(())) => {
                    // No client request is sent to the candidate until its
                    // activation and private ping have both succeeded. Nothing
                    // mutating is ever retried on either child.
                    let mut old = std::mem::replace(&mut serving, candidate);
                    let _ = events.send(serving.event()).await;
                    let _ = timeout(DRAIN_TIMEOUT, old.control("retire")).await;
                    old.stop().await;
                }
                result => {
                    candidate.stop().await;
                    let _ = timeout(DRAIN_TIMEOUT, serving.control("resume")).await;
                    let reason = match result {
                        Ok(Err(error)) => error.to_string(),
                        Err(_) => "activation/drain timed out".into(),
                        Ok(Ok(())) => unreachable!(),
                    };
                    let _ = events.send(Event::Deferred(reason)).await;
                }
            }
            continue;
        }

        let drain_deadline = prepared.as_ref().map(|(_, deadline)| *deadline)
            .unwrap_or_else(|| Instant::now() + DRAIN_TIMEOUT);
        tokio::select! {
            frame = input.read() => {
                let Some(frame) = frame? else {
                    serving.input.shutdown().await?;
                    if let Some(child) = serving.child.as_mut() {
                        if timeout(Duration::from_secs(10), child.wait()).await.is_err() {
                            let _ = child.kill().await;
                        }
                    }
                    return Ok(());
                };
                let message: Value = serde_json::from_slice(&frame)?;
                if !initialized && message["method"] == "notifications/initialized" {
                    // Each backend already got its own private initialized
                    // notification. Never initialize the external client twice.
                    initialized = true;
                    continue;
                }
                pending.client(&frame)?;
                write(&mut serving.input, &frame).await?;
            }
            frame = serving.read_public() => {
                let Some(frame) = frame? else { bail!("serving MCP backend closed") };
                pending.server(&frame)?;
                write(&mut output, &frame).await?;
            }
            backend = updates.recv(), if updates_open && preparing.is_none() && prepared.is_none() => {
                if let Some(backend) = backend {
                    let initialize = initialize.clone();
                    preparing = Some(tokio::spawn(async move {
                        timeout(STARTUP_TIMEOUT, Connection::prepare(backend, &initialize))
                            .await.context("candidate initialization timed out")?
                    }));
                } else { updates_open = false; }
            }
            result = async { preparing.as_mut().unwrap().await }, if preparing.is_some() => {
                preparing.take();
                match result {
                    Ok(Ok(candidate)) if serving.incompatibility(&candidate).is_none() => {
                        prepared = Some((candidate, Instant::now() + DRAIN_TIMEOUT));
                        let _ = events.send(Event::Prepared).await;
                    }
                    Ok(Ok(mut candidate)) => {
                        let reason = serving.incompatibility(&candidate).unwrap();
                        candidate.stop().await;
                        let _ = events.send(Event::Deferred(reason)).await;
                    }
                    result => {
                        let reason = match result {
                            Ok(Err(error)) => error.to_string(),
                            Err(error) => error.to_string(),
                            _ => unreachable!(),
                        };
                        let _ = events.send(Event::Deferred(reason)).await;
                    }
                }
            }
            () = tokio::time::sleep_until(drain_deadline), if prepared.is_some() => {}
        }
    }
}

struct Frames<R> { reader: R, bytes: Vec<u8> }

impl<R: AsyncRead + Unpin> Frames<R> {
    fn new(reader: R) -> Self { Self { reader, bytes: Vec::new() } }

    // Cancellation retains partial bytes in this object, including read-ahead.
    async fn read(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            if let Some(end) = self.bytes.iter().position(|byte| *byte == b'\n') {
                return Ok(Some(self.bytes.drain(..=end).collect()));
            }
            if self.bytes.len() >= FRAME_LIMIT { bail!("MCP frame exceeds bounded front buffer") }
            let mut chunk = [0; 8192];
            let limit = chunk.len().min(FRAME_LIMIT - self.bytes.len());
            let count = self.reader.read(&mut chunk[..limit]).await?;
            if count == 0 {
                if !self.bytes.is_empty() { bail!("MCP stream ended with a partial frame") }
                return Ok(None);
            }
            self.bytes.extend_from_slice(&chunk[..count]);
        }
    }

    async fn try_read(&mut self) -> std::task::Poll<Result<Option<Vec<u8>>>> {
        use std::future::{Future, poll_fn};
        use std::task::Poll;
        let future = self.read();
        tokio::pin!(future);
        poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
    }
}

async fn write(output: &mut (impl AsyncWrite + Unpin), bytes: &[u8]) -> Result<()> {
    output.write_all(bytes).await?;
    output.flush().await?;
    Ok(())
}

async fn write_json(output: &mut (impl AsyncWrite + Unpin), message: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(message)?;
    bytes.push(b'\n');
    write(output, &bytes).await
}

#[derive(Default)]
struct Pending { to_server: BTreeSet<String>, to_client: BTreeSet<String> }

impl Pending {
    fn observe(frame: &[u8], requests: &mut BTreeSet<String>, responses: &mut BTreeSet<String>) -> Result<()> {
        let message: Value = serde_json::from_slice(frame)?;
        if let Some(id) = message.get("id").filter(|id| !id.is_null()) {
            let id = serde_json::to_string(id)?;
            if message.get("method").is_some() {
                if requests.len() >= 4096 || requests.iter().map(String::len).sum::<usize>() + id.len() > FRAME_LIMIT {
                    bail!("MCP pending request identities exceed bounded front capacity");
                }
                if !requests.insert(id) { bail!("duplicate pending MCP request identity") }
            } else { responses.remove(&id); }
        }
        Ok(())
    }
    fn client(&mut self, frame: &[u8]) -> Result<()> { Self::observe(frame, &mut self.to_server, &mut self.to_client) }
    fn server(&mut self, frame: &[u8]) -> Result<()> { Self::observe(frame, &mut self.to_client, &mut self.to_server) }
    fn is_empty(&self) -> bool { self.to_server.is_empty() && self.to_client.is_empty() }
}

struct Connection {
    input: Writer,
    output: Frames<Reader>,
    control_input: Writer,
    control_output: Frames<Reader>,
    child: Option<tokio::process::Child>,
    image: Option<crate::mcp::socket::ExeIdentity>,
    info: Value,
    tools: Vec<Value>,
    private: BTreeSet<String>,
    buffered: VecDeque<Vec<u8>>,
}

impl Connection {
    async fn prepare(backend: Backend, initialize: &Value) -> Result<Self> {
        let mut connection = Self { input: backend.input, output: Frames::new(backend.output),
            control_input: backend.control_input, control_output: Frames::new(backend.control_output),
            child: backend.child, image: backend.image, info: Value::Null, tools: Vec::new(), private: BTreeSet::new(),
            buffered: VecDeque::new() };
        connection.info = connection.rpc("initialize", initialize.get("params").cloned()).await?;
        write_json(&mut connection.input, &json!({"jsonrpc":"2.0", "method":"notifications/initialized"})).await?;
        connection.tools = connection.catalog().await?;
        if !connection.buffered.is_empty() { bail!("candidate sent unsolicited protocol work during readiness") }
        Ok(connection)
    }

    async fn rpc(&mut self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = format!("cassy-front-{}", uuid::Uuid::new_v4());
        self.private.insert(serde_json::to_string(&id)?);
        let mut request = json!({"jsonrpc":"2.0", "id":id, "method":method});
        if let Some(params) = params { request["params"] = params; }
        write_json(&mut self.input, &request).await?;
        loop {
            let frame = self.output.read().await?.context("backend closed during readiness")?;
            let message: Value = serde_json::from_slice(&frame)?;
            if message.get("method").is_none() && message["id"] == id {
                self.private.remove(&serde_json::to_string(&id)?);
                if let Some(error) = message.get("error") { bail!("backend {method}: {error}") }
                return message.get("result").cloned().context("backend response has no result");
            }
            let buffered_bytes: usize = self.buffered.iter().map(Vec::len).sum();
            if buffered_bytes + frame.len() > FRAME_LIMIT { bail!("backend readiness buffer limit reached") }
            self.buffered.push_back(frame);
        }
    }

    async fn catalog(&mut self) -> Result<Vec<Value>> {
        let mut tools = Vec::new();
        let mut cursor = None;
        let mut seen = BTreeSet::new();
        loop {
            let page = self.rpc("tools/list", cursor.as_ref().map(|cursor| json!({"cursor":cursor}))).await?;
            tools.extend(page["tools"].as_array().context("backend tools/list lacks tools")?.iter().cloned());
            if serde_json::to_vec(&tools)?.len() > FRAME_LIMIT { bail!("backend tool catalog exceeds front buffer") }
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            let Some(next) = cursor.as_ref() else { break };
            if seen.len() >= 1024 || seen.iter().map(String::len).sum::<usize>() + next.len() > FRAME_LIMIT {
                bail!("backend tools/list pagination exceeds front capacity");
            }
            if !seen.insert(next.clone()) { bail!("backend repeats tools/list pagination cursor") }
        }
        tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        Ok(tools)
    }

    fn incompatibility(&self, candidate: &Self) -> Option<String> {
        if self.info["protocolVersion"] != candidate.info["protocolVersion"] {
            return Some(format!("candidate protocolVersion {} differs from negotiated {}",
                candidate.info["protocolVersion"], self.info["protocolVersion"]));
        }
        if self.info["capabilities"] != candidate.info["capabilities"] {
            return Some("candidate capabilities differ from the initialized session".into());
        }
        if self.info["serverInfo"]["name"] != candidate.info["serverInfo"]["name"] {
            return Some("candidate server identity differs from the initialized session".into());
        }
        if self.tools != candidate.tools {
            return Some("candidate tools/list schema differs from the initialized session".into());
        }
        None
    }

    async fn control(&mut self, action: &str) -> Result<()> {
        let id = uuid::Uuid::new_v4().to_string();
        write_json(&mut self.control_input, &json!({"id":id, "action":action})).await?;
        loop {
            let reply = self.control_output.read().await?.context("backend control closed")?;
            let reply: Value = serde_json::from_slice(&reply)?;
            if reply["id"] != id { continue; }
            if reply["ok"] != true { bail!("backend {action}: {}", reply["error"]) }
            return Ok(());
        }
    }

    async fn activate(&mut self) -> Result<()> {
        if self.image.as_ref().is_some_and(crate::mcp::socket::ExeIdentity::is_stale) {
            bail!("installed backend changed during readiness; retaining serving backend");
        }
        self.control("activate").await?;
        if self.catalog().await? != self.tools { bail!("activated backend tool schema changed") }
        self.rpc("ping", None).await?;
        if !self.buffered.is_empty() { bail!("candidate emitted protocol work before confirmation") }
        Ok(())
    }

    async fn read_public(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            let Some(frame) = self.output.read().await? else { return Ok(None) };
            let message: Value = serde_json::from_slice(&frame)?;
            if message.get("method").is_none()
                && self.private.remove(&serde_json::to_string(&message["id"])?) { continue; }
            return Ok(Some(frame));
        }
    }

    async fn collect_ready(&mut self) -> Result<()> {
        use std::future::{Future, poll_fn};
        use std::task::Poll;
        loop {
            let ready = {
                let future = self.read_public();
                tokio::pin!(future);
                poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
            };
            match ready {
                Poll::Ready(Ok(Some(frame))) => {
                    if self.buffered.iter().map(Vec::len).sum::<usize>() + frame.len() > FRAME_LIMIT {
                        bail!("backend drain buffer limit reached");
                    }
                    self.buffered.push_back(frame);
                }
                Poll::Ready(Ok(None)) => bail!("serving backend closed at switch boundary"),
                Poll::Ready(Err(error)) => return Err(error),
                Poll::Pending => return Ok(()),
            }
        }
    }

    fn event(&self) -> Event {
        Event::Serving { version: self.info["serverInfo"]["version"].as_str().unwrap_or("unknown").into(),
            pid: self.child.as_ref().and_then(tokio::process::Child::id), image: self.image.clone() }
    }

    async fn stop(&mut self) {
        let _ = self.input.shutdown().await;
        if let Some(child) = self.child.as_mut() {
            if timeout(Duration::from_secs(2), child.wait()).await.is_err() { let _ = child.kill().await; }
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct ServingState {
    pub front_pid: u32,
    pub harness_pid: u32,
    pub backend_pid: u32,
    pub front_version: String,
    pub backend_version: String,
    pub deferred: Option<String>,
    pub front_starttime: u64,
    pub backend_starttime: u64,
}

pub(super) fn reported_state(root: &std::path::Path, front_pid: u32) -> Option<ServingState> {
    let state: ServingState = serde_json::from_slice(&std::fs::read(
        root.join("mcp-fronts").join(format!("{front_pid}.json"))).ok()?).ok()?;
    (state.front_pid == front_pid).then_some(state)
}

pub(crate) fn serving_state(root: &std::path::Path, front_pid: u32) -> Option<ServingState> {
    let state = reported_state(root, front_pid)?;
    (crate::agent_id::get_parent_of_pid(front_pid) == Some(state.harness_pid)
        && crate::agent_id::get_parent_of_pid(state.backend_pid) == Some(front_pid)
        && crate::mcp::daemon::pid_matches_fingerprint(front_pid, state.front_starttime)
        && crate::mcp::daemon::pid_matches_fingerprint(state.backend_pid, state.backend_starttime))
        .then_some(state)
}

#[cfg(unix)]
fn spawn_backend(program: &std::path::Path, standby: bool) -> Result<Backend> {
    use std::os::fd::AsRawFd;
    use std::process::Stdio;
    let image = crate::mcp::socket::ExeIdentity::from_exe_path(program.to_owned());
    let (control, child_control) = std::os::unix::net::UnixStream::pair()?;
    let fd = child_control.as_raw_fd();
    let mut command = tokio::process::Command::new(program);
    command.args(["serve", "--backend", "--control-fd", &fd.to_string()]);
    if standby { command.arg("--standby"); }
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).kill_on_drop(true);
    // SAFETY: after fork this closure performs only the async-signal-safe
    // fcntl call. Exactly the intentional private control socket survives exec.
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) == -1 { return Err(std::io::Error::last_os_error()); }
            Ok(())
        });
    }
    let mut child = command.spawn().context("start private MCP backend")?;
    drop(child_control);
    if image.is_stale() { bail!("installed binary changed while starting backend") }
    control.set_nonblocking(true)?;
    let control = tokio::net::UnixStream::from_std(control)?;
    let (control_read, control_write) = control.into_split();
    Ok(Backend { input: Box::new(child.stdin.take().context("backend stdin")?),
        output: Box::new(child.stdout.take().context("backend stdout")?),
        control_input: Box::new(control_write), control_output: Box::new(control_read),
        child: Some(child), image: Some(image) })
}

#[cfg(unix)]
pub(super) async fn run() -> Result<()> {
    let root = super::runtime::resolve_mcp_serve_root()?;
    let watchdog = super::parent_watchdog::spawn();
    let program = std::env::args_os().next().map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute()).unwrap_or(std::env::current_exe()?);
    let backend = spawn_backend(&program, false)?;
    let image = backend.image.clone();
    let image = std::sync::Arc::new(std::sync::Mutex::new(image));
    let (updates, update_rx) = mpsc::channel(1);
    let (events, mut event_rx) = mpsc::channel(8);
    let image_watch = image.clone();
    let watcher = tokio::spawn(async move {
        let mut ticks = tokio::time::interval(Duration::from_secs(30));
        loop {
            ticks.tick().await;
            let stale = image_watch.lock().map(|image| image.as_ref()
                .is_some_and(crate::mcp::socket::ExeIdentity::is_stale)).unwrap_or(false);
            if !stale || updates.capacity() == 0 { continue; }
            match spawn_backend(&program, true) {
                Ok(backend) => if updates.send(backend).await.is_err() { break; },
                Err(error) => eprintln!("[Cassy] MCP upgrade deferred; current backend remains serving: {error}"),
            }
        }
    });
    let pid = std::process::id();
    let path = root.join("mcp-fronts").join(format!("{pid}.json"));
    let report_path = path.clone();
    let reporter = tokio::spawn(async move {
        let mut state: Option<ServingState> = None;
        while let Some(event) = event_rx.recv().await {
            match event {
                Event::Serving { version, pid: Some(backend_pid), image: serving_image } => {
                    if let Ok(mut image) = image.lock() { *image = serving_image; }
                    eprintln!("[Cassy] MCP serving backend {version} (PID {backend_pid}); stable front {} (PID {pid})", env!("CARGO_PKG_VERSION"));
                    state = Some(ServingState { front_pid: pid,
                        harness_pid: std::os::unix::process::parent_id(), backend_pid,
                        front_version: env!("CARGO_PKG_VERSION").into(), backend_version: version, deferred: None,
                        front_starttime: crate::mcp::daemon::read_pid_starttime(pid).unwrap_or(0),
                        backend_starttime: crate::mcp::daemon::read_pid_starttime(backend_pid).unwrap_or(0) });
                }
                Event::Deferred(reason) => {
                    eprintln!("[Cassy] MCP upgrade deferred; current backend remains serving: {reason}");
                    if let Some(state) = &mut state { state.deferred = Some(reason); }
                }
                _ => continue,
            }
            if let Some(state) = &state {
                let save = || -> Result<()> {
                    std::fs::create_dir_all(report_path.parent().unwrap())?;
                    let temp = report_path.with_extension("tmp");
                    std::fs::write(&temp, serde_json::to_vec(state)?)?;
                    std::fs::rename(temp, &report_path)?;
                    Ok(())
                };
                if let Err(error) = save() { eprintln!("[Cassy] MCP serving-version report failed: {error}"); }
            }
        }
        state.is_some()
    });
    let result = tokio::select! {
        result = relay(tokio::io::stdin(), tokio::io::stdout(), backend, update_rx, events) => result,
        () = watchdog.tripped() => Ok(()),
    };
    watcher.abort();
    // Drain the serving report before deciding whether this front ever owned
    // the session. Only the front releases leases on a real client shutdown;
    // failed candidates and retired children must never do so.
    if reporter.await.unwrap_or(false) {
        if let Ok(id) = crate::agent_id::read_session_for_mcp(&root) {
            super::runtime::release_agent_tasks(&root, &id, "front_transport_closed")?;
        }
    }
    let _ = std::fs::remove_file(path);
    result
}

#[cfg(test)]
#[path = "front/tests.rs"]
mod tests;
