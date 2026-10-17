//! Terminal interface for the same external-thread authority service used by MCP.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, bail};
use clap::Subcommand;
use serde_json::{Value, json};

use crate::coordination::external_work::{
    ExternalWorkCommand, ExternalWorkService, HttpCloudCoordinationPort, OperationContext,
    RemoteError, SqliteRequestJournal, VioletThreadResolver, decode_violet_result,
};

#[derive(Debug, Clone, Subcommand)]
pub enum ExternalWorkCommands {
    /// Claim a Slack thread using its permalink.
    Claim {
        #[arg(long)]
        external_ref: String,
        #[arg(long)]
        request_id: Option<String>,
    },
    /// Reconcile a prior claim or mutation by its original request ID.
    ClaimStatus {
        #[arg(long)]
        request_id: String,
    },
    /// Release an external thread claim.
    ReleaseClaim {
        #[arg(long)]
        resource_id: String,
        #[arg(long)]
        expected_generation: u64,
        #[arg(long)]
        request_id: Option<String>,
    },
    /// Offer a claim to another eligible supervisor slot.
    TransferClaim {
        #[arg(long)]
        resource_id: String,
        #[arg(long)]
        expected_generation: u64,
        #[arg(long)]
        to_slot_id: String,
        #[arg(long)]
        offer_ttl_secs: Option<u32>,
        #[arg(long)]
        request_id: Option<String>,
    },
    /// Accept a claim handoff offer.
    AcceptClaim {
        #[arg(long)]
        handoff_id: String,
        #[arg(long)]
        expected_generation: u64,
        #[arg(long)]
        request_id: Option<String>,
    },
    /// Extend a live claim's idle lease before it lapses.
    RenewClaim {
        #[arg(long)]
        resource_id: String,
        #[arg(long)]
        expected_generation: u64,
        #[arg(long)]
        request_id: Option<String>,
    },
    /// Register a reply intent under the current claim; prints the exact violet_post fields.
    ReplyIntent {
        #[arg(long)]
        resource_id: String,
        #[arg(long)]
        expected_generation: u64,
        #[arg(long)]
        occurrence_id: Option<String>,
        #[arg(long)]
        request_id: Option<String>,
    },
}

pub fn execute(
    command: &ExternalWorkCommands,
    _cli: &super::Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    use crate::store::{AgentStore, open_agent_store};

    let session_id = std::env::var("CAS_SESSION_ID")
        .ok()
        .filter(|id| !id.trim().is_empty())
        .or_else(|| crate::agent_id::read_session_for_mcp(cas_root).ok())
        .context("No authenticated Cassy session; start a factory session first")?;
    let agents = open_agent_store(cas_root).context("Could not open the registered agent store")?;
    let agent = agents
        .get(&session_id)
        .context("The current Cassy session is not registered")?;
    if agent.role != crate::types::AgentRole::Supervisor {
        bail!("External thread authority is available only to the authenticated supervisor.");
    }
    let project_id = crate::cloud::resolve_canonical_id(cas_root)
        .context("This project has no canonical Cloud project identity")?;
    let cloud = crate::cloud::CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root)?;
    let token = cloud
        .token
        .clone()
        .filter(|token| !token.is_empty())
        .context("External thread authority requires Cassy Cloud login")?;

    let caller = proxy_caller(&agents, &agent)?;
    let violet: Arc<dyn VioletThreadResolver> = build_violet(cas_root, caller)?;
    let journal =
        SqliteRequestJournal::open(cas_root).context("Could not open the request journal")?;
    let service = ExternalWorkService::new(
        journal,
        Arc::new(HttpCloudCoordinationPort::new(cloud.endpoint, token)),
        violet,
        OperationContext {
            project_id,
            registration_id: agent.id,
        },
    );
    let result = service.execute(to_command(command))?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn to_command(command: &ExternalWorkCommands) -> ExternalWorkCommand {
    match command {
        ExternalWorkCommands::Claim {
            external_ref,
            request_id,
        } => ExternalWorkCommand::Claim {
            external_ref: external_ref.clone(),
            request_id: request_id.clone(),
        },
        ExternalWorkCommands::ClaimStatus { request_id } => ExternalWorkCommand::ClaimStatus {
            request_id: request_id.clone(),
        },
        ExternalWorkCommands::ReleaseClaim {
            resource_id,
            expected_generation,
            request_id,
        } => ExternalWorkCommand::ReleaseClaim {
            resource_id: resource_id.clone(),
            expected_generation: *expected_generation,
            request_id: request_id.clone(),
        },
        ExternalWorkCommands::TransferClaim {
            resource_id,
            expected_generation,
            to_slot_id,
            offer_ttl_secs,
            request_id,
        } => ExternalWorkCommand::TransferClaim {
            resource_id: resource_id.clone(),
            expected_generation: *expected_generation,
            to_slot_id: to_slot_id.clone(),
            offer_ttl_secs: *offer_ttl_secs,
            request_id: request_id.clone(),
        },
        ExternalWorkCommands::AcceptClaim {
            handoff_id,
            expected_generation,
            request_id,
        } => ExternalWorkCommand::AcceptClaim {
            handoff_id: handoff_id.clone(),
            expected_generation: *expected_generation,
            request_id: request_id.clone(),
        },
        ExternalWorkCommands::RenewClaim {
            resource_id,
            expected_generation,
            request_id,
        } => ExternalWorkCommand::RenewClaim {
            resource_id: resource_id.clone(),
            expected_generation: *expected_generation,
            request_id: request_id.clone(),
        },
        ExternalWorkCommands::ReplyIntent {
            resource_id,
            expected_generation,
            occurrence_id,
            request_id,
        } => ExternalWorkCommand::ReplyIntent {
            resource_id: resource_id.clone(),
            expected_generation: *expected_generation,
            occurrence_id: occurrence_id.clone(),
            request_id: request_id.clone(),
        },
    }
}

