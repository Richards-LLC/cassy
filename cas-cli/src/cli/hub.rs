use std::fs::OpenOptions;
use std::future::IntoFuture;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;

use crate::ai_enrichment::HttpAiEnrichmentProvider;
use crate::cli::Cli;
use crate::config::{Config, HubTransportMode};
use crate::hub::{
    AuthStore, DEFAULT_HUB_PORT, DEFAULT_VIEWER_QUEUE_CAPACITY, DaemonConnector, HubProcessRecord,
    HubLockHolder, HubRuntimePaths, HubState, LocalSessionReadModel, MachineEventBus,
    MachineIdentityStore, MachineMetadata, MachineTransport, PairingPrefill, PreAuthAuthorizer, Scope,
    SessionCatalog,
    SessionMultiplexer, TailscaleServeManager, TailscaleServeReceipt, TransportSecurity,
    load_cloud_device_suggestions, router, spawn_attention_enricher, validate_control_bind,
};

const HUB_LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(10);
const HUB_CONNECTION_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const HUB_LAUNCH_TIMEOUT: Duration = Duration::from_secs(5);
// Leave the launcher's readiness budget time to observe a peer or child exit.
const HUB_STARTUP_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const _: () = assert!(HUB_STARTUP_LOCK_TIMEOUT.as_nanos() < HUB_LAUNCH_TIMEOUT.as_nanos());
const HUB_PEER_RECORD_PUBLISH_TIMEOUT: Duration = Duration::from_millis(250);
const HUB_PEER_RECORD_POLL_INTERVAL: Duration = Duration::from_millis(10);

fn hub_launch_timeout(tailscale_serve: bool) -> Duration {
    if tailscale_serve {
        HUB_LAUNCH_TIMEOUT + TailscaleServeManager::successful_ensure_budget()
    } else {
        HUB_LAUNCH_TIMEOUT
    }
}

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
    /// Publish through tailnet-only Tailscale Serve HTTPS (on by default)
    #[arg(long, global = true, conflicts_with = "no_tailscale_serve")]
    pub tailscale_serve: bool,
    /// Keep this launch loopback-only (overrides hub.transport_mode for this launch)
    #[arg(long, visible_alias = "loopback", global = true, conflicts_with = "tailscale_serve")]
    pub no_tailscale_serve: bool,
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
    Stop(HubStopArgs),
    /// Stop and start the hub while preserving machine identity
    ///
    /// Detached restart: one `{stop,start}` JSON; start errors include `stop`.
    ///
    /// Stop keys: `stopped`, `pid`, `tailscale_serve_removed`, `tailscale_warning`.
    ///
    /// Stop also has `already_satisfied`; supervised restart reports hub status.
    ///
    /// Start also has `already_satisfied` when a peer launches the requested hub.
    Restart(HubServeArgs),
    /// Install, inspect, or remove boot-persistent hub supervision
    Service(HubServiceArgs),
    /// Mint a ten-minute one-time browser pairing invitation
    Pair(HubPairArgs),
    /// Approve a Commander page's short-code pairing request through Petra Stella Cloud
    Authorize(HubAuthorizeArgs),
    /// List or revoke paired Commander devices
    Auth(HubAuthArgs),
    /// Enroll this hub in your account's cloud operator inbox and approve devices
    Operator(super::hub_operator::HubOperatorArgs),
    /// Internal reaper for a factory daemon in a separate service unit.
    #[command(hide = true)]
    ReapDaemon(HubReapDaemonArgs),
}

#[derive(Args, Debug, Clone)]
pub struct HubReapDaemonArgs {
    #[arg(long)]
    pub session: String,
    #[arg(long)]
    pub cwd: std::path::PathBuf,
    #[arg(long)]
    pub workers: u8,
    #[arg(long)]
    pub supervisor_cli: String,
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
    /// Install and start a user-level hub service with Tailscale Serve by default
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
    /// Hub address the link prefills in the pairing form (defaults to the
    /// running hub's Tailscale Serve URL, then `hub.public_url`)
    #[arg(long)]
    pub hub_url: Option<String>,
    /// cas-e719: run only the operator-only gate, print `launch check: ...`
    /// and exit before minting anything (the release operator launch smoke).
    #[arg(long, hide = true)]
    pub check_launch: bool,
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
    /// Skip only this host's public Hub readiness check.
    ///
    /// This does not bypass URL validation, consent, authentication, origin
    /// checks, or one-time token handling.
    #[arg(long)]
    pub skip_hub_readiness: bool,
    /// Approve without an interactive confirmation prompt
    #[arg(long)]
    pub yes: bool,
    /// cas-e719: run only the operator-only gate, print `launch check: ...`
    /// and exit before contacting the relay (the release operator launch smoke).
    #[arg(long, hide = true)]
    pub check_launch: bool,
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
    /// Launch-owned channel for a detached child's startup failure.
    #[arg(long, hide = true)]
    pub startup_error_file: Option<std::path::PathBuf>,
    /// Cassy-created cgroup scope passed through the detached launch barrier.
    #[arg(long, hide = true)]
    pub cgroup: Option<std::path::PathBuf>,
    /// Ownership evidence captured before the launcher removes an old record.
    #[arg(long, hide = true)]
    pub prior_tailscale_serve_port: Option<u16>,
    #[arg(long, hide = true)]
    pub prior_tailscale_serve_target: Option<String>,
    /// Reclaim a wedged hub lock without waiting for the lifecycle timeout.
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug, Clone, Default)]
pub struct HubStopArgs {
    /// Reclaim a wedged hub lock without waiting for the lifecycle timeout.
    #[arg(long)]
    pub force: bool,
}

