//! Read-only inventory of Violet discovery and Cloud binding authority.
//! Listing is never a mapping proof or permission to claim/read Slack work.
use super::hub::VioletError;
use crate::cloud::CloudConfig;
use crate::coordination::binding_intake::IntakeJournal;
use crate::coordination::binding_reconciliation::{Binding, BindingMode, SCHEMA, StrictActivation};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, io::Read, path::Path, time::Duration};

#[derive(clap::Args, Debug, Clone)]
pub struct ChannelsArgs {
    /// Violet hub endpoint, as in `cas integrate violet --url`.
    #[arg(long, default_value = cas_types::violet_compatibility::violet_hub_url())]
    pub url: String,
    /// An existing supervisor registration of your account and this project.
    /// Without one, known channels are checked but Cloud-only discovery is incomplete.
    #[arg(long)]
    pub registration_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Mapping {
    pub channel_id: String,
    #[serde(default)]
    pub channel_name: String,
    pub project_id: String,
}

#[derive(Serialize)]
pub struct Channel {
    channel_id: String,
    channel_name: String,
    configured: bool,
    violet_project_id: Option<String>,
    /// Violet GET lists discovery rows; only reconciliation obtains server proof.
    violet_verification: &'static str,
    cloud_project_id: Option<String>,
    cloud_workspace_id: Option<String>,
    cloud_verified: Option<bool>,
    strict_activation: Option<StrictActivation>,
    flags: Vec<&'static str>,
    remedy: Option<String>,
}

#[derive(Serialize)]
pub struct Report {
    pub ok: bool,
    pub complete: bool,
    project_id: String,
    channels: Vec<Channel>,
    errors: Vec<String>,
}

/// No writes: no ensure-registration, mapping upsert, pause, claim or activation.
pub fn inspect(root: Option<&Path>, args: &ChannelsArgs) -> Result<Report, VioletError> {
    let root = root.ok_or_else(|| {
        VioletError::local(
            "project_required",
            "run `cas violet channels` in a Cassy project",
        )
    })?;
    let project_id = crate::cloud::resolve_canonical_id(root).ok_or_else(|| {
        VioletError::local(
            "project_unlinked",
            "run `cas cloud team show` to check this project's link",
        )
    })?;
    let config =
        CloudConfig::load_from_cas_dir_inheriting_user_credentials(root).map_err(|_| {
            VioletError::local(
                "not_logged_in",
                "run `cas login`, then `cas violet channels`",
            )
        })?;
    let token = config
        .token
        .as_deref()
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| {
            VioletError::local(
                "not_logged_in",
                "run `cas login`, then `cas violet channels`",
            )
        })?;
    let team = CloudConfig::load_from_cas_dir(root)
        .ok()
        .and_then(|config| config.active_team_id())
        .ok_or_else(|| {
            VioletError::local(
                "project_unlinked",
                "run `cas cloud team show` to check this project's team link",
            )
        })?;
    // Reuse ordinary-client origin/credential validation, without invoking repair.
    crate::coordination::binding_repair::OrdinaryBindingClient::new(&config.endpoint, token)
        .map_err(|error| VioletError::local("invalid_cloud_config", error))?;
    let slack = crate::config::Config::load(root)
        .map_err(|_| {
            VioletError::local("invalid_config", "check .cas/config.toml with `cas doctor`")
        })?
        .slack
        .unwrap_or_default();
    slack
        .validate_channels()
        .map_err(|error| VioletError::local("invalid_config", error))?;
    let configured = slack.channels;
    let registration = args.registration_id.clone().or_else(|| {
        // cli_self_id's PID fallback opens/migrates the local agent store.
        // This read-only command must not create a DB just to discover an ID.
        ["CAS_SESSION_ID", "CLAUDE_CODE_SESSION_ID"]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok())
            .find(|value| !value.trim().is_empty())
    });
    let mut report = Report {
        ok: false,
        complete: false,
        project_id: project_id.clone(),
        channels: Vec::new(),
        errors: Vec::new(),
    };
    let mut candidates: BTreeMap<String, bool> =
        configured.into_iter().map(|id| (id, true)).collect();
    // Keep previously gated channels visible even if both remote rows vanished.
    let journal = IntakeJournal::new(root);
    if journal.path().exists() {
        match journal.read() {
            Ok(state) => {
                for policy in state.policies {
                    if policy.scope.team_id == team
                        && policy.scope.canonical_project_id == project_id
                    {
                        candidates.entry(policy.scope.channel_id).or_insert(false);
                    }
                }
            }
            Err(error) => report.errors.push(format!(
                "Local intake inventory unavailable: {error}; run `cas doctor`"
            )),
        }
    }
    let mappings = match list_mappings(&args.url, token, None) {
        Ok(rows) => Some(rows),
        Err(error) => {
            report.errors.push(format!(
                "Violet inventory: {error}; run `cas integrate violet`, then retry"
            ));
            None
        }
    };
    if let Some(rows) = &mappings {
        for row in rows.iter().filter(|row| row.project_id == project_id) {
            candidates.entry(row.channel_id.clone()).or_insert(false);
        }
    }
    // Only a registered supervisor may enumerate Cloud's project inventory.
    // Never create or borrow another account's registration to hide this limit.
    if let Some(registration) = registration {
        match get_json(
            &config.endpoint,
            "/api/coordination/bindings/activation",
            token,
            &[("registration_id", &registration)],
            // Discovery uses Cloud's deployed default cache headers. Each
            // candidate still needs the no-store authority read below.
            false,
        ) {
            Ok(value) => match inventory_channels(&value, &project_id) {
                Ok(ids) => {
                    if ids.len() >= 100 {
                        report.errors.push("Cloud inventory reached its 100-binding server limit; completeness cannot be proven by this API".into());
                    }
                    for id in ids {
                        candidates.entry(id).or_insert(false);
                    }
                }
                Err(error) => report.errors.push(error),
            },
            Err(error) => report.errors.push(format!(
                "Cloud inventory: {error}; use an existing supervisor's `--registration-id`"
            )),
        }
    } else {
        report.errors.push("Cloud-only discovery unavailable: rerun with `--registration-id <existing-supervisor-id>`; known channels are still checked".into());
    }
    for (id, configured) in candidates {
        if !channel_id(&id) {
            report.errors.push(
                "Invalid configured/journal channel ID; use C... or G... IDs in slack.channels"
                    .into(),
            );
            continue;
        }
        let mapping = mappings
            .as_ref()
            .and_then(|rows| rows.iter().find(|row| row.channel_id == id));
        let observed = get_json(
            &config.endpoint,
            "/api/coordination/bindings/authority",
            token,
            &[("team_id", &team), ("channel_id", &id)],
            true,
        )
        .and_then(|value| authority(value, &team, &id));
        let mut flags = Vec::new();
        if mappings.is_none() {
            flags.push("violet_unknown");
        } else if mapping.is_none() {
            flags.push("violet_unmapped");
        }
        let (binding, strict_activation) = match observed {
            Ok(observation) => observation,
            Err(error) => {
                report.errors.push(format!(
                    "Cloud channel {id}: {error}; run `cas cloud team show`, then retry"
                ));
                flags.push("cloud_unknown");
                (None, None)
            }
        };
        if !flags.contains(&"cloud_unknown") && binding.is_none() {
            flags.push("cloud_unmapped");
        }
        if mapping.is_some_and(|row| row.project_id != project_id)
            || binding
                .as_ref()
                .is_some_and(|row| row.project_id != project_id)
        {
            flags.push("project_mismatch");
        }
        if binding.as_ref().is_some_and(|row| !row.verified) {
            flags.push("cloud_unverified");
        }
        if binding.as_ref().is_some_and(|row| row.verified)
            && strict_activation != Some(StrictActivation::Evidenced)
        {
            flags.push("cloud_unevidenced");
        }
        let remedy = if flags.contains(&"project_mismatch") {
            Some("inspect both project links before remapping; run `cas cloud team show`".into())
        } else if flags.contains(&"violet_unmapped") {
            Some(format!(
                "run `cas integrate violet --channel {id}`, then `cas update`"
            ))
        } else if flags.is_empty() {
            None
        } else {
            Some("run `cas update` for ordinary authorized reconciliation; then rerun `cas violet channels`".into())
        };
        report.channels.push(Channel {
            channel_id: id,
            channel_name: mapping
                .map(|row| row.channel_name.clone())
                .unwrap_or_default(),
            configured,
            violet_project_id: mapping.map(|row| row.project_id.clone()),
            violet_verification: "discovery_only",
            cloud_project_id: binding.as_ref().map(|row| row.project_id.clone()),
            cloud_workspace_id: binding.as_ref().map(|row| row.workspace_id.clone()),
            cloud_verified: binding.as_ref().map(|row| row.verified),
            strict_activation,
            flags,
            remedy,
        });
    }
    report.complete = report.errors.is_empty();
    report.ok = report.complete && report.channels.iter().all(|row| row.flags.is_empty());
    Ok(report)
}