#[cfg(feature = "mcp-proxy")]
fn proxy_caller(
    agents: &Arc<dyn crate::store::AgentStore>,
    agent: &crate::types::Agent,
) -> anyhow::Result<cmcp_core::ProxyCaller> {
    let mut active_task_ids = agents
        .list_agent_leases(&agent.id)
        .context("Could not read the supervisor's active task leases")?
        .into_iter()
        .map(|lease| lease.task_id)
        .collect::<Vec<_>>();
    active_task_ids.sort();
    Ok(cmcp_core::ProxyCaller {
        agent_id: agent.id.clone(),
        role: agent.role,
        session_id: agent.id.clone(),
        factory_session: agent.factory_session.clone(),
        active_task_ids,
    })
}

#[cfg(not(feature = "mcp-proxy"))]
fn proxy_caller(
    _agents: &Arc<dyn crate::store::AgentStore>,
    _agent: &crate::types::Agent,
) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(feature = "mcp-proxy")]
struct CliViolet {
    runtime: Mutex<tokio::runtime::Runtime>,
    proxy: Arc<cmcp_core::ProxyEngine>,
    caller: cmcp_core::ProxyCaller,
}

#[cfg(feature = "mcp-proxy")]
impl VioletThreadResolver for CliViolet {
    fn resolve(&self, external_ref: &str, timeout: Duration) -> Result<Value, RemoteError> {
        let args = serde_json::Map::from_iter([("external_ref".to_string(), json!(external_ref))]);
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| RemoteError::unavailable("Violet runtime lock poisoned"))?;
        let result = runtime.block_on(async {
            tokio::time::timeout(
                timeout,
                self.proxy.call_tool(
                    &self.caller,
                    cmcp_core::config::VIOLET_SERVER,
                    "violet_read",
                    Some(args),
                ),
            )
            .await
        });
        match result {
            Ok(Ok(value)) => decode_violet_result(value),
            Ok(Err(error)) => Err(RemoteError::unavailable(
                cmcp_core::describe_upstream_call_error(&error),
            )),
            Err(_) => Err(RemoteError::timeout("Violet thread resolution timed out")),
        }
    }
}

#[cfg(feature = "mcp-proxy")]
fn build_violet(
    cas_root: &Path,
    caller: cmcp_core::ProxyCaller,
) -> anyhow::Result<Arc<dyn VioletThreadResolver>> {
    let config = crate::mcp::load_proxy_config_for_process(cas_root)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let proxy =
        Arc::new(runtime.block_on(cmcp_core::ProxyEngine::from_configs(config.servers.clone()))?);
    runtime.block_on(crate::mcp::install_proxy_policy(&proxy, &config));
    Ok(Arc::new(CliViolet {
        runtime: Mutex::new(runtime),
        proxy,
        caller,
    }))
}

#[cfg(not(feature = "mcp-proxy"))]
fn build_violet(_cas_root: &Path, _caller: ()) -> anyhow::Result<Arc<dyn VioletThreadResolver>> {
    struct Unavailable;
    impl VioletThreadResolver for Unavailable {
        fn resolve(&self, _external_ref: &str, _timeout: Duration) -> Result<Value, RemoteError> {
            Err(RemoteError::unavailable(
                "this build has no MCP proxy; rebuild with mcp-proxy enabled",
            ))
        }
    }
    Ok(Arc::new(Unavailable))
}
