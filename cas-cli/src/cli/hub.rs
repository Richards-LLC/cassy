use std::fs::OpenOptions;
use std::net::{IpAddr, SocketAddr};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};

use crate::cli::Cli;
use crate::hub::{
    AuthStore, DEFAULT_HUB_PORT, DEFAULT_VIEWER_QUEUE_CAPACITY, DaemonConnector, HubProcessRecord,
    HubRuntimePaths, HubState, LocalSessionReadModel, MachineEventBus, MachineIdentityStore,
    MachineMetadata, MachineTransport, PreAuthAuthorizer, Scope, SessionCatalog,
    SessionMultiplexer, TailscaleServeManager, TransportSecurity, load_cloud_device_suggestions,
    router, validate_control_bind,
};

#[derive(Args, Debug, Clone)]
pub struct HubArgs {
    /// Publish the loopback hub through tailnet-only Tailscale Serve HTTPS
    #[arg(long, global = true)]
    pub tailscale_serve: bool,
    /// Tailscale Serve HTTPS port (443 is the stable no-port URL)
    #[arg(long, global = true, default_value_t = 443)]
    pub tailscale_serve_port: u16,
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
    /// Mint a ten-minute one-time browser pairing invitation
    Pair(HubPairArgs),
    /// List or revoke paired Commander devices
    Auth(HubAuthArgs),
}

#[derive(Args, Debug, Clone)]
pub struct HubPairArgs {
    /// Exact controller origin to authorize (scheme, host, and port)
    #[arg(long)]
    pub origin: String,
    /// Maximum scopes the pairing exchange may request
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "machine:read,session:read,pane:read"
    )]
    pub scopes: Vec<String>,
}

#[derive(Args, Debug, Clone)]
pub struct HubAuthArgs {
    #[command(subcommand)]
    pub command: HubAuthCommands,
}

#[derive(Subcommand, Debug, Clone)]
pub enum HubAuthCommands {
    /// List paired devices without credentials or key material
    List,
    /// Revoke a device immediately and disconnect its live sockets
    Revoke { device_id: String },
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
        HubCommands::Start(serve) => {
            start(&serve, cli, args.tailscale_serve, args.tailscale_serve_port)
        }
        HubCommands::Serve(serve) => {
            serve_foreground(&serve, args.tailscale_serve, args.tailscale_serve_port)
        }
        HubCommands::Status => status(cli),
        HubCommands::Stop => stop(cli),
        HubCommands::Restart(serve) => {
            let _ = stop(cli);
            start(&serve, cli, args.tailscale_serve, args.tailscale_serve_port)
        }
        HubCommands::Pair(pair) => pair_device(&pair, cli),
        HubCommands::Auth(auth) => manage_auth(&auth, cli),
    }
}

fn start(args: &HubServeArgs, cli: &Cli, tailscale_serve: bool, tailscale_port: u16) -> Result<()> {
    let paths = HubRuntimePaths::default_for_user()?;
    validate_control_bind(
        SocketAddr::new(args.bind, args.port),
        TransportSecurity::Plaintext,
    )?;
    crate::hub::ensure_private_dir(paths.root())?;
    if let Ok(record) = paths.read_process_record() {
        if record_is_live(&record) {
            let endpoint = record
                .public_url
                .clone()
                .unwrap_or_else(|| format!("http://{}:{}", record.bind, record.port));
            anyhow::bail!(
                "cas hub is already running at {} (pid {}, version {})",
                endpoint,
                record.pid,
                record.version
            );
        }
        paths.remove_process_record()?;
    }
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
    if tailscale_serve {
        command
            .arg("--tailscale-serve")
            .arg("--tailscale-serve-port")
            .arg(tailscale_port.to_string());
    }
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
                    let endpoint = record
                        .public_url
                        .as_deref()
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("http://{}:{}", record.bind, record.port));
                    println!("CAS hub started at {endpoint} (pid {})", record.pid);
                    if let Some(warning) = &record.transport_warning {
                        eprintln!(
                            "Tailscale Serve unavailable: {warning}; local hub remains healthy"
                        );
                    }
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

