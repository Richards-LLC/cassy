//! Private child endpoint. Standby negotiation never opens stores, registers
//! an agent or starts background work. Only its owning front can activate it.
use std::sync::{Arc, OnceLock, atomic::{AtomicUsize, Ordering}};
use anyhow::{Context, Result, bail};
use rmcp::{Service, ServiceExt};
use rmcp::model::{ClientRequest, ClientNotification, ServerResult, ServerInfo};
use rmcp::service::{RoleServer, RequestContext, NotificationContext};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::mcp::tools::CasService;
use super::CasCore;

#[derive(Clone)]
struct Switchable {
    passive: CasService,
    active: Arc<OnceLock<CasService>>,
    handlers: Arc<AtomicUsize>,
    idle: Arc<tokio::sync::Notify>,
}

struct HandlerPermit(Switchable);
impl Drop for HandlerPermit {
    fn drop(&mut self) {
        if self.0.handlers.fetch_sub(1, Ordering::AcqRel) == 1 { self.0.idle.notify_waiters(); }
    }
}

impl Switchable {
    fn standby(root: std::path::PathBuf) -> Self {
        let core = CasCore::with_daemon(root, None, None);
        #[cfg(feature = "mcp-proxy")]
        let passive = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let passive = CasService::new(core);
        Self { passive, active: Arc::new(OnceLock::new()), handlers: Arc::new(AtomicUsize::new(0)),
            idle: Arc::new(tokio::sync::Notify::new()) }
    }

    fn permit(&self) -> HandlerPermit {
        self.handlers.fetch_add(1, Ordering::AcqRel);
        HandlerPermit(self.clone())
    }

    async fn wait_idle(&self) {
        loop {
            let changed = self.idle.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.handlers.load(Ordering::Acquire) == 0 { return; }
            changed.await;
        }
    }
}

impl Service<RoleServer> for Switchable {
    fn handle_request(&self, request: ClientRequest, context: RequestContext<RoleServer>)
        -> impl std::future::Future<Output=Result<ServerResult, rmcp::ErrorData>> + Send + '_ {
        let permit = self.permit();
        async move {
            let _permit = permit;
            let service = match self.active.get() {
                Some(service) => service,
                None if matches!(&request, ClientRequest::InitializeRequest(_)
                    | ClientRequest::ListToolsRequest(_) | ClientRequest::PingRequest(_)) => &self.passive,
                None => return Err(rmcp::ErrorData::internal_error("MCP backend is not active", None)),
            };
            Service::handle_request(service, request, context).await
        }
    }

    fn handle_notification(&self, notification: ClientNotification, context: NotificationContext<RoleServer>)
        -> impl std::future::Future<Output=Result<(), rmcp::ErrorData>> + Send + '_ {
        let permit = self.permit();
        async move {
            let _permit = permit;
            Service::handle_notification(self.active.get().unwrap_or(&self.passive), notification, context).await
        }
    }

    fn get_info(&self) -> ServerInfo {
        Service::get_info(self.active.get().unwrap_or(&self.passive))
    }
}

/// FD ownership is established from a live Unix stream socket and its peer,
/// never an environment-provided session or harness PID.
fn control_socket(fd: i32) -> Result<tokio::net::UnixStream> {
    use std::os::fd::FromRawFd;
    if fd < 3 { bail!("private MCP control requires a separate socket descriptor") }
    let mut kind: libc::c_int = 0;
    let mut len = std::mem::size_of_val(&kind) as libc::socklen_t;
    // SAFETY: the output pointers describe live appropriately-sized values;
    // getsockopt checks that the untrusted descriptor is an open socket.
    let valid = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_TYPE,
        (&mut kind as *mut libc::c_int).cast(), &mut len) };
    if valid != 0 || kind != libc::SOCK_STREAM { bail!("invalid private MCP control socket") }
    // SAFETY: the descriptor above is valid, is transferred exclusively to
    // this private entry point, and is now owned by the UnixStream.
    let stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
    stream.set_nonblocking(true)?;
    let stream = tokio::net::UnixStream::from_std(stream)?;
    let peer = stream.peer_cred()?;
    let parent = std::os::unix::process::parent_id();
    // SAFETY: geteuid is a plain process identity query.
    if peer.uid() != unsafe { libc::geteuid() } || peer.pid() != Some(parent as i32) {
        bail!("private MCP control peer is not the owning front");
    }
    // Refuse a nested harness pretending to be the front. The existing
    // fingerprinted factory-owner guard still validates the resolved harness.
    let deadline = crate::bounded_process::Deadline::after(std::time::Duration::from_millis(500));
    let probe = |field: &str| -> Result<String> {
        let output = crate::bounded_process::run_command(
            std::process::Command::new("ps").args(["-p", &parent.to_string(), "-o", field]),
            deadline, std::time::Duration::from_millis(500),
        ).map_err(|error| anyhow::anyhow!("cannot verify owning front: {error:?}"))?;
        if !output.status.success() { bail!("owning front process is unavailable") }
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    };
    // BSD ps truncates comm to 15 columns when another field follows it,
    // even with -ww. Query each field separately; keep paths with spaces.
    let executable = probe("comm=")?;
    let command = probe("args=")?;
    if std::path::Path::new(&executable).file_name() != std::env::current_exe()?.file_name()
        || !command.split_whitespace().any(|arg| arg == "serve")
        || command.split_whitespace().any(|arg| arg == "--backend") {
        bail!("private MCP backend parent is not a stable cas serve front");
    }
    let harness = crate::agent_id::get_parent_of_pid(parent).context("owning front's harness is unavailable")?;
    crate::agent_id::bind_mcp_front_harness(harness)?;
    Ok(stream)
}