pub fn render(report: &Report) -> String {
    let findings = report
        .channels
        .iter()
        .filter(|row| !row.flags.is_empty())
        .count();
    let mut out = format!(
        "Slack bindings: {} | {} channels, {} findings\n",
        if report.ok { "healthy" } else { "attention" },
        report.channels.len(),
        findings
    );
    out.push_str(if report.ok {
        "Cloud verification and Violet discovery agree.\n"
    } else {
        "-> Inspect the findings below; repair with `cas update`.\n"
    });
    for row in &report.channels {
        let verified = match row.cloud_verified {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        };
        for line in [
            format!(
                "{}{}",
                row.channel_id,
                if row.configured { " (configured)" } else { "" }
            ),
            format!(
                "  Violet project: {} (discovery)",
                row.violet_project_id
                    .as_deref()
                    .unwrap_or("unmapped/unknown")
            ),
            format!(
                "  Cloud project: {}; verified: {verified}",
                row.cloud_project_id
                    .as_deref()
                    .unwrap_or("unmapped/unknown")
            ),
        ] {
            out.push_str(&crate::cloud::presence::wrap_line(&line));
        }
        if !row.flags.is_empty() {
            out.push_str(&crate::cloud::presence::wrap_line(&format!(
                "  Flags: {}",
                row.flags.join(", ")
            )));
        }
        if let Some(remedy) = &row.remedy {
            out.push_str(&crate::cloud::presence::wrap_line(&format!(
                "  -> {remedy}"
            )));
        }
    }
    for error in &report.errors {
        out.push_str(&crate::cloud::presence::wrap_line(&format!(
            "Inventory incomplete: {error}"
        )));
    }
    out
}

