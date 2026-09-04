use std::fs::OpenOptions;
use std::future::IntoFuture;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};

use crate::ai_enrichment::HttpAiEnrichmentProvider;
use crate::cli::Cli;
use crate::config::Config;
use crate::hub::{
    AuthStore, DEFAULT_HUB_PORT, DEFAULT_VIEWER_QUEUE_CAPACITY, DaemonConnector, HubProcessRecord,
    HubRuntimePaths, HubState, LocalSessionReadModel, MachineEventBus, MachineIdentityStore,
    MachineMetadata, MachineTransport, PreAuthAuthorizer, Scope, SessionCatalog,
    SessionMultiplexer, TailscaleServeManager, TransportSecurity, load_cloud_device_suggestions,
    router, spawn_attention_enricher, validate_control_bind,
};

const HUB_LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(10);
const HUB_CONNECTION_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const HUB_LAUNCH_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HubLaunchOrigin {
    Cli,
    Update,
    Worker,
}

impl HubLaunchOrigin {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Update => "update",
            Self::Worker => "worker",
        }
    }
}

fn cli_launch_origin() -> HubLaunchOrigin {
    if std::env::var("CAS_AGENT_ROLE").ok().as_deref() == Some("worker") {
        HubLaunchOrigin::Worker
    } else {
        HubLaunchOrigin::Cli
    }
}

/// A hub launched by a factory worker must leave both worker containment
/// tiers. The process-group tier is handled by `setsid`; the cgroup tier needs
/// the shared-server sibling scope and a pre-exec barrier.
fn factory_worker_session() -> Option<String> {
    if std::env::var("CAS_AGENT_ROLE").ok().as_deref() != Some("worker") {
        return None;
    }
    std::env::var("CAS_FACTORY_SESSION")
        .ok()
        .filter(|session| !session.trim().is_empty())
}

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
    /// Install, inspect, or remove boot-persistent hub supervision
    Service(HubServiceArgs),
    /// Mint a ten-minute one-time browser pairing invitation
    Pair(HubPairArgs),
    /// Approve a Commander page's short-code pairing request through Petra Stella Cloud
    Authorize(HubAuthorizeArgs),
    /// List or revoke paired Commander devices
    Auth(HubAuthArgs),
}

#[derive(Args, Debug, Clone)]
pub struct HubServiceArgs {
    #[command(subcommand)]
    pub command: HubServiceCommands,
}

