//! `cas hub operator`: this hub's place in the account's durable operator
//! inbox on Petra Stella Cloud (cas-9b7d; cloud contract §5).
//!
//! - `enroll` makes this hub a machine of the logged-in account (§5.4).
//! - `approve` / `deny` decide a Commander device's sign-in code (§5.2), the
//!   CLI alternative to the cloud approval page.
//! - `status`, `principals`, `revoke-device`, `revoke-machine` inspect and
//!   clean up (§5.6).
//!
//! The PSC bearer from `cas login` is read when a command runs and used only
//! on account-authority routes. It is never printed, logged or stored with
//! the machine principal.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

use crate::cli::Cli;
use crate::cloud::CloudConfig;
use crate::hub::operator_inbox::machine::{
    AccountAuthority, CommandScope, EnrollRequest, HttpClient, PrincipalKind, PrincipalStore,
    UreqHttp, decide_device, enroll_machine, issuer_keys, list_principals, revoke_principal,
};
use crate::hub::{HubRuntimePaths, MachineIdentityStore};

#[derive(Args, Debug, Clone)]
pub struct HubOperatorArgs {
    #[command(subcommand)]
    pub command: HubOperatorCommands,
}

#[derive(Subcommand, Debug, Clone)]
pub enum HubOperatorCommands {
    /// Enroll this hub in your account's operator inbox
    Enroll(OperatorEnrollArgs),
    /// Show this hub's operator inbox enrollment
    Status,
    /// Approve a Commander sign-in code (for example ABCD-EFGH)
    Approve(OperatorApproveArgs),
    /// Deny a Commander sign-in code
    Deny(OperatorCodeArgs),
    /// List the account's enrolled devices and machines
    Principals,
    /// Revoke a device; the cloud rotates the inbox key
    RevokeDevice(OperatorIdArgs),
    /// Revoke a machine (this hub when no ID is given)
    RevokeMachine(OperatorOptionalIdArgs),
    /// Publish this project's future operator messages to the inbox
    Bind,
    /// Stop publishing this project's new operator messages (pending ones still drain)
    Detach,
    /// Upload this project's pending operator messages now
    Drain,
}

#[derive(Args, Debug, Clone)]
pub struct OperatorEnrollArgs {
    /// Label shown on devices (defaults to the host name)
    #[arg(long)]
    pub label: Option<String>,
    /// Canonical project IDs this hub may publish to (repeatable)
    #[arg(long = "project")]
    pub projects: Vec<String>,
    /// Also request the presence-report capability (monitoring stays off until approved)
    #[arg(long)]
    pub presence: bool,
    /// Replace an existing enrollment of this hub
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug, Clone)]
pub struct OperatorCodeArgs {
    /// Code displayed by Commander
    pub code: String,
}

#[derive(Args, Debug, Clone)]
pub struct OperatorApproveArgs {
    /// Code displayed by Commander
    pub code: String,
    /// Let the device manage the account's devices and machines
    #[arg(long)]
    pub manage: bool,
    /// Let the device queue offline messages to `project` on this hub (repeatable;
    /// `project/session` narrows it to one session routing ID)
    #[arg(long = "command-scope")]
    pub command_scopes: Vec<String>,
}

#[derive(Args, Debug, Clone)]
pub struct OperatorIdArgs {
    pub id: String,
}

#[derive(Args, Debug, Clone)]
pub struct OperatorOptionalIdArgs {
    pub id: Option<String>,
}

fn authority() -> Result<AccountAuthority> {
    let user = CloudConfig::load_user().unwrap_or_default();
    let config = if user.is_logged_in() {
        user
    } else {
        CloudConfig::load().unwrap_or_default()
    };
    let bearer = config
        .token
        .clone()
        .filter(|token| !token.is_empty())
        .context("Not logged in to Petra Stella Cloud. Run `cas login` first.")?;
    Ok(AccountAuthority::new(&config.endpoint, bearer))
}