pub(crate) fn list_mappings(
    hub: &str,
    token: &str,
    project: Option<&str>,
) -> Result<Vec<Mapping>, String> {
    #[derive(Deserialize)]
    struct Listing {
        channels: Vec<Mapping>,
    }
    let query: Vec<_> = project
        .map(|project| vec![("project_id", project)])
        .unwrap_or_default();
    let listing: Listing =
        serde_json::from_value(get_json(hub, "/api/channels", token, &query, false)?)
            .map_err(|_| "invalid Violet channel listing".to_string())?;
    let mut known = std::collections::BTreeSet::new();
    if listing.channels.iter().any(|row| {
        !channel_id(&row.channel_id)
            || row.project_id.is_empty()
            || !known.insert(row.channel_id.clone())
    }) {
        return Err("invalid or duplicate Violet mapping".into());
    }
    Ok(listing.channels)
}

fn get_json(
    origin: &str,
    path: &str,
    token: &str,
    query: &[(&str, &str)],
    no_store: bool,
) -> Result<Value, String> {
    let mut url = url::Url::parse(origin).map_err(|_| "invalid service URL")?;
    let loopback = url.host_str().is_some_and(|host| {
        host.parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    });
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("unsupported service URL; use HTTPS without embedded credentials".into());
    }
    url.set_path(path);
    url.query_pairs_mut()
        .clear()
        .extend_pairs(query.iter().copied());
    // An empty query is omitted, matching the documented listing route.
    if query.is_empty() {
        url.set_query(None);
    }
    let response = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(Duration::from_secs(10))
        .build()
        .get(url.as_str())
        .set("Authorization", &format!("Bearer {token}"))
        .call();
    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(_) => return Err("service unreachable".into()),
    };
    let status = response.status();
    if no_store
        && !response.header("Cache-Control").is_some_and(|header| {
            header
                .split(',')
                .any(|value| value.trim().eq_ignore_ascii_case("no-store"))
        })
    {
        return Err("Cloud response lacks no-store; verification unknown".into());
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "incomplete service response")?;
    if bytes.len() > 1024 * 1024 {
        return Err("service response exceeds byte budget".into());
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| "invalid service JSON")?;
    if !(200..300).contains(&status) {
        let codes: Vec<&str> = ["code", "reason", "next_action"]
            .into_iter()
            .filter_map(|key| value[key].as_str())
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 80
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
            })
            .collect();
        return Err(format!("HTTP {status} ({})", codes.join(" / ")));
    }
    Ok(value)
}