impl Default for HubServeArgs {
    fn default() -> Self {
        Self {
            bind: IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            port: DEFAULT_HUB_PORT,
            launched_by: "cli".to_owned(),
            launched_at: None,
            startup_error_file: None,
            cgroup: None,
            prior_tailscale_serve_port: None,
            prior_tailscale_serve_target: None,
            force: false,
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
    transport_mode: HubTransportMode,
    tailscale_port: u16,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct HubTransportReport {
    status: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actual_target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remedy: Option<String>,
}

impl HubTransportReport {
    fn ok(message: impl Into<String>) -> Self {
        Self {
            status: "ok".to_owned(),
            message: message.into(),
            expected_target: None,
            actual_target: None,
            remedy: None,
        }
    }

    fn fail(
        message: impl Into<String>,
        expected_target: Option<String>,
        actual_target: Option<String>,
    ) -> Self {
        Self::fail_with_remedy(
            message,
            expected_target,
            actual_target,
            "Run `cas hub restart --tailscale-serve` to republish the route.".to_owned(),
        )
    }

    fn fail_with_remedy(
        message: impl Into<String>,
        expected_target: Option<String>,
        actual_target: Option<String>,
        remedy: String,
    ) -> Self {
        Self {
            status: "fail".to_owned(),
            message: message.into(),
            expected_target,
            actual_target,
            remedy: Some(remedy),
        }
    }

    fn wedged_lock(holder: &HubLockHolder, tailscale: bool) -> Self {
        let command = if tailscale {
            "cas hub restart --force --tailscale-serve"
        } else {
            "cas hub restart --force"
        };
        let state = lock_holder_display_state_with_timeout(holder, hub_launch_timeout(tailscale));
        let remedy = match state {
            HubDisplayState::Starting { wedged: false, .. } | HubDisplayState::Stopping { .. } => {
                "Run `cas hub status` again after this phase completes.".to_owned()
            }
            _ => format!("Run `{command}` to recover the hub."),
        };
        Self::fail_with_remedy(
            format!("hub machine lock: {}", hub_state_label(state, holder.pid)),
            None,
            None,
            remedy,
        )
    }

    fn unavailable(error: impl Into<String>) -> Self {
        Self::fail(
            format!("cannot inspect the CAS-created Tailscale Serve route: {}", error.into()),
            None,
            None,
        )
    }

    fn supervised_unavailable(warning: impl Into<String>) -> Self {
        Self::fail_with_remedy(
            format!(
                "hub is supervised but not publishable: Tailscale Serve is unavailable: {}",
                warning.into()
            ),
            None,
            None,
            "Run `cas hub restart --tailscale-serve`.".to_owned(),
        )
    }

    pub(crate) fn is_failure(&self) -> bool {
        self.status == "fail"
    }

    pub(crate) fn is_supervised_unavailable(&self) -> bool {
        self.message
            .starts_with("hub is supervised but not publishable")
    }

    pub(crate) fn is_signed_in_loopback_warning(&self) -> bool {
        self.message
            .starts_with("hub is loopback-only while Tailscale is signed in")
            || self
                .message
                .starts_with("hub is loopback-only; Tailscale is signed in")
    }

    pub(crate) fn message_with_remedy(&self) -> String {
        match &self.remedy {
            Some(remedy) if self.message.contains('\n') => format!("{}\n{remedy}", self.message),
            Some(remedy) => format!("{}; {remedy}", self.message),
            None => self.message.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HubRestartOutcome {
    pub(crate) transport_error: Option<String>,
    pub(crate) previous_version: Option<String>,
    pub(crate) current_version: Option<String>,
    pub(crate) service_managed: bool,
    pub(crate) prior_state: String,
    pub(crate) action: String,
    pub(crate) verified: bool,
    pub(crate) loopback_verified: bool,
    pub(crate) transport_verified: Option<bool>,
    pub(crate) transport_warning: Option<String>,
    pub(crate) recovery_attempted: bool,
    pub(crate) public_url: Option<String>,
    pub(crate) failure: Option<String>,
    pub(crate) remedy: Option<String>,
}

struct HubUpdateVerification {
    public_url: Option<String>,
    transport_verified: Option<bool>,
    transport_warning: Option<String>,
    remedy: Option<String>,
}

fn default_hub_command() -> HubCommands {
    HubCommands::Status
}

/// The machine hub reads host configuration, independent of the invoking project.
/// An old process record/receipt preserves ports, never the publication policy.
fn host_transport_mode() -> Result<HubTransportMode> {
    let root = crate::store::known_repos::host_cas_dir();
    Ok(Config::load(&root)?.hub.unwrap_or_default().transport_mode())
}

fn host_tailscale_default() -> Result<bool> {
    Ok(host_transport_mode()?.publishes())
}

// Required explicitly refuses publication failure. Auto always preserves local
// access and retries publication without asking the service manager to restart.
fn publication_failure_is_fatal(mode: HubTransportMode) -> bool {
    mode == HubTransportMode::Required
}

fn tailscale_policy(explicit_on: bool, explicit_off: bool, configured: bool) -> bool {
    !explicit_off && (explicit_on || configured)
}

fn resolved_tailscale_request(
    requested: bool,
    requested_port: u16,
    owned_receipt: Option<&TailscaleServeReceipt>,
) -> (bool, u16) {
    let port = if requested && requested_port == 443 {
        owned_receipt.map_or(requested_port, |receipt| receipt.https_port)
    } else {
        requested_port
    };
    (requested, port)
}

fn resolve_lifecycle_tailscale_request(
    requested: bool,
    requested_port: u16,
    paths: &HubRuntimePaths,
) -> Result<(bool, u16)> {
    // Bad/unavailable Serve state is diagnosed by ensure() in the child, which
    // can retain a healthy loopback listener; it must not prevent the launch.
    let receipt = if requested {
        TailscaleServeManager::new(paths.root())
            .owned_receipt()
            .ok()
            .flatten()
    } else {
        None
    };
    let (enabled, port) = resolved_tailscale_request(
        requested,
        requested_port,
        receipt.as_ref(),
    );
    // A deliberate stop/loopback switch removes the ownership receipt, but
    // its verified teardown still carries the operator's stable HTTPS port.
    let port = if enabled && requested_port == 443 && receipt.is_none() {
        TailscaleServeManager::new(paths.root()).last_https_port().unwrap_or(port)
    } else { port };
    Ok((enabled, port))
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
    publication_required: bool,
) -> HubStartDecision {
    let version_drift = record.version != binary_version;
    let tailscale_flags_differ = tailscale_enabled(record) != tailscale_serve
        || (tailscale_serve
            && record
                .tailscale_serve_port
                .is_some_and(|port| port != tailscale_port));
    let target_missing =
        tailscale_serve && !serve_request_settled(record, publication_required);
    let flags_differ = record.bind != args.bind.to_string()
        || record.port != args.port
        || tailscale_flags_differ
        || target_missing;

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
    publication_required: bool,
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
    if tailscale_serve && !serve_request_settled(record, publication_required) {
        return false;
    }
    true
}

/// A warned Auto hub satisfies local lifecycle requests while its background
/// publication is pending. Required callers demand a published route.
fn serve_request_settled(record: &HubProcessRecord, publication_required: bool) -> bool {
    match (&record.tailscale_serve_target, &record.transport_warning) {
        (Some(_), None) => true,
        (_, Some(_)) => !publication_required,
        (None, None) => false,
    }
}

fn publication_required() -> bool {
    host_transport_mode().map_or(true, |mode| mode == HubTransportMode::Required)
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
    Stopped(HubStopReceipt),
    /// A concurrent command already produced the hub the caller was going to
    /// relaunch. Only reachable with a [`RelaunchIntent`].
    ///
    /// The caller MUST return success directly rather than continuing into its
    /// relaunch: the teardown steps after the wait (`remove_process_record`,
    /// cgroup kill, Tailscale disable) would otherwise dismantle a healthy hub
    /// this command does not own.
    AlreadySatisfied(Box<HubProcessRecord>),
}

#[derive(Serialize)]
struct HubStopReceipt {
    stopped: bool,
    pid: Option<u32>,
    tailscale_serve_removed: bool,
    tailscale_warning: Option<String>,
}

#[derive(Serialize)]
struct HubRestartStopReceipt {
    stopped: bool,
    pid: Option<u32>,
    tailscale_serve_removed: Option<bool>,
    tailscale_warning: Option<String>,
    already_satisfied: bool,
}

struct HubStartOutcome {
    record: HubProcessRecord,
    already_satisfied: bool,
}

impl HubStartOutcome {
    fn into_restart_json(self) -> Result<serde_json::Value> {
        let mut value = serde_json::to_value(self.record)?;
        let object = value
            .as_object_mut()
            .context("hub process record must serialize as a JSON object")?;
        object.insert(
            "already_satisfied".to_owned(),
            serde_json::Value::Bool(self.already_satisfied),
        );
        Ok(value)
    }
}

impl From<HubStopReceipt> for HubRestartStopReceipt {
    fn from(stop: HubStopReceipt) -> Self {
        Self {
            stopped: stop.stopped,
            pid: stop.pid,
            tailscale_serve_removed: Some(stop.tailscale_serve_removed),
            tailscale_warning: stop.tailscale_warning,
            already_satisfied: false,
        }
    }
}

impl HubRestartStopReceipt {
    fn already_satisfied() -> Self {
        Self {
            stopped: false,
            pid: None,
            tailscale_serve_removed: None,
            tailscale_warning: None,
            already_satisfied: true,
        }
    }
}

enum StopWait {
    /// Keep the machine exclusive until old-generation cleanup finishes.
    Quiescent(crate::hub::HubInstanceLock),
    AlreadySatisfied(Box<HubProcessRecord>),
}

/// Wait for exclusive cleanup ownership, unless the relaunch is already done.
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
    force: bool,
    stop_deadline: Option<crate::bounded_process::Deadline>,
) -> Result<StopWait> {
    #[cfg(debug_assertions)]
    mark_restart_stop_waiter_for_test()?;

    let deadline = Instant::now() + timeout;
    loop {
        ensure_stop_budget(stop_deadline)?;
        let holder = paths
            .lock_holders()
            .into_iter()
            .find(|holder| force || Some(holder.pid) != settle_pid);
        if let Some(holder) = holder.as_ref() && force {
            terminate_lock_holder_before(paths, holder, stop_deadline)?;
            continue;
        }
        let lock = paths.try_acquire_instance_lock()?;
        // A forced recovery owns the machine lock, which is authoritative for
        // hub lifetime. Older detached launchers can leave a reaped-late PID
        // after their listener and lock are already gone; waiting on kill(0)
        // alone then reports a false stop timeout.
        let quiescent = lock.is_some()
            && (force || settle_pid.is_none_or(|pid| !process_is_running(pid)));
        if quiescent {
            let mut lock = lock.expect("quiescence requires the machine lock");
            lock.set_phase("stopping")?;
            return Ok(StopWait::Quiescent(lock));
        }
        drop(lock);
        if let Some(intent) = relaunch
            && let Ok(record) = paths.read_process_record()
            && stale_pid != Some(record.pid)
            && running_hub_satisfies_request(
                &record,
                intent.args,
                intent.tailscale_serve,
                intent.tailscale_port,
                env!("CARGO_PKG_VERSION"),
                publication_required(),
            )
            && record_is_ready(paths, &record)
        {
            return Ok(StopWait::AlreadySatisfied(Box::new(record)));
        }
        if Instant::now() >= deadline {
            if let Some(holder) = holder.as_ref()
                && (force || !holder.is_stopping())
            {
                terminate_lock_holder_before(paths, holder, stop_deadline)?;
                continue;
            }
            match settle_pid {
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

fn terminate_lock_holder(paths: &HubRuntimePaths, holder: &HubLockHolder) -> Result<()> {
    terminate_lock_holder_before(paths, holder, None)
}

fn terminate_lock_holder_before(
    paths: &HubRuntimePaths,
    holder: &HubLockHolder,
    stop_deadline: Option<crate::bounded_process::Deadline>,
) -> Result<()> {
    anyhow::ensure!(
        holder.pid != std::process::id(),
        "refusing to terminate the current cas hub lifecycle process"
    );
    eprintln!("{}", lock_holder_termination_message(paths, holder));

    #[cfg(unix)]
    {
        // SAFETY: the lock file itself proves this PID owns the private hub
        // lock; SIGTERM gives a normal hub a chance to release it cleanly.
        let result = unsafe { libc::kill(holder.pid as libc::pid_t, libc::SIGTERM) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error).context("terminate wedged cas hub lock holder");
            }
        }
    }
    #[cfg(windows)]
    {
        Command::new("taskkill")
            .args(["/PID", &holder.pid.to_string()])
            .status()
            .context("terminate wedged cas hub lock holder")?;
    }

    let graceful_deadline = Instant::now() + stop_wait_budget(stop_deadline, HUB_CONNECTION_DRAIN_TIMEOUT);
    while Instant::now() < graceful_deadline {
        if let Some(lock) = paths.try_acquire_instance_lock()? {
            drop(lock);
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    #[cfg(unix)]
    {
        // SAFETY: this is the same private lock owner that did not release
        // after SIGTERM; SIGKILL is the bounded final recovery step.
        let result = unsafe { libc::kill(holder.pid as libc::pid_t, libc::SIGKILL) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error).context("force-terminate wedged cas hub lock holder");
            }
        }
    }
    #[cfg(windows)]
    {
        Command::new("taskkill")
            .args(["/F", "/PID", &holder.pid.to_string()])
            .status()
            .context("force-terminate wedged cas hub lock holder")?;
    }

    let kill_deadline = Instant::now() + stop_wait_budget(stop_deadline, HUB_CONNECTION_DRAIN_TIMEOUT);
    while Instant::now() < kill_deadline {
        if let Some(lock) = paths.try_acquire_instance_lock()? {
            drop(lock);
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!(
        "cas hub lock holder pid {} did not release the machine lock after termination",
        holder.pid
    )
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
        hold_hub_test_barrier(
            "CAS_TEST_HUB_LAUNCH_WAIT_BARRIER",
            "launch-waiter",
            "release-launch-waiter",
            std::process::id(),
        )?;
        if let Ok(record) = paths.read_process_record()
            && running_hub_satisfies_request(
                &record,
                args,
                tailscale_serve,
                tailscale_port,
                env!("CARGO_PKG_VERSION"),
                publication_required(),
            )
            && record_is_ready(paths, &record)
        {
            return Ok(LaunchWait::AlreadySatisfied(Box::new(record)));
        }
        if Instant::now() >= deadline {
            if tailscale_serve
                && let Some(holder) = paths.lock_holders().into_iter().find(|holder| {
                    holder.phase.as_deref() == Some("starting")
                        && holder.age.is_some_and(|age| age < hub_launch_timeout(true))
                })
            {
                anyhow::bail!(
                    "cas hub launch waiter: {}; run `cas hub status` after startup completes",
                    hub_state_label(
                        lock_holder_display_state_with_timeout(&holder, hub_launch_timeout(true)),
                        holder.pid
                    )
                );
            }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HubDisplayState {
    Exited,
    Starting { age_secs: u64, wedged: bool },
    Stopping { age_secs: u64 },
    Unresponsive { age_secs: u64 },
    Running,
}

fn record_age_secs(record: &HubProcessRecord) -> u64 {
    chrono::DateTime::parse_from_rfc3339(&record.started_at)
        .ok()
        .map(|started| {
            (chrono::Utc::now() - started.with_timezone(&chrono::Utc))
                .num_seconds()
                .max(0) as u64
        })
        .unwrap_or(0)
}

pub(super) fn hub_display_state(
    paths: &HubRuntimePaths,
    record: &HubProcessRecord,
) -> HubDisplayState {
    if !process_is_running(record.pid) {
        return HubDisplayState::Exited;
    }
    let ready = record_is_ready(paths, record);
    if ready {
        return HubDisplayState::Running;
    }
    let holder = paths
        .lock_holders()
        .into_iter()
        .find(|holder| holder.pid == record.pid);
    let age_secs = holder
        .as_ref()
        .and_then(|holder| holder.age)
        .map(|age| age.as_secs())
        .unwrap_or_else(|| record_age_secs(record));
    if holder.as_ref().and_then(|holder| holder.phase.as_deref()) == Some("starting") {
        return HubDisplayState::Starting {
            age_secs,
            wedged: age_secs >= hub_launch_timeout(record.tailscale_cli.is_some()).as_secs(),
        };
    }
    if holder.as_ref().and_then(|holder| holder.phase.as_deref()) == Some("stopping") {
        return HubDisplayState::Stopping { age_secs };
    }
    HubDisplayState::Unresponsive { age_secs }
}

pub(super) fn hub_state_remedy(state: HubDisplayState, pid: u32) -> String {
    match state {
        HubDisplayState::Exited => "Run `cas hub start`.".to_owned(),
        HubDisplayState::Starting { wedged: false, .. } => {
            "Wait for startup, then run `cas hub status` again.".to_owned()
        }
        HubDisplayState::Starting { wedged: true, .. } => {
            "Run `cas hub restart --force` to recover wedged startup.".to_owned()
        }
        HubDisplayState::Stopping { .. } => "Wait for shutdown, then run `cas hub status` again.".to_owned(),
        HubDisplayState::Unresponsive { .. } if cfg!(target_os = "macos") => {
            format!("Run `sample {pid}` for evidence; then `cas hub restart --force`.")
        }
        HubDisplayState::Unresponsive { .. } => {
            "Run `cas hub restart --force` to recover the hub.".to_owned()
        }
        HubDisplayState::Running => String::new(),
    }
}

/// The hub-line remedy for a hub with a runtime record. An exited hub that
/// recorded Serve gives the same single command as the Serve line, so the
/// post-reboot screen offers one remedy rather than two (cas-ea2b).
pub(super) fn hub_record_remedy(record: &HubProcessRecord, state: HubDisplayState) -> String {
    if state == HubDisplayState::Exited && tailscale_enabled(record) {
        "Run `cas hub restart --tailscale-serve`.".to_owned()
    } else {
        hub_state_remedy(state, record.pid)
    }
}

fn hub_record_state_json(record: &HubProcessRecord, state: HubDisplayState) -> serde_json::Value {
    let mut json = hub_state_json(state, record.pid);
    json["remedy"] = hub_record_remedy(record, state).into();
    json
}

pub(super) fn hub_state_label(state: HubDisplayState, pid: u32) -> String {
    match state {
        HubDisplayState::Exited => format!("last pid {pid} exited"),
        HubDisplayState::Starting { age_secs, wedged: false } => {
            format!("pid {pid} is starting for {age_secs}s")
        }
        HubDisplayState::Starting { age_secs, wedged: true } => {
            format!("pid {pid} is wedged in startup after {age_secs}s")
        }
        HubDisplayState::Stopping { age_secs } => {
            format!("pid {pid} is stopping; lock held for {age_secs}s")
        }
        HubDisplayState::Unresponsive { age_secs } => {
            format!("pid {pid} is running for {age_secs}s but not answering")
        }
        HubDisplayState::Running => format!("pid {pid} is running and ready"),
    }
}

pub(super) fn lock_holder_display_state(holder: &HubLockHolder) -> HubDisplayState {
    lock_holder_display_state_with_timeout(holder, HUB_LAUNCH_TIMEOUT)
}

fn lock_holder_display_state_with_timeout(
    holder: &HubLockHolder,
    launch_timeout: Duration,
) -> HubDisplayState {
    let age_secs = holder.age.map(|age| age.as_secs()).unwrap_or(0);
    if holder.phase.as_deref() == Some("starting") {
        HubDisplayState::Starting {
            age_secs,
            wedged: age_secs >= launch_timeout.as_secs(),
        }
    } else if holder.phase.as_deref() == Some("stopping") {
        HubDisplayState::Stopping { age_secs }
    } else {
        HubDisplayState::Unresponsive { age_secs }
    }
}

fn lock_holder_state_json(holder: &HubLockHolder) -> serde_json::Value {
    hub_state_json(lock_holder_display_state(holder), holder.pid)
}

fn lock_holder_termination_label(paths: &HubRuntimePaths, holder: &HubLockHolder) -> String {
    let state = paths
        .read_process_record()
        .ok()
        .filter(|record| record.pid == holder.pid)
        .map(|record| hub_display_state(paths, &record))
        .unwrap_or_else(|| lock_holder_display_state(holder));
    hub_state_label(state, holder.pid)
}

fn lock_holder_termination_message(paths: &HubRuntimePaths, holder: &HubLockHolder) -> String {
    format!(
        "cas hub {}; terminating lock holder",
        lock_holder_termination_label(paths, holder)
    )
}

fn hub_state_json(state: HubDisplayState, pid: u32) -> serde_json::Value {
    let (kind, age_secs) = match state {
        HubDisplayState::Exited => ("exited", None),
        HubDisplayState::Starting { age_secs, wedged: false } => ("starting", Some(age_secs)),
        HubDisplayState::Starting { age_secs, wedged: true } => ("startup_wedged", Some(age_secs)),
        HubDisplayState::Stopping { age_secs } => ("stopping", Some(age_secs)),
        HubDisplayState::Unresponsive { age_secs } => ("unresponsive", Some(age_secs)),
        HubDisplayState::Running => ("running", None),
    };
    serde_json::json!({
        "kind": kind,
        "pid": pid,
        "age_secs": age_secs,
        "message": hub_state_label(state, pid),
        "remedy": hub_state_remedy(state, pid),
    })
}

fn render_status(
    record: &HubProcessRecord,
    state: HubDisplayState,
    binary_version: &str,
) -> String {
    let mut rendered = if state == HubDisplayState::Running {
        let endpoint = record
            .public_url
            .as_deref()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("http://{}:{}", record.bind, record.port));
        format!(
            "Cassy hub is running and ready at {endpoint}\n  pid {}, version {}, binary: {binary_version}",
            record.pid, record.version
        )
    } else {
        format!(
            "Cassy hub: {}\n  remedy: {}\n  started by {} at {}\n  version {}, binary: {binary_version}",
            hub_state_label(state, record.pid),
            hub_record_remedy(record, state),
            record.launched_by.as_deref().unwrap_or("unknown"),
            record
                .launched_at
                .as_deref()
                .unwrap_or(&record.started_at),
            record.version,
        )
    };
    if let Some(warning) = version_warning(state, &record.version, binary_version) {
        rendered.push_str(&format!("\nWarning: {warning}"));
    }
    rendered
}

fn version_warning(
    state: HubDisplayState,
    running_version: &str,
    binary_version: &str,
) -> Option<String> {
    (state == HubDisplayState::Running && running_version != binary_version).then(|| {
        format!(
            "hub version drift (running {running_version}, binary {binary_version})\n  Fix: cas hub service install"
        )
    })
}

pub fn execute(args: &HubArgs, cli: &Cli) -> Result<()> {
    let result = execute_command(args, cli);
    let required_policy = matches!(
        args.command,
        Some(HubCommands::Start(_) | HubCommands::Restart(_))
    )
        && host_transport_mode().is_ok_and(|mode| mode == HubTransportMode::Required);
    // Warn on every interactive path, including an already-running hub and JSON
    // output. Keep diagnostics on stderr so stdout remains one JSON document.
    if !required_policy
        && matches!(args.command, Some(HubCommands::Start(_) | HubCommands::Restart(_)))
        && let Ok(paths) = HubRuntimePaths::default_for_user()
        && let Ok(record) = paths.read_process_record()
        && let Some(warning) = &record.transport_warning
        && record_is_ready(&paths, &record)
    {
        eprintln!("{}", format_hub_transport_warning(warning));
    }
    result
}

fn format_hub_transport_warning(warning: &str) -> String {
    format!(
        "Tailscale Serve inactive; hub is loopback-only.\n\
         Cause: {warning}\n\
         Publication retries automatically in the background."
    )
}

fn required_transport_refusal(
    args: &HubArgs,
    paths: &HubRuntimePaths,
    issue: Option<&str>,
) -> String {
    let (operation, loopback, retry) = match &args.command {
        Some(HubCommands::Restart(_)) => {
            let state = paths
                .read_process_record()
                .ok()
                .filter(|record| record_is_ready(paths, record))
                .map(|record| {
                    format!(
                        "Restart refused; existing hub (pid {}) remains running.",
                        record.pid
                    )
                })
                .unwrap_or_else(|| "Restart refused before stopping any hub.".to_owned());
            (state, "cas hub restart --loopback", "cas hub restart")
        }
        _ => (
            "Start refused; no hub was started.".to_owned(),
            "cas hub start --loopback",
            "cas hub start",
        ),
    };
    let mut message = format!("Hub transport required: Tailscale is unavailable.\n{operation}");
    if let Some(issue) = issue {
        message.push('\n');
        message.push_str(issue);
    }
    message.push_str(&format!(
        "\nRetry: TAILSCALE_SOCKET=<signed-in-socket> {retry}.\nOr run `{loopback}`."
    ));
    message
}

fn execute_command(args: &HubArgs, cli: &Cli) -> Result<()> {
    let requested = match &args.command {
        Some(HubCommands::Start(_) | HubCommands::Restart(_) | HubCommands::Serve(_))
        | Some(HubCommands::Service(HubServiceArgs {
            command: HubServiceCommands::Install(_),
            ..
        })) => tailscale_policy(
            args.tailscale_serve,
            args.no_tailscale_serve,
            if args.tailscale_serve || args.no_tailscale_serve {
                true
            } else {
                host_tailscale_default()?
            },
        ),
        _ => args.tailscale_serve,
    };
    if requested && host_transport_mode()? == HubTransportMode::Required
        && matches!(args.command, Some(HubCommands::Start(_) | HubCommands::Restart(_))) {
        let paths = HubRuntimePaths::default_for_user()?;
        let manager = TailscaleServeManager::new(paths.root());
        if !manager.cli_available() {
            anyhow::bail!(required_transport_refusal(args, &paths, None));
        }
        let issue = manager.login_issue().unwrap_or_else(|error| Some(error.to_string()));
        if let Some(issue) = issue {
            anyhow::bail!(required_transport_refusal(args, &paths, Some(&issue)));
        }
    }
    match args.command.clone().unwrap_or_else(default_hub_command) {
        HubCommands::Start(serve) => start(&serve, cli, requested, args.tailscale_serve_port),
        HubCommands::ReapDaemon(reap) => reap_factory_daemon(&reap),
        HubCommands::Serve(serve) => serve_foreground(&serve, requested, args.tailscale_serve_port),
        HubCommands::Status => status(cli),
        HubCommands::Stop(stop_args) => stop(cli, stop_args.force),
        HubCommands::Restart(serve) => {
            let paths = HubRuntimePaths::default_for_user()?;
            let (tailscale_serve, tailscale_port) =
                resolve_lifecycle_tailscale_request(requested, args.tailscale_serve_port, &paths)?;
            if super::hub_service::restart_supervised(cli, tailscale_serve, tailscale_port)? {
                return status(cli);
            }
            // `hub restart` is a stop-to-relaunch, so its stop carries the
            // intent: if a concurrent lifecycle command already produced a hub
            // with these flags, the restart's goal is met and waiting out the
            // machine lock would be exactly the cas-bf90 stall. Plain
            // `hub stop` still goes through stop(), which passes no intent.
            match stop_with_output(
                cli,
                !cli.json,
                Some(RelaunchIntent {
                    args: &serve,
                    tailscale_serve,
                    tailscale_port,
                }),
                serve.force,
            )? {
                StopOutcome::AlreadySatisfied(record) => {
                    if cli.json {
                        let stop = HubRestartStopReceipt::already_satisfied();
                        let start = HubStartOutcome {
                            record: *record,
                            already_satisfied: true,
                        }
                        .into_restart_json()?;
                        println!(
                            "{}",
                            serde_json::json!({"stop": stop, "start": start})
                        );
                    } else {
                        println!(
                            "hub already running (pid {}, version {}) — a concurrent start won the race",
                            record.pid, record.version
                        );
                    }
                    Ok(())
                }
                StopOutcome::Stopped(stop) => {
                    let stop = HubRestartStopReceipt::from(stop);
                    let start = match start_with_output_resolved(
                        &serve,
                        cli,
                        tailscale_serve,
                        tailscale_port,
                        !cli.json,
                        cli_launch_origin(),
                    ) {
                        Ok(start) => start,
                        Err(error) if cli.json => {
                            let message = super::json_error_message(&error);
                            println!(
                                "{}",
                                serde_json::json!({
                                    "status": "error",
                                    "message": message,
                                    "stop": stop,
                                })
                            );
                            return Err(super::JsonReportedFailure(error).into());
                        }
                        Err(error) => return Err(error),
                    };
                    if cli.json {
                        let start = start.into_restart_json()?;
                        println!("{}", serde_json::json!({"stop": stop, "start": start}));
                    }
                    Ok(())
                }
            }
        }
        HubCommands::Service(service) => super::hub_service::manage_service(
            &service.command,
            cli,
            requested,
            args.tailscale_serve_port,
        ),
        HubCommands::Pair(pair) => pair_device(&pair, cli),
        HubCommands::Authorize(authorize) => super::hub_reverse_pairing::authorize(&authorize, cli),
        HubCommands::Auth(auth) => manage_auth(&auth, cli),
        HubCommands::Operator(operator) => super::hub_operator::execute(&operator, cli),
    }
}

#[cfg(unix)]
fn reap_factory_daemon(args: &HubReapDaemonArgs) -> Result<()> {
    use std::process::{Command, Stdio};
    let executable = std::env::current_exe()?;
    let store = crate::hub::DaemonExitEvidenceStore::default_for_user()
        .ok_or_else(|| anyhow::anyhow!("home directory unavailable for daemon exit receipts"))?;
    let supervisor_cli = args
        .supervisor_cli
        .parse::<cas_mux::SupervisorCli>()
        .map_err(|error| anyhow::anyhow!(error))?;
    anyhow::ensure!(
        matches!(
            supervisor_cli,
            cas_mux::SupervisorCli::Claude
                | cas_mux::SupervisorCli::Codex
                | cas_mux::SupervisorCli::Grok
        ),
        "unsupported supervisor CLI"
    );
    let child = Command::new(executable)
        .args(["factory", "daemon", "--session", &args.session, "--cwd"])
        .arg(&args.cwd)
        .args([
            "--workers",
            &args.workers.to_string(),
            "--supervisor-cli",
            supervisor_cli.backend().name(),
            "--worker-cli",
            supervisor_cli.backend().name(),
            "--foreground",
        ])
        .stdin(Stdio::null())
        .spawn()?;
    let receipt = crate::hub::reap_spawned_daemon(&args.session, child, store)?;
    let code = match receipt.exit {
        crate::hub::ProcessExit::Code(code) => code,
        crate::hub::ProcessExit::Signal(signal) => 128 + signal,
    };
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

#[cfg(not(unix))]
fn reap_factory_daemon(_args: &HubReapDaemonArgs) -> Result<()> {
    anyhow::bail!("factory daemon reaper requires Unix")
}

fn start(args: &HubServeArgs, cli: &Cli, tailscale_serve: bool, tailscale_port: u16) -> Result<()> {
    if super::hub_service::installed_service_exists()? {
        validate_control_bind(SocketAddr::new(args.bind, args.port), TransportSecurity::Plaintext)?;
        let paths = HubRuntimePaths::default_for_user()?;
        let (enabled, port) = resolve_lifecycle_tailscale_request(tailscale_serve, tailscale_port, &paths)?;
        let record = paths.read_process_record().ok();
        anyhow::ensure!(args.port == 0 || args.port == DEFAULT_HUB_PORT
            || record.as_ref().is_some_and(|record| record.port == args.port),
            "The installed hub service owns its listener port.\nUse `cas hub restart`, or uninstall the service to choose another port.");
        let mut request = args.clone();
        if request.port == DEFAULT_HUB_PORT {
            // A flagless start inherits the installed service's listener.
            request.port = 0;
        }
        if record.as_ref().is_some_and(|record| {
            record.launched_by.as_deref() == Some("service")
                && record_is_ready(&paths, record)
                && running_hub_satisfies_request(
                    record, &request, enabled, port, env!("CARGO_PKG_VERSION"),
                    publication_required(),
                )
        }) {
            return status(cli);
        }
        if super::hub_service::restart_supervised(cli, enabled, port)? {
            return status(cli);
        }
    }
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
    let (tailscale_serve, tailscale_port) = resolve_lifecycle_tailscale_request(
        tailscale_serve,
        tailscale_port,
        &paths,
    )?;
    start_with_output_resolved(
        args,
        cli,
        tailscale_serve,
        tailscale_port,
        emit_output,
        launch_origin,
    )
    .map(|_| ())
}

fn peer_hub_record_after_child_exit(
    paths: &HubRuntimePaths,
    launcher_pid: u32,
) -> Result<Option<u32>> {
    let mut deadline = None;
    loop {
        let peer_holders = paths
            .lock_holders()
            .into_iter()
            .filter(|holder| holder.pid != launcher_pid && process_is_running(holder.pid))
            .map(|holder| holder.pid)
            .collect::<Vec<_>>();
        if let Some(record) = paths.read_process_record().ok()
            && peer_holders.contains(&record.pid)
        {
            return Ok(Some(record.pid));
        }
        if peer_holders.is_empty() {
            return Ok(None);
        }
        if deadline.is_none() {
            hold_hub_test_barrier(
                "CAS_TEST_HUB_PEER_RECORD_WAIT_BARRIER",
                "peer-record-wait",
                "release-peer-record-wait",
                launcher_pid,
            )?;
            deadline = Some(Instant::now() + HUB_PEER_RECORD_PUBLISH_TIMEOUT);
        } else if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Ok(None);
        }
        std::thread::sleep(HUB_PEER_RECORD_POLL_INTERVAL);
    }
}

fn start_with_output_resolved(
    args: &HubServeArgs,
    cli: &Cli,
    tailscale_serve: bool,
    tailscale_port: u16,
    emit_output: bool,
    launch_origin: HubLaunchOrigin,
) -> Result<HubStartOutcome> {
    let paths = HubRuntimePaths::default_for_user()?;
    validate_control_bind(
        SocketAddr::new(args.bind, args.port),
        TransportSecurity::Plaintext,
    )?;
    crate::hub::ensure_private_dir(paths.root())?;
    match paths.read_process_record() {
        Ok(record) if record_is_ready(&paths, &record) => {
            match decide_live_start(
                &record,
                args,
                tailscale_serve,
                tailscale_port,
                env!("CARGO_PKG_VERSION"),
                publication_required(),
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
                                render_status(
                                    &record,
                                    HubDisplayState::Running,
                                    env!("CARGO_PKG_VERSION"),
                                )
                            );
                        }
                    }
                    return Ok(HubStartOutcome {
                        record,
                        already_satisfied: true,
                    });
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
                        args.force,
                    )? {
                        if emit_output && cli.json {
                            println!("{}", serde_json::to_string(&record)?);
                        } else if emit_output {
                            println!(
                                "hub already running (pid {}, version {}) — a concurrent start won the race",
                                record.pid, record.version
                            );
                        }
                        return Ok(HubStartOutcome {
                            record: *record,
                            already_satisfied: true,
                        });
                    }
                    return start_with_output_resolved(
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
            return Ok(HubStartOutcome {
                record: *record,
                already_satisfied: true,
            });
        }
    };
    cleanup_stale_launcher_files(paths.root());
    let prior_record = paths.read_process_record().ok();
    let stale_cgroup = prior_record.as_ref().and_then(|record| record.cgroup.clone());
    let prior_serve_target = prior_record.as_ref().and_then(|record| {
        Some((record.tailscale_serve_port?, record.tailscale_serve_target.clone()?))
    });
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
    let startup_error_file = paths
        .root()
        .join(format!(".launcher-{stamp}-{}.error", std::process::id()));
    let _startup_error_file_guard = ScopedFile::new(startup_error_file.clone());
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
        .arg("--startup-error-file")
        .arg(&startup_error_file)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(error_log));
    if tailscale_serve {
        command
            .arg("--tailscale-serve")
            .arg("--tailscale-serve-port")
            .arg(tailscale_port.to_string());
    } else {
        command.arg("--no-tailscale-serve");
    }
    if let Some(executable) = std::env::var_os("TAILSCALE").filter(|value| !value.is_empty()) {
        command.env("TAILSCALE", executable);
    }
    if let Some(socket) = std::env::var_os("TAILSCALE_SOCKET")
        .or_else(|| TailscaleServeManager::new(paths.root()).socket().map(ToOwned::to_owned)) {
        command.env("TAILSCALE_SOCKET", socket);
    }
    if let Some((port, target)) = prior_serve_target {
        command
            .arg("--prior-tailscale-serve-port")
            .arg(port.to_string())
            .arg("--prior-tailscale-serve-target")
            .arg(target);
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
        crate::ui::factory::cgroup::join_shared_scope(
            &session,
            &format!("hub-{launcher_pid}"),
            launcher_pid,
        )
    });
    drop(launch_guard);
    hold_hub_test_barrier(
        "CAS_TEST_HUB_LAUNCH_BARRIER",
        "launcher",
        "release-launchers",
        launcher_pid,
    )?;
    let launch_metadata = cgroup
        .as_deref()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Err(error) = std::fs::write(&launch_file, launch_metadata) {
        terminate_failed_launch(&mut child, cgroup.as_deref());
        return Err(error.into());
    }
    hold_hub_test_barrier(
        "CAS_TEST_HUB_LAUNCH_BARRIER",
        "monitor",
        "release-monitors",
        launcher_pid,
    )?;

    let deadline = Instant::now() + hub_launch_timeout(tailscale_serve);
    while Instant::now() < deadline {
        if let Ok(record) = paths.read_process_record() {
            if record_is_ready(&paths, &record) {
                let already_satisfied = record.pid != launcher_pid
                    || record.launched_at.as_deref() != Some(launched_at.as_str());
                if already_satisfied {
                    terminate_failed_launch(&mut child, cgroup.as_deref());
                }
                if let Err(error) = hold_hub_test_barrier(
                    "CAS_TEST_HUB_READY_BARRIER",
                    "ready-observer",
                    "release-ready-observers",
                    launcher_pid,
                ) {
                    terminate_failed_launch(&mut child, cgroup.as_deref());
                    return Err(error);
                }
                if tailscale_serve && host_transport_mode()? == HubTransportMode::Required
                    && let Some(warning) = &record.transport_warning
                {
                    let warning = warning.clone();
                    stop_with_output(cli, false, None, false)?;
                    anyhow::bail!("Tailscale Serve failed: {warning}; use `cas hub start --loopback` for local access");
                }
                if emit_output && cli.json {
                    println!("{}", serde_json::to_string(&record)?);
                } else if emit_output {
                    let endpoint = record
                        .public_url
                        .as_deref()
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("http://{}:{}", record.bind, record.port));
                    println!("Cassy hub started at {endpoint} (pid {})", record.pid);
                }
                return Ok(HubStartOutcome {
                    record,
                    already_satisfied,
                });
            }
        }
        if let Some(status) = child.try_wait().context("poll detached cas hub")? {
            let winner_pid = peer_hub_record_after_child_exit(&paths, launcher_pid)?;
            if let Some(winner_pid) = winner_pid {
                let error = anyhow::anyhow!(
                    "another cas hub instance won the machine lock (pid {winner_pid}); replacement process exited with {status} before becoming ready; inspect {}",
                    paths.log_path().display()
                );
                terminate_failed_launch(&mut child, cgroup.as_deref());
                return Err(error);
            }
            let startup_reason = std::fs::read_to_string(&startup_error_file)
                .ok()
                .filter(|reason| !reason.trim().is_empty())
                .map(|reason| format!(": {}", reason.trim()))
                .unwrap_or_default();
            let error = anyhow::anyhow!(
                "cas hub replacement exited with {status} before becoming ready{startup_reason}; inspect {}",
                paths.log_path().display()
            );
            terminate_failed_launch(&mut child, cgroup.as_deref());
            return Err(error);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if let Ok(record) = paths.read_process_record()
        && record.pid != launcher_pid
        && record_is_live(&record)
        && running_hub_satisfies_request(
            &record,
            args,
            tailscale_serve,
            tailscale_port,
            env!("CARGO_PKG_VERSION"),
            publication_required(),
        )
    {
        if record_is_ready(&paths, &record) {
            terminate_failed_launch(&mut child, cgroup.as_deref());
            return Ok(HubStartOutcome {
                record,
                already_satisfied: true,
            });
        }
        if paths
            .lock_holders()
            .iter()
            .any(|holder| holder.pid == record.pid)
        {
            let pid = record.pid;
            terminate_failed_launch(&mut child, cgroup.as_deref());
            anyhow::bail!(
                "another cas hub instance won the machine lock (pid {pid}); this restart launch did not become ready before timeout; inspect {}",
                paths.log_path().display()
            );
        }
    }
    let error = anyhow::anyhow!(
        "cas hub did not become ready; inspect {}",
        paths.log_path().display()
    );
    terminate_failed_launch(&mut child, cgroup.as_deref());
    Err(error)
}

struct ScopedFile(std::path::PathBuf);

fn cleanup_stale_launcher_files(root: &std::path::Path) {
    cleanup_stale_launcher_files_with(root, process_is_running);
}

fn cleanup_stale_launcher_files_with(
    root: &std::path::Path,
    is_alive: impl Fn(u32) -> bool,
) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|file_type| file_type.is_file()) {
            continue;
        }
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some(name) = name.strip_prefix(".launcher-") else {
            continue;
        };
        let Some((stem, extension)) = name.rsplit_once('.') else {
            continue;
        };
        if !matches!(extension, "pid" | "go" | "error") {
            continue;
        }
        let Some((stamp, pid)) = stem.rsplit_once('-') else {
            continue;
        };
        if stamp.parse::<i64>().is_err() {
            continue;
        }
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        if pid == 0 || pid > i32::MAX as u32 || is_alive(pid) {
            continue;
        }
        let _ = std::fs::remove_file(entry.path());
    }
}

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

/// One hub.log breadcrumb, prefixed with an RFC 3339 UTC timestamp so the
/// log can be lined up against the service manager's journal.
fn hub_log_line(message: &str) -> String {
    format!(
        "{} {message}",
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    )
}

fn serve_foreground(args: &HubServeArgs, tailscale_serve: bool, tailscale_port: u16) -> Result<()> {
    // A Commander client that disconnects mid-response must not kill the hub
    // (cas-621ec): hyper writes with writev, which raises SIGPIPE.
    crate::server_signals::ignore_sigpipe_for_server();
    let addr = SocketAddr::new(args.bind, args.port);
    validate_control_bind(addr, TransportSecurity::Plaintext)?;
    let paths = HubRuntimePaths::default_for_user()?;
    // Create the log before any optional external probe. A failed or
    // non-responsive Tailscale CLI must leave an operator-visible startup
    // breadcrumb even when the service manager captures stdout separately.
    crate::hub::ensure_private_dir(paths.root())?;
    let mut startup_log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.log_path())?;
    // One "starting" line per process start. The matching "exited" line below
    // tells a clean stop or an error apart from a kill: a start line with no
    // exit line before the next start means the process died by signal.
    writeln!(
        startup_log,
        "{}",
        hub_log_line(&format!(
            "cas hub serve starting (tailscale_serve={tailscale_serve}, bind={addr}, pid={}, launched_by={})",
            std::process::id(),
            args.launched_by,
        ))
    )?;
    // cas-cb94: launchd hands the hub a soft limit of 256 descriptors; a
    // request burst exhausted it on prowl and every auth-store read failed.
    let nofile_limit = match crate::hub::open_files::raise_nofile_limit(
        crate::hub::open_files::HUB_NOFILE_TARGET,
    ) {
        Ok(limit) => {
            writeln!(
                startup_log,
                "{}",
                hub_log_line(&format!(
                    "cas hub serve open-file limit {} (was {}, hard {})",
                    limit.soft, limit.before, limit.hard
                ))
            )?;
            Some(limit.soft)
        }
        Err(error) => {
            writeln!(
                startup_log,
                "{}",
                hub_log_line(&format!("cas hub serve open-file limit unchanged: {error}"))
            )?;
            None
        }
    };
    startup_log.flush()?;
    let result = serve_foreground_logged(
        args,
        tailscale_serve,
        tailscale_port,
        addr,
        paths,
        startup_log.try_clone()?,
        nofile_limit,
    );
    let exit_line = match &result {
        Ok(()) => format!("cas hub serve exited cleanly (pid={})", std::process::id()),
        Err(error) => format!(
            "cas hub serve exited with error (pid={}): {error:#}",
            std::process::id()
        ),
    };
    let _ = writeln!(startup_log, "{}", hub_log_line(&exit_line));
    let _ = startup_log.flush();
    result
}

fn serve_foreground_logged(
    args: &HubServeArgs,
    tailscale_serve: bool,
    tailscale_port: u16,
    addr: SocketAddr,
    paths: HubRuntimePaths,
    mut startup_log: std::fs::File,
    nofile_limit: Option<u64>,
) -> Result<()> {
    let (tailscale_serve, tailscale_port) = resolve_lifecycle_tailscale_request(
        tailscale_serve,
        tailscale_port,
        &paths,
    )?;
    let transport_mode = if tailscale_serve {
        match host_transport_mode()? {
            // An explicit --tailscale-serve overrides the saved loopback choice.
            HubTransportMode::Loopback => HubTransportMode::Auto,
            configured => configured,
        }
    } else {
        HubTransportMode::Loopback
    };
    // AuthStore::open takes an exclusive auth.lock. Do this before claiming
    // hub.lock: a stale auth writer must never leave a launchd hub process
    // holding the machine lock while it waits indefinitely.
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
        // Bind before hub.lock so a failed port acquisition cannot strand a
        // machine-lock owner with no listener to diagnose or stop.
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let actual = listener.local_addr()?;
        let lock = paths.try_acquire_instance_lock()?;
        if lock.is_none() {
            hold_hub_test_barrier(
                "CAS_TEST_HUB_STARTUP_LOCK_BARRIER",
                "startup-lock-waiter",
                "release-startup-lock",
                std::process::id(),
            )?;
        }
        // The launcher hands ownership to this process without an atomic lock
        // transfer. Let a concurrent stop finish its bounded cleanup rather
        // than treating a transient owner in that gap as a failed launch.
        let mut lock = match lock {
            Some(lock) => lock,
            None => match paths.wait_for_instance_lock(HUB_STARTUP_LOCK_TIMEOUT) {
                Ok(lock) => lock,
                Err(error) => {
                    // Keep this launch's actual failure separate from shared hub.log.
                    if let Some(path) = &args.startup_error_file {
                        let _ = std::fs::write(path, format!("{error:#}"));
                    }
                    return Err(error);
                }
            },
        };
        let tailscale_manager = TailscaleServeManager::new(paths.root());
        let prior_serve_target = args.prior_tailscale_serve_port.zip(args.prior_tailscale_serve_target.clone())
            .or_else(|| paths.read_process_record().ok().and_then(|previous| {
                Some((previous.tailscale_serve_port?, previous.tailscale_serve_target?))
            }))
            .filter(|_| tailscale_serve);
        let started_at = chrono::Utc::now().to_rfc3339();
        let mut record = HubProcessRecord {
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
            public_url: None,
            // A legacy runtime record may be the only ownership evidence.
            // Carry it through pending and failed starts until publication
            // replaces it below. public_url=None and the starting lock phase
            // keep this old route from being mistaken for a ready new shim.
            tailscale_serve_port: prior_serve_target.as_ref().map(|(port, _)| *port),
            tailscale_cli: tailscale_serve.then(|| tailscale_manager.executable_display()),
            tailscale_serve_target: prior_serve_target.as_ref().map(|(_, target)| target.clone()),
            transport_warning: None,
            nofile_limit,
        };
        hold_hub_test_barrier(
            "CAS_TEST_HUB_RECORD_PUBLICATION_BARRIER",
            "hub-record-pending",
            "release-hub-record",
            record.pid,
        )?;
        paths.write_process_record(&record)?;
        hold_hub_test_barrier(
            "CAS_TEST_HUB_STARTUP_BARRIER",
            "hub-starting",
            "release-hub-startup",
            record.pid,
        )?;
        let mut startup_guard = HubStartupGuard::new(paths.clone(), tailscale_manager.clone());

        let (tailscale_listener, tailscale, transport_warning) = if tailscale_serve {
            let proxy_listener =
                tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
            let proxy_port = proxy_listener.local_addr()?.port();
            let ensure_started = Instant::now();
            let service_startup = args.launched_by == "service";
            // Service Auto begins serving immediately, even if a CLI command
            // hangs. Its publication work runs beside both listeners below.
            let ensure_result = if service_startup && transport_mode == HubTransportMode::Auto {
                Err(anyhow::anyhow!("Tailscale Serve publication pending; retrying in background"))
            } else if service_startup {
                tailscale_manager.ensure_for_service(proxy_port, tailscale_port, prior_serve_target.clone()).await
            } else {
                tailscale_manager.ensure_with_prior_target(proxy_port, tailscale_port, prior_serve_target.clone())
            };
            let _ = writeln!(
                startup_log,
                "{}",
                hub_log_line(&format!(
                    "Tailscale Serve ensure {} in {}ms",
                    match &ensure_result {
                        Ok(_) => "succeeded".to_owned(),
                        Err(error) => format!("failed: {error:#}"),
                    },
                    ensure_started.elapsed().as_millis(),
                )),
            );
            match ensure_result {
                Ok(receipt) => (Some(proxy_listener), Some(receipt), None),
                Err(error) if publication_failure_is_fatal(transport_mode) => {
                    if service_startup {
                        // Keep the cause observable between manager restarts. The
                        // next start captures old ownership before replacing this
                        // record, and a successful publication clears the warning.
                        let _ = tailscale_manager.disable_owned();
                        record.transport_warning = Some(format!("{error:#}"));
                        paths.write_process_record(&record)?;
                        startup_guard.disarm();
                    }
                    let context = if service_startup {
                        "Tailscale Serve service startup exhausted its bounded retries"
                    } else {
                        "Hub transport required: Tailscale Serve startup failed"
                    };
                    return Err(error).context(context);
                }
                Err(error) => {
                    let warning = error.to_string();
                    tracing::warn!(%warning, "Tailscale Serve refused; keeping Commander loopback-only");
                    (Some(proxy_listener), None, Some(warning))
                }
            }
        } else {
            (None, None, None)
        };
        record.public_url = tailscale.as_ref().map(|receipt| receipt.public_url.clone());
        // Auto fallback can be a successful local start while the old Serve
        // route still points at a dead shim. Keep exact legacy ownership for
        // background recovery; public_url never advertises it as ready.
        if let Some(receipt) = &tailscale {
            record.tailscale_serve_port = Some(receipt.https_port);
            record.tailscale_serve_target = Some(receipt.local_target.clone());
        }
        record.transport_warning = transport_warning;
        paths.write_process_record(&record)?;

        let catalog = SessionCatalog::new(LocalSessionReadModel::default());
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
        let publication_cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (publication_stop, publication_stopped) = tokio::sync::watch::channel(false);
        let publication_task = if tailscale.is_none() && let Some(proxy) = &tailscale_listener {
            let proxy_port = proxy.local_addr()?.port();
            let manager = tailscale_manager.clone().with_cancellation(publication_cancelled.clone());
            let state = state.clone();
            let paths = paths.clone();
            let record = record.clone();
            let log = startup_log.try_clone()?;
            Some(tokio::spawn(recover_tailscale_publication(
                manager, proxy_port, tailscale_port, prior_serve_target,
                paths, record, state, log, publication_stopped,
            )))
        } else { None };
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

        // cas-9b7d: upload bound projects' operator messages to the account
        // inbox, independent of viewer presence. Idle until enrolled.
        let operator_inbox_task =
            crate::hub::operator_inbox::drain::spawn_drain_loop(paths.root().to_path_buf());
        // cas-f1fb: sync registered cloud-linked stores no session has open.
        let idle_sync_loop =
            crate::hub::idle_sync::spawn_idle_sync_loop(paths.root().to_path_buf());
        let presence_task =
            crate::hub::operator_inbox::presence::spawn_presence_loop(paths.root().to_path_buf());
        // cas-2f55: delete secrets saved with a stated time once it passes.
        let secret_sweep_task = crate::hub::secret_auto_delete::spawn_sweep_loop();

        lock.set_phase("running")?;
        let result = if let Some(proxy_listener) = tailscale_listener {
            serve_with_trusted_tls_proxy(listener, state, proxy_listener).await
        } else {
            serve_with_bounded_connection_drain(listener, router(state)).await
        };
        // Join the current CLI probe before owned-route cleanup. Cancelling
        // only its async parent would leave spawn_blocking free to republish
        // after stop. The probe reaps its own process group on cancellation.
        publication_cancelled.store(true, std::sync::atomic::Ordering::Release);
        let _ = publication_stop.send(true);
        if let Some(task) = publication_task { let _ = task.await; }
        event_task.abort();
        operator_inbox_task.abort();
        idle_sync_loop.stop();
        presence_task.abort();
        secret_sweep_task.abort();
        if let Some(task) = attention_task {
            task.abort();
        }
        let _ = lock.set_phase("stopping");
        startup_guard.disarm();
        // A service manager stops a foreground hub with SIGTERM instead of
        // routing through `cas hub stop`. Always tear down only Cassy's exact
        // owned mapping here so an uninstall/reboot cannot leave a stale
        // Tailscale Serve publication behind. A restart republishes it from
        // the same private receipt and keeps machine identity/auth untouched.
        let _ = tailscale_manager.disable_owned();
        paths.remove_process_record()?;
        #[cfg(debug_assertions)]
        hold_instance_lock_after_record_removal_for_test()?;
        result
    })
}

/// Auto publication recovers in place; the listeners, pairing state and
/// process identity survive every failure. Foreign handlers still pass through
/// the manager's exact ownership checks on every attempt.
async fn recover_tailscale_publication<R: crate::hub::SessionReadModel>(
    manager: TailscaleServeManager,
    proxy_port: u16,
    https_port: u16,
    prior_target: Option<(u16, String)>,
    paths: HubRuntimePaths,
    mut record: HubProcessRecord,
    state: HubState<R>,
    mut log: std::fs::File,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let mut delay = Duration::from_secs(1);
    loop {
        if *stop.borrow() { return; }
        let attempt_manager = manager.clone();
        let prior = prior_target.clone();
        let result = tokio::task::spawn_blocking(move || {
            attempt_manager.ensure_with_prior_target(proxy_port, https_port, prior)
        }).await;
        if *stop.borrow() { return; }
        let result = result.unwrap_or_else(|error| Err(anyhow::anyhow!("Tailscale publication task failed: {error}")));
        match result {
            Ok(receipt) => {
                record.public_url = Some(receipt.public_url.clone());
                record.tailscale_serve_port = Some(receipt.https_port);
                record.tailscale_serve_target = Some(receipt.local_target);
                record.transport_warning = None;
                match paths.write_process_record(&record) {
                    Ok(()) => {
                        state.publish_tailscale_transport(&receipt.public_url);
                        let _ = writeln!(log, "{}", hub_log_line("Tailscale Serve publication recovered in background"));
                        return;
                    }
                    Err(error) => {
                        // Retry persistence too; the owned receipt safely
                        // recognizes this same live shim on the next pass.
                        record.public_url = None;
                        record.transport_warning = Some(format!("Cannot record Tailscale publication: {error}"));
                    }
                }
            }
            Err(error) => {
                record.transport_warning = Some(format!("{error:#}"));
                let _ = paths.write_process_record(&record);
            }
        }
        if let Some(warning) = &record.transport_warning {
            let _ = writeln!(log, "{}", hub_log_line(&format!(
                "Tailscale Serve unavailable; keeping loopback and retrying in {}s: {warning}", delay.as_secs())));
        }
        tokio::select! {
            _ = stop.changed() => return,
            _ = tokio::time::sleep(delay) => {},
        }
        delay = (delay * 2).min(Duration::from_secs(30));
    }
}

struct HubStartupGuard {
    paths: HubRuntimePaths,
    tailscale_manager: TailscaleServeManager,
    armed: bool,
}

impl HubStartupGuard {
    fn new(paths: HubRuntimePaths, tailscale_manager: TailscaleServeManager) -> Self {
        Self {
            paths,
            tailscale_manager,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for HubStartupGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.tailscale_manager.disable_owned();
            let _ = self.paths.remove_process_record();
        }
    }
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
fn hold_hub_test_barrier(
    environment: &str,
    marker: &str,
    release: &str,
    pid: u32,
) -> Result<()> {
    use std::fs;
    use std::path::PathBuf;

    let Some(root) = std::env::var_os(environment) else {
        return Ok(());
    };
    let root = PathBuf::from(root);
    fs::create_dir_all(&root)?;
    fs::write(
        root.join(format!("{marker}-{pid}")),
        format!("{}\n", std::process::id()),
    )?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while !root.join(release).exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    anyhow::ensure!(
        root.join(release).exists(),
        "test hub barrier {environment} timed out"
    );
    Ok(())
}

#[cfg(not(debug_assertions))]
fn hold_hub_test_barrier(
    _environment: &str,
    _marker: &str,
    _release: &str,
    _pid: u32,
) -> Result<()> {
    Ok(())
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

#[cfg(debug_assertions)]
fn mark_restart_stop_waiter_for_test() -> Result<()> {
    let Some(root) = std::env::var_os("CAS_TEST_HUB_LOCK_RELEASE_BARRIER") else {
        return Ok(());
    };
    let root = std::path::PathBuf::from(root);
    std::fs::write(
        root.join(format!("restart-stop-waiter-{}", std::process::id())),
        b"waiting\n",
    )?;
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

/// The account's home from the passwd database, never `$HOME` (which any
/// process can point at a scratch directory).
fn account_home() -> Option<std::path::PathBuf> {
    nix::unistd::User::from_uid(nix::unistd::getuid())
        .ok()
        .flatten()
        .map(|user| user.dir)
}

/// cas-3c26: whether `hub_root` is the account's own machine hub, the one
/// that controls the operator's real factory panes. A hub under another
/// `HOME` (a test fixture's) controls nothing real. Unknown fails closed.
pub(crate) fn pairing_targets_operator_hub(
    hub_root: &std::path::Path,
    account_home: Option<&std::path::Path>,
) -> bool {
    let Some(home) = account_home else {
        return true;
    };
    let operator_root = HubRuntimePaths::for_home(home).root().to_path_buf();
    let canonical =
        |path: &std::path::Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    canonical(hub_root) == canonical(&operator_root)
}

/// cas-3c26: admit a `cas hub pair`. Minting an invitation hands a device
/// the scopes it names (`pane:input`, `message:send` drive this machine's
/// agents), so only the operator does it: an agent context is refused, then
/// the operator confirms the origin and scopes. `confirm` receives the
/// summary and answers the prompt.
pub(crate) fn pairing_admission(
    context: &crate::config::operator_policy::InvocationContext,
    origin: &str,
    scopes: &[Scope],
    confirm: &mut dyn FnMut(&str) -> bool,
) -> std::result::Result<(), String> {
    if let Some(refusal) = crate::config::operator_policy::operator_action_refusal(
        "Pairing a Commander device",
        "run `cas hub pair` yourself from your own terminal",
        context,
    ) {
        return Err(refusal);
    }
    let names = |control: bool| {
        let names: Vec<&str> = scopes
            .iter()
            .filter(|scope| super::hub_reverse_pairing::is_control_scope(**scope) == control)
            .map(|scope| scope.as_str())
            .collect();
        if names.is_empty() {
            "none".to_string()
        } else {
            names.join(", ")
        }
    };
    let summary = format!(
        "Pair a Commander device from {origin}\nRead scopes: {}\nControl scopes: {} \
         (control lets the device type into panes, message and interrupt this machine's agents)",
        names(false),
        names(true)
    );
    if !confirm(&summary) {
        return Err("Pairing not confirmed by the operator; no invitation was minted.".to_string());
    }
    Ok(())
}

fn pair_device(args: &HubPairArgs, cli: &Cli) -> Result<()> {
    let scopes: Vec<Scope> = args
        .scopes
        .iter()
        .map(|scope| Scope::parse(scope))
        .collect::<Result<_>>()?;
    // cas-e719: the launch smoke runs under a throwaway HOME, whose hub the
    // gate below would not consider the operator's, so it checks the gate
    // itself rather than through `pairing_targets_operator_hub`.
    if args.check_launch {
        if let Some(refusal) = crate::config::operator_policy::operator_action_refusal(
            "Pairing a Commander device",
            "run `cas hub pair` yourself from your own terminal",
            &crate::config::operator_policy::InvocationContext::from_process(),
        ) {
            anyhow::bail!(refusal);
        }
        println!("launch check: the operator may pair a Commander device");
        return Ok(());
    }
    // cas-3c26: only the operator mints an invitation for their own machine
    // hub, and only after confirming the origin and scopes at their own
    // terminal. A hub under another HOME (a journey fixture) controls nothing
    // real and is not gated.
    let operator_hub = pairing_targets_operator_hub(
        HubRuntimePaths::default_for_user()?.root(),
        account_home().as_deref(),
    );
    if operator_hub {
        pairing_admission(
            &crate::config::operator_policy::InvocationContext::from_process(),
            &args.origin,
            &scopes,
            &mut |summary| {
                eprintln!("{summary}");
                inquire::Confirm::new("Mint this one-time pairing invitation?")
                    .with_default(false)
                    .prompt()
                    .unwrap_or(false)
            },
        )
        .map_err(anyhow::Error::msg)?;
    }
    let scopes = scopes.into_iter().collect();
    let paths = HubRuntimePaths::default_for_user()?;
    let configured_hub_url = crate::store::find_cas_root()
        .ok()
        .and_then(|cas_root| Config::load(&cas_root).ok())
        .and_then(|config| config.hub.and_then(|hub| hub.public_url));
    let prefill = PairingPrefill {
        hub_url: super::hub_reverse_pairing::pairing_prefill_hub_url(
            &paths,
            args.hub_url.as_deref(),
            configured_hub_url.as_deref(),
        )?,
        machine_label: Some(super::hub_reverse_pairing::machine_display_label()),
    };
    let invitation = auth_store()?
        .mint_pairing(&args.origin, scopes, chrono::Utc::now())?
        .with_prefill(prefill);
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
    let probe_window_started_at = chrono::Utc::now();
    let paths = HubRuntimePaths::default_for_user()?;
    let record = match paths.read_process_record() {
        Ok(record) => record,
        Err(error) => {
            let holder = paths.lock_holders().into_iter().next();
            let service_warning =
                super::hub_service::inactive_detached_warning(&paths, None)?;
            // An installed service owns (re)starts; `cas hub start` would only
            // add an unsupervised detached hub next to it.
            let missing_remedy = if service_warning.is_some() {
                "Run `cas hub restart`."
            } else {
                "Run `cas hub start`."
            };
            let transport = hub_transport_report(&paths, None);
            if cli.json {
                let runtime_receipt = runtime_receipt(
                    &paths, None, None, &transport, probe_window_started_at,
                );
                println!(
                    "{}",
                    serde_json::json!({
                        "running": false,
                        "state": holder.as_ref().map(lock_holder_state_json).unwrap_or_else(|| serde_json::json!({
                            "kind": "missing",
                            "pid": null,
                            "age_secs": null,
                            "message": "no runtime record",
                            "remedy": missing_remedy,
                        })),
                        "record": null,
                        "binary": env!("CARGO_PKG_VERSION"),
                        "lock_holder": holder.as_ref().map(lock_holder_json),
                        "tailscale_serve": transport,
                        "service_warning": service_warning,
                        "service_status": service_status(service_warning),
                        "runtime_receipt": runtime_receipt,
                    })
                );
            } else if let Some(holder) = &holder {
                let state = lock_holder_display_state(holder);
                println!(
                    "Cassy hub: {}; no runtime record",
                    hub_state_label(state, holder.pid)
                );
                println!("  remedy: {}", hub_state_remedy(state, holder.pid));
                if let Some(warning) = service_warning {
                    println!("ERROR: {warning}");
                }
                println!("{}", render_transport_status(&transport));
            } else {
                println!("Cassy hub is not running: no runtime record");
                println!("  remedy: {missing_remedy}");
                if let Some(warning) = service_warning {
                    println!("ERROR: {warning}");
                }
            }
            let detail = holder
                .as_ref()
                .map(|holder| {
                    format!(
                        "lock holder pid {} ({}) — {}",
                        holder.pid,
                        holder.age_label(),
                        transport.message_with_remedy()
                    )
                })
                .unwrap_or_else(|| error.to_string());
            return Err(super::JsonReportedFailure(anyhow::anyhow!(
                "cas hub runtime record unavailable: {detail}"
            ))
            .into());
        }
    };
    let record_state = hub_display_state(&paths, &record);
    // A replacement can own the machine lock before publishing its record.
    // Keep the dead record as history, but report the live holder's identity
    // and phase, just as `hub service status` does (cas-ebbb).
    let holder = if record_state == HubDisplayState::Exited {
        paths.lock_holders().into_iter().next()
    } else {
        None
    };
    let state = holder.as_ref().map(lock_holder_display_state).unwrap_or(record_state);
    let pid = holder.as_ref().map(|holder| holder.pid).unwrap_or(record.pid);
    let live = state == HubDisplayState::Running;
    let service_warning = super::hub_service::inactive_detached_warning(&paths, Some(&record))?;
    let transport = hub_transport_report(&paths, Some(&record));
    // cas-0140: a failing audit writer refuses every audited request and
    // used to be visible only as a log that fell silent.
    let audit = crate::hub::audit_writer_report(paths.root(), chrono::Utc::now());
    let open_files = crate::hub::open_files::OpenFilesReport::for_process(
        pid,
        if holder.is_some() { None } else { record.nofile_limit },
    );
    if cli.json {
        let runtime_receipt = runtime_receipt(
            &paths, Some(&record), Some(state), &transport, probe_window_started_at,
        );
        println!(
            "{}",
            serde_json::json!({
                "running": live,
                "state": holder.as_ref().map(lock_holder_state_json)
                    .unwrap_or_else(|| hub_record_state_json(&record, state)),
                "record": record,
                "lock_holder": holder.as_ref().map(lock_holder_json),
                "binary": env!("CARGO_PKG_VERSION"),
                "version_warning": version_warning(state, &record.version, env!("CARGO_PKG_VERSION")),
                "tailscale_serve": transport,
                "service_warning": service_warning,
                "service_status": service_status(service_warning),
                "audit": audit,
                "open_files": open_files,
                "runtime_receipt": runtime_receipt,
            })
        );
    } else {
        if holder.is_some() {
            println!("Cassy hub: {}", hub_state_label(state, pid));
            println!("  remedy: {}", hub_state_remedy(state, pid));
        } else {
            println!(
                "{}",
                render_status(&record, state, env!("CARGO_PKG_VERSION"))
            );
        }
        if let Some(warning) = service_warning {
            println!("ERROR: {warning}");
        }
        println!("{}", render_transport_status(&transport));
        println!("{}", render_audit_status(&audit));
        println!("{}", render_open_files_status(&open_files));
    }
    status_verdict(live, service_warning, &transport, &audit)
        .map_err(|error| super::JsonReportedFailure(error).into())
}

/// Failures found after `hub status` printed its report.
fn status_verdict(
    live: bool,
    service_warning: Option<&str>,
    transport: &HubTransportReport,
    audit: &crate::hub::AuditWriterReport,
) -> Result<()> {
    if transport.is_supervised_unavailable() {
        anyhow::bail!("Tailscale Serve publication failed; see status above");
    }
    anyhow::ensure!(live, "cas hub is not ready; see status above");
    // cas-621ec: a hub serving outside its installed service has no restart
    // supervision; that is an error, not a footnote.
    if let Some(warning) = service_warning {
        anyhow::bail!("cas hub {warning}");
    }
    anyhow::ensure!(
        !transport.is_failure(),
        "Tailscale Serve check failed: {}",
        transport.message_with_remedy()
    );
    anyhow::ensure!(!audit.is_failure(), "hub audit writer is failing: {}", audit.message);
    Ok(())
}

fn runtime_receipt(
    paths: &HubRuntimePaths,
    record: Option<&HubProcessRecord>,
    state: Option<HubDisplayState>,
    transport: &HubTransportReport,
    probe_window_started_at: chrono::DateTime<chrono::Utc>,
) -> crate::hub::observation::RuntimeReceipt {
    use crate::hub::observation::{
        Observation, ObservationState, collect_runtime_receipt, requested_publication,
    };
    let hub = match state {
        Some(HubDisplayState::Running) => Observation::new(
            ObservationState::Healthy,
            "loopback_health_and_lock_ready",
            "current_status_probe",
        ),
        Some(HubDisplayState::Exited) => Observation::new(
            ObservationState::Failed,
            "hub_process_exited",
            "current_status_probe",
        ),
        Some(HubDisplayState::Unresponsive { .. }) => Observation::new(
            ObservationState::Failed,
            "hub_health_unresponsive",
            "current_status_probe",
        ),
        Some(HubDisplayState::Starting { wedged: true, .. }) => Observation::new(
            ObservationState::Failed,
            "hub_startup_wedged",
            "current_status_probe",
        ),
        Some(HubDisplayState::Starting { .. }) => Observation::new(
            ObservationState::Unknown,
            "hub_starting",
            "current_status_probe",
        ),
        Some(HubDisplayState::Stopping { .. }) => Observation::new(
            ObservationState::Unknown,
            "hub_stopping",
            "current_status_probe",
        ),
        None => Observation::new(
            ObservationState::Unknown,
            "runtime_record_unavailable",
            "current_status_probe",
        ),
    };
    let publication = if transport.is_failure() {
        Observation::new(
            ObservationState::Failed,
            "owned_serve_route_not_verified",
            "current_status_probe",
        )
    } else if record.is_some_and(|record| record.transport_warning.is_some()) {
        Observation::new(
            ObservationState::Failed,
            "serve_publication_unavailable",
            "current_status_probe",
        )
    } else if requested_publication(paths) == Some(false)
        && record.is_none_or(|r| r.tailscale_serve_target.is_none())
    {
        Observation::new(
            ObservationState::Disabled,
            "host_publication_opt_out",
            "host_config_and_current_status_probe",
        )
    } else if state == Some(HubDisplayState::Running)
        && record.is_some_and(|r| {
            tailscale_enabled(r) && r.tailscale_serve_target.is_some() && r.public_url.is_some()
        })
    {
        Observation::new(
            ObservationState::Healthy,
            "owned_serve_route_matches_live_hub",
            "current_status_probe",
        )
    } else {
        Observation::new(
            ObservationState::Unknown,
            "no_owned_publication_observed",
            "current_status_probe",
        )
    };
    collect_runtime_receipt(paths, record, hub, publication, probe_window_started_at)
}
/// The status screen's audit line (cas-0140): OK with the last row's age, or
/// FAIL with when the writer started failing and why.
fn render_audit_status(report: &crate::hub::AuditWriterReport) -> String {
    let verdict = match report.status {
        "failing" => "FAIL",
        "unknown" => "WARN",
        _ => "OK",
    };
    // cas-cfea: the healthy idle explanation ("last row Nh ago; a hub writes
    // rows only for authenticated requests") overran 80 columns on one line.
    // Keep the verdict and age on the status line and indent the explanation
    // beneath it; `--json` keeps the one-line message.
    if report.status == "ok"
        && let Some((age, why)) = report.message.split_once("; ")
    {
        return format!("Audit log: {verdict} - {age}\n  {why}");
    }
    format!("Audit log: {verdict} - {}", report.message)
}

fn render_open_files_status(report: &crate::hub::open_files::OpenFilesReport) -> String {
    let verdict = match report.status {
        "warning" => "WARN",
        "ok" => "OK",
        _ => "UNKNOWN",
    };
    format!("Open files: {verdict} - {}", report.message)
}

/// `--json` severity of the installed-service finding (cas-621ec).
fn service_status(finding: Option<&str>) -> &'static str {
    match finding {
        Some(
            super::hub_service::MANAGER_TIMEOUT_WARNING
            | super::hub_service::MANAGER_UNAVAILABLE_WARNING,
        ) => "unknown",
        Some(_) => "error",
        None => "ok",
    }
}

fn render_transport_status(report: &HubTransportReport) -> String {
    if !report.is_failure() {
        if report.is_signed_in_loopback_warning() {
            return "Tailscale Serve: WARN - Tailscale is signed in; hub remains loopback-only"
                .to_owned();
        }
        if report
            .message
            .starts_with("hub is loopback-only; Tailscale Serve publication")
        {
            return "Tailscale Serve: WARN - unavailable; hub remains loopback-only".to_owned();
        }
        if report
            .message
            .starts_with("hub is loopback-only; no CAS-created Tailscale Serve route")
        {
            return "Tailscale Serve: OK - loopback-only; no CAS-created route".to_owned();
        }
        let message = report
            .message
            .strip_prefix("Tailscale Serve ")
            .unwrap_or(&report.message);
        return format!("Tailscale Serve: OK - {message}");
    }

    let mut lines = if let Some(cause) = report.message.strip_prefix(
        "hub is supervised but not publishable: Tailscale Serve is unavailable: "
    ) {
        let mut lines = vec!["Tailscale Serve: FAIL - supervised hub is not publishable".to_owned()];
        lines.extend(crate::cloud::presence::wrap_line(&format!("Cause: {cause}"))
            .lines().map(|line| format!("  {line}")));
        lines
    } else if let Some(issue) = report.message.strip_prefix("Tailscale: ") {
        issue.lines().enumerate().map(|(index, line)| {
            if index == 0 { format!("Tailscale Serve: FAIL - {line}") }
            else { format!("  {line}") }
        }).collect()
    } else if report.message.starts_with("hub machine lock") {
        vec![format!(
            "Tailscale Serve: FAIL - {}",
            report.message.trim_start_matches("hub machine lock: ")
        )]
    } else if report.message.ends_with(NO_LIVE_SHIM) {
        // GH #1258: a route left behind with no live shim is missing, even
        // when it still names the recorded target.
        vec!["Tailscale Serve: FAIL - the hub owns no live Serve shim for its route".to_owned()]
    } else if report.expected_target.is_some() && report.actual_target.is_some() {
        vec!["Tailscale Serve: FAIL - route target differs from the live hub shim".to_owned()]
    } else {
        vec!["Tailscale Serve: FAIL - CAS ownership receipt is missing".to_owned()]
    };
    match (&report.expected_target, &report.actual_target) {
        (Some(expected), Some(actual)) if expected == actual => {
            lines.push(format!("  route: {expected}"));
        }
        (expected, actual) => {
            lines.extend(expected.iter().map(|expected| format!("  expected: {expected}")));
            lines.extend(actual.iter().map(|actual| format!("  actual: {actual}")));
        }
    }
    if let Some(remedy) = &report.remedy {
        lines.push(format!("  remedy: {remedy}"));
    }
    lines.join("\n")
}

fn lock_holder_json(holder: &HubLockHolder) -> serde_json::Value {
    serde_json::json!({
        "pid": holder.pid,
        "age": holder.age_label(),
        "phase": holder.phase,
        "command": holder.command,
    })
}

fn stop(cli: &Cli, force: bool) -> Result<()> {
    let deadline = super::hub_service::stop_deadline();
    super::hub_service::stop_supervised(deadline)?;
    // No relaunch intent: a live hub can never mean success here, so this
    // keeps the pre-cas-bf90 behaviour exactly.
    stop_with_output_before(cli, true, None, force, Some(deadline)).map(|_| ())
}

/// Stop a live hub that is outside the installed service before the service
/// manager is asked to start its own instance.
pub(crate) fn stop_for_service(cli: &Cli, force: bool) -> Result<()> {
    stop_with_output(cli, false, None, force).map(|_| ())
}

fn stop_with_output(
    cli: &Cli,
    emit_output: bool,
    relaunch: Option<RelaunchIntent<'_>>,
    force: bool,
) -> Result<StopOutcome> {
    stop_with_output_before(cli, emit_output, relaunch, force, None)
}

fn stop_wait_budget(deadline: Option<crate::bounded_process::Deadline>, cap: Duration) -> Duration {
    deadline.map_or(cap, |deadline| deadline.remaining().min(cap))
}

fn ensure_stop_budget(deadline: Option<crate::bounded_process::Deadline>) -> Result<()> {
    anyhow::ensure!(deadline.is_none_or(|deadline| !deadline.remaining().is_zero()),
        "cas hub stop exceeded its 15s end-state budget");
    Ok(())
}

fn stop_with_output_before(
    cli: &Cli,
    emit_output: bool,
    relaunch: Option<RelaunchIntent<'_>>,
    force: bool,
    stop_deadline: Option<crate::bounded_process::Deadline>,
) -> Result<StopOutcome> {
    ensure_stop_budget(stop_deadline)?;
    let paths = HubRuntimePaths::default_for_user()?;
    let record = paths.read_process_record().ok();
    let stale_pid = record.as_ref().map(|record| record.pid);
    let hub_cgroup = record.as_ref().and_then(|record| record.cgroup.clone());
    let manager = TailscaleServeManager::new(paths.root());
    let tailscale_manager = match stop_deadline {
        Some(deadline) => manager.within_deadline(deadline),
        None => manager,
    };
    // Capture the exact mapping we own before asking the hub to exit. The
    // foreground process now always tears it down on SIGTERM, so stop must
    // judge the final outcome instead of which process issued `serve off`.
    let owned_tailscale_receipt = tailscale_manager.owned_receipt();
    let cleanup_lock = if record.as_ref().is_some_and(record_is_live) {
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
        hold_hub_test_barrier(
            "CAS_TEST_HUB_STOP_WAIT_BARRIER",
            "stop-wait",
            "release-stop-wait",
            std::process::id(),
        )?;
        wait_for_stop_or_satisfying_hub(
            &paths,
            Some(record.pid),
            stale_pid,
            stop_wait_budget(stop_deadline, HUB_LIFECYCLE_TIMEOUT),
            relaunch.as_ref(),
            force,
            stop_deadline,
        )?
    } else {
        // Record absence does not authorize stale cleanup: a shutting-down hub
        // may already have removed it while still holding the machine lock.
        wait_for_stop_or_satisfying_hub(
            &paths,
            None,
            stale_pid,
            stop_wait_budget(stop_deadline, HUB_LIFECYCLE_TIMEOUT),
            relaunch.as_ref(),
            force,
            stop_deadline,
        )?
    };
    let cleanup_lock = match cleanup_lock {
        StopWait::AlreadySatisfied(record) => {
            // Adoption skips teardown of the live peer, but not removal of an
            // empty old scope. Never kill here or remove a shared/ancestor
            // scope containing the winner (including legacy shared scopes).
            if let Some(old_scope) = hub_cgroup.as_deref()
                && record.cgroup.as_deref().is_none_or(|live_scope| !live_scope.starts_with(old_scope))
            {
                crate::ui::factory::cgroup::remove_scope(old_scope);
            }
            return Ok(StopOutcome::AlreadySatisfied(record));
        }
        StopWait::Quiescent(lock) => lock,
    };
    hold_hub_test_barrier(
        "CAS_TEST_HUB_STOP_CLEANUP_BARRIER",
        "stop-cleanup",
        "release-stop-cleanup",
        std::process::id(),
    )?;
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
    ensure_stop_budget(stop_deadline)?;
    // Launchers and foreground hubs publish under this same machine lock.
    // Releasing it before cleanup would let a replacement's record or owned
    // Tailscale mapping be removed by the previous generation's stop.
    drop(cleanup_lock);
    let receipt = HubStopReceipt {
        stopped: true,
        pid: record.as_ref().map(|record| record.pid),
        tailscale_serve_removed: matches!(&tailscale_outcome, Ok((true, _))),
        tailscale_warning: tailscale_outcome
            .as_ref()
            .err()
            .map(ToString::to_string),
    };
    if emit_output && cli.json {
        println!("{}", serde_json::to_string(&receipt)?);
    } else if emit_output {
        if let Some(record) = &record {
            println!("Cassy hub stopped (pid {})", record.pid);
        } else {
            println!("Cassy hub was not running");
        }
        match &tailscale_outcome {
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
    Ok(StopOutcome::Stopped(receipt))
}

fn update_prior_state(
    paths: &HubRuntimePaths,
    record: Option<&HubProcessRecord>,
    holder: Option<&HubLockHolder>,
    has_receipt: bool,
) -> &'static str {
    let state = record.map(|record| hub_display_state(paths, record)).or_else(|| {
        holder.map(lock_holder_display_state)
    });
    match state {
        Some(HubDisplayState::Exited) => "exited",
        Some(HubDisplayState::Starting { wedged: false, .. }) => "starting",
        Some(HubDisplayState::Starting { wedged: true, .. }) => "startup_wedged",
        Some(HubDisplayState::Stopping { .. }) => "stopping",
        Some(HubDisplayState::Unresponsive { .. }) => "unresponsive",
        Some(HubDisplayState::Running) => "running",
        None if has_receipt => "exited",
        None => "none",
    }
}

fn update_restart_spec(
    record: Option<&HubProcessRecord>,
    receipt: Option<&TailscaleServeReceipt>,
    transport_mode: HubTransportMode,
) -> Result<HubRestartSpec> {
    let bind = record
        .map(|record| record.bind.parse())
        .transpose()
        .context("invalid bind address in hub process record")?
        .unwrap_or(IpAddr::from([127, 0, 0, 1]));
    Ok(HubRestartSpec {
        bind,
        // A recordless legacy route does not preserve the old bind port. Let
        // the kernel choose a free port rather than colliding with another
        // hub's well-known port on the same machine.
        port: record.map_or(0, |record| record.port),
        transport_mode,
        tailscale_port: receipt
            .map(|receipt| receipt.https_port)
            .or_else(|| record.and_then(|record| record.tailscale_serve_port))
            .unwrap_or(443),
    })
}

fn verify_updated_hub(
    paths: &HubRuntimePaths,
    binary_version: &str,
    spec: &HubRestartSpec,
) -> Result<HubUpdateVerification> {
    let record = paths.read_process_record().context("new hub has no process record")?;
    anyhow::ensure!(record.version == binary_version, "hub still runs version {}", record.version);
    anyhow::ensure!(record.bind == spec.bind.to_string() && (spec.port == 0 || record.port == spec.port),
        "hub restarted with different bind or port");
    let holder = paths.read_lock_owner().context("new hub does not hold its machine lock")?;
    anyhow::ensure!(holder.pid == record.pid && holder.phase == "running",
        "new hub lock is not in running phase");
    anyhow::ensure!(record_is_live(&record), "new hub loopback /v1/health is not ready");
    if !spec.transport_mode.publishes() {
        anyhow::ensure!(
            !tailscale_enabled(&record),
            "hub still requests Tailscale Serve despite opt-out"
        );
        return Ok(HubUpdateVerification {
            public_url: None,
            transport_verified: None,
            transport_warning: None,
            remedy: None,
        });
    }
    if let Some(warning) = record.transport_warning.as_deref() {
        spec.transport_mode.require_publication(false).with_context(|| warning.to_owned())?;
        return Ok(unavailable_update_transport(warning));
    }
    let manager = TailscaleServeManager::new(paths.root());
    let receipt = manager.owned_receipt()?.context("Tailscale Serve ownership receipt is missing")?;
    anyhow::ensure!(receipt.https_port == spec.tailscale_port,
        "Tailscale Serve published the wrong HTTPS port");
    anyhow::ensure!(record.tailscale_serve_target.as_deref() == Some(receipt.local_target.as_str()),
        "Tailscale Serve receipt does not name the new hub shim");
    let handlers = manager.serve_handlers(spec.tailscale_port)?;
    anyhow::ensure!(handlers == vec![("/".to_owned(), receipt.local_target.clone())],
        "Tailscale Serve status does not route to the new hub shim");
    let public_url = record.public_url.as_deref().context("new hub has no Tailscale public URL")?;
    let health_url = format!("{}/v1/health", public_url.trim_end_matches('/'));
    let public_health: Result<serde_json::Value> = ureq::get(&health_url)
        .timeout(Duration::from_secs(5))
        .call()
        .with_context(|| format!("public Tailscale /v1/health unavailable at {health_url}"))
        .and_then(|response| {
            response
                .into_json()
                .context("public Tailscale /v1/health returned invalid JSON")
        });
    let public_failure = match public_health {
        Ok(health) if health["schema_version"] == 1 && health["ready"] == true => None,
        Ok(_) => Some("public Tailscale /v1/health did not report ready".to_owned()),
        Err(error) => Some(format!("{error:#}")),
    };
    spec.transport_mode.require_publication(public_failure.is_none())?;
    let remedy = public_failure.as_ref().map(|_| {
        "Check MagicDNS and `tailscale status`, then retry the public URL; the hub and Serve route are healthy.".to_owned()
    });
    Ok(HubUpdateVerification {
        public_url: Some(public_url.to_owned()),
        transport_verified: Some(public_failure.is_none()),
        transport_warning: public_failure.map(|failure| format!("{health_url}: {failure}")),
        remedy,
    })
}

fn unavailable_update_transport(warning: &str) -> HubUpdateVerification {
    HubUpdateVerification {
        public_url: None,
        transport_verified: Some(false),
        transport_warning: Some(format!(
            "Tailscale Serve inactive: {warning}; hub remains loopback-only"
        )),
        remedy: Some("Check `tailscale status`, then run `cas hub restart`.".to_owned()),
    }
}

fn should_start_stopped_hub(service_installed: bool, identity_exists: bool) -> bool {
    service_installed || identity_exists
}

fn finish_update_hub_verification(
    outcome: &mut HubRestartOutcome,
    verification: HubUpdateVerification,
    binary_version: &str,
    cli: &Cli,
) {
    outcome.verified = true;
    outcome.loopback_verified = true;
    outcome.transport_verified = verification.transport_verified;
    outcome.transport_warning = verification.transport_warning;
    outcome.current_version = Some(binary_version.to_owned());
    outcome.public_url = verification.public_url;
    outcome.failure = None;
    outcome.remedy = verification.remedy;
    if !cli.json {
        let transport = if outcome.transport_verified == Some(false) {
            if outcome.public_url.is_some() {
                "loopback (public Tailscale URL needs attention)"
            } else {
                "loopback (Tailscale Serve inactive)"
            }
        } else {
            outcome.public_url.as_deref().unwrap_or("loopback")
        };
        let restart = if outcome.action == "restarted" || outcome.action == "started" {
            format!(
                " → {}{}",
                outcome.action,
                if outcome.recovery_attempted {
                    " after one recovery"
                } else {
                    ""
                }
            )
        } else {
            String::new()
        };
        println!(
            "cas update: hub was {}{restart} → verified at {transport}",
            outcome.prior_state,
        );
        if let Some(warning) = outcome.transport_warning.as_deref() {
            eprintln!("{}", format_update_transport_warning(warning));
            if let Some(remedy) = outcome.remedy.as_deref() {
                eprintln!("cas update: {remedy}");
            }
        }
    }
}

fn format_update_transport_warning(warning: &str) -> String {
    if let Some(cause) = warning.strip_prefix("Tailscale Serve inactive: ") {
        format!("cas update: Tailscale Serve inactive:\n  {cause}")
    } else {
        format!("cas update: {warning}")
    }
}

fn capture_update_hub_evidence(paths: &HubRuntimePaths, reason: &str) {
    use std::io::{Read, Seek, SeekFrom};
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let evidence = paths.root().join(format!("update-recovery-{stamp}.txt"));
    let mut tail = Vec::new();
    if let Ok(mut log) = std::fs::File::open(paths.log_path()) {
        if let Ok(len) = log.metadata().map(|metadata| metadata.len()) {
            let _ = log.seek(SeekFrom::Start(len.saturating_sub(8192)));
            let _ = log.read_to_end(&mut tail);
        }
    }
    let _ = std::fs::write(&evidence, format!(
        "verification failure: {reason}\nprocess record: {:?}\nlock: {:?}\nhub.log tail:\n{}",
        paths.read_process_record().ok(),
        paths.read_lock_owner(),
        String::from_utf8_lossy(&tail),
    ));
    #[cfg(target_os = "macos")]
    if let Some(pid) = paths.read_process_record().ok().map(|record| record.pid) {
        let _ = Command::new("sample")
            .args([pid.to_string(), "2".to_owned(), "-file".to_owned(),
                paths.root().join(format!("update-recovery-{stamp}.sample.txt")).display().to_string()])
            .status();
    }
    #[cfg(target_os = "linux")]
    if let Some(pid) = paths.read_process_record().ok().map(|record| record.pid) {
        if let Ok(stack) = std::fs::read(format!("/proc/{pid}/stack")) {
            let _ = std::fs::write(paths.root().join(format!("update-recovery-{stamp}.stack.txt")), stack);
        }
    }
}

/// After a binary swap, converge every previously present hub to the new
/// binary and verify both loopback and the requested transport.
pub(crate) fn restart_stale_hub(
    binary_version: &str,
    cli: &Cli,
) -> Result<HubRestartOutcome> {
    // cas-621ec: rewrite an installed unit to the current restart policy
    // before any early return below, so a hub that needs no restart, or is
    // not running at all, still gets the new policy on this update.
    let transport_mode = host_transport_mode()?;
    let tailscale_serve = transport_mode.publishes();
    // cas-93356: this update installed a newer release, which supersedes a hotfix pin.
    let pin_advanced = super::hub_service::advance_managed_hub_pin()
        .context("cannot advance the hub pin; see `scripts/hub-pin.sh inspect` before retrying `cas update`")?;
    // Never restart through a definition we could not validate or refresh:
    // the manager would simply launch its old candidate again.
    let service_changed = super::hub_service::refresh_installed_service(tailscale_serve)
        .context("cannot refresh hub service; run `cas hub service install` before retrying `cas update`")?
        || pin_advanced;
    let paths = HubRuntimePaths::default_for_user()?;
    let record = paths.read_process_record().ok();
    let holder = paths.lock_holders().into_iter().next();
    let receipt = TailscaleServeManager::new(paths.root())
        .owned_receipt()
        .ok()
        .flatten();
    let prior_state =
        update_prior_state(&paths, record.as_ref(), holder.as_ref(), receipt.is_some());
    let mut spec = update_restart_spec(record.as_ref(), receipt.as_ref(), transport_mode)?;
    if record.is_none()
        && receipt.is_none()
        && let Some(port) = super::hub_service::installed_https_port()?
    {
        spec.tailscale_port = port;
    }
    let mut outcome = HubRestartOutcome {
        prior_state: prior_state.to_owned(),
        action: "skipped".to_owned(),
        previous_version: record.as_ref().map(|record| record.version.clone()),
        ..Default::default()
    };
    if prior_state == "none"
        && !should_start_stopped_hub(
            super::hub_service::installed_service_exists()?,
            paths.root().join("machine-id").is_file(),
        )
    {
        outcome.remedy = Some("Hub not running; start with `cas hub start`.".to_owned());
        if !cli.json {
            println!("cas update: hub not running; start with `cas hub start`");
        }
        return Ok(outcome);
    }
    if !service_changed
        && prior_state == "running"
        && record
            .as_ref()
            .is_some_and(|record| record.version == binary_version)
        && let Ok(verification) = verify_updated_hub(&paths, binary_version, &spec)
    {
        outcome.action = "verified".to_owned();
        outcome.service_managed = record
            .as_ref()
            .is_some_and(|record| record.launched_by.as_deref() == Some("service"));
        finish_update_hub_verification(&mut outcome, verification, binary_version, cli);
        return Ok(outcome);
    }
    let args = HubServeArgs {
        bind: spec.bind,
        port: spec.port,
        ..HubServeArgs::default()
    };
    let attempt = || -> Result<bool> {
        // The service manager owns its own stop/start sequence; a detached hub
        // must be stopped through the same force path as `cas hub restart`.
        if super::hub_service::restart_supervised(cli, spec.transport_mode.publishes(), spec.tailscale_port)? {
            return Ok(true);
        }
        let stopped = stop_with_output(
            cli,
            false,
            Some(RelaunchIntent {
                args: &args,
                tailscale_serve: spec.transport_mode.publishes(),
                tailscale_port: spec.tailscale_port,
            }),
            true,
        )?;
        if !matches!(stopped, StopOutcome::AlreadySatisfied(_)) {
            start_with_output_from(
                &args,
                cli,
                spec.transport_mode.publishes(),
                spec.tailscale_port,
                false,
                HubLaunchOrigin::Update,
            )?;
        }
        Ok(false)
    };
    outcome.action = if matches!(prior_state, "none" | "exited") {
        "started"
    } else {
        "restarted"
    }
    .to_owned();
    // Starting a previously absent hub is best effort: one bounded attempt
    // leaves an explicit receipt but cannot block installation/refresh.
    let attempts = if prior_state == "none" { 1 } else { 2 };
    for number in 0..attempts {
        if number == 1 {
            outcome.recovery_attempted = true;
        }
        let result = attempt().and_then(|service_managed| {
            outcome.service_managed = service_managed;
            verify_updated_hub(&paths, binary_version, &spec)
        });
        match result {
            Ok(verification) => {
                finish_update_hub_verification(&mut outcome, verification, binary_version, cli);
                return Ok(outcome);
            }
            Err(error) => {
                let message = format!("{error:#}");
                if number == 0 {
                    capture_update_hub_evidence(&paths, &message);
                }
                outcome.failure = Some(message);
            }
        }
    }
    outcome.action = if prior_state == "none" {
        "start_failed"
    } else {
        "failed"
    }
    .to_owned();
    if let Ok(record) = paths.read_process_record()
        && record.version == binary_version
    {
        outcome.current_version = Some(binary_version.to_owned());
        outcome.loopback_verified = record_is_ready(&paths, &record);
        outcome.public_url = record.public_url;
    }
    outcome.transport_verified = spec.transport_mode.publishes().then_some(false);
    let remedy = if spec.transport_mode.publishes() {
        "Inspect ~/.cas/hub/update-recovery-*.txt, then run `cas hub restart --force --tailscale-serve`."
    } else {
        "Inspect ~/.cas/hub/update-recovery-*.txt, then run `cas hub restart --force`."
    };
    outcome.remedy = Some(remedy.to_owned());
    if spec.transport_mode.publishes()
        && (prior_state != "none" || transport_mode == HubTransportMode::Required) {
        outcome.transport_error = outcome.failure.as_ref().map(|failure| {
            format!("cas update: Tailscale Serve verification failed: {failure}; {remedy}")
        });
    }
    if !cli.json {
        let operation = if prior_state == "none" {
            "hub start failed; update continues"
        } else {
            "hub restart verification failed after one recovery"
        };
        eprintln!(
            "cas update: {operation}: {}; {remedy}",
            outcome.failure.as_deref().unwrap_or("unknown failure")
        );
    }
    Ok(outcome)
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

pub(crate) fn record_is_live(record: &HubProcessRecord) -> bool {
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

/// A startup record is durable before optional transport setup completes. A
/// lifecycle caller may treat it as ready only after the lock owner reaches
/// the running phase. Missing metadata is accepted only for records written
/// by older binaries, which had no phase marker. Current-version records fail
/// closed when the metadata is missing or being rewritten: `set_phase` updates
/// the lock file in place, and accepting a transiently empty file would expose
/// the startup record before optional transport fields (including `public_url`)
/// have been written.
fn record_is_ready(paths: &HubRuntimePaths, record: &HubProcessRecord) -> bool {
    if record.version == env!("CARGO_PKG_VERSION")
        && record.tailscale_cli.is_some()
        && record.public_url.is_none()
        && record.transport_warning.is_none()
    {
        // The startup record advertises the requested optional transport
        // before ensure() has either published it or recorded its warning.
        // Keep lifecycle callers from treating that intermediate shape as
        // ready even if they observe a phase update racing the record rename.
        return false;
    }
    if !record_is_live(record) {
        return false;
    }
    match paths.read_lock_owner() {
        Some(owner) => {
            owner.pid == record.pid
                && (owner.phase == "running"
                    || (owner.phase.is_empty() && record.version != env!("CARGO_PKG_VERSION")))
        }
        None => record.version != env!("CARGO_PKG_VERSION"),
    }
}

pub(crate) fn hub_transport_report(
    paths: &HubRuntimePaths,
    record: Option<&HubProcessRecord>,
) -> HubTransportReport {
    let manager = TailscaleServeManager::new(paths.root());
    let live = record.is_some_and(|record| record_is_ready(paths, record));
    if live && let Some(warning) = record.and_then(|record| record.transport_warning.as_deref()) {
        return HubTransportReport::ok(format!(
            "hub is loopback-only; Tailscale Serve publication unavailable: {warning}; publication retries automatically"
        ));
    }
    let receipt = match manager.owned_receipt() {
        Ok(receipt) => receipt,
        Err(error) => return HubTransportReport::unavailable(error.to_string()),
    };
    if !live
        && let Some(record) = record
        && record.launched_by.as_deref() == Some("service")
        && let Some(warning) = &record.transport_warning
    {
        return HubTransportReport::supervised_unavailable(warning);
    }
    if let Some(holder) = paths
        .lock_holders()
        .into_iter()
        .find(|holder| !holder.is_stopping())
        && !live
    {
        return HubTransportReport::wedged_lock(
            &holder,
            receipt.is_some() || record.is_some_and(tailscale_enabled),
        );
    }
    if receipt.is_some() {
        let issue = manager.login_issue().unwrap_or_else(|error| Some(error.to_string()));
        if let Some(issue) = issue {
            return HubTransportReport::fail_with_remedy(issue, None, None,
                "Select the signed-in daemon with TAILSCALE_SOCKET, then restart.".to_owned());
        }
    }
    let Some(receipt) = receipt else {
        if record.is_some_and(|record| {
            tailscale_enabled(record) && record.transport_warning.is_none()
        }) {
            return HubTransportReport::fail(
                "the hub record advertises Tailscale Serve but its CAS ownership receipt is missing",
                record.and_then(|record| record.tailscale_serve_target.clone()),
                None,
            );
        }
        if let Some(warning) = record.and_then(|record| record.transport_warning.as_deref()) {
            return HubTransportReport::ok(format!(
                "hub is loopback-only; Tailscale Serve publication was unavailable: {warning}"
            ));
        }
        if live && manager.is_logged_in().unwrap_or(false) {
            return HubTransportReport::ok(
                "hub is loopback-only while Tailscale is signed in; run `cas hub restart --tailscale-serve` to publish the route",
            );
        }
        return HubTransportReport::ok(
            "hub is loopback-only; no CAS-created Tailscale Serve route is recorded",
        );
    };
    let handlers = match manager.serve_handlers(receipt.https_port) {
        Ok(handlers) => handlers,
        Err(error) => return HubTransportReport::unavailable(error.to_string()),
    };
    classify_tailscale_serve(record, live, Some(&receipt), &handlers)
}

/// GH #1258: how a missing-shim failure ends its message, so the status
/// headline names the missing shim rather than a target mismatch.
const NO_LIVE_SHIM: &str = "the current hub does not own a live Serve shim";

fn classify_tailscale_serve(
    record: Option<&HubProcessRecord>,
    live: bool,
    receipt: Option<&TailscaleServeReceipt>,
    handlers: &[(String, String)],
) -> HubTransportReport {
    let Some(receipt) = receipt else {
        return HubTransportReport::ok(
            "hub is loopback-only; no CAS-created Tailscale Serve route is recorded",
        );
    };
    let expected = record
        .and_then(|record| record.tailscale_serve_target.clone())
        .unwrap_or_else(|| receipt.local_target.clone());
    let actual = actual_serve_target(handlers);

    if !live
        || !record.is_some_and(tailscale_enabled)
        || record
            .and_then(|record| record.tailscale_serve_target.as_deref())
            != Some(receipt.local_target.as_str())
    {
        return HubTransportReport::fail(
            format!(
                "CAS-created Tailscale Serve route targets {}, but {NO_LIVE_SHIM}",
                actual.as_deref().unwrap_or("no handler")
            ),
            Some(expected),
            actual,
        );
    }

    let expected_handlers = vec![("/".to_owned(), expected.clone())];
    if handlers != expected_handlers {
        return HubTransportReport::fail(
            format!(
                "Tailscale Serve route target {} does not match the live hub shim {expected}",
                actual.as_deref().unwrap_or("no handler")
            ),
            Some(expected),
            actual,
        );
    }

    HubTransportReport::ok(format!(
        "Tailscale Serve route targets the live hub shim at {expected}"
    ))
}

fn actual_serve_target(handlers: &[(String, String)]) -> Option<String> {
    if handlers.len() == 1 && handlers[0].0 == "/" {
        return Some(handlers[0].1.clone());
    }
    (!handlers.is_empty()).then(|| {
        handlers
            .iter()
            .map(|(path, target)| format!("{path} -> {target}"))
            .collect::<Vec<_>>()
            .join(", ")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invocation(
        ancestors: &[&str],
        tty: bool,
    ) -> crate::config::operator_policy::InvocationContext {
        crate::config::operator_policy::InvocationContext {
            env_names: Default::default(),
            ancestors: ancestors.iter().map(|line| line.to_string()).collect(),
            stdin_is_terminal: tty,
            stdout_is_terminal: tty,
            cgroup: String::new(),
        }
    }

    /// cas-3c26: only the account's own hub is gated; a fixture hub under a
    /// scratch HOME is not, and an unknown account home fails closed.
    #[test]
    fn cas_3c26_only_the_operators_own_hub_is_gated() {
        let account = tempfile::tempdir().unwrap();
        let fixture = tempfile::tempdir().unwrap();
        let operator_hub = HubRuntimePaths::for_home(account.path());
        std::fs::create_dir_all(operator_hub.root()).unwrap();
        assert!(pairing_targets_operator_hub(
            operator_hub.root(),
            Some(account.path())
        ));
        let fixture_hub = HubRuntimePaths::for_home(fixture.path());
        assert!(!pairing_targets_operator_hub(
            fixture_hub.root(),
            Some(account.path())
        ));
        assert!(
            pairing_targets_operator_hub(fixture_hub.root(), None),
            "unknown fails closed"
        );
        // A symlink to the operator's hub is the operator's hub.
        let link = fixture.path().join("link-home");
        std::os::unix::fs::symlink(account.path(), &link).unwrap();
        assert!(pairing_targets_operator_hub(
            HubRuntimePaths::for_home(&link).root(),
            Some(account.path())
        ));
    }

    /// cas-3c26: an agent context is refused before any prompt, even through
    /// a PTY wrapper; the operator must confirm the origin and scopes.
    #[test]
    fn cas_3c26_hub_pair_refuses_agents_and_requires_operator_confirmation() {
        let scopes = [Scope::MachineRead, Scope::PaneInput, Scope::MessageSend];
        let origin = "https://commander.example";
        let mut asked = Vec::new();

        let agent = invocation(&["script -qc cas hub pair", "claude"], true);
        let refused = pairing_admission(&agent, origin, &scopes, &mut |summary| {
            asked.push(summary.to_string());
            true
        })
        .unwrap_err();
        assert!(refused.contains("refused"), "{refused}");
        assert!(asked.is_empty(), "an agent is never even asked");

        let operator = invocation(&["-bash"], true);
        let declined = pairing_admission(&operator, origin, &scopes, &mut |summary| {
            asked.push(summary.to_string());
            false
        })
        .unwrap_err();
        assert!(declined.contains("not confirmed"), "{declined}");
        let summary = asked.pop().expect("the operator was asked");
        assert!(summary.contains(origin), "{summary}");
        assert!(summary.contains("pane:input"), "{summary}");
        assert!(summary.contains("message:send"), "{summary}");
        assert!(summary.to_lowercase().contains("control"), "{summary}");

        assert_eq!(
            pairing_admission(&operator, origin, &scopes, &mut |_| true),
            Ok(())
        );
        // No terminal, so no confirmation: refused.
        assert!(
            pairing_admission(&invocation(&["-bash"], false), origin, &scopes, &mut |_| {
                true
            })
            .is_err()
        );
    }

    #[test]
    fn hub_log_lines_carry_an_rfc3339_utc_timestamp() {
        let message = "cas hub serve starting (tailscale_serve=false, bind=127.0.0.1:4173)";
        let line = hub_log_line(message);
        let (stamp, rest) = line.split_once(' ').unwrap();
        chrono::DateTime::parse_from_rfc3339(stamp)
            .unwrap_or_else(|error| panic!("{stamp:?} is not RFC 3339: {error}"));
        assert!(stamp.ends_with('Z'), "{stamp}");
        assert_eq!(rest, message);
    }

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
            tailscale_serve_target: tailscale_serve_port
                .map(|port| format!("http://127.0.0.1:{port}")),
            transport_warning: None,
            nofile_limit: None,
        }
    }

    #[test]
    fn missing_runtime_receipt_is_read_only_and_unknown() {
        use crate::hub::observation::ObservationState;
        let home = tempfile::tempdir().unwrap();
        let paths = HubRuntimePaths::for_home(home.path());
        let receipt = runtime_receipt(
            &paths,
            None,
            None,
            &HubTransportReport::ok("no CAS-created route"),
            chrono::Utc::now(),
        );
        assert_eq!(receipt.hub.state, ObservationState::Unknown);
        assert_eq!(receipt.serve_publication.state, ObservationState::Unknown);
        // Phase B supports Cloud presence; this local, read-only probe has no
        // authenticated Cloud snapshot and therefore cannot know its health.
        assert_eq!(
            receipt.independent_monitoring.state,
            ObservationState::Unknown
        );
        // pin: local diagnostic receipts name why remote monitoring is unknown.
        assert_eq!(receipt.independent_monitoring.reason, "cloud_snapshot_required");
        assert!(!home.path().join(".cas").exists());
    }

    #[test]
    fn runtime_receipt_preserves_transport_failure_beside_healthy_loopback() {
        use crate::hub::observation::ObservationState;
        let home = tempfile::tempdir().unwrap();
        let paths = HubRuntimePaths::for_home(home.path());
        let live = record(env!("CARGO_PKG_VERSION"), DEFAULT_HUB_PORT, Some(443));
        let transport = HubTransportReport::fail("owned route missing", None, None);
        let receipt = runtime_receipt(
            &paths,
            Some(&live),
            Some(HubDisplayState::Running),
            &transport,
            chrono::Utc::now(),
        );
        assert_eq!(receipt.hub.state, ObservationState::Healthy);
        assert_eq!(receipt.serve_publication.state, ObservationState::Failed);
        assert_eq!(
            receipt.external_reachability.state,
            ObservationState::Unknown
        );
        assert_eq!(receipt.factory.jobs_resumed, None);
        assert!(!receipt.boot_prerequisites.reboot_verified);
    }

    #[test]
    fn lifecycle_preserves_serve_port_but_never_overrides_opt_out() {
        let receipt = TailscaleServeReceipt {
            schema_version: 1,
            public_url: "https://hub.example/".to_owned(),
            local_target: "http://127.0.0.1:43813".to_owned(),
            https_port: 8443,
            created_by_cas: true,
            executable: "tailscale".to_owned(),
            socket: None,
            status_before: serde_json::json!({}),
            status_after: serde_json::json!({}),
            recorded_at: "2026-09-08T00:00:00Z".to_owned(),
        };

        assert_eq!(
            resolved_tailscale_request(true, 443, Some(&receipt)),
            (true, 8443)
        );
        assert_eq!(
            resolved_tailscale_request(true, 9443, Some(&receipt)),
            (true, 9443)
        );
        assert_eq!(
            resolved_tailscale_request(false, 443, Some(&receipt)),
            (false, 443)
        );
        assert_eq!(resolved_tailscale_request(true, 443, None), (true, 443));
    }

    #[test]
    fn serve_report_fails_when_route_target_does_not_match_live_shim() {
        let mut live = record(env!("CARGO_PKG_VERSION"), DEFAULT_HUB_PORT, Some(443));
        live.tailscale_serve_target = Some("http://127.0.0.1:43813".to_owned());
        let receipt = TailscaleServeReceipt {
            schema_version: 1,
            public_url: "https://hub.example/".to_owned(),
            local_target: "http://127.0.0.1:43813".to_owned(),
            https_port: 443,
            created_by_cas: true,
            executable: "tailscale".to_owned(),
            socket: None,
            status_before: serde_json::json!({}),
            status_after: serde_json::json!({}),
            recorded_at: "2026-09-08T00:00:00Z".to_owned(),
        };

        let report = classify_tailscale_serve(
            Some(&live),
            true,
            Some(&receipt),
            &[("/".to_owned(), "http://127.0.0.1:33427".to_owned())],
        );
        assert_eq!(report.status, "fail");
        assert!(report.message.contains("does not match the live hub shim"));
        assert_eq!(report.expected_target.as_deref(), Some("http://127.0.0.1:43813"));
        assert_eq!(report.actual_target.as_deref(), Some("http://127.0.0.1:33427"));
        assert!(report
            .remedy
            .as_deref()
            .is_some_and(|remedy| remedy.contains("cas hub restart --tailscale-serve")));
        assert!(
            render_transport_status(&report)
                .starts_with("Tailscale Serve: FAIL - route target differs from the live hub shim\n"),
            "{}",
            render_transport_status(&report)
        );

        // GH #1258: after a reboot the route still names the recorded target,
        // but no live hub owns a shim behind it. Equal targets are not a
        // mismatch; the shim is missing.
        let orphaned = classify_tailscale_serve(
            Some(&live),
            false,
            Some(&receipt),
            &[("/".to_owned(), "http://127.0.0.1:43813".to_owned())],
        );
        assert_eq!(
            render_transport_status(&orphaned),
            "Tailscale Serve: FAIL - the hub owns no live Serve shim for its route\n  \
             route: http://127.0.0.1:43813\n  \
             remedy: Run `cas hub restart --tailscale-serve` to republish the route."
        );
    }

    #[test]
    fn transport_status_renders_healthy_route_loopback_and_unavailable_shapes() {
        let healthy = HubTransportReport::ok(
            "Tailscale Serve route targets the live hub shim at http://127.0.0.1:38063",
        );
        assert_eq!(
            render_transport_status(&healthy),
            "Tailscale Serve: OK - route targets the live hub shim at http://127.0.0.1:38063"
        );

        let loopback = HubTransportReport::ok(
            "hub is loopback-only; no CAS-created Tailscale Serve route is recorded",
        );
        assert_eq!(
            render_transport_status(&loopback),
            "Tailscale Serve: OK - loopback-only; no CAS-created route"
        );

        let unavailable = HubTransportReport::ok(
            "hub is loopback-only; Tailscale Serve publication was unavailable: tailscale is not installed",
        );
        assert_eq!(
            render_transport_status(&unavailable),
            "Tailscale Serve: WARN - unavailable; hub remains loopback-only"
        );
    }

    #[test]
    fn supervised_unavailable_transport_is_a_failure_with_one_recovery_command() {
        let report = HubTransportReport::supervised_unavailable("tailscale command timed out");
        assert!(report.is_failure());
        assert!(report.message.contains("supervised but not publishable"));
        assert!(report
            .remedy
            .as_deref()
            .is_some_and(|remedy| remedy.contains(
                "cas hub restart --tailscale-serve"
            )));
        assert_eq!(
            render_transport_status(&report),
            "Tailscale Serve: FAIL - supervised hub is not publishable\n  Cause: tailscale command timed out\n  remedy: Run `cas hub restart --tailscale-serve`."
        );
    }

    #[test]
    fn transport_status_renders_signed_in_loopback_warning() {
        let report = HubTransportReport::ok(
            "hub is loopback-only while Tailscale is signed in; run `cas hub restart --tailscale-serve` to publish the route",
        );
        assert_eq!(
            render_transport_status(&report),
            "Tailscale Serve: WARN - Tailscale is signed in; hub remains loopback-only"
        );
    }

    #[test]
    fn wedged_lock_report_names_holder_and_force_remedy() {
        let holder = HubLockHolder {
            pid: 804,
            age: Some(Duration::from_secs(91)),
            phase: Some("starting".to_owned()),
            command: Some("cas hub serve".to_owned()),
        };
        let report = HubTransportReport::wedged_lock(&holder, true);

        assert_eq!(report.status, "fail");
        assert!(report.message.contains("pid 804 is wedged in startup after 91s"));
        assert!(report
            .remedy
            .as_deref()
            .is_some_and(|remedy| remedy.contains("cas hub restart --force --tailscale-serve")));
        assert_eq!(
            render_transport_status(&report),
            "Tailscale Serve: FAIL - pid 804 is wedged in startup after 91s\n  remedy: Run `cas hub restart --force --tailscale-serve` to recover the hub."
        );
    }

    #[test]
    fn slow_tailscale_start_is_still_starting_within_ensure_budget() {
        let holder = HubLockHolder {
            pid: 804,
            age: Some(Duration::from_secs(10)),
            phase: Some("starting".to_owned()),
            command: Some("cas hub serve --tailscale-serve".to_owned()),
        };
        let report = HubTransportReport::wedged_lock(&holder, true);
        assert!(report.message.contains("is starting for 10s"), "{}", report.message);
        assert!(
            report.remedy.as_deref().unwrap().contains("status` again"),
            "{:?}",
            report.remedy
        );
        assert_eq!(hub_launch_timeout(false), HUB_LAUNCH_TIMEOUT);
        assert!(hub_launch_timeout(true) > Duration::from_secs(10));
    }

    #[test]
    fn running_lock_transport_failure_does_not_claim_startup_is_wedged() {
        let holder = HubLockHolder {
            pid: 804,
            age: Some(Duration::from_secs(91)),
            phase: Some("running".to_owned()),
            command: None,
        };
        let report = HubTransportReport::wedged_lock(&holder, true);
        let rendered = render_transport_status(&report);
        assert!(rendered.contains("but not answering"), "{rendered}");
        assert!(!rendered.contains("startup is wedged"), "{rendered}");
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
                VERSION,
                false
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
                VERSION,
                false
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
                VERSION,
                false
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
                VERSION,
                false
            ));
            // Wanted Serve, found plain.
            assert!(!running_hub_satisfies_request(
                &without_ts,
                &ephemeral_args(),
                true,
                TS_PORT,
                VERSION,
                false
            ));
            // Wanted Serve, found Serve on the same port.
            assert!(running_hub_satisfies_request(
                &with_ts,
                &ephemeral_args(),
                true,
                TS_PORT,
                VERSION,
                false
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
                VERSION,
                false
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
                VERSION,
                false
            ));
        }

        #[test]
        fn a_loopback_fallback_settles_serve_only_in_auto_mode() {
            // The hub asked for Serve and fell back to loopback with a warning.
            let mut fallback = record(VERSION, 35053, None);
            fallback.tailscale_cli = Some("tailscale".to_owned());
            fallback.transport_warning = Some("tailscale CLI not found".to_owned());
            let args = ephemeral_args();
            // Auto keeps the live fallback while publication recovers, so a
            // concurrent lifecycle command does not restart or wait it out.
            assert!(running_hub_satisfies_request(
                &fallback, &args, true, TS_PORT, VERSION, false
            ));
            // Required mode still demands publication before accepting it.
            assert!(!running_hub_satisfies_request(
                &fallback, &args, true, TS_PORT, VERSION, true
            ));
            let mut keep_args = args;
            keep_args.port = 35053;
            assert_eq!(
                decide_live_start(&fallback, &keep_args, true, TS_PORT, VERSION, false),
                HubStartDecision::Keep
            );
            assert!(matches!(
                decide_live_start(&fallback, &keep_args, true, TS_PORT, VERSION, true),
                HubStartDecision::Restart { flags_differ: true, .. }
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
                    false,
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
            HubDisplayState::Running,
            "3.7.7",
        );

        assert!(rendered.contains("version 3.4.1"), "{rendered}");
        assert!(rendered.contains("binary: 3.7.7"), "{rendered}");
        assert!(rendered.contains("version drift"), "{rendered}");
        assert!(rendered.contains("cas hub service install"), "{rendered}");
        let current = render_status(
            &record("3.7.7", DEFAULT_HUB_PORT, None),
            HubDisplayState::Running,
            "3.7.7",
        );
        assert!(!current.contains("version drift"), "{current}");
    }

    #[test]
    fn stale_status_rendering_shows_launcher_metadata() {
        let mut stale = record("3.4.1", DEFAULT_HUB_PORT, None);
        stale.launched_by = Some("update".to_owned());
        stale.launched_at = Some("2026-09-01T12:34:56Z".to_owned());

        let rendered = render_status(&stale, HubDisplayState::Exited, "3.7.7");

        assert!(rendered.contains("last pid 42 exited"), "{rendered}");
        assert!(rendered.contains("cas hub start"), "{rendered}");
        assert!(!rendered.contains("version drift"), "{rendered}");
        assert!(!rendered.contains("cas hub service install"), "{rendered}");
        assert!(
            rendered.contains("started by update at 2026-09-01T12:34:56Z"),
            "{rendered}"
        );
    }

    #[test]
    fn stale_launcher_files_are_removed_without_touching_live_files() {
        let root = tempfile::tempdir().unwrap();
        let stale_pid = 111;
        let live_pid = 222;
        for extension in ["pid", "go", "error"] {
            std::fs::write(
                root.path().join(format!(".launcher-7-{stale_pid}.{extension}")),
                b"stale",
            )
            .unwrap();
            std::fs::write(
                root.path().join(format!(".launcher-8-{live_pid}.{extension}")),
                b"live",
            )
            .unwrap();
        }
        let unrelated = root.path().join(".launcher-unrelated.error");
        std::fs::write(&unrelated, b"unrelated").unwrap();

        cleanup_stale_launcher_files_with(root.path(), |pid| pid == live_pid);

        for extension in ["pid", "go", "error"] {
            assert!(
                !root
                    .path()
                    .join(format!(".launcher-7-{stale_pid}.{extension}"))
                    .exists()
            );
            assert!(
                root.path()
                    .join(format!(".launcher-8-{live_pid}.{extension}"))
                    .exists()
            );
        }
        assert!(unrelated.exists());
    }

    /// cas-0140: the status screen tells a quiet audit log (OK, with the last
    /// row's age) from a failing writer (FAIL, with since-when and why).
    #[test]
    fn status_audit_line_tells_a_quiet_log_from_a_failing_writer() {
        let temp = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now();
        let empty = crate::hub::audit_writer_report(temp.path(), now);
        assert_eq!(render_audit_status(&empty), "Audit log: OK - no audit rows yet");
        std::fs::write(temp.path().join(crate::hub::AUDIT_LOG_FILE), b"{}\n").unwrap();
        let quiet = crate::hub::audit_writer_report(temp.path(), now + chrono::Duration::hours(18));
        assert!(!quiet.is_failure());
        let quiet_text = render_audit_status(&quiet);
        assert!(quiet_text.starts_with("Audit log: OK - last row 1"), "{quiet_text}");
        // cas-cfea: the healthy idle line keeps its explanation within 80 columns;
        // --json still carries the one-line message.
        assert!(quiet_text.contains("authenticated requests"), "{quiet_text}");
        assert!(quiet_text.lines().all(|line| line.chars().count() <= 80), "{quiet_text}");
        assert!(serde_json::to_value(&quiet).unwrap()["message"].as_str().unwrap().contains("; a hub writes rows only"));
        let failure = crate::hub::AuditHealth {
            failing_since: now,
            last_failure_at: now,
            failures: 3,
            last_action: "dpop_auth".into(),
            last_error: "hub auth state must have mode 0600".into(),
        };
        std::fs::write(temp.path().join(crate::hub::AUDIT_HEALTH_FILE), serde_json::to_vec(&failure).unwrap()).unwrap();
        let failing = crate::hub::audit_writer_report(temp.path(), now);
        assert!(failing.is_failure());
        let line = render_audit_status(&failing);
        assert!(line.starts_with("Audit log: FAIL - writes failing since "), "{line}");
        assert!(line.contains("3 failures, last on dpop_auth") && line.contains("mode 0600"), "{line}");
        let json = serde_json::to_value(&failing).unwrap();
        assert_eq!(json["status"], "failing");
        assert_eq!(json["failure"]["failures"], 3);
    }

    #[test]
    fn status_text_and_json_distinguish_lifecycle_states() {
        let record = record("3.4.1", DEFAULT_HUB_PORT, None);
        let unresponsive_remedy = if cfg!(target_os = "macos") {
            "Run `sample 42` for evidence; then `cas hub restart --force`."
        } else {
            "Run `cas hub restart --force` to recover the hub."
        };
        let cases = [
            (HubDisplayState::Exited, "exited", "last pid 42 exited", "cas hub start"),
            (HubDisplayState::Starting { age_secs: 3, wedged: false }, "starting", "pid 42 is starting for 3s", "cas hub status"),
            (HubDisplayState::Starting { age_secs: 12, wedged: true }, "startup_wedged", "pid 42 is wedged in startup after 12s", "cas hub restart --force"),
            (HubDisplayState::Stopping { age_secs: 4 }, "stopping", "pid 42 is stopping; lock held for 4s", "cas hub status"),
            (HubDisplayState::Unresponsive { age_secs: 91 }, "unresponsive", "pid 42 is running for 91s but not answering", unresponsive_remedy),
            (HubDisplayState::Running, "running", "Cassy hub is running and ready", ""),
        ];
        for (state, kind, text, remedy) in cases {
            let rendered = render_status(&record, state, "3.7.7");
            let json = hub_state_json(state, record.pid);
            assert!(rendered.contains(text), "{rendered}");
            assert!(rendered.contains(remedy), "{rendered}");
            assert_eq!(json["kind"], kind);
            assert_eq!(json["pid"], 42);
            assert!(json["remedy"].as_str().unwrap().contains(remedy));
            if state == (HubDisplayState::Unresponsive { age_secs: 91 }) {
                assert_eq!(json["remedy"], unresponsive_remedy);
                assert_eq!(rendered.contains("sample 42"), cfg!(target_os = "macos"));
            }
        }
        // cas-ea2b: after a reboot an exited hub that recorded Serve gives the
        // Serve line's single remedy, never a competing `cas hub start`.
        let served = self::record("3.4.1", DEFAULT_HUB_PORT, Some(443));
        let rendered = render_status(&served, HubDisplayState::Exited, "3.7.7");
        let json = hub_record_state_json(&served, HubDisplayState::Exited);
        assert!(rendered.contains("remedy: Run `cas hub restart --tailscale-serve`."), "{rendered}");
        assert!(!rendered.contains("cas hub start"), "{rendered}");
        assert_eq!(json["remedy"], "Run `cas hub restart --tailscale-serve`.");
    }

    #[test]
    fn live_process_classification_uses_lock_phase_without_changing_readiness() {
        let temp = crate::test_support::private_hub_tempdir();
        let paths = HubRuntimePaths::new(temp.path());
        let mut lock = paths.acquire_instance_lock().unwrap();
        let mut record = record(env!("CARGO_PKG_VERSION"), 0, None);
        record.pid = std::process::id();
        record.started_at = chrono::Utc::now().to_rfc3339();
        paths.write_process_record(&record).unwrap();

        assert!(matches!(hub_display_state(&paths, &record), HubDisplayState::Starting { wedged: false, .. }));
        lock.set_phase("running").unwrap();
        assert!(matches!(hub_display_state(&paths, &record), HubDisplayState::Unresponsive { .. }));
        record.pid = 999_999;
        assert_eq!(hub_display_state(&paths, &record), HubDisplayState::Exited);
    }

    #[test]
    fn lock_holder_termination_label_names_its_phase() {
        let paths = HubRuntimePaths::new("/nonexistent/cas-c67c");
        let mut holder = HubLockHolder {
            pid: 804,
            age: Some(Duration::from_secs(91)),
            phase: Some("starting".to_owned()),
            command: None,
        };
        assert!(lock_holder_termination_message(&paths, &holder).contains("wedged in startup"));
        holder.phase = Some("running".to_owned());
        let message = lock_holder_termination_message(&paths, &holder);
        assert!(message.contains("but not answering"), "{message}");
        assert!(!message.contains("no completed runtime state"), "{message}");
        holder.phase = Some("stopping".to_owned());
        assert!(lock_holder_termination_message(&paths, &holder).contains("is stopping"));
    }

    #[test]
    fn update_publication_policy_ignores_old_record_flags_and_honors_opt_out() {
        let old = record("3.4.1", 4310, None);
        let spec =
            update_restart_spec(Some(&old), None, HubTransportMode::Auto).unwrap();
        assert!(
            spec.transport_mode.publishes(),
            "old loopback record must adopt the default"
        );
        assert_eq!(spec.port, old.port);
        let published = record("3.4.1", 4310, Some(8443));
        let disabled = update_restart_spec(
            Some(&published),
            None,
            HubTransportMode::Loopback,
        )
        .unwrap();
        assert!(
            !disabled.transport_mode.publishes(),
            "config=false overrides old publication"
        );
        assert_eq!(disabled.tailscale_port, 8443);
        assert!(
            tailscale_policy(true, false, false),
            "explicit enable overrides config"
        );
        assert!(
            !tailscale_policy(false, true, true),
            "explicit disable overrides default"
        );
    }

    #[test]
    fn stopped_hub_start_requires_installed_service_or_existing_identity() {
        assert!(should_start_stopped_hub(true, false));
        assert!(should_start_stopped_hub(false, true));
        assert!(!should_start_stopped_hub(false, false));
    }

    #[test]
    fn auto_publication_failure_keeps_local_service_cas_3f03e() {
        assert!(!publication_failure_is_fatal(HubTransportMode::Auto));
        assert!(publication_failure_is_fatal(HubTransportMode::Required));
        assert!(!publication_failure_is_fatal(HubTransportMode::Loopback));
    }

    #[test]
    fn host_hub_transport_mode_survives_config_round_trip_cas_3f03e() {
        let env = crate::test_support::TestEnvGuard::temp_home();
        let root = env.home().join(".cas");
        let mut config = Config::default();
        assert_eq!(config.get("hub.transport_mode").as_deref(), Some("auto"));
        for mode in ["required", "loopback", "auto"] {
            config.set("hub.transport_mode", mode).unwrap();
            config.save(&root).unwrap();
            let loaded = Config::load(&root).unwrap();
            assert_eq!(loaded.get("hub.transport_mode").as_deref(), Some(mode));
            // Restart and update resolve the host file, not a stale process record.
            let policy = host_transport_mode().unwrap();
            assert_eq!(policy.as_str(), mode);
            let spec = update_restart_spec(Some(&record("old", 4310, Some(8443))), None, policy).unwrap();
            assert_eq!(spec.transport_mode, policy);
            assert_eq!(spec.transport_mode.publishes(), mode != "loopback");
            assert_eq!(policy.require_publication(false).is_err(), mode == "required");
            assert!(policy.require_publication(true).is_ok());
        }
        assert!(config.set("hub.transport_mode", "invalid").is_err());
        // Existing host opt-outs remain effective until a mode is explicitly set.
        let legacy: Config = toml::from_str("[hub]\ntailscale_serve = false\n").unwrap();
        assert_eq!(legacy.get("hub.transport_mode").as_deref(), Some("loopback"));
    }

    #[test]
    fn update_transport_failure_is_a_warning_with_working_recovery() {
        let verification = unavailable_update_transport("tailscale CLI is unavailable");
        assert_eq!(verification.transport_verified, Some(false));
        assert_eq!(
            format_update_transport_warning(
                verification.transport_warning.as_deref().unwrap()
            ),
            "cas update: Tailscale Serve inactive:\n  tailscale CLI is unavailable; hub remains loopback-only"
        );
        assert!(verification.remedy.unwrap().contains("cas hub restart"));
    }
}
