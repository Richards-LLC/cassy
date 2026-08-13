//! Petra Stella Cloud relay client for the `cas hub authorize` half of reverse pairing.
//!
//! The wire fields here deliberately mirror `docs/specs/hub-reverse-pairing.md` §4.

use std::collections::BTreeSet;
use std::io::IsTerminal;

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cli::Cli;
use crate::cli::hub::{HubAuthorizeArgs, record_is_live};
use crate::cloud::CloudConfig;
use crate::hub::{AuthStore, HubRuntimePaths, MachineIdentityStore, Scope};

const RELAY_PREFIX: &str = "/api/hub/pairing";
const WIRE_VERSION: u8 = 1;

#[derive(Debug, Deserialize)]
struct ClaimResponse {
    wire_version: u8,
    authorization_id: String,
    pairing_request_id: String,
    controller_origin: String,
    requested_scopes: Vec<Scope>,
    claim_expires_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CompleteResponse {
    wire_version: u8,
    status: String,
    pairing_request_id: String,
    delivery_id: String,
    relay_expires_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct ClaimRequest<'a> {
    wire_version: u8,
    user_code: &'a str,
    authorize_nonce: &'a str,
}

#[derive(Debug, Serialize)]
struct CompleteRequest<'a> {
    wire_version: u8,
    authorize_nonce: &'a str,
    hub_url: &'a str,
    machine_label: &'a str,
    invitation_url: &'a str,
    invitation_expires_at: DateTime<Utc>,
    granted_scopes: &'a BTreeSet<Scope>,
}

#[derive(Debug, Serialize)]
struct CancelRequest<'a> {
    wire_version: u8,
    authorize_nonce: &'a str,
}

struct RelayClient {
    endpoint: String,
    token: String,
}

impl RelayClient {
    fn from_config(config: CloudConfig) -> Result<Self> {
        let token = config
            .token
            .filter(|token| !token.is_empty())
            .context("Not logged in to Petra Stella Cloud. Run `cas login` first.")?;
        Ok(Self {
            endpoint: config.endpoint.trim_end_matches('/').to_owned(),
            token,
        })
    }

    fn claim(&self, code: &str, nonce: &str) -> Result<ClaimResponse> {
        self.post_json(
            "/authorizations",
            &ClaimRequest {
                wire_version: WIRE_VERSION,
                user_code: code,
                authorize_nonce: nonce,
            },
        )
    }

    fn complete(
        &self,
        authorization_id: &str,
        nonce: &str,
        hub_url: &str,
        machine_label: &str,
        invitation_url: &str,
        invitation_expires_at: DateTime<Utc>,
        granted_scopes: &BTreeSet<Scope>,
    ) -> Result<CompleteResponse> {
        self.post_json(
            &format!("/authorizations/{authorization_id}/complete"),
            &CompleteRequest {
                wire_version: WIRE_VERSION,
                authorize_nonce: nonce,
                hub_url,
                machine_label,
                invitation_url,
                invitation_expires_at,
                granted_scopes,
            },
        )
    }

