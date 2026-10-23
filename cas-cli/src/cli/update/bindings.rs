//! cas-9021 (GH #1267): reconcile a project's Violet-mapped Slack channels
//! with Cloud managed bindings through the ordinary user credential.
//!
//! Violet's channel list is discovery, never authority: Cloud verifies each
//! mapping and derives the workspace. Every repair goes through the durable
//! intake gate and local runtime drain in `coordination::binding_repair`.

use std::path::Path;

use serde::Serialize;

use super::ProjectPhase;
use crate::cloud::CloudConfig;
use crate::coordination::binding_intake::{IntakeJournal, IntakeScope};
use crate::coordination::binding_reconciliation::NextAction;
use crate::coordination::binding_repair::{OrdinaryBindingClient, RepairOutcome, RepairReport, RuntimeSnapshot};

/// The Violet hub authenticates production Cloud bearers only. A project on
/// another Cloud (staging, a local test server) has no hub to ask.
const PRODUCTION_CLOUD: &str = "https://petra-stella-cloud.vercel.app";
const CHANNEL_ROUTE: &str = "/api/channels";

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct ChannelMapping {
    channel_id: String,
    #[serde(default)]
    channel_name: String,
    project_id: String,
}

#[derive(Debug, Serialize)]
struct ChannelResult {
    channel_id: String,
    channel_name: String,
    outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cause: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remedy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    activation_id: Option<String>,
}

/// `None` when the project has nothing to reconcile, so its receipt keeps
/// today's shape: not cloud-linked or logged in, not on production Cloud, or
/// no Violet mapping and no local intake gate.
pub(super) fn reconcile_project_bindings(
    cas_root: &Path,
    dry_run: bool,
    json: bool,
    inventory: &RuntimeSnapshot,
) -> Option<ProjectPhase> {
    let config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root).ok()?;
    if config.endpoint.trim_end_matches('/') != PRODUCTION_CLOUD {
        return None;
    }
    reconcile_with(
        cas_root,
        &config,
        // cas-types owns the hub URL; the proxy crate only re-exports it and is
        // absent from `--no-default-features` builds.
        cas_types::violet_compatibility::violet_hub_url(),
        dry_run,
        json,
        inventory,
    )
}

fn reconcile_with(
    cas_root: &Path,
    config: &CloudConfig,
    hub_url: &str,
    dry_run: bool,
    json: bool,
    inventory: &RuntimeSnapshot,
) -> Option<ProjectPhase> {
    if !cas_root.join("cloud.json").exists() || !config.is_logged_in() {
        return None;
    }
    let token = config.token.as_deref()?.trim();
    let team_id = CloudConfig::load_from_cas_dir(cas_root).ok()?.active_team_id()?;
    let project_id = crate::cloud::resolve_canonical_id(cas_root)?;

    // Channels this host already gated stay candidates even if their Violet
    // mapping disappeared: Cloud's answer is what settles them.
    let journal = IntakeJournal::new(cas_root);
    let mut channels: Vec<(String, String)> = Vec::new();
    if journal.path().exists() {
        match journal.read() {
            Ok(state) => channels.extend(
                state
                    .policies
                    .iter()
                    .filter(|policy| {
                        policy.scope.team_id == team_id
                            && policy.scope.canonical_project_id == project_id
                    })
                    .map(|policy| (policy.scope.channel_id.clone(), String::new())),
            ),
            Err(error) => {
                return Some(ProjectPhase::Warning(format!(
                    "Slack bindings: intake gate unreadable ({error}); check `{}`",
                    journal.path().display()
                )));
            }
        }
    }
    match list_channels(hub_url, token, &project_id) {
        Ok(mapped) => {
            for mapping in mapped.into_iter().filter(|m| m.project_id == project_id) {
                match channels.iter_mut().find(|(id, _)| *id == mapping.channel_id) {
                    Some(known) => known.1 = mapping.channel_name,
                    None => channels.push((mapping.channel_id, mapping.channel_name)),
                }
            }
        }
        Err(error) if channels.is_empty() => {
            return Some(ProjectPhase::Warning(format!(
                "Slack bindings: could not list Violet channel mappings ({error}); rerun `cas update`"
            )));
        }
        Err(error) => tracing::warn!(%error, "Violet channel listing failed; reconciling gated channels only"),
    }
    if channels.is_empty() {
        return None;
    }
    channels.sort();
    channels.dedup_by(|a, b| a.0 == b.0);
    if dry_run {
        let ids: Vec<&str> = channels.iter().map(|(id, _)| id.as_str()).collect();
        return Some(ProjectPhase::Planned(format!(
            "Slack bindings: would reconcile {}",
            ids.join(", ")
        )));
    }
    let client = match OrdinaryBindingClient::new(&config.endpoint, token) {
        Ok(client) => client,
        Err(error) => return Some(ProjectPhase::Warning(format!("Slack bindings: {error}; run `cas login`"))),
    };
    let results: Vec<ChannelResult> = channels
        .into_iter()
        .map(|(channel_id, channel_name)| {
            let scope = IntakeScope {
                team_id: team_id.clone(),
                canonical_project_id: project_id.clone(),
                channel_id: channel_id.clone(),
            };
            match client.repair_with_inventory(cas_root, &scope, inventory) {
                Ok(report) => channel_result(channel_name, report),
                Err(error) => ChannelResult {
                    channel_id,
                    channel_name,
                    outcome: "blocked",
                    code: None,
                    next_action: None,
                    cause: Some(error),
                    remedy: Some("resolve the cause above, then rerun `cas update`".into()),
                    operation_id: None,
                    activation_id: None,
                },
            }
        })
        .collect();
    print_results(&project_id, &results, json);
    Some(phase(&results))
}