fn inventory_channels(value: &Value, project: &str) -> Result<Vec<String>, String> {
    if value["schema"] != "cassy.coordination/v1" || value["code"] != "completed" {
        return Err(
            "Cloud binding inventory refused; check the supervisor registration with `cas peers`"
                .into(),
        );
    }
    value["bindings"]
        .as_array()
        .ok_or("invalid Cloud binding inventory")?
        .iter()
        .map(|row| {
            let binding = &row["binding"];
            let id = binding["channel_id"].as_str().filter(|id| channel_id(id));
            if binding["project_id"] != project {
                return Err("Cloud inventory scope mismatch".into());
            }
            id.map(str::to_owned)
                .ok_or_else(|| "invalid Cloud inventory channel".into())
        })
        .collect()
}

fn authority(
    value: Value,
    team: &str,
    channel: &str,
) -> Result<(Option<Binding>, Option<StrictActivation>), String> {
    let keys = ["schema", "code", "binding", "strict_activation"];
    if value["schema"] != SCHEMA
        || value["code"] != "completed"
        || !value.as_object().is_some_and(|object| {
            object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key))
        })
    {
        return Err("invalid/refused Cloud authority response; verification unknown".into());
    }
    let binding: Option<Binding> =
        serde_json::from_value(value["binding"].clone()).map_err(|_| "invalid Cloud binding")?;
    let strict: Option<StrictActivation> =
        serde_json::from_value(value["strict_activation"].clone())
            .map_err(|_| "invalid Cloud activation state")?;
    if let Some(binding) = &binding {
        let timestamp = |value: &Option<String>| {
            value.as_ref().is_none_or(|value| {
                value.ends_with('Z') && chrono::DateTime::parse_from_rfc3339(value).is_ok()
            })
        };
        if binding.team_id != team
            || binding.channel_id != channel
            || binding.provider != "slack"
            || !binding.workspace_id.starts_with('T')
            || binding.workspace_id.len() < 3
            || !binding
                .workspace_id
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
            || uuid::Uuid::parse_str(&binding.id).is_err()
            || binding.project_id.is_empty()
            || binding.project_id.len() > 512
            || binding.project_id.contains('\0')
            || binding.fence_generation > 9_007_199_254_740_991
            || !timestamp(&binding.activated_at)
            || !timestamp(&binding.legacy_fenced_at)
            || strict.is_none()
            || (strict == Some(StrictActivation::Evidenced)
                && (binding.mode != BindingMode::Managed
                    || !binding.verified
                    || binding.fence_generation == 0
                    || binding.activated_at.is_none()
                    || binding.legacy_fenced_at.is_none()))
        {
            return Err("Cloud binding identity/evidence mismatch; verification unknown".into());
        }
    } else if strict.is_some() {
        return Err("Cloud activation without a binding".into());
    }
    Ok((binding, strict))
}

fn channel_id(id: &str) -> bool {
    (id.starts_with('C') || id.starts_with('G'))
        && (3..=31).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}