    fn cancel(&self, authorization_id: &str, nonce: &str) -> Result<()> {
        let url = format!(
            "{}{}{}",
            self.endpoint,
            RELAY_PREFIX,
            format!("/authorizations/{authorization_id}/cancel")
        );
        match ureq::post(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .send_json(CancelRequest {
                wire_version: WIRE_VERSION,
                authorize_nonce: nonce,
            }) {
            Ok(_) => Ok(()),
            Err(error) => Err(relay_error(error)),
        }
    }

    fn post_json<T: serde::de::DeserializeOwned, B: Serialize>(
        &self,
        suffix: &str,
        body: &B,
    ) -> Result<T> {
        let url = format!("{}{}{}", self.endpoint, RELAY_PREFIX, suffix);
        let response = ureq::post(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .send_json(body)
            .map_err(relay_error)?;
        let response: T = response.into_json().context("invalid relay response")?;
        Ok(response)
    }
}

pub(super) fn authorize(args: &HubAuthorizeArgs, cli: &Cli) -> Result<()> {
    let user_cloud = CloudConfig::load_user().unwrap_or_default();
    let cloud = if user_cloud.is_logged_in() {
        user_cloud
    } else {
        CloudConfig::load().unwrap_or_default()
    };
    let relay = RelayClient::from_config(cloud)?;
    let code = args.code.trim().to_ascii_uppercase();
    anyhow::ensure!(!code.is_empty(), "pairing code must not be empty");
    let nonce = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    let claim = relay.claim(&code, &nonce)?;
    anyhow::ensure!(
        claim.wire_version == WIRE_VERSION,
        "unsupported relay wire version"
    );

    let granted = granted_scopes(&claim.requested_scopes, args.scopes.as_deref())?;

    let confirmed = confirm(args.yes, cli, &claim, &granted)?;
    if !confirmed {
        // The lease also self-releases after 120 seconds, but cancelling promptly
        // makes a declined code usable immediately by the intended machine.
        let _ = relay.cancel(&claim.authorization_id, &nonce);
        if cli.json {
            println!("{}", serde_json::json!({"status":"declined"}));
        } else {
            println!("Authorization declined; no invitation was minted.");
        }
        return Ok(());
    }

    let hub_url = resolve_hub_url(args.hub_url.as_deref(), &claim.controller_origin)?;
    let paths = HubRuntimePaths::default_for_user()?;
    let machine = MachineIdentityStore::new(paths.root()).load_or_create()?;
    let auth = AuthStore::open(paths.root(), machine.id)?;
    let invitation = auth.mint_pairing(&claim.controller_origin, granted.clone(), Utc::now())?;
    let machine_label = hostname::get()
        .ok()
        .and_then(|hostname| hostname.into_string().ok())
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| "CAS machine".to_owned());
    let complete = relay.complete(
        &claim.authorization_id,
        &nonce,
        &hub_url,
        &machine_label,
        &invitation.url,
        invitation.expires_at,
        &granted,
    )?;
    anyhow::ensure!(
        complete.wire_version == WIRE_VERSION && complete.status == "ready",
        "invalid relay completion response"
    );
    anyhow::ensure!(
        complete.pairing_request_id == claim.pairing_request_id,
        "relay completion response did not match the claimed request"
    );
    if cli.json {
        println!("{}", serde_json::to_string(&complete)?);
    } else {
        println!(
            "Commander pairing invitation delivered for {}.",
            claim.controller_origin
        );
        println!(
            "Relay expires at {}.",
            complete.relay_expires_at.to_rfc3339()
        );
    }
    Ok(())
}

fn confirm(yes: bool, cli: &Cli, claim: &ClaimResponse, granted: &BTreeSet<Scope>) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if cli.json || !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    println!("Commander origin: {}", claim.controller_origin);
    println!(
        "Requested scopes: {}",
        display_scopes(&claim.requested_scopes)
    );
    println!("Machine-granted scopes: {}", display_scopes(granted));
    println!("Claim expires at: {}", claim.claim_expires_at.to_rfc3339());
    Ok(
        inquire::Confirm::new("Deliver a one-time pairing invitation?")
            .with_default(false)
            .prompt()
            .unwrap_or(false),
    )
}

fn resolve_hub_url(explicit: Option<&str>, controller_origin: &str) -> Result<String> {
    if let Some(url) = explicit {
        return Ok(url.to_owned());
    }
    let paths = HubRuntimePaths::default_for_user()?;
    let record = paths.read_process_record()?;
    anyhow::ensure!(
        record_is_live(&record),
        "cas hub is not running; start it before authorizing a Commander page"
    );
    if let Some(url) = record.public_url {
        return Ok(url);
    }
    if is_loopback_origin(controller_origin) {
        return Ok(format!("http://{}:{}", record.bind, record.port));
    }
    anyhow::bail!(
        "Hub has no public URL. Start it with `cas hub --tailscale-serve start` or pass --hub-url."
    )
}

fn is_loopback_origin(origin: &str) -> bool {
    origin.starts_with("http://127.") || origin.starts_with("http://[::1]")
}

fn granted_scopes(
    requested: &[Scope],
    override_scopes: Option<&[String]>,
) -> Result<BTreeSet<Scope>> {
    let requested: BTreeSet<Scope> = requested.iter().copied().collect();
    anyhow::ensure!(!requested.is_empty(), "relay returned an empty scope set");
    let granted = match override_scopes {
        Some(scopes) => scopes
            .iter()
            .map(|scope| Scope::parse(scope))
            .collect::<Result<BTreeSet<_>>>()?,
        None => requested.clone(),
    };
    anyhow::ensure!(!granted.is_empty(), "granted scopes must not be empty");
    anyhow::ensure!(
        granted.is_subset(&requested),
        "--scopes may reduce the page-requested scopes but may not add scopes"
    );
    Ok(granted)
}

