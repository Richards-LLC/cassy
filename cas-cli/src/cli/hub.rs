use std::fs::OpenOptions;
use std::net::{IpAddr, SocketAddr};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};

use crate::cli::Cli;
use crate::hub::{
    DEFAULT_HUB_PORT, DEFAULT_VIEWER_QUEUE_CAPACITY, DaemonConnector, HubProcessRecord,
    HubRuntimePaths, HubState, LocalSessionReadModel, MachineEventBus, MachineIdentityStore,
    PreAuthAuthorizer, SessionCatalog, SessionMultiplexer, TransportSecurity, router,
    validate_control_bind,
};

#[derive(Args, Debug, Clone)]
pub struct HubArgs {
    #[command(subcommand)]
    pub command: Option<HubCommands>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum HubCommands {
    /// Start the machine hub as a detached, single-instance service
    Start(HubServeArgs),
    /// Run the hub in the foreground (service-manager entrypoint)
    #[command(hide = true)]
    Serve(HubServeArgs),
    /// Report the durable hub process and endpoint state
    Status,
    /// Gracefully stop the machine hub
    Stop,
    /// Stop and start the hub while preserving machine identity
    Restart(HubServeArgs),
}

#[derive(Args, Debug, Clone)]
pub struct HubServeArgs {
    /// Stable listener address (plaintext is restricted to loopback)
    #[arg(long, default_value = "127.0.0.1")]
    pub bind: IpAddr,
    /// Stable listener port
    #[arg(long, default_value_t = DEFAULT_HUB_PORT)]
    pub port: u16,
}

impl Default for HubServeArgs {
    fn default() -> Self {
        Self {
            bind: IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            port: DEFAULT_HUB_PORT,
        }
    }
}

pub fn execute(args: &HubArgs, cli: &Cli) -> Result<()> {
    match args
        .command
        .clone()
        .unwrap_or(HubCommands::Start(HubServeArgs::default()))
    {
        HubCommands::Start(serve) => start(&serve, cli),
        HubCommands::Serve(serve) => serve_foreground(&serve),
        HubCommands::Status => status(cli),
        HubCommands::Stop => stop(cli),
        HubCommands::Restart(serve) => {
            let _ = stop(cli);
            start(&serve, cli)
        }
    }
}

fn start(args: &HubServeArgs, cli: &Cli) -> Result<()> {
    let paths = HubRuntimePaths::default_for_user()?;
    if let Ok(record) = paths.read_process_record() {
        if record_is_live(&record) {
            anyhow::bail!(
                "cas hub is already running at http://{}:{} (pid {}, version {})",
                record.bind,
                record.port,
                record.pid,
                record.version
            );
        }
        paths.remove_process_record()?;
    }
    validate_control_bind(
        SocketAddr::new(args.bind, args.port),
        TransportSecurity::Plaintext,
    )?;

    crate::hub::ensure_private_dir(paths.root())?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.log_path())?;
    let error_log = log.try_clone()?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("hub")
        .arg("serve")
        .arg("--bind")
        .arg(args.bind.to_string())
        .arg("--port")
        .arg(args.port.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(error_log));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and runs in the child between fork and exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    command.spawn().context("spawn detached cas hub")?;

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(record) = paths.read_process_record() {
            if record_is_live(&record) {
                if cli.json {
                    println!("{}", serde_json::to_string(&record)?);
                } else {
                    println!(
                        "CAS hub started at http://{}:{} (pid {})",
                        record.bind, record.port, record.pid
                    );
                }
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!(
        "cas hub did not become ready; inspect {}",
        paths.log_path().display()
    )
}

fn serve_foreground(args: &HubServeArgs) -> Result<()> {
    let addr = SocketAddr::new(args.bind, args.port);
    validate_control_bind(addr, TransportSecurity::Plaintext)?;
    let paths = HubRuntimePaths::default_for_user()?;
    let _lock = paths.acquire_instance_lock()?;
    let machine = MachineIdentityStore::new(paths.root()).load_or_create()?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let actual = listener.local_addr()?;
        let record = HubProcessRecord {
            pid: std::process::id(),
            bind: actual.ip().to_string(),
            port: actual.port(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            started_at: chrono::Utc::now().to_rfc3339(),
        };
        paths.write_process_record(&record)?;

        let catalog = SessionCatalog::new(LocalSessionReadModel);
        let events = MachineEventBus::new(1024);
        let connector = DaemonConnector::new(
            SessionMultiplexer::new(DEFAULT_VIEWER_QUEUE_CAPACITY),
            events.clone(),
        );
        let state = HubState::new(
            catalog.clone(),
            Arc::new(PreAuthAuthorizer),
            machine,
            connector,
            events.clone(),
        );
        let event_catalog = catalog.clone();
        let event_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                interval.tick().await;
                if let Ok(sessions) = event_catalog.list().await {
                    events
                        .reconcile_sessions(sessions.into_iter().map(|session| session.name))
                        .await;
                }
            }
        });

        let result = axum::serve(listener, router(state))
            .with_graceful_shutdown(shutdown_signal())
            .await
            .context("Commander hub server failed");
        event_task.abort();
        paths.remove_process_record()?;
        result
    })
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn status(cli: &Cli) -> Result<()> {
    let paths = HubRuntimePaths::default_for_user()?;
    let record = paths.read_process_record()?;
    let live = record_is_live(&record);
    if cli.json {
        println!("{}", serde_json::json!({"running":live,"record":record}));
    } else if live {
        println!(
            "CAS hub is running at http://{}:{} (pid {}, version {})",
            record.bind, record.port, record.pid, record.version
        );
    } else {
        println!(
            "CAS hub is not running; stale record for pid {} remains",
            record.pid
        );
    }
    anyhow::ensure!(live, "cas hub is not running");
    Ok(())
}

fn stop(cli: &Cli) -> Result<()> {
    let paths = HubRuntimePaths::default_for_user()?;
    let record = paths.read_process_record()?;
    if record_is_live(&record) {
        #[cfg(unix)]
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(record.pid as i32),
            nix::sys::signal::Signal::SIGTERM,
        )?;
        #[cfg(windows)]
        Command::new("taskkill")
            .args(["/PID", &record.pid.to_string()])
            .status()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while process_is_running(record.pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        anyhow::ensure!(
            !process_is_running(record.pid),
            "cas hub did not stop cleanly"
        );
    }
    paths.remove_process_record()?;
    if cli.json {
        println!("{}", serde_json::json!({"stopped":true,"pid":record.pid}));
    } else {
        println!("CAS hub stopped (pid {})", record.pid);
    }
    Ok(())
}

fn process_is_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok()
    }
    #[cfg(not(unix))]
    {
        pid == std::process::id()
    }
}

fn record_is_live(record: &HubProcessRecord) -> bool {
    if !process_is_running(record.pid) {
        return false;
    }
    let url = format!("http://{}:{}/v1/health", record.bind, record.port);
    ureq::get(&url)
        .timeout(Duration::from_millis(500))
        .call()
        .ok()
        .and_then(|response| response.into_json::<serde_json::Value>().ok())
        .is_some_and(|health| {
            health
                .get("schema_version")
                .and_then(|value| value.as_u64())
                == Some(1)
                && health.get("ready").and_then(|value| value.as_bool()) == Some(true)
        })
}