fn print(cli: &Cli, value: &Value, human: impl FnOnce()) -> Result<()> {
    if cli.json {
        println!("{}", serde_json::to_string(value)?);
    } else {
        human();
    }
    Ok(())
}

fn parse_scopes(hub_id: &str, raw: &[String]) -> Result<Vec<CommandScope>> {
    raw.iter()
        .map(|entry| {
            let (project, session) = match entry.split_once('/') {
                Some((project, session)) => (project, Some(session.to_owned())),
                None => (entry.as_str(), None),
            };
            let valid = |value: &str| {
                !value.is_empty()
                    && value.len() <= 200
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._:@-".contains(&b))
            };
            if !valid(project) || session.as_deref().is_some_and(|s| !valid(s)) {
                bail!("command scope `{entry}` is not a routing ID (project or project/session)");
            }
            Ok(CommandScope {
                hub_id: hub_id.to_owned(),
                project_id: project.to_owned(),
                session_id: session,
                operations: vec!["operator_message".into()],
            })
        })
        .collect()
}

pub(super) fn execute(args: &HubOperatorArgs, cli: &Cli) -> Result<()> {
    let paths = HubRuntimePaths::default_for_user()?;
    let store = PrincipalStore::new(paths.root());
    let http: Arc<dyn HttpClient> = Arc::new(UreqHttp::default());
    match &args.command {
        HubOperatorCommands::Status => {
            let principal = store.load()?;
            let value = match &principal {
                Some(p) => json!({
                    "enrolled": true, "cloud": p.cloud_origin, "account_id": p.account_id,
                    "machine_id": p.machine_id, "hub_id": p.hub_id, "label": p.label,
                    "projects": p.projects, "grant_generation": p.grant_generation,
                    "feed_generation": p.feed_generation, "enrolled_at": p.enrolled_at,
                }),
                None => json!({"enrolled": false}),
            };
            print(cli, &value, || match &principal {
                Some(p) => {
                    println!(
                        "Operator inbox: enrolled as {} on {}",
                        p.label, p.cloud_origin
                    );
                    println!("  machine   {}", p.machine_id);
                    println!("  hub       {}", p.hub_id);
                    if p.projects.is_empty() {
                        println!("  projects  none yet");
                    } else {
                        println!("  projects  {}", p.projects.join(", "));
                    }
                }
                None => println!(
                    "Operator inbox: not enrolled. Run `cas hub operator enroll` to keep supervisor messages readable while this hub is off."
                ),
            })
        }
        HubOperatorCommands::Enroll(enroll) => {
            if let Some(existing) = store.load()?
                && !enroll.force
            {
                bail!(
                    "This hub is already enrolled as machine {} ({}). Pass --force to replace it; its unclaimed offline messages will expire.",
                    existing.machine_id,
                    existing.label
                );
            }
            let authority = authority()?;
            let hub = MachineIdentityStore::new(paths.root()).load_or_create()?;
            let label = enroll
                .label
                .clone()
                .unwrap_or_else(super::hub_reverse_pairing::machine_display_label);
            let issuer = issuer_keys(Arc::clone(&http), &authority.cloud_origin);
            let principal = enroll_machine(
                http.as_ref(),
                &authority,
                &issuer,
                &EnrollRequest {
                    hub_id: &hub.id,
                    label: &label,
                    projects: &enroll.projects,
                    presence: enroll.presence,
                },
            )?;
            store.save(&principal)?;
            let value = json!({"enrolled": true, "machine_id": principal.machine_id, "hub_id": principal.hub_id, "account_id": principal.account_id});
            print(cli, &value, || {
                println!(
                    "Enrolled {} as machine {}. Supervisor messages from this hub now reach your signed-in devices even when it is off.",
                    principal.label, principal.machine_id
                );
            })
        }
        HubOperatorCommands::Approve(approve) => {
            let authority = authority()?;
            let hub_id = store.load()?.map(|principal| principal.hub_id).or_else(|| {
                MachineIdentityStore::new(paths.root())
                    .load()
                    .ok()
                    .map(|identity| identity.id)
            });
            let scopes = match (&hub_id, approve.command_scopes.is_empty()) {
                (_, true) => Vec::new(),
                (Some(hub_id), false) => parse_scopes(hub_id, &approve.command_scopes)?,
                (None, false) => bail!("Command scopes need an enrolled hub on this machine."),
            };
            let code = approve.code.trim().to_ascii_uppercase();
            let reply = decide_device(
                http.as_ref(),
                &authority,
                &code,
                true,
                approve.manage,
                &scopes,
            )?;
            print(cli, &reply, || {
                println!("Approved {code}. The device can now read your operator inbox.");
                if !scopes.is_empty() {
                    println!(
                        "It may queue messages for {} scope(s) on this hub.",
                        scopes.len()
                    );
                }
            })
        }
        HubOperatorCommands::Deny(deny) => {
            let authority = authority()?;
            let code = deny.code.trim().to_ascii_uppercase();
            let reply = decide_device(http.as_ref(), &authority, &code, false, false, &[])?;
            print(cli, &reply, || println!("Denied {code}."))
        }
        HubOperatorCommands::Principals => {
            let authority = authority()?;
            let reply = list_principals(http.as_ref(), &authority)?;
            print(cli, &reply, || {
                for device in reply["devices"].as_array().into_iter().flatten() {
                    println!(
                        "device   {}  {}  {}",
                        device["device_id"].as_str().unwrap_or("?"),
                        device["status"].as_str().unwrap_or("?"),
                        device["label"].as_str().unwrap_or("(label unavailable)")
                    );
                }
                for machine in reply["machines"].as_array().into_iter().flatten() {
                    println!(
                        "machine  {}  {}  {}",
                        machine["machine_id"].as_str().unwrap_or("?"),
                        machine["status"].as_str().unwrap_or("?"),
                        machine["label"].as_str().unwrap_or("(label unavailable)")
                    );
                }
            })
        }
        HubOperatorCommands::RevokeDevice(target) => {
            let authority = authority()?;
            let reply =
                revoke_principal(http.as_ref(), &authority, PrincipalKind::Device, &target.id)?;
            print(cli, &reply, || {
                println!("Revoked device {}. The inbox key was rotated.", target.id)
            })
        }
        HubOperatorCommands::Bind => bind(&store, http, cli),
        HubOperatorCommands::Detach => {
            let queue = project_queue()?;
            let detached = queue.detach_operator_feed(chrono::Utc::now())?;
            print(cli, &json!({"detached": detached}), || {
                if detached {
                    println!(
                        "Detached. New operator messages from this project stay local; pending ones still upload."
                    );
                } else {
                    println!("This project was not bound.");
                }
            })
        }
        HubOperatorCommands::Drain => drain_now(&store, http, cli),
        HubOperatorCommands::RevokeMachine(target) => {
            let authority = authority()?;
            let local = store.load()?;
            let id = match (&target.id, &local) {
                (Some(id), _) => id.clone(),
                (None, Some(principal)) => principal.machine_id.clone(),
                (None, None) => bail!("This hub is not enrolled; pass a machine ID."),
            };
            let reply = revoke_principal(http.as_ref(), &authority, PrincipalKind::Machine, &id)?;
            if local
                .as_ref()
                .is_some_and(|principal| principal.machine_id == id)
            {
                store.remove()?;
            }
            print(cli, &reply, || println!("Revoked machine {id}."))
        }
    }
}