fn display_scopes(scopes: impl IntoIterator<Item = impl std::borrow::Borrow<Scope>>) -> String {
    scopes
        .into_iter()
        .map(|scope| scope.borrow().as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn relay_error(error: ureq::Error) -> anyhow::Error {
    let (status, envelope) = match error {
        ureq::Error::Status(status, response) => {
            let body = response.into_json::<serde_json::Value>().ok();
            (Some(status), body)
        }
        ureq::Error::Transport(error) => return anyhow::anyhow!("relay unavailable: {error}"),
    };
    let code = envelope
        .as_ref()
        .and_then(|body| body.get("error"))
        .and_then(|value| value.as_str())
        .unwrap_or("relay_error");
    let description = envelope
        .as_ref()
        .and_then(|body| body.get("error_description"))
        .and_then(|value| value.as_str())
        .unwrap_or("request was refused");
    relay_error_from_parts(status, code, description)
}

fn relay_error_from_parts(status: Option<u16>, code: &str, description: &str) -> anyhow::Error {
    match (status, code) {
        (Some(404), "invalid_code") => {
            anyhow::anyhow!("No live pairing request matches that code.")
        }
        (Some(410), "expired_code") => anyhow::anyhow!("The pairing code has expired."),
        (Some(409), "code_claimed") => {
            anyhow::anyhow!("That pairing code is already claimed by another machine.")
        }
        (Some(401), "unauthorized") => {
            anyhow::anyhow!("PSC authorization was refused. Run `cas login` first.")
        }
        _ => anyhow::anyhow!("relay request failed ({code}): {description}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_four_claim_fixture_is_byte_faithful() {
        let expected: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../hub-web/src/fixtures/hub-reverse-pairing/claim-request.json"
        )))
        .unwrap();
        let request = serde_json::to_value(ClaimRequest {
            wire_version: 1,
            user_code: "K7MW-4H2Q",
            authorize_nonce: "base64url-32-random-bytes",
        })
        .unwrap();
        assert_eq!(request, expected);
        assert_eq!(
            format!("{RELAY_PREFIX}/authorizations"),
            "/api/hub/pairing/authorizations"
        );
    }

    #[test]
    fn section_four_completion_fixture_uses_kebab_case_scopes() {
        let expected: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../hub-web/src/fixtures/hub-reverse-pairing/complete-request.json"
        )))
        .unwrap();
        let scopes = Scope::default_read_only();
        let request = serde_json::to_value(CompleteRequest {
            wire_version: 1,
            authorize_nonce: "base64url-32-random-bytes",
            hub_url: "https://workstation.tail.example:443/",
            machine_label: "Studio workstation",
            invitation_url: "https://commander.example/#pair=base64url-32-random-bytes&hub=machine-uuid",
            invitation_expires_at: "2026-08-11T20:11:30Z".parse().unwrap(),
            granted_scopes: &scopes,
        })
        .unwrap();
        assert_eq!(request, expected);
        assert_eq!(
            format!("{RELAY_PREFIX}/authorizations/id/complete"),
            "/api/hub/pairing/authorizations/id/complete"
        );
    }

    #[test]
    fn documented_wrong_code_and_expiry_are_honest() {
        let invalid = relay_error_from_parts(Some(404), "invalid_code", "safe text");
        assert!(invalid.to_string().contains("No live pairing request"));
        let expired = relay_error_from_parts(Some(410), "expired_code", "safe text");
        assert!(expired.to_string().contains("expired"));
    }

    #[test]
    fn scopes_can_reduce_the_claim_but_never_escalate_it() {
        let requested = vec![Scope::MachineRead, Scope::SessionRead, Scope::PaneRead];
        let reduced = granted_scopes(&requested, Some(&["machine:read".to_owned()])).unwrap();
        assert_eq!(reduced, [Scope::MachineRead].into_iter().collect());
        let escalation = granted_scopes(&requested, Some(&["hub:admin".to_owned()])).unwrap_err();
        assert!(escalation.to_string().contains("may reduce"));
    }

    #[test]
    fn loopback_fallback_requires_a_literal_http_loopback_origin() {
        for origin in [
            "http://127.0.0.1",
            "http://127.42.0.1:42759",
            "http://[::1]:42759",
        ] {
            assert!(is_loopback_origin(origin), "{origin}");
        }

        for origin in [
            "http://127.evil.com",
            "http://127.0.0.1.evil.com",
            "http://127.0.0.1@evil.example",
            "http://attacker@127.0.0.1",
            "http://127.0.0.1/pair",
            "http://[::1]/pair",
            "http://127.0.0.1?next=https://evil.example",
            "http://127.0.0.1#fragment",
            "http://localhost:42759",
            "http://192.168.1.1:42759",
            "https://127.0.0.1:42759",
        ] {
            assert!(!is_loopback_origin(origin), "{origin}");
        }
    }
}