#[derive(Args, Debug, Clone, Default)]
pub struct HubServiceInstallArgs {
    /// Preview the service definition and manager actions without changing the host
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Subcommand, Debug, Clone)]
pub enum HubServiceCommands {
    /// Install and start a user-level service for the loopback hub
    Install(HubServiceInstallArgs),
    /// Report service-manager supervision alongside hub health
    Status,
    /// Stop and remove service-manager supervision without touching hub identity or auth
    Uninstall,
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
pub struct HubAuthorizeArgs {
    /// Eight-character code displayed by Commander (for example K7MW-4H2Q)
    pub code: String,
    /// Reduce the page-requested scopes; may never add a scope
    #[arg(long, value_delimiter = ',')]
    pub scopes: Option<Vec<String>>,
    /// Public hub URL when the hub record has no Tailscale Serve URL
    #[arg(long)]
    pub hub_url: Option<String>,
    /// Approve without an interactive confirmation prompt
    #[arg(long)]
    pub yes: bool,
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
    /// Internal provenance for the durable process record.
    #[arg(long, hide = true, default_value = "cli")]
    pub launched_by: String,
    /// Internal timestamp captured by the detached launcher.
    #[arg(long, hide = true)]
    pub launched_at: Option<String>,
    /// Cassy-created cgroup scope passed through the detached launch barrier.
    #[arg(long, hide = true)]
    pub cgroup: Option<std::path::PathBuf>,
}

impl Default for HubServeArgs {
    fn default() -> Self {
        Self {
            bind: IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            port: DEFAULT_HUB_PORT,
            launched_by: "cli".to_owned(),
            launched_at: None,
            cgroup: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HubStartDecision {
    Keep,
    Restart {
        version_drift: bool,
        flags_differ: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HubRestartSpec {
    bind: IpAddr,
    port: u16,
    tailscale_serve: bool,
    tailscale_port: u16,
}

fn default_hub_command() -> HubCommands {
    HubCommands::Status
}

fn tailscale_enabled(record: &HubProcessRecord) -> bool {
    record.tailscale_cli.is_some()
        || record.tailscale_serve_port.is_some()
        || record.public_url.is_some()
}

fn decide_live_start(
    record: &HubProcessRecord,
    args: &HubServeArgs,
    tailscale_serve: bool,
    tailscale_port: u16,
    binary_version: &str,
) -> HubStartDecision {
    let version_drift = record.version != binary_version;
    let tailscale_flags_differ = tailscale_enabled(record) != tailscale_serve
        || (tailscale_serve
            && record
                .tailscale_serve_port
                .is_some_and(|port| port != tailscale_port));
    let flags_differ = record.bind != args.bind.to_string()
        || record.port != args.port
        || tailscale_flags_differ;

    if version_drift || flags_differ {
        HubStartDecision::Restart {
            version_drift,
            flags_differ,
        }
    } else {
        HubStartDecision::Keep
    }
}

/// Does an already-running hub satisfy the launch we were about to perform?
///
/// cas-bf90. Two concurrent lifecycle commands (`hub start` and `hub restart`)
/// both stop the old hub and both try to launch a replacement. Whichever
/// arrives second then waits on the machine lock — but the winner's brand-new
/// hub legitimately *holds* that lock, so the loser's wait condition can never
/// become true. It burned the full [`HUB_LIFECYCLE_TIMEOUT`] and then reported
/// failure, even though the state it wanted ("a hub matching my flags is
/// running") had already been reached. Measured before this predicate existed:
/// the losing command failed in ~80% of concurrent iterations, always after a
/// full 10 s stall.
///
/// The waiters asked "is the lock free?" when what they care about is "is a
/// satisfying hub live?". This answers the second question.
///
/// Deliberately NOT [`decide_live_start`]: that function drives whether to
/// restart, and its `record.port != args.port` comparison is right there and
/// must not change. Here an ephemeral request (`--port 0`) means "any port is
/// acceptable", so a hub on a kernel-assigned port does satisfy it — otherwise
/// this predicate could never be true for the `--port 0` callers that hit the
/// race most often.
fn running_hub_satisfies_request(
    record: &HubProcessRecord,
    args: &HubServeArgs,
    tailscale_serve: bool,
    tailscale_port: u16,
    binary_version: &str,
) -> bool {
    if record.version != binary_version || record.bind != args.bind.to_string() {
        return false;
    }
    // Port 0 asks the kernel to choose; any concrete port honours that request.
    if args.port != 0 && record.port != args.port {
        return false;
    }
    if tailscale_enabled(record) != tailscale_serve {
        return false;
    }
    // A specific Serve port was requested: the live hub must be using it.
    if tailscale_serve
        && record
            .tailscale_serve_port
            .is_some_and(|port| port != tailscale_port)
    {
        return false;
    }
    true
}

/// What the caller intends to do once the hub is stopped.
///
/// cas-bf90. `stop_with_output` serves two very different callers. A plain
/// `cas hub stop` demands the machine actually end up without a hub, so a live
/// hub appearing mid-wait is a reason to keep waiting, never a success. A
/// stop-to-relaunch (`hub restart`, or `hub start` when flags differ) only
/// wants "a hub with these flags is running" — and a concurrent command may
/// have produced exactly that while we waited. Passing the intent explicitly
/// keeps those two meanings apart instead of overloading one wait.
struct RelaunchIntent<'a> {
    args: &'a HubServeArgs,
    tailscale_serve: bool,
    tailscale_port: u16,
}

/// How a stop resolved.
enum StopOutcome {
    /// The hub is gone and the machine is quiescent.
    Stopped,
    /// A concurrent command already produced the hub the caller was going to
    /// relaunch. Only reachable with a [`RelaunchIntent`].
    ///
    /// The caller MUST return success directly rather than continuing into its
    /// relaunch: the teardown steps after the wait (`remove_process_record`,
    /// cgroup kill, Tailscale disable) would otherwise dismantle a healthy hub
    /// this command does not own.
    AlreadySatisfied(Box<HubProcessRecord>),
}

/// Wait for the machine to go quiescent, unless the relaunch is already done.
///
/// `settle_pid` is the hub we just signalled: quiescence additionally requires
/// that process to be gone. `stale_pid` is never accepted as satisfying — the
/// hub we are trying to replace must not be mistaken for the replacement.
fn wait_for_stop_or_satisfying_hub(
    paths: &HubRuntimePaths,
    settle_pid: Option<u32>,
    stale_pid: Option<u32>,
    timeout: Duration,
    relaunch: Option<&RelaunchIntent<'_>>,
) -> Result<Option<HubProcessRecord>> {
    let deadline = Instant::now() + timeout;
    loop {
        let lock = paths.try_acquire_instance_lock()?;
        let quiescent = lock.is_some()
            && settle_pid.is_none_or(|pid| !process_is_running(pid));
        drop(lock);
        if quiescent {
            return Ok(None);
        }
        if let Some(intent) = relaunch
            && let Ok(record) = paths.read_process_record()
            && stale_pid != Some(record.pid)
            && running_hub_satisfies_request(
                &record,
                intent.args,
                intent.tailscale_serve,
                intent.tailscale_port,
                env!("CARGO_PKG_VERSION"),
            )
            && record_is_live(&record)
        {
            return Ok(Some(record));
        }
        if Instant::now() >= deadline {
            return match settle_pid {
                Some(pid) => anyhow::bail!(
                    "cas hub pid {pid} or its machine lock remained live after {:.1}s; no replacement was started",
                    timeout.as_secs_f64()
                ),
                None => anyhow::bail!(
                    "cas hub machine lock remained held after {:.1}s; the old instance may still be shutting down and no replacement was started",
                    timeout.as_secs_f64()
                ),
            };
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// How a launch attempt resolved its race with a concurrent lifecycle command.
enum LaunchWait {
    /// We hold the machine lock and are responsible for launching.
    Acquired(crate::hub::HubInstanceLock),
    /// Someone else already launched the hub we wanted; nothing left to do.
    AlreadySatisfied(Box<HubProcessRecord>),
}

/// Wait for the machine lock, but stop early if the state we wanted arrives.
///
/// cas-bf90. [`crate::hub::HubRuntimePaths::wait_for_instance_lock`] asks only
/// "is the lock free?". When a concurrent `hub start`/`hub restart` has just
/// launched a healthy replacement, that replacement holds the lock for its
/// whole life, so the question can never become true and the caller fails after
/// the full timeout — despite the outcome it wanted already existing. This asks
/// the question the caller actually cares about alongside the lock.
///
/// The liveness probe is deliberately last: it is an HTTP health call, so the
/// cheap record/flag comparison rejects non-matching hubs first, and the probe
/// only runs while we are genuinely contended.
fn wait_for_lock_or_satisfying_hub(
    paths: &HubRuntimePaths,
    timeout: Duration,
    args: &HubServeArgs,
    tailscale_serve: bool,
    tailscale_port: u16,
) -> Result<LaunchWait> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(lock) = paths.try_acquire_instance_lock()? {
            return Ok(LaunchWait::Acquired(lock));
        }
        if let Ok(record) = paths.read_process_record()
            && running_hub_satisfies_request(
                &record,
                args,
                tailscale_serve,
                tailscale_port,
                env!("CARGO_PKG_VERSION"),
            )
            && record_is_live(&record)
        {
            return Ok(LaunchWait::AlreadySatisfied(Box::new(record)));
        }
        if Instant::now() >= deadline {
            // Distinct from the runtime's generic lock-wait message on purpose:
            // three separate sites could previously emit identical text, so an
            // operator (or a test) could not tell which wait actually expired.
            anyhow::bail!(
                "cas hub launch waiter: machine lock remained held after {:.1}s and no running hub matched the requested flags; no replacement was started",
                timeout.as_secs_f64()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn restart_spec_for_record(
    record: &HubProcessRecord,
    binary_version: &str,
) -> Result<Option<HubRestartSpec>> {
    if record.version == binary_version {
        return Ok(None);
    }
    Ok(Some(HubRestartSpec {
        bind: record
            .bind
            .parse()
            .with_context(|| format!("invalid bind address in hub record: {}", record.bind))?,
        port: record.port,
        tailscale_serve: tailscale_enabled(record),
        tailscale_port: record.tailscale_serve_port.unwrap_or(443),
    }))
}

fn render_status(record: &HubProcessRecord, live: bool, binary_version: &str) -> String {
    if live {
        let endpoint = record
            .public_url
            .as_deref()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("http://{}:{}", record.bind, record.port));
        format!(
            "Cassy hub is running at {endpoint} (pid {}, version {}, binary: {binary_version})",
            record.pid, record.version
        )
    } else {
        format!(
            "Cassy hub is not running (last pid {} exited; started by {} at {}) \
             (version {}, binary: {binary_version})",
            record.pid,
            record.launched_by.as_deref().unwrap_or("unknown"),
            record
                .launched_at
                .as_deref()
                .unwrap_or(&record.started_at),
            record.version,
        )
    }
}

pub fn execute(args: &HubArgs, cli: &Cli) -> Result<()> {
    match args
        .command
        .clone()
        .unwrap_or_else(default_hub_command)
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
            // `hub restart` is a stop-to-relaunch, so its stop carries the
            // intent: if a concurrent lifecycle command already produced a hub
            // with these flags, the restart's goal is met and waiting out the
            // machine lock would be exactly the cas-bf90 stall. Plain
            // `hub stop` still goes through stop(), which passes no intent.
            match stop_with_output(
                cli,
                true,
                Some(RelaunchIntent {
                    args: &serve,
                    tailscale_serve: args.tailscale_serve,
                    tailscale_port: args.tailscale_serve_port,
                }),
            )? {
                StopOutcome::AlreadySatisfied(record) => {
                    if cli.json {
                        println!("{}", serde_json::to_string(&record)?);
                    } else {
                        println!(
                            "hub already running (pid {}, version {}) — a concurrent start won the race",
                            record.pid, record.version
                        );
                    }
                    Ok(())
                }
                StopOutcome::Stopped => {
                    start(&serve, cli, args.tailscale_serve, args.tailscale_serve_port)
                }
            }
        }
        HubCommands::Service(service) => super::hub_service::manage_service(
            &service.command,
            cli,
            args.tailscale_serve,
            args.tailscale_serve_port,
        ),
        HubCommands::Pair(pair) => pair_device(&pair, cli),
        HubCommands::Authorize(authorize) => super::hub_reverse_pairing::authorize(&authorize, cli),
        HubCommands::Auth(auth) => manage_auth(&auth, cli),
    }
}

fn start(args: &HubServeArgs, cli: &Cli, tailscale_serve: bool, tailscale_port: u16) -> Result<()> {
    start_with_output_from(
        args,
        cli,
        tailscale_serve,
        tailscale_port,
        true,
        cli_launch_origin(),
    )
}

fn start_with_output_from(
    args: &HubServeArgs,
    cli: &Cli,
    tailscale_serve: bool,
    tailscale_port: u16,
    emit_output: bool,
    launch_origin: HubLaunchOrigin,
) -> Result<()> {
    let paths = HubRuntimePaths::default_for_user()?;
    validate_control_bind(
        SocketAddr::new(args.bind, args.port),
        TransportSecurity::Plaintext,
    )?;
    crate::hub::ensure_private_dir(paths.root())?;
    match paths.read_process_record() {
        Ok(record) if record_is_live(&record) => {
            match decide_live_start(
                &record,
                args,
                tailscale_serve,
                tailscale_port,
                env!("CARGO_PKG_VERSION"),
            ) {
                HubStartDecision::Keep => {
                    if emit_output {
                        if cli.json {
                            println!(
                                "{}",
                                serde_json::json!({
                                    "running": true,
                                    "record": record,
                                    "binary": env!("CARGO_PKG_VERSION"),
                                })
                            );
                        } else {
                            println!(
                                "{}",
                                render_status(&record, true, env!("CARGO_PKG_VERSION"))
                            );
                        }
                    }
                    return Ok(());
                }
                HubStartDecision::Restart {
                    version_drift,
                    flags_differ,
                } => {
                    if emit_output && !cli.json {
                        let detail = match (version_drift, flags_differ) {
                            (true, true) => format!(
                                "binary is {} / flags differ",
                                env!("CARGO_PKG_VERSION")
                            ),
                            (true, false) => {
                                format!("binary is {}", env!("CARGO_PKG_VERSION"))
                            }
                            (false, true) => "flags differ".to_owned(),
                            (false, false) => unreachable!("restart requires drift"),
                        };
                        println!(
                            "hub running (pid {}, version {}) — {detail}; restarting…",
                            record.pid, record.version
                        );
                    }
                    if let StopOutcome::AlreadySatisfied(record) = stop_with_output(
                        cli,
                        emit_output,
                        Some(RelaunchIntent {
                            args,
                            tailscale_serve,
                            tailscale_port,
                        }),
                    )? {
                        if emit_output && cli.json {
                            println!("{}", serde_json::to_string(&record)?);
                        } else if emit_output {
                            println!(
                                "hub already running (pid {}, version {}) — a concurrent start won the race",
                                record.pid, record.version
                            );
                        }
                        return Ok(());
                    }
                    return start_with_output_from(
                        args,
                        cli,
                        tailscale_serve,
                        tailscale_port,
                        emit_output,
                        launch_origin,
                    );
                }
            }
        }
        Ok(_) | Err(_) => {}
    }
    // A missing/stale PID record is not ownership evidence. Acquire the
    // authoritative machine lock before cleaning stale state or launching a
    // replacement, then release it immediately before the child takes over.
    //
    // cas-bf90: race a concurrent lifecycle command rather than only the lock.
    // If that command's replacement hub is already up and satisfies what we
    // were about to launch, we are done — waiting out the full timeout on a
    // lock its healthy hub legitimately holds produced a guaranteed stall and a
    // spurious failure.
    let launch_guard = match wait_for_lock_or_satisfying_hub(
        &paths,
        HUB_LIFECYCLE_TIMEOUT,
        args,
        tailscale_serve,
        tailscale_port,
    )? {
        LaunchWait::Acquired(lock) => lock,
        LaunchWait::AlreadySatisfied(record) => {
            if emit_output && cli.json {
                println!("{}", serde_json::to_string(&record)?);
            } else if emit_output {
                println!(
                    "hub already running (pid {}, version {}) — a concurrent start won the race",
                    record.pid, record.version
                );
            }
            return Ok(());
        }
    };
    let stale_cgroup = paths
        .read_process_record()
        .ok()
        .and_then(|record| record.cgroup);
    // A killed hub cannot tear down its owned proxy. Once exclusive ownership
    // is proven, remove only the exact unchanged mapping described by its
    // private receipt; this also recovers record-absent abrupt deaths.
    let _ = TailscaleServeManager::new(paths.root()).disable_owned();
    if let Some(cgroup) = stale_cgroup {
        let _ = crate::ui::factory::cgroup::kill_scope(&cgroup);
        crate::ui::factory::cgroup::remove_scope(&cgroup);
    }
    paths.remove_process_record()?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.log_path())?;
    let error_log = log.try_clone()?;
    let launched_at = chrono::Utc::now().to_rfc3339();
    let stamp = chrono::Utc::now().timestamp_micros();
    let launcher_pid_file = paths
        .root()
        .join(format!(".launcher-{stamp}-{}.pid", std::process::id()));
    let launch_file = paths
        .root()
        .join(format!(".launcher-{stamp}-{}.go", std::process::id()));
    let _launcher_pid_file_guard = ScopedFile::new(launcher_pid_file.clone());
    let _launch_file_guard = ScopedFile::new(launch_file.clone());

    // The shell is only a barrier launcher. It is placed in its final cgroup
    // before it forks or execs anything, then execs the hub in the same fresh
    // session. Thus the recorded hub pid is also the session/process-group
    // leader and worker cgroup teardown cannot reach it.
    #[cfg(unix)]
    let launcher_script = r#"printf '%s' "$$" > "$1"; while [ ! -f "$2" ]; do /bin/sleep 0.01; done; cgroup=; IFS= read -r cgroup < "$2" || :; shift 2; if [ -n "$cgroup" ]; then set -- "$@" --cgroup "$cgroup"; fi; exec "$0" "$@""#;
    #[cfg(not(unix))]
    let launcher_script = r#"printf '%s' "$$" > "$1"; while [ ! -f "$2" ]; do sleep 0.01; done; cgroup=; IFS= read -r cgroup < "$2" || :; shift 2; if [ -n "$cgroup" ]; then set -- "$@" --cgroup "$cgroup"; fi; exec "$0" "$@""#;
    let executable = std::env::current_exe()?;
    #[cfg(unix)]
    let mut command = Command::new("/bin/sh");
    #[cfg(not(unix))]
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(launcher_script)
        .arg(&executable)
        .arg(&launcher_pid_file)
        .arg(&launch_file)
        .arg("hub")
        .arg("serve")
        .arg("--bind")
        .arg(args.bind.to_string())
        .arg("--port")
        .arg(args.port.to_string())
        .arg("--launched-by")
        .arg(launch_origin.as_str())
        .arg("--launched-at")
        .arg(&launched_at)
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
        // SAFETY: setsid is async-signal-safe and runs in the child between
        // fork and exec. The shell then execs the hub without forking again.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command.spawn().context("spawn detached cas hub launcher")?;
    let launcher_pid = match read_published_pid(&launcher_pid_file) {
        Ok(pid) => pid,
        Err(error) => {
            terminate_failed_launch(&mut child, None);
            return Err(error);
        }
    };
    let cgroup = factory_worker_session().and_then(|session| {
        crate::ui::factory::cgroup::join_shared_scope(&session, "hub", launcher_pid)
    });
    drop(launch_guard);
    let launch_metadata = cgroup
        .as_deref()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Err(error) = std::fs::write(&launch_file, launch_metadata) {
        terminate_failed_launch(&mut child, cgroup.as_deref());
        return Err(error.into());
    }

    let deadline = Instant::now() + HUB_LAUNCH_TIMEOUT;
    while Instant::now() < deadline {
        if let Ok(record) = paths.read_process_record() {
            if record_is_live(&record) {
                if emit_output && cli.json {
                    println!("{}", serde_json::to_string(&record)?);
                } else if emit_output {
                    let endpoint = record
                        .public_url
                        .as_deref()
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("http://{}:{}", record.bind, record.port));
                    println!("Cassy hub started at {endpoint} (pid {})", record.pid);
                    if let Some(warning) = &record.transport_warning {
                        eprintln!(
                            "Tailscale Serve unavailable: {warning}; local hub remains healthy"
                        );
                    }
                }
                return Ok(());
            }
        }
        if let Some(status) = child.try_wait().context("poll detached cas hub")? {
            if let Ok(record) = paths.read_process_record()
                && record_is_live(&record)
            {
                anyhow::bail!(
                    "another cas hub instance won the machine lock (pid {}); replacement process exited with {}",
                    record.pid,
                    status
                );
            }
            let error = anyhow::anyhow!(
                "cas hub replacement exited with {status} before becoming ready; inspect {}",
                paths.log_path().display()
            );
            terminate_failed_launch(&mut child, cgroup.as_deref());
            return Err(error);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let error = anyhow::anyhow!(
        "cas hub did not become ready; inspect {}",
        paths.log_path().display()
    );
    terminate_failed_launch(&mut child, cgroup.as_deref());
    Err(error)
}

struct ScopedFile(std::path::PathBuf);

impl ScopedFile {
    fn new(path: std::path::PathBuf) -> Self {
        Self(path)
    }
}

impl Drop for ScopedFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn read_published_pid(path: &std::path::Path) -> Result<u32> {
    let deadline = Instant::now() + HUB_LAUNCH_TIMEOUT;
    loop {
        if let Ok(contents) = std::fs::read_to_string(path)
            && let Ok(pid) = contents.trim().parse::<u32>()
        {
            return Ok(pid);
        }
        if Instant::now() >= deadline {
            anyhow::bail!("hub launcher never published its pid");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn terminate_failed_launch(
    child: &mut std::process::Child,
    cgroup: Option<&std::path::Path>,
) {
    if let Some(cgroup) = cgroup {
        let _ = crate::ui::factory::cgroup::kill_scope(cgroup);
    }
    let _ = child.kill();
    let _ = child.wait();
    if let Some(cgroup) = cgroup {
        crate::ui::factory::cgroup::remove_scope(cgroup);
    }
}

fn serve_foreground(args: &HubServeArgs, tailscale_serve: bool, tailscale_port: u16) -> Result<()> {
    let addr = SocketAddr::new(args.bind, args.port);
    validate_control_bind(addr, TransportSecurity::Plaintext)?;
    let paths = HubRuntimePaths::default_for_user()?;
    let _lock = paths.acquire_instance_lock()?;
    let machine = MachineIdentityStore::new(paths.root()).load_or_create()?;
    let auth = AuthStore::open(paths.root(), machine.id.clone())?;
    // Commander hub is machine-scoped, so its one AI-enrichment opt-in comes
    // from the host config rather than whichever project launched the daemon.
    let ai_enrichment = dirs::home_dir()
        .map(|home| {
            Config::load(&home.join(".cas"))
                .unwrap_or_default()
                .factory()
                .ai_enrichment
        })
        .unwrap_or_default();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let launched_by = args.launched_by.clone();
    let launched_at = args.launched_at.clone();
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let actual = listener.local_addr()?;
        let tailscale_manager = TailscaleServeManager::new(paths.root());
        let (tailscale_listener, tailscale, transport_warning) = if tailscale_serve {
            let proxy_listener =
                tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
            let proxy_port = proxy_listener.local_addr()?.port();
            match tailscale_manager.ensure(proxy_port, tailscale_port) {
                Ok(receipt) => (Some(proxy_listener), Some(receipt), None),
                Err(error) => {
                    let warning = error.to_string();
                    tracing::warn!(%warning, "Tailscale Serve refused; keeping Commander loopback-only");
                    (None, None, Some(warning))
                }
            }
        } else {
            (None, None, None)
        };
        let started_at = chrono::Utc::now().to_rfc3339();
        let record = HubProcessRecord {
            pid: std::process::id(),
            sid: current_session_id(),
            pgid: current_process_group_id(),
            bind: actual.ip().to_string(),
            port: actual.port(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            started_at: started_at.clone(),
            cgroup: args.cgroup.clone(),
            launched_by: Some(launched_by),
            launched_at: Some(launched_at.unwrap_or(started_at)),
            public_url: tailscale.as_ref().map(|receipt| receipt.public_url.clone()),
            tailscale_serve_port: tailscale.as_ref().map(|receipt| receipt.https_port),
            tailscale_cli: tailscale_serve.then(|| tailscale_manager.executable_display()),
            transport_warning,
        };
        if let Err(error) = paths.write_process_record(&record) {
            let _ = tailscale_manager.disable_owned();
            return Err(error);
        }

        let catalog = SessionCatalog::new(LocalSessionReadModel);
        let events = MachineEventBus::open(1024, paths.events_path())?;
        let attention_task = ai_enrichment.enabled.then(|| {
            let receiver = events.enable_enrichment();
            spawn_attention_enricher(
                events.clone(),
                receiver,
                Arc::new(HttpAiEnrichmentProvider::new(ai_enrichment.clone())),
            )
        });
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

        let result = if let Some(proxy_listener) = tailscale_listener {
            serve_with_trusted_tls_proxy(listener, state, proxy_listener).await
        } else {
            serve_with_bounded_connection_drain(listener, router(state)).await
        };
        event_task.abort();
        if let Some(task) = attention_task {
            task.abort();
        }
        paths.remove_process_record()?;
        #[cfg(debug_assertions)]
        hold_instance_lock_after_record_removal_for_test()?;
        // A service manager stops a foreground hub with SIGTERM instead of
        // routing through `cas hub stop`. Always tear down only Cassy's exact
        // owned mapping here so an uninstall/reboot cannot leave a stale
        // Tailscale Serve publication behind. A restart republishes it from
        // the same private receipt and keeps machine identity/auth untouched.
        let _ = tailscale_manager.disable_owned();
        result
    })
}

async fn serve_with_bounded_connection_drain(
    listener: tokio::net::TcpListener,
    app: axum::Router,
) -> Result<()> {
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let server = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_requested(shutdown_rx))
        .into_future();
    tokio::pin!(server);

    tokio::select! {
        result = &mut server => result.context("Commander hub server failed"),
        _ = shutdown_signal() => {
            let _ = shutdown_tx.send(true);
            match tokio::time::timeout(HUB_CONNECTION_DRAIN_TIMEOUT, &mut server).await {
                Ok(result) => result.context("Commander hub server failed"),
                Err(_) => {
                    // Axum's graceful shutdown deliberately waits for upgraded
                    // WebSockets and other active requests forever. Returning
                    // drops the server future; the enclosing runtime then closes
                    // those tasks so Commander clients observe a non-normal close
                    // and reconnect to the replacement hub.
                    tracing::warn!(
                        timeout_seconds = HUB_CONNECTION_DRAIN_TIMEOUT.as_secs_f64(),
                        "Commander connection drain expired; force-closing live clients"
                    );
                    Ok(())
                }
            }
        }
    }
}

#[cfg(debug_assertions)]
fn hold_instance_lock_after_record_removal_for_test() -> Result<()> {
    use std::fs;
    use std::path::PathBuf;

    let Some(root) = std::env::var_os("CAS_TEST_HUB_LOCK_RELEASE_BARRIER") else {
        return Ok(());
    };
    let root = PathBuf::from(root);
    fs::create_dir_all(&root)?;
    let claim = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join("claimed"));
    match claim {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    fs::write(root.join("record-removed-lock-held"), b"ready\n")?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while !root.join("release").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    anyhow::ensure!(
        root.join("release").exists(),
        "test hub lock-release barrier timed out"
    );
    Ok(())
}

async fn serve_with_trusted_tls_proxy<R: crate::hub::SessionReadModel>(
    plaintext_listener: tokio::net::TcpListener,
    state: HubState<R>,
    trusted_proxy_listener: tokio::net::TcpListener,
) -> Result<()> {
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let plaintext = axum::serve(plaintext_listener, router(state.clone()))
        .with_graceful_shutdown(shutdown_requested(shutdown_rx.clone()))
        .into_future();
    let trusted_proxy = axum::serve(
        trusted_proxy_listener,
        router(state.with_response_transport(TransportSecurity::TrustedLoopbackTlsProxy)),
    )
    .with_graceful_shutdown(shutdown_requested(shutdown_rx))
    .into_future();
    tokio::pin!(plaintext);
    tokio::pin!(trusted_proxy);

    enum FirstExit {
        Shutdown,
        Plaintext(std::io::Result<()>),
        TrustedProxy(std::io::Result<()>),
    }
    let first = tokio::select! {
        _ = shutdown_signal() => FirstExit::Shutdown,
        result = &mut plaintext => FirstExit::Plaintext(result),
        result = &mut trusted_proxy => FirstExit::TrustedProxy(result),
    };
    let _ = shutdown_tx.send(true);

    match first {
        FirstExit::Shutdown => {
            match tokio::time::timeout(HUB_CONNECTION_DRAIN_TIMEOUT, async {
                tokio::join!(&mut plaintext, &mut trusted_proxy)
            })
            .await
            {
                Ok((plaintext, trusted_proxy)) => {
                    plaintext.context("Commander loopback listener failed")?;
                    trusted_proxy.context("Commander trusted proxy listener failed")?;
                }
                Err(_) => {
                    tracing::warn!(
                        timeout_seconds = HUB_CONNECTION_DRAIN_TIMEOUT.as_secs_f64(),
                        "Commander connection drain expired; force-closing live clients"
                    );
                }
            }
            Ok(())
        }
        FirstExit::Plaintext(result) => {
            let _ = trusted_proxy.await;
            result.context("Commander loopback listener failed")?;
            anyhow::bail!("Commander loopback listener exited unexpectedly")
        }
        FirstExit::TrustedProxy(result) => {
            let _ = plaintext.await;
            result.context("Commander trusted proxy listener failed")?;
            anyhow::bail!("Commander trusted proxy listener exited unexpectedly")
        }
    }
}

async fn shutdown_requested(mut receiver: tokio::sync::watch::Receiver<bool>) {
    if *receiver.borrow_and_update() {
        return;
    }
    let _ = receiver.changed().await;
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
        println!(
            "Scopes: {}",
            invitation
                .scopes
                .iter()
                .map(|scope| scope.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        if !invitation.scopes.contains(&Scope::PaneInput) {
            println!(
                "Read-only. To type into panes, send messages, and interrupt, re-run with:\n  cas hub pair --origin {} --scopes machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt",
                args.origin
            );
        }
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
        println!(
            "{}",
            serde_json::json!({
                "running": live,
                "record": record,
                "binary": env!("CARGO_PKG_VERSION"),
            })
        );
    } else {
        println!(
            "{}",
            render_status(&record, live, env!("CARGO_PKG_VERSION"))
        );
    }
    anyhow::ensure!(live, "cas hub is not running");
    Ok(())
}

fn stop(cli: &Cli) -> Result<()> {
    // No relaunch intent: a live hub can never mean success here, so this
    // keeps the pre-cas-bf90 behaviour exactly.
    stop_with_output(cli, true, None).map(|_| ())
}

fn stop_with_output(
    cli: &Cli,
    emit_output: bool,
    relaunch: Option<RelaunchIntent<'_>>,
) -> Result<StopOutcome> {
    let paths = HubRuntimePaths::default_for_user()?;
    let record = paths.read_process_record().ok();
    let stale_pid = record.as_ref().map(|record| record.pid);
    let hub_cgroup = record.as_ref().and_then(|record| record.cgroup.clone());
    let tailscale_manager = TailscaleServeManager::new(paths.root());
    // Capture the exact mapping we own before asking the hub to exit. The
    // foreground process now always tears it down on SIGTERM, so stop must
    // judge the final outcome instead of which process issued `serve off`.
    let owned_tailscale_receipt = tailscale_manager.owned_receipt();
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
        if let Some(satisfying) = wait_for_stop_or_satisfying_hub(
            &paths,
            Some(record.pid),
            stale_pid,
            HUB_LIFECYCLE_TIMEOUT,
            relaunch.as_ref(),
        )? {
            return Ok(StopOutcome::AlreadySatisfied(Box::new(satisfying)));
        }
    } else {
        // Record absence does not authorize stale cleanup: a shutting-down hub
        // may already have removed it while still holding the machine lock.
        if let Some(satisfying) = wait_for_stop_or_satisfying_hub(
            &paths,
            None,
            stale_pid,
            HUB_LIFECYCLE_TIMEOUT,
            relaunch.as_ref(),
        )? {
            return Ok(StopOutcome::AlreadySatisfied(Box::new(satisfying)));
        }
    }
    if let Some(cgroup) = hub_cgroup {
        if let Err(error) = crate::ui::factory::cgroup::kill_scope(&cgroup) {
            tracing::warn!(
                cgroup = %cgroup.display(),
                error = %error,
                "cas-8716: failed to drain the detached hub cgroup"
            );
        }
        crate::ui::factory::cgroup::remove_scope(&cgroup);
    }
    let tailscale_result = tailscale_manager.disable_owned();
    let tailscale_outcome = match tailscale_result {
        Ok(Some(receipt)) => Ok((true, Some(receipt))),
        Ok(None) => match owned_tailscale_receipt {
            Ok(Some(receipt)) => tailscale_manager
                .mapping_is_absent(&receipt)
                .map(|absent| (absent, None)),
            Ok(None) => Ok((false, None)),
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    };
    paths.remove_process_record()?;
    if emit_output && cli.json {
        println!(
            "{}",
            serde_json::json!({
                "stopped":true,
                "pid":record.as_ref().map(|record| record.pid),
                "tailscale_serve_removed":matches!(&tailscale_outcome, Ok((true, _))),
                "tailscale_warning":tailscale_outcome.as_ref().err().map(ToString::to_string),
            })
        );
    } else if emit_output {
        if let Some(record) = &record {
            println!("Cassy hub stopped (pid {})", record.pid);
        } else {
            println!("Cassy hub was not running");
        }
        match tailscale_outcome {
            Ok((true, Some(receipt))) => println!(
                "Removed Cassy Tailscale Serve mapping at {}",
                receipt.public_url
            ),
            Ok((true, None)) => {
                println!("Cassy Tailscale Serve mapping was removed as the hub exited")
            }
            Ok((false, _)) => {}
            Err(error) => eprintln!("Tailscale Serve mapping left untouched: {error}"),
        }
    }
    Ok(StopOutcome::Stopped)
}

/// Restart a live hub left behind by an older Cassy binary. This is called by
/// `cas update` after the replacement version is known; a missing, dead, or
/// already-current hub is intentionally a no-op.
pub(crate) fn restart_stale_hub(binary_version: &str, cli: &Cli) -> Result<bool> {
    let paths = HubRuntimePaths::default_for_user()?;
    let Ok(record) = paths.read_process_record() else {
        return Ok(false);
    };
    if !record_is_live(&record) {
        return Ok(false);
    }
    let Some(spec) = restart_spec_for_record(&record, binary_version)? else {
        return Ok(false);
    };

    if !cli.json {
        println!(
            "cas update: restarting stale hub (pid {}, version {} -> {})",
            record.pid, record.version, binary_version
        );
    }
    let args = HubServeArgs {
        bind: spec.bind,
        port: spec.port,
        ..HubServeArgs::default()
    };
    // A stale-version restart is a relaunch: if a concurrent command already
    // produced a hub on the new binary, that is the outcome this wanted.
    if let StopOutcome::AlreadySatisfied(_) = stop_with_output(
        cli,
        !cli.json,
        Some(RelaunchIntent {
            args: &args,
            tailscale_serve: spec.tailscale_serve,
            tailscale_port: spec.tailscale_port,
        }),
    )? {
        return Ok(true);
    }
    start_with_output_from(
        &args,
        cli,
        spec.tailscale_serve,
        spec.tailscale_port,
        !cli.json,
        HubLaunchOrigin::Update,
    )?;
    Ok(true)
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

fn current_session_id() -> Option<u32> {
    #[cfg(unix)]
    {
        // SAFETY: getsid(0) only reads the calling process's session id.
        let sid = unsafe { libc::getsid(0) };
        (sid >= 0).then_some(sid as u32)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn current_process_group_id() -> Option<u32> {
    #[cfg(unix)]
    {
        // SAFETY: getpgrp only reads the calling process's process-group id.
        let pgid = unsafe { libc::getpgrp() };
        (pgid >= 0).then_some(pgid as u32)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

pub(super) fn record_is_live(record: &HubProcessRecord) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn record(version: &str, port: u16, tailscale_serve_port: Option<u16>) -> HubProcessRecord {
        HubProcessRecord {
            pid: 42,
            sid: None,
            pgid: None,
            bind: "127.0.0.1".to_owned(),
            port,
            version: version.to_owned(),
            started_at: "2026-09-01T12:00:00Z".to_owned(),
            cgroup: None,
            launched_by: None,
            launched_at: None,
            public_url: tailscale_serve_port.map(|port| format!("https://hub.example:{port}")),
            tailscale_serve_port,
            tailscale_cli: tailscale_serve_port.map(|_| "tailscale".to_owned()),
            transport_warning: None,
        }
    }

    #[test]
    fn bare_hub_defaults_to_status() {
        assert!(matches!(default_hub_command(), HubCommands::Status));
    }

    /// cas-bf90: the predicate that lets a losing concurrent lifecycle command
    /// stop waiting on a lock the winner's healthy hub legitimately holds.
    mod running_hub_satisfies {
        use super::*;

        const VERSION: &str = env!("CARGO_PKG_VERSION");
        /// The Serve port default, mirrored from `record.tailscale_serve_port.unwrap_or(443)`.
        const TS_PORT: u16 = 443;

        fn ephemeral_args() -> HubServeArgs {
            let mut args = HubServeArgs::default();
            args.port = 0;
            args
        }

        #[test]
        fn an_ephemeral_request_is_satisfied_by_any_kernel_assigned_port() {
            // The case that matters: `--port 0` is what the concurrent
            // lifecycle callers use, so if this were false the fix would be a
            // no-op exactly where the race happens.
            let live = record(VERSION, 35053, None);
            assert!(running_hub_satisfies_request(
                &live,
                &ephemeral_args(),
                false,
                TS_PORT,
                VERSION
            ));
        }

        #[test]
        fn an_explicit_port_request_is_not_satisfied_by_a_different_port() {
            let mut args = HubServeArgs::default();
            args.port = 4173;
            let live = record(VERSION, 35053, None);
            assert!(!running_hub_satisfies_request(
                &live,
                &args,
                false,
                TS_PORT,
                VERSION
            ));
        }

        #[test]
        fn a_version_mismatch_is_never_satisfying() {
            // Version drift must still force a restart: accepting an old binary
            // here would silently keep a stale hub alive.
            let stale = record("0.0.1-old", 35053, None);
            assert!(!running_hub_satisfies_request(
                &stale,
                &ephemeral_args(),
                false,
                TS_PORT,
                VERSION
            ));
        }

        #[test]
        fn tailscale_flag_drift_in_either_direction_is_not_satisfying() {
            let with_ts = record(VERSION, 35053, Some(TS_PORT));
            let without_ts = record(VERSION, 35053, None);
            // Wanted plain, found Serve-enabled.
            assert!(!running_hub_satisfies_request(
                &with_ts,
                &ephemeral_args(),
                false,
                TS_PORT,
                VERSION
            ));
            // Wanted Serve, found plain.
            assert!(!running_hub_satisfies_request(
                &without_ts,
                &ephemeral_args(),
                true,
                TS_PORT,
                VERSION
            ));
            // Wanted Serve, found Serve on the same port.
            assert!(running_hub_satisfies_request(
                &with_ts,
                &ephemeral_args(),
                true,
                TS_PORT,
                VERSION
            ));
        }

        #[test]
        fn a_different_tailscale_serve_port_is_not_satisfying() {
            let other_port = record(VERSION, 35053, Some(8443));
            assert!(!running_hub_satisfies_request(
                &other_port,
                &ephemeral_args(),
                true,
                TS_PORT,
                VERSION
            ));
        }

        #[test]
        fn a_different_bind_address_is_not_satisfying() {
            let mut live = record(VERSION, 35053, None);
            live.bind = "0.0.0.0".to_owned();
            assert!(!running_hub_satisfies_request(
                &live,
                &ephemeral_args(),
                false,
                TS_PORT,
                VERSION
            ));
        }
    }

    #[test]
    fn live_start_decision_table_covers_version_and_flag_drift() {
        let same_args = HubServeArgs::default();
        let cases = [
            (
                "same version and flags",
                record(env!("CARGO_PKG_VERSION"), DEFAULT_HUB_PORT, None),
                same_args.clone(),
                false,
                443,
                HubStartDecision::Keep,
            ),
            (
                "different version",
                record("3.4.1", DEFAULT_HUB_PORT, None),
                same_args.clone(),
                false,
                443,
                HubStartDecision::Restart {
                    version_drift: true,
                    flags_differ: false,
                },
            ),
            (
                "different listener port",
                record(env!("CARGO_PKG_VERSION"), DEFAULT_HUB_PORT, None),
                HubServeArgs {
                    port: DEFAULT_HUB_PORT + 1,
                    ..same_args.clone()
                },
                false,
                443,
                HubStartDecision::Restart {
                    version_drift: false,
                    flags_differ: true,
                },
            ),
            (
                "different tailscale flag",
                record(env!("CARGO_PKG_VERSION"), DEFAULT_HUB_PORT, None),
                same_args,
                true,
                443,
                HubStartDecision::Restart {
                    version_drift: false,
                    flags_differ: true,
                },
            ),
        ];

        for (name, live, args, tailscale_serve, tailscale_port, expected) in cases {
            assert_eq!(
                decide_live_start(
                    &live,
                    &args,
                    tailscale_serve,
                    tailscale_port,
                    env!("CARGO_PKG_VERSION"),
                ),
                expected,
                "case: {name}"
            );
        }
    }

    #[test]
    fn status_rendering_shows_record_and_binary_versions() {
        let rendered = render_status(
            &record("3.4.1", DEFAULT_HUB_PORT, None),
            true,
            "3.7.7",
        );

        assert!(rendered.contains("version 3.4.1"), "{rendered}");
        assert!(rendered.contains("binary: 3.7.7"), "{rendered}");
    }

    #[test]
    fn stale_status_rendering_shows_launcher_metadata() {
        let mut stale = record("3.4.1", DEFAULT_HUB_PORT, None);
        stale.launched_by = Some("update".to_owned());
        stale.launched_at = Some("2026-09-01T12:34:56Z".to_owned());

        let rendered = render_status(&stale, false, "3.7.7");

        assert!(
            rendered.contains(
                "not running (last pid 42 exited; started by update at 2026-09-01T12:34:56Z)"
            ),
            "{rendered}"
        );
    }

    #[test]
    fn update_restart_spec_is_present_only_for_a_stale_live_record() {
        let stale = restart_spec_for_record(&record("3.4.1", 4310, Some(8443)), "3.7.7")
            .unwrap()
            .expect("stale hub must be restarted");
        assert_eq!(stale.bind, "127.0.0.1".parse::<IpAddr>().unwrap());
        assert_eq!(stale.port, 4310);
        assert!(stale.tailscale_serve);
        assert_eq!(stale.tailscale_port, 8443);

        assert!(
            restart_spec_for_record(&record("3.7.7", 4310, Some(8443)), "3.7.7")
                .unwrap()
                .is_none(),
            "matching hub version must not trigger update restart"
        );
    }
}