fn project_queue() -> Result<cas_store::SqlitePromptQueueStore> {
    use cas_store::PromptQueueStore as _;
    let cas_root = crate::store::find_cas_root()
        .context("Run this inside a Cassy project (no .cas directory found).")?;
    let queue = cas_store::SqlitePromptQueueStore::open(&cas_root)?;
    queue.init()?;
    Ok(queue)
}

/// Bind this project's prompt database to the enrolled machine's audience.
/// The project must be one of the machine's granted projects; if it is not,
/// the account authority adds it (§5.6) before anything is bound.
fn bind(store: &PrincipalStore, http: Arc<dyn HttpClient>, cli: &Cli) -> Result<()> {
    let mut principal = store
        .load()?
        .context("This hub is not enrolled. Run `cas hub operator enroll` first.")?;
    let cas_root = crate::store::find_cas_root()
        .context("Run this inside a Cassy project (no .cas directory found).")?;
    let project_id = crate::cloud::resolve_canonical_id(&cas_root)
        .context("This project has no canonical project ID.")?;
    if !cas_store::is_routing_id(&project_id) {
        bail!(
            "The project ID `{project_id}` has characters the inbox routing rules refuse (only A-Z a-z 0-9 . _ : @ / -)."
        );
    }
    if !principal.projects.contains(&project_id) {
        let authority = authority()?;
        let mut projects = principal.projects.clone();
        projects.push(project_id.clone());
        crate::hub::operator_inbox::machine::set_machine_projects(
            http.as_ref(),
            &authority,
            &principal.machine_id,
            &projects,
        )?;
        principal.projects = projects;
        store.save(&principal)?;
    }
    let queue = project_queue()?;
    let binding = queue.bind_operator_feed(&cas_store::OperatorFeedBinding {
        account_id: principal.account_id.clone(),
        machine_id: principal.machine_id.clone(),
        hub_id: principal.hub_id.clone(),
        project_id: project_id.clone(),
        bound_at: chrono::Utc::now().to_rfc3339(),
    })?;
    let value =
        json!({"bound": true, "project_id": binding.project_id, "bound_at": binding.bound_at});
    print(cli, &value, || {
        println!(
            "Bound {project_id}. Operator messages recorded from now on reach your signed-in devices; earlier ones stay local."
        );
    })
}