fn list_channels(hub_url: &str, token: &str, project_id: &str) -> Result<Vec<ChannelMapping>, String> {
    #[derive(serde::Deserialize)]
    struct Listing {
        channels: Vec<ChannelMapping>,
    }
    let mut url = url::Url::parse(hub_url).map_err(|_| "invalid Violet hub URL".to_string())?;
    url.set_path(CHANNEL_ROUTE);
    url.set_fragment(None);
    url.query_pairs_mut().clear().append_pair("project_id", project_id);
    let response = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .get(url.as_str())
        .set("Authorization", &format!("Bearer {token}"))
        .call();
    match response {
        Ok(response) => response
            .into_json::<Listing>()
            .map(|listing| listing.channels)
            .map_err(|_| "invalid Violet channel listing".into()),
        Err(ureq::Error::Status(status, _)) => Err(format!("Violet HTTP {status}")),
        Err(ureq::Error::Transport(_)) => Err("Violet hub unreachable".into()),
    }
}

fn channel_result(channel_name: String, report: RepairReport) -> ChannelResult {
    let (code, next_action) = report
        .response
        .as_ref()
        .map(|response| (response.code, response.next_action))
        .unzip();
    let retry_after = report.response.as_ref().and_then(|response| response.retry_after_secs);
    let outcome = match report.outcome {
        RepairOutcome::Managed => "managed",
        RepairOutcome::Blocked => "blocked",
        RepairOutcome::Unknown => "unknown",
    };
    let remedy = match (&report.outcome, next_action) {
        (RepairOutcome::Managed, _) => None,
        // The real blocker first; a definite answer to the retry settles it.
        (RepairOutcome::Unknown, Some(action)) => Some(format!(
            "{}; legacy intake stays paused until Cloud answers definitely",
            remedy(action, &report.channel_id, retry_after)
        )),
        (RepairOutcome::Unknown, None) => Some(
            "rerun `cas update` to retry this channel; legacy intake stays paused until Cloud answers definitely".into(),
        ),
        (_, Some(action)) => Some(remedy(action, &report.channel_id, retry_after)),
        (_, None) => Some("resolve the cause above, then rerun `cas update`".into()),
    };
    let wire = |value: serde_json::Value| value.as_str().map(str::to_owned);
    ChannelResult {
        channel_id: report.channel_id,
        channel_name,
        outcome,
        code: code.and_then(|code| wire(serde_json::json!(code))),
        next_action: next_action.and_then(|action| wire(serde_json::json!(action))),
        cause: report.error,
        remedy,
        operation_id: Some(report.operation_id.to_string()),
        activation_id: report.activation_id,
    }
}

/// Cloud's own quiet window, not a client constant, says how long to wait.
fn remedy(action: NextAction, channel: &str, retry_after_secs: Option<u64>) -> String {
    let wait = retry_after_secs.map_or_else(|| "a few minutes".to_owned(), |secs| format!("{secs}s"));
    match action {
        NextAction::StopLegacyIntake => format!(
            "wait {wait} for legacy intake on every machine to go quiet, then rerun `cas update`"
        ),
        NextAction::InviteViolet => format!("run `/invite @Violet` in channel {channel}, then rerun `cas update`"),
        NextAction::Authenticate => "run `cas login`, then rerun `cas update`".into(),
        NextAction::RestoreMembership => "check your Cassy Cloud team membership, then rerun `cas update`".into(),
        NextAction::LinkTeamProject => "run `cas cloud team show` to check this project's team link, then rerun `cas update`".into(),
        NextAction::ExplicitRelink | NextAction::OperatorRepair => format!(
            "resolve the mapping with `cas integrate violet --channel {channel}`, then rerun `cas update`"
        ),
        NextAction::UpgradeClient => "run `cas update` again after installing the newest Cassy".into(),
        NextAction::RetryLater | NextAction::FixRequest | NextAction::None => {
            format!("retry `cas update` after {wait}")
        }
    }
}

fn print_results(project_id: &str, results: &[ChannelResult], json: bool) {
    if json {
        println!(
            "{}",
            serde_json::json!({ "binding_reconciliation": { "project_id": project_id, "channels": results } })
        );
        return;
    }
    for result in results {
        let name = if result.channel_name.is_empty() {
            result.channel_id.clone()
        } else {
            format!("#{} ({})", result.channel_name, result.channel_id)
        };
        match (&result.cause, &result.remedy) {
            (None, None) => println!("Slack {name}: {}", result.outcome),
            (cause, remedy) => println!(
                "Slack {name}: {} — {}{}",
                result.outcome,
                cause.as_deref().unwrap_or("not managed"),
                remedy.as_deref().map(|r| format!("; {r}")).unwrap_or_default()
            ),
        }
    }
}

fn phase(results: &[ChannelResult]) -> ProjectPhase {
    let open: Vec<&ChannelResult> = results.iter().filter(|r| r.outcome != "managed").collect();
    let Some(first) = open.first() else {
        return ProjectPhase::Ok(format!("{} Slack channel(s) managed", results.len()));
    };
    let more = match open.len() {
        1 => String::new(),
        n => format!(" (+{} more)", n - 1),
    };
    ProjectPhase::Warning(format!(
        "Slack {} {}: {}{more}; {}",
        first.channel_id,
        first.outcome,
        first.cause.as_deref().unwrap_or("not managed"),
        first.remedy.as_deref().unwrap_or("rerun `cas update`")
    ))
}

#[cfg(test)]
#[path = "bindings_tests.rs"]
mod tests;