fn serve_foreground(args: &HubServeArgs, tailscale_serve: bool, tailscale_port: u16) -> Result<()> {
    let addr = SocketAddr::new(args.bind, args.port);
    validate_control_bind(addr, TransportSecurity::Plaintext)?;
    let paths = HubRuntimePaths::default_for_user()?;
    let _lock = paths.acquire_instance_lock()?;
    let machine = MachineIdentityStore::new(paths.root()).load_or_create()?;
    let auth = AuthStore::open(paths.root(), machine.id.clone())?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let actual = listener.local_addr()?;
        let (tailscale, transport_warning) = if tailscale_serve {
            match TailscaleServeManager::new(paths.root()).ensure(actual.port(), tailscale_port) {
                Ok(receipt) => (Some(receipt), None),
                Err(error) => {
                    let warning = error.to_string();
                    tracing::warn!(%warning, "Tailscale Serve refused; keeping Commander loopback-only");
                    (None, Some(warning))
                }
            }
        } else {
            (None, None)
        };
        let record = HubProcessRecord {
            pid: std::process::id(),
            bind: actual.ip().to_string(),
            port: actual.port(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            started_at: chrono::Utc::now().to_rfc3339(),
            public_url: tailscale.as_ref().map(|receipt| receipt.public_url.clone()),
            tailscale_serve_port: tailscale.as_ref().map(|receipt| receipt.https_port),
            transport_warning,
        };
        paths.write_process_record(&record)?;

        let catalog = SessionCatalog::new(LocalSessionReadModel);
        let events = MachineEventBus::new(1024);
        let connector = DaemonConnector::new(
            SessionMultiplexer::new(DEFAULT_VIEWER_QUEUE_CAPACITY),
            events.clone(),
        );
        let metadata = MachineMetadata {
            transport: MachineTransport {
                kind: if tailscale.is_some() {
                    "tailscale_serve".to_owned()
                } else {
                    "loopback".to_owned()
                },
                public_url: tailscale.as_ref().map(|receipt| receipt.public_url.clone()),
            },
            cloud_devices: load_cloud_device_suggestions(),
        };
        let mut state = HubState::new(
            catalog.clone(),
            Arc::new(PreAuthAuthorizer),
            machine,
            connector,
            events.clone(),
        )
        .with_auth(auth)
        .with_effective_origin(format!("http://{actual}"))
        .with_machine_metadata(metadata);
        if let Some(public_url) = tailscale.as_ref().map(|receipt| &receipt.public_url) {
            state = state.with_effective_origin(public_url.trim_end_matches('/'));
        }
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

fn auth_store() -> Result<AuthStore> {
    let paths = HubRuntimePaths::default_for_user()?;
    let machine = MachineIdentityStore::new(paths.root()).load_or_create()?;
    AuthStore::open(paths.root(), machine.id)
}

fn pair_device(args: &HubPairArgs, cli: &Cli) -> Result<()> {
    let scopes = args
        .scopes
        .iter()
        .map(|scope| Scope::parse(scope))
        .collect::<Result<_>>()?;
    let invitation = auth_store()?.mint_pairing(&args.origin, scopes, chrono::Utc::now())?;
    if cli.json {
        println!(
            "{}",
            serde_json::json!({
                "url": invitation.url,
                "expires_at": invitation.expires_at,
                "scopes": invitation.scopes,
            })
        );
    } else {
        println!(
            "Pair Commander before {}",
            invitation.expires_at.to_rfc3339()
        );
        println!("{}", invitation.url);
        let code = qrcode::QrCode::new(invitation.url.as_bytes())?;
        println!(
            "{}",
            code.render::<qrcode::render::unicode::Dense1x2>()
                .quiet_zone(true)
                .build()
        );
    }
    Ok(())
}

fn manage_auth(args: &HubAuthArgs, cli: &Cli) -> Result<()> {
    let auth = auth_store()?;
    match &args.command {
        HubAuthCommands::List => {
            let devices = auth.list_devices()?;
            if cli.json {
                println!("{}", serde_json::to_string(&devices)?);
            } else if devices.is_empty() {
                println!("No paired Commander devices");
            } else {
                for device in devices {
                    println!(
                        "{}  {} / {}  {}  {}",
                        device.device_id,
                        device.operator_label,
                        device.device_label,
                        device.controller_origin,
                        if device.revoked_at.is_some() {
                            "revoked"
                        } else {
                            "active"
                        }
                    );
                }
            }
        }
        HubAuthCommands::Revoke { device_id } => {
            auth.revoke_device(device_id, chrono::Utc::now())?;
            if cli.json {
                println!("{}", serde_json::json!({"revoked":device_id}));
            } else {
                println!("Revoked Commander device {device_id}");
            }
        }
    }
    Ok(())
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
        let endpoint = record
            .public_url
            .as_deref()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("http://{}:{}", record.bind, record.port));
        println!(
            "CAS hub is running at {} (pid {}, version {})",
            endpoint, record.pid, record.version
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
    let record = paths.read_process_record().ok();
    if record.as_ref().is_some_and(record_is_live) {
        let record = record.as_ref().expect("live record exists");
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
    let tailscale_result = TailscaleServeManager::new(paths.root()).disable_owned();
    paths.remove_process_record()?;
    if cli.json {
        println!(
            "{}",
            serde_json::json!({
                "stopped":true,
                "pid":record.as_ref().map(|record| record.pid),
                "tailscale_serve_removed":matches!(&tailscale_result, Ok(Some(_))),
                "tailscale_warning":tailscale_result.as_ref().err().map(ToString::to_string),
            })
        );
    } else {
        if let Some(record) = &record {
            println!("CAS hub stopped (pid {})", record.pid);
        } else {
            println!("CAS hub was not running");
        }
        match tailscale_result {
            Ok(Some(receipt)) => println!(
                "Removed CAS Tailscale Serve mapping at {}",
                receipt.public_url
            ),
            Ok(None) => {}
            Err(error) => eprintln!("Tailscale Serve mapping left untouched: {error}"),
        }
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