fn drain_now(store: &PrincipalStore, http: Arc<dyn HttpClient>, cli: &Cli) -> Result<()> {
    use crate::hub::operator_inbox::drain::{drain_project, fetch_epoch_policy};
    use crate::hub::operator_inbox::machine::MachineTransport;
    let principal = store
        .load()?
        .context("This hub is not enrolled. Run `cas hub operator enroll` first.")?;
    let queue = project_queue()?;
    let transport =
        MachineTransport::new(Arc::clone(&http), principal.clone(), Some(store.clone()))
            .map_err(|error| anyhow::anyhow!("machine principal keys are unreadable: {error}"))?;
    let issuer = issuer_keys(Arc::clone(&http), &principal.cloud_origin);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut total = crate::hub::operator_inbox::drain::DrainReport::default();
    loop {
        let policy = fetch_epoch_policy(&transport, &issuer, &principal)?;
        let relay = crate::hub::operator_inbox::MachineRelay::new(transport.clone());
        let report = runtime.block_on(drain_project(&queue, &relay, &policy, &principal))?;
        total.claimed += report.claimed;
        total.stored += report.stored;
        total.parked += report.parked;
        total.retry += report.retry;
        total.frozen |= report.frozen;
        if report.claimed == 0 || report.frozen || (report.stored == 0 && report.resealed == 0) {
            break;
        }
    }
    let backlog = queue.operator_cloud_backlog()?;
    let value = json!({"uploaded": total.stored, "parked": total.parked, "retrying": total.retry,
        "frozen": total.frozen, "pending": backlog.pending, "oldest_pending_at": backlog.oldest_pending_at});
    print(cli, &value, || {
        println!(
            "Uploaded {} message(s); {} pending, {} parked.",
            total.stored, backlog.pending, backlog.parked
        );
        if total.frozen {
            println!(
                "The cloud is holding uploads while it rotates keys; messages stay queued here."
            );
        }
    })
}