pub(crate) async fn run(fd: i32, standby: bool) -> Result<()> {
    let control = control_socket(fd)?;
    let front_pid = std::os::unix::process::parent_id();
    let root = super::runtime::resolve_mcp_serve_root()?;
    let watchdog = super::parent_watchdog::spawn();
    if !crate::store::detect::cas_store_present(&root) { bail!("no initialized Cassy store; run cas init first") }
    if standby && crate::migration::check_migrations(&root)?.has_pending() {
        bail!("candidate requires schema migrations; reconnect after an explicit migration instead");
    }
    let switchable = Switchable::standby(root.clone());
    let input = super::pre_initialize_guard::spawn(tokio::io::stdin(), tokio::io::stdout());
    let server = switchable.clone().serve((input, tokio::io::stdout())).await?;
    let cancel = server.cancellation_token();
    let waiting = server.waiting();
    tokio::pin!(waiting);
    let (read, mut write) = control.into_split();
    let mut control = BufReader::new(read);
    let mut daemon = None;
    let mut retire = false;
    let end_reason = loop {
        let mut line = String::new();
        tokio::select! {
            result = &mut waiting => {
                if let Err(error) = result { eprintln!("[Cassy] MCP backend terminated: {error}"); }
                break "transport_eof";
            }
            () = watchdog.tripped() => break "parent_watchdog",
            count = control.read_line(&mut line) => {
                if count? == 0 { break "front_eof"; }
                if line.len() > 4096 { bail!("private MCP control frame is too large") }
                let request: Value = serde_json::from_str(&line)?;
                let result = match request["action"].as_str() {
                    Some("activate") if switchable.active.get().is_none() => {
                        match super::runtime::prepare_service(root.clone()).await {
                            Ok((service, running_daemon)) => {
                                daemon = running_daemon;
                                let _ = switchable.active.set(service);
                                Ok(())
                            }
                            Err(error) => Err(error),
                        }
                    }
                    Some("activate") => Ok(()),
                    Some("resume") => {
                        if let Some(service) = switchable.active.get()
                            && let Some(Some(id)) = service.inner.agent_id.get() {
                            service.inner.ensure_agent_active(id).map_err(|error| anyhow::anyhow!("{error}"))
                        } else { Ok(()) }
                    }
                    Some("idle") => { switchable.wait_idle().await; Ok(()) }
                    Some("retire") => { switchable.wait_idle().await; retire = true; Ok(()) }
                    _ => Err(anyhow::anyhow!("unknown private backend control action")),
                };
                let reply = match result {
                    Ok(()) => json!({"id":request["id"], "ok":true}),
                    Err(error) => json!({"id":request["id"], "ok":false, "error":error.to_string()}),
                };
                write.write_all(&serde_json::to_vec(&reply)?).await?;
                write.write_all(b"\n").await?;
                write.flush().await?;
                if retire { break "backend_retired"; }
            }
        }
    };
    cancel.cancel();
    if let Some(daemon) = daemon { daemon.shutdown(); }
    // Retirement is a transport replacement, never a session shutdown.
    // A rejected standby also never owned the session's task leases.
    if !retire && switchable.active.get().is_some()
        && !crate::mcp::daemon::pid_alive(front_pid)
        && super::front::reported_state(&root, front_pid)
            .is_some_and(|state| state.backend_pid == std::process::id()) {
        if let Ok(id) = crate::agent_id::read_session_for_mcp(&root) {
            super::runtime::release_agent_tasks(&root, &id, end_reason)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn background_tool_work_defers_backend_swap_cas_5fd5() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let backend = Switchable::standby(dir.path().join(".cas"));
        let service = backend.passive.clone();
        backend.active.set(service.clone()).ok().unwrap();
        let finish = Arc::new(tokio::sync::Notify::new());
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let work_finish = finish.clone();
        let work_finished = finished.clone();
        let arguments = json!({"action":"close", "id":"cas-background"});
        let response = service.call_with_deadline_and_grace("task", arguments.as_object(),
            std::time::Duration::from_secs(60), std::time::Duration::ZERO, async move {
                crate::mcp::tools::service::mutation_receipt::task_committed("cas-background");
                work_finish.notified().await;
                work_finished.store(true, Ordering::Release);
                Ok(rmcp::model::CallToolResult::success(Vec::new()))
            }).await.unwrap();
        assert_ne!(response.is_error, Some(true));
        assert!(!finished.load(Ordering::Acquire), "the response must precede background completion");
        // The front's candidate switch uses this bounded idle handshake after
        // the client has received its early answer. It must defer retirement.
        assert!(tokio::time::timeout(std::time::Duration::from_secs(45), backend.wait_idle()).await.is_err(),
            "a new install must not retire the backend while promised tool work is still running");
        finish.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(1), backend.wait_idle()).await
            .expect("the same backend can retire once background work completes");
        assert!(finished.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn standby_negotiates_real_rmcp_without_opening_a_store_cas_81c4() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("absent-cas-root");
        let standby = Switchable::standby(root.clone());
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(async move {
            standby.serve(server_io).await.unwrap().waiting().await.unwrap();
        });
        let client = ().serve(client_io).await.unwrap();
        let tools = client.list_tools(None).await.unwrap();
        assert!(!tools.tools.is_empty());
        assert!(!root.exists(), "standby handshake/tools must not create a store or register an agent");
        client.cancel().await.unwrap();
        server.await.unwrap();
    }
}
