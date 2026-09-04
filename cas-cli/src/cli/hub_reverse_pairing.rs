//! Petra Stella Cloud relay client for the `cas hub authorize` half of reverse pairing.
//!
//! The wire fields here deliberately mirror `docs/specs/hub-reverse-pairing.md` §4.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::{Host, Url};

use crate::cli::Cli;
use crate::cli::hub::{HubAuthorizeArgs, record_is_live};
use crate::cloud::CloudConfig;
use crate::hub::{
    AuthStore, HubRuntimePaths, MachineIdentityStore, PairingInvitationTarget, Scope,
};

const RELAY_PREFIX: &str = "/api/hub/pairing";
const WIRE_VERSION: u8 = 1;
const LAST_HUB_URL_FILE: &str = "last-public-url";

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

trait PairingRelay {
    fn claim(&self, code: &str, nonce: &str) -> Result<ClaimResponse>;

    fn complete(
        &self,
        authorization_id: &str,
        nonce: &str,
        hub_url: &str,
        machine_label: &str,
        invitation_url: &str,
        invitation_expires_at: DateTime<Utc>,
        granted_scopes: &BTreeSet<Scope>,
    ) -> Result<CompleteResponse>;

    fn cancel(&self, authorization_id: &str, nonce: &str) -> Result<()>;
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

    fn post_json<T: serde::de::DeserializeOwned, B: Serialize>(
        &self,
        suffix: &str,
        body: &B,
    ) -> Result<T> {
        let url = format!("{}{}{}", self.endpoint, RELAY_PREFIX, suffix);
        let diagnostic_body = relay_request_diagnostic(body);
        let response = ureq::post(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .send_json(body)
            .map_err(|error| relay_error_with_request(error, body))
            .with_context(|| {
                format!("relay POST {suffix} failed; request body: {diagnostic_body}")
            })?;
        let response: T = response.into_json().context("invalid relay response")?;
        Ok(response)
    }
}

impl PairingRelay for RelayClient {
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
}

#[derive(Debug, Serialize, Deserialize)]
struct SavedAuthorizationAttempt {
    user_code: String,
    authorize_nonce: String,
    created_at: DateTime<Utc>,
}

struct AuthorizationAttempt {
    path: PathBuf,
    nonce: String,
}

impl AuthorizationAttempt {
    fn load_or_create(root: &Path, code: &str) -> Result<Self> {
        let digest = hex::encode(Sha256::digest(code.as_bytes()));
        let path = root.join(format!(".authorize-{digest}.json"));
        if path.exists() {
            ensure_private_attempt_file(&path)?;
            let saved: SavedAuthorizationAttempt = serde_json::from_slice(&fs::read(&path)?)
                .context("invalid saved hub authorization attempt")?;
            anyhow::ensure!(
                saved.user_code == code,
                "saved hub authorization attempt does not match its pairing code"
            );
            let decoded = URL_SAFE_NO_PAD
                .decode(saved.authorize_nonce.as_bytes())
                .context("invalid nonce in saved hub authorization attempt")?;
            anyhow::ensure!(
                decoded.len() == 32,
                "invalid nonce in saved hub authorization attempt"
            );
            if Utc::now().signed_duration_since(saved.created_at) >= chrono::Duration::minutes(10) {
                fs::remove_file(&path).context("remove expired hub authorization attempt")?;
                return Self::load_or_create(root, code);
            }
            return Ok(Self {
                path,
                nonce: saved.authorize_nonce,
            });
        }

        let nonce = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
        let saved = SavedAuthorizationAttempt {
            user_code: code.to_owned(),
            authorize_nonce: nonce.clone(),
            created_at: Utc::now(),
        };
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => {
                file.write_all(&serde_json::to_vec(&saved)?)?;
                file.sync_all()?;
                Ok(Self { path, nonce })
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Self::load_or_create(root, code)
            }
            Err(error) => Err(error).context("save hub authorization attempt"),
        }
    }

    fn finish(self) -> Result<()> {
        match fs::remove_file(self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("remove completed hub authorization attempt"),
        }
    }
}

fn ensure_private_attempt_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "saved hub authorization attempt is not a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            metadata.mode() & 0o777 == 0o600,
            "saved hub authorization attempt must have mode 0600"
        );
        anyhow::ensure!(
            metadata.uid() == unsafe { libc::geteuid() },
            "saved hub authorization attempt has the wrong owner"
        );
    }
    Ok(())
}

pub(super) fn authorize(args: &HubAuthorizeArgs, cli: &Cli) -> Result<()> {
    let user_cloud = CloudConfig::load_user().unwrap_or_default();
    let cloud = if user_cloud.is_logged_in() {
        user_cloud
    } else {
        CloudConfig::load().unwrap_or_default()
    };
    let relay = RelayClient::from_config(cloud)?;
    let paths = HubRuntimePaths::default_for_user()?;
    let config_root = crate::store::find_cas_root().ok();
    let configured_hub_url = config_root
        .as_deref()
        .and_then(|cas_root| crate::config::Config::load(cas_root).ok())
        .and_then(|config| config.hub.and_then(|hub| hub.public_url));
    authorize_with_relay_with_config(
        args,
        cli,
        &relay,
        &paths,
        configured_hub_url.as_deref(),
        config_root.as_deref(),
    )
}

fn authorize_with_relay(
    args: &HubAuthorizeArgs,
    cli: &Cli,
    relay: &impl PairingRelay,
    paths: &HubRuntimePaths,
) -> Result<()> {
    authorize_with_relay_with_config(args, cli, relay, paths, None, None)
}

fn authorize_with_relay_with_config(
    args: &HubAuthorizeArgs,
    cli: &Cli,
    relay: &impl PairingRelay,
    paths: &HubRuntimePaths,
    configured_hub_url: Option<&str>,
    config_root: Option<&Path>,
) -> Result<()> {
    let code = args.code.trim().to_ascii_uppercase();
    anyhow::ensure!(!code.is_empty(), "pairing code must not be empty");
    let override_scopes = parse_override_scopes(args.scopes.as_deref())?;
    let hub_url = resolve_hub_url(paths, args.hub_url.as_deref(), configured_hub_url)?;
    let machine = MachineIdentityStore::new(paths.root()).load_or_create()?;
    let auth = AuthStore::open(paths.root(), machine.id)?;
    let machine_label = hostname::get()
        .ok()
        .and_then(|hostname| hostname.into_string().ok())
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| "Cassy machine".to_owned());
    let attempt = AuthorizationAttempt::load_or_create(paths.root(), &code)?;
    let claim = relay.claim(&code, &attempt.nonce)?;
    anyhow::ensure!(
        claim.wire_version == WIRE_VERSION,
        "unsupported relay wire version"
    );

    let granted = granted_scopes(&claim.requested_scopes, override_scopes.as_ref())?;

    let confirmed = confirm(args.yes, cli, &claim, &granted)?;
    if !confirmed {
        // The lease also self-releases after 120 seconds, but cancelling promptly
        // makes a declined code usable immediately by the intended machine.
        if relay
            .cancel(&claim.authorization_id, &attempt.nonce)
            .is_ok()
        {
            attempt.finish()?;
        }
        if cli.json {
            println!("{}", serde_json::json!({"status":"declined"}));
        } else {
            println!("Authorization declined; no invitation was minted.");
        }
        return Ok(());
    }

    let invitation = auth.mint_pairing(&claim.controller_origin, granted.clone(), Utc::now())?;
    let relay_invitation_url = invitation.url_for(PairingInvitationTarget::HostedRelay);
    let complete = relay.complete(
        &claim.authorization_id,
        &attempt.nonce,
        &hub_url,
        &machine_label,
        &relay_invitation_url,
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
    persist_last_hub_url(paths, &hub_url)?;
    if args.hub_url.is_some()
        && let Some(config_root) = config_root
    {
        persist_configured_hub_url(config_root, &hub_url)?;
    }
    attempt.finish()?;
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
    print_confirmation_summary(claim, granted);
    println!("Claim expires at: {}", claim.claim_expires_at.to_rfc3339());
    Ok(
        inquire::Confirm::new("Deliver a one-time pairing invitation?")
            .with_default(false)
            .prompt()
            .unwrap_or(false),
    )
}

/// Print the consent boundary for a relay request. Control scopes are separate
/// from discovery so an operator cannot mistake terminal input for read-only
/// pairing while approving a code from an untrusted page.
fn print_confirmation_summary(claim: &ClaimResponse, granted: &BTreeSet<Scope>) {
    println!("Commander origin to verify: {}", claim.controller_origin);
    println!(
        "{summary}",
        summary = confirmation_scope_summary(claim, granted)
    );
    println!("Verify the Commander origin before approving this one-time invitation.");
}

fn confirmation_scope_summary(claim: &ClaimResponse, granted: &BTreeSet<Scope>) -> String {
    format!(
        "Scopes requested: read: {}; control: {}\nScopes granted: read: {}; control: {}",
        display_scopes_by_kind(&claim.requested_scopes, false),
        display_scopes_by_kind(&claim.requested_scopes, true),
        display_scopes_by_kind(granted, false),
        display_scopes_by_kind(granted, true),
    )
}

fn display_scopes_by_kind(
    scopes: impl IntoIterator<Item = impl std::borrow::Borrow<Scope>>,
    control: bool,
) -> String {
    let scopes = scopes
        .into_iter()
        .map(|scope| *scope.borrow())
        .filter(|scope| is_control_scope(*scope) == control)
        .map(Scope::as_str)
        .collect::<Vec<_>>();
    if scopes.is_empty() {
        "none".to_owned()
    } else {
        scopes.join(", ")
    }
}

fn is_control_scope(scope: Scope) -> bool {
    matches!(
        scope,
        Scope::PaneInput
            | Scope::MessageSend
            | Scope::PaneInterrupt
            | Scope::FactoryManage
            | Scope::HubAdmin
    )
}

fn resolve_hub_url(
    paths: &HubRuntimePaths,
    explicit: Option<&str>,
    configured_hub_url: Option<&str>,
) -> Result<String> {
    let record = paths.read_process_record()?;
    anyhow::ensure!(
        record_is_live(&record),
        "cas hub is not running; start it before authorizing a Commander page"
    );
    let remembered_hub_url = read_last_hub_url(paths)?;
    let url = explicit
        .or(record.public_url.as_deref())
        .or(configured_hub_url)
        .or(remembered_hub_url.as_deref())
        .context(
            "hub is running without a public URL; pass --hub-url https://<commander-host> (remembered for next time) or `cas hub restart --tailscale-serve`",
        )?;
    let parsed = validate_hub_url(url)?;
    Ok(parsed.origin().ascii_serialization())
}

fn validate_hub_url(url: &str) -> Result<Url> {
    let normalized = normalize_hub_url(url);
    let parsed = Url::parse(&normalized).context("invalid hub URL")?;
    anyhow::ensure!(
        parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.host().is_some()
            && parsed.path() == "/"
            && parsed.query().is_none()
            && parsed.fragment().is_none(),
        "hub URL must be an HTTPS origin or an HTTP IP-loopback origin"
    );
    anyhow::ensure!(
        parsed.scheme() == "https" || is_loopback_origin(&normalized),
        "hub URL must be an HTTPS origin or an HTTP IP-loopback origin"
    );
    Ok(parsed)
}

fn normalize_hub_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.contains("://") {
        return trimmed.to_owned();
    }

    // A literal IP-loopback address is safe for local development over HTTP;
    // other bare hosts default to HTTPS so Commander origins remain secure.
    let loopback_candidate = format!("http://{trimmed}");
    if is_loopback_origin(&loopback_candidate) {
        loopback_candidate
    } else {
        format!("https://{trimmed}")
    }
}

fn last_hub_url_path(paths: &HubRuntimePaths) -> PathBuf {
    paths.root().join(LAST_HUB_URL_FILE)
}

fn read_last_hub_url(paths: &HubRuntimePaths) -> Result<Option<String>> {
    let path = last_hub_url_path(paths);
    match fs::read_to_string(&path) {
        Ok(url) => {
            let url = url.trim();
            Ok((!url.is_empty()).then(|| url.to_owned()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("read remembered hub URL at {}", path.display()))
        }
    }
}

fn persist_last_hub_url(paths: &HubRuntimePaths, hub_url: &str) -> Result<()> {
    crate::hub::ensure_private_dir(paths.root())?;
    let target = last_hub_url_path(paths);
    let temporary = paths
        .root()
        .join(format!(".{LAST_HUB_URL_FILE}.{}.tmp", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .with_context(|| format!("open remembered hub URL at {}", temporary.display()))?;
    file.write_all(hub_url.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(&temporary, &target)
        .with_context(|| format!("save remembered hub URL at {}", target.display()))?;
    Ok(())
}

fn persist_configured_hub_url(cas_root: &Path, hub_url: &str) -> Result<()> {
    let mut config = crate::config::Config::load(cas_root)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    config
        .hub
        .get_or_insert_with(crate::config::HubConfig::default)
        .public_url = Some(hub_url.to_owned());
    config
        .save_toml(cas_root)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(())
}

fn is_loopback_origin(origin: &str) -> bool {
    let Ok(url) = Url::parse(origin) else {
        return false;
    };
    if url.scheme() != "http"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        Some(Host::Domain(_)) | None => false,
    }
}

fn granted_scopes(
    requested: &[Scope],
    override_scopes: Option<&BTreeSet<Scope>>,
) -> Result<BTreeSet<Scope>> {
    let requested: BTreeSet<Scope> = requested.iter().copied().collect();
    anyhow::ensure!(!requested.is_empty(), "relay returned an empty scope set");
    let granted = match override_scopes {
        Some(scopes) => scopes.clone(),
        None => requested.clone(),
    };
    anyhow::ensure!(!granted.is_empty(), "granted scopes must not be empty");
    anyhow::ensure!(
        granted.is_subset(&requested),
        "--scopes may reduce the page-requested scopes but may not add scopes"
    );
    Ok(granted)
}

fn parse_override_scopes(scopes: Option<&[String]>) -> Result<Option<BTreeSet<Scope>>> {
    scopes
        .map(|scopes| {
            scopes
                .iter()
                .map(|scope| Scope::parse(scope))
                .collect::<Result<BTreeSet<_>>>()
        })
        .transpose()
}

const RELAY_INVITATION_CONTRACT: &str = "exactly two fragment keys (`pair` and `hub`), a 32-byte base64url `pair` secret, a `hub` value of at most 128 bytes, root path `/`, and no query";

fn validate_relay_invitation_url(invitation_url: &str) -> Result<()> {
    let parsed = Url::parse(invitation_url).context("invalid relay invitation URL")?;
    anyhow::ensure!(
        parsed.host_str().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.path() == "/"
            && parsed.query().is_none(),
        "relay invitation URL must use {RELAY_INVITATION_CONTRACT}"
    );
    let fragment = parsed
        .fragment()
        .context("relay invitation URL must use {RELAY_INVITATION_CONTRACT}")?;
    let fields = fragment.split('&').collect::<Vec<_>>();
    anyhow::ensure!(
        fields.len() == 2,
        "relay invitation URL must use {RELAY_INVITATION_CONTRACT}"
    );

    let mut pair = None;
    let mut hub = None;
    for field in fields {
        let (key, value) = field
            .split_once('=')
            .context("relay invitation URL must use {RELAY_INVITATION_CONTRACT}")?;
        anyhow::ensure!(
            !value.is_empty(),
            "relay invitation URL must use {RELAY_INVITATION_CONTRACT}"
        );
        match key {
            "pair" => anyhow::ensure!(
                pair.replace(value).is_none(),
                "relay invitation URL must use {RELAY_INVITATION_CONTRACT}"
            ),
            "hub" => anyhow::ensure!(
                hub.replace(value).is_none(),
                "relay invitation URL must use {RELAY_INVITATION_CONTRACT}"
            ),
            _ => anyhow::bail!("relay invitation URL must use {RELAY_INVITATION_CONTRACT}"),
        }
    }

    let pair = pair.context("relay invitation URL must use {RELAY_INVITATION_CONTRACT}")?;
    let hub = hub.context("relay invitation URL must use {RELAY_INVITATION_CONTRACT}")?;
    let pair = URL_SAFE_NO_PAD
        .decode(pair)
        .context("relay invitation URL must use {RELAY_INVITATION_CONTRACT}")?;
    anyhow::ensure!(
        pair.len() == 32 && hub.len() <= 128,
        "relay invitation URL must use {RELAY_INVITATION_CONTRACT}"
    );
    Ok(())
}

fn relay_error(error: ureq::Error) -> anyhow::Error {
    let (status, code, description) = relay_error_parts(error);
    relay_error_from_parts(status, &code, &description)
}

fn relay_error_with_request<B: Serialize>(error: ureq::Error, body: &B) -> anyhow::Error {
    let (status, code, description) = relay_error_parts(error);
    if code == "invalid_invitation" {
        let invitation_url = serde_json::to_value(body)
            .ok()
            .and_then(|body| {
                body.get("invitation_url")
                    .and_then(|url| url.as_str())
                    .map(str::to_owned)
            })
            .map(|url| relay_invitation_shape(&url))
            .unwrap_or_else(|| "<unavailable>".to_owned());
        return anyhow::anyhow!(
            "relay rejected invitation_url (invalid_invitation): the relay requires {RELAY_INVITATION_CONTRACT}; sent URL shape: {invitation_url}"
        );
    }
    relay_error_from_parts(status, &code, &description)
}

fn relay_error_parts(error: ureq::Error) -> (Option<u16>, String, String) {
    let (status, envelope) = match error {
        ureq::Error::Status(status, response) => {
            let body = response.into_json::<serde_json::Value>().ok();
            (Some(status), body)
        }
        ureq::Error::Transport(error) => {
            return (None, "relay_unavailable".to_owned(), error.to_string());
        }
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
    (status, code.to_owned(), description.to_owned())
}

fn relay_invitation_shape(invitation_url: &str) -> String {
    let Ok(parsed) = Url::parse(invitation_url) else {
        return "<invalid URL>".to_owned();
    };
    let hub = parsed
        .fragment()
        .and_then(|fragment| {
            fragment
                .split('&')
                .find_map(|field| field.strip_prefix("hub="))
        })
        .unwrap_or("<missing>");
    format!(
        "{}/#pair=<redacted>&hub={hub}",
        parsed.origin().ascii_serialization()
    )
}

fn relay_request_diagnostic(body: &impl Serialize) -> String {
    let mut value = match serde_json::to_value(body) {
        Ok(value) => value,
        Err(error) => return format!("<could not serialize request: {error}>"),
    };
    redact_relay_secrets(&mut value);
    serde_json::to_string(&value)
        .unwrap_or_else(|error| format!("<could not format request: {error}>"))
}

fn redact_relay_secrets(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(fields) => {
            for (name, value) in fields {
                if matches!(
                    name.as_str(),
                    "authorize_nonce" | "invitation_url" | "user_code"
                ) {
                    *value = serde_json::Value::String("<redacted>".to_owned());
                } else {
                    redact_relay_secrets(value);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_relay_secrets(value);
            }
        }
        _ => {}
    }
}

fn relay_error_from_parts(status: Option<u16>, code: &str, description: &str) -> anyhow::Error {
    match (status, code) {
        (None, "relay_unavailable") => anyhow::anyhow!("relay unavailable: {description}"),
        (Some(404), "invalid_code") => {
            anyhow::anyhow!("No live pairing request matches that code.")
        }
        (Some(410), "expired_code") => anyhow::anyhow!("The pairing code has expired."),
        (Some(409), "code_claimed") => {
            anyhow::anyhow!(
                "That pairing code has a live claim from a different machine or an earlier local attempt whose saved claim state is unavailable. This machine automatically resumes claims whose state is still present."
            )
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
    fn section_four_completion_fixture_pins_relay_accepted_canonical_origin() {
        let expected: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../hub-web/src/fixtures/hub-reverse-pairing/complete-request.json"
        )))
        .unwrap();
        let scopes = Scope::default_read_only();
        let request = serde_json::to_value(CompleteRequest {
            wire_version: 1,
            authorize_nonce: "base64url-32-random-bytes",
            hub_url: "https://workstation.tail.example",
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
    fn completion_failure_diagnostic_shows_canonical_origin_without_secrets() {
        let scopes = Scope::default_read_only();
        let diagnostic = relay_request_diagnostic(&CompleteRequest {
            wire_version: 1,
            authorize_nonce: "secret-authorize-nonce",
            hub_url: "https://workstation.tail.example",
            machine_label: "Studio workstation",
            invitation_url: "https://commander.example/#pair=secret-invitation-token",
            invitation_expires_at: "2026-08-11T20:11:30Z".parse().unwrap(),
            granted_scopes: &scopes,
        });

        assert!(
            diagnostic.contains(r#""hub_url":"https://workstation.tail.example""#),
            "{diagnostic}"
        );
        assert!(diagnostic.contains(r#""authorize_nonce":"<redacted>""#));
        assert!(diagnostic.contains(r#""invitation_url":"<redacted>""#));
        assert!(!diagnostic.contains("secret-authorize-nonce"));
        assert!(!diagnostic.contains("secret-invitation-token"));
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
        let reduced_override = parse_override_scopes(Some(&["machine:read".to_owned()])).unwrap();
        let reduced = granted_scopes(&requested, reduced_override.as_ref()).unwrap();
        assert_eq!(reduced, [Scope::MachineRead].into_iter().collect());
        let escalation_override = parse_override_scopes(Some(&["hub:admin".to_owned()])).unwrap();
        let escalation = granted_scopes(&requested, escalation_override.as_ref()).unwrap_err();
        assert!(escalation.to_string().contains("may reduce"));
    }

    #[test]
    fn confirmation_separates_terminal_control_from_read_access() {
        let scopes = [
            Scope::MachineRead,
            Scope::SessionRead,
            Scope::PaneRead,
            Scope::PaneInput,
            Scope::MessageSend,
            Scope::PaneInterrupt,
        ];
        assert_eq!(
            display_scopes_by_kind(scopes, false),
            "machine:read, session:read, pane:read"
        );
        assert_eq!(
            display_scopes_by_kind(scopes, true),
            "pane:input, message:send, pane:interrupt"
        );
        assert_eq!(display_scopes_by_kind([Scope::MachineRead], true), "none");
    }

    #[test]
    fn confirmation_scope_summary_prints_requested_and_granted_blocks_once() {
        let scopes = [Scope::MachineRead, Scope::SessionRead, Scope::PaneInput];
        let claim = ClaimResponse {
            wire_version: WIRE_VERSION,
            authorization_id: "authorization".to_owned(),
            pairing_request_id: "pairing-request".to_owned(),
            controller_origin: "https://commander.example".to_owned(),
            requested_scopes: scopes.to_vec(),
            claim_expires_at: Utc::now(),
        };
        let granted: BTreeSet<_> = scopes.iter().copied().collect();
        let summary = confirmation_scope_summary(&claim, &granted);

        assert_eq!(summary.matches("Scopes requested:").count(), 1);
        assert_eq!(summary.matches("Scopes granted:").count(), 1);
        assert!(summary.contains("read: machine:read, session:read"));
        assert!(summary.contains("control: pane:input"));
    }

    #[test]
    fn loopback_hub_url_requires_a_literal_http_loopback_origin() {
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

    #[test]
    fn bare_hub_hosts_are_https_and_bare_ip_loopback_is_http() {
        assert_eq!(
            validate_hub_url("hub.petrastella.io")
                .unwrap()
                .origin()
                .ascii_serialization(),
            "https://hub.petrastella.io"
        );
        assert_eq!(
            validate_hub_url("127.0.0.1:4173")
                .unwrap()
                .origin()
                .ascii_serialization(),
            "http://127.0.0.1:4173"
        );
    }

    #[test]
    fn hub_url_resolution_obeys_explicit_record_config_then_remembered_precedence() {
        let temp = tempfile::tempdir().unwrap();
        let paths = HubRuntimePaths::new(temp.path().join("hub"));
        let (port, health) = serve_ready_health_checks(4);
        write_live_record(&paths, port, Some("https://record.example"));

        assert_eq!(
            resolve_hub_url(&paths, Some("explicit.example"), Some("config.example")).unwrap(),
            "https://explicit.example"
        );
        assert_eq!(
            resolve_hub_url(&paths, None, Some("config.example")).unwrap(),
            "https://record.example"
        );

        write_live_record(&paths, port, None);
        assert_eq!(
            resolve_hub_url(&paths, None, Some("config.example")).unwrap(),
            "https://config.example"
        );
        fs::create_dir_all(paths.root()).unwrap();
        fs::write(paths.root().join(LAST_HUB_URL_FILE), "remembered.example\n").unwrap();
        assert_eq!(
            resolve_hub_url(&paths, None, None).unwrap(),
            "https://remembered.example"
        );
        health.join().unwrap();
    }

    #[test]
    fn running_hub_without_public_origin_has_state_aware_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let paths = HubRuntimePaths::new(temp.path().join("hub"));
        let (port, health) = serve_ready_health_checks(1);
        write_live_record(&paths, port, None);

        let error = resolve_hub_url(&paths, None, None).unwrap_err().to_string();
        assert!(error.contains("hub is running without a public URL"));
        assert!(error.contains("cas hub restart --tailscale-serve"));
        assert!(!error.contains("cas hub --tailscale-serve start"));
        health.join().unwrap();
    }

    #[derive(Default)]
    struct RecordingRelay {
        claims: std::sync::Mutex<Vec<(String, String)>>,
        completed_hub_urls: std::sync::Mutex<Vec<String>>,
        completed_invitation_urls: std::sync::Mutex<Vec<String>>,
        active_claim: std::sync::Mutex<Option<(String, String)>>,
        fail_first_completion: std::sync::atomic::AtomicBool,
    }

    impl RecordingRelay {
        fn failing_first_completion() -> Self {
            let relay = Self::default();
            relay
                .fail_first_completion
                .store(true, std::sync::atomic::Ordering::SeqCst);
            relay
        }
    }

    impl PairingRelay for RecordingRelay {
        fn claim(&self, code: &str, nonce: &str) -> Result<ClaimResponse> {
            let mut active = self.active_claim.lock().unwrap();
            match active.as_ref() {
                Some((claimed_code, claimed_nonce))
                    if claimed_code == code && claimed_nonce == nonce => {}
                Some(_) => anyhow::bail!("code_claimed"),
                None => *active = Some((code.to_owned(), nonce.to_owned())),
            }
            self.claims
                .lock()
                .unwrap()
                .push((code.to_owned(), nonce.to_owned()));
            Ok(ClaimResponse {
                wire_version: 1,
                authorization_id: "0209c457-4798-413a-8a52-47f7b25e9d61".to_owned(),
                pairing_request_id: "9ef5a981-0c32-44b4-9c8a-d1f8e4858e77".to_owned(),
                controller_origin: "https://commander.example".to_owned(),
                requested_scopes: Scope::default_read_only().into_iter().collect(),
                claim_expires_at: Utc::now() + chrono::Duration::seconds(120),
            })
        }

        fn complete(
            &self,
            _authorization_id: &str,
            _nonce: &str,
            hub_url: &str,
            _machine_label: &str,
            _invitation_url: &str,
            _invitation_expires_at: DateTime<Utc>,
            _granted_scopes: &BTreeSet<Scope>,
        ) -> Result<CompleteResponse> {
            self.completed_hub_urls
                .lock()
                .unwrap()
                .push(hub_url.to_owned());
            self.completed_invitation_urls
                .lock()
                .unwrap()
                .push(_invitation_url.to_owned());
            if self
                .fail_first_completion
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                anyhow::bail!("deliberate completion failure");
            }
            Ok(CompleteResponse {
                wire_version: 1,
                status: "ready".to_owned(),
                pairing_request_id: "9ef5a981-0c32-44b4-9c8a-d1f8e4858e77".to_owned(),
                delivery_id: "75128f2d-845b-4d2b-9d42-ffdb74661ca2".to_owned(),
                relay_expires_at: Utc::now() + chrono::Duration::minutes(10),
            })
        }

        fn cancel(&self, _authorization_id: &str, _nonce: &str) -> Result<()> {
            Ok(())
        }
    }

    fn test_cli() -> Cli {
        Cli {
            json: true,
            full: false,
            verbose: false,
            command: None,
        }
    }

    fn authorize_args(code: &str) -> HubAuthorizeArgs {
        HubAuthorizeArgs {
            code: code.to_owned(),
            scopes: None,
            hub_url: None,
            yes: true,
        }
    }

    fn serve_ready_health_checks(count: usize) -> (u16, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let body = r#"{"schema_version":1,"ready":true}"#;
            for _ in 0..count {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
        });
        (port, handle)
    }

    fn write_live_record(paths: &HubRuntimePaths, port: u16, public_url: Option<&str>) {
        paths
            .write_process_record(&crate::hub::HubProcessRecord {
                pid: std::process::id(),
                sid: None,
                pgid: None,
                bind: "127.0.0.1".to_owned(),
                port,
                version: "test".to_owned(),
                started_at: Utc::now().to_rfc3339(),
                cgroup: None,
                launched_by: None,
                launched_at: None,
                public_url: public_url.map(str::to_owned),
                tailscale_serve_port: Some(443),
                tailscale_cli: None,
                transport_warning: None,
            })
            .unwrap();
    }

    #[test]
    fn stopped_hub_does_not_consume_code_and_same_code_claims_after_start() {
        let temp = tempfile::tempdir().unwrap();
        let paths = HubRuntimePaths::new(temp.path().join("hub"));
        let relay = RecordingRelay::default();
        let args = authorize_args("K7MW-4H2Q");

        let stopped = authorize_with_relay(&args, &test_cli(), &relay, &paths).unwrap_err();
        assert!(stopped.to_string().contains("no cas hub runtime record"));
        assert!(relay.claims.lock().unwrap().is_empty());

        let (port, health) = serve_ready_health_checks(2);
        write_live_record(&paths, port, None);
        let unpublished = authorize_with_relay(&args, &test_cli(), &relay, &paths).unwrap_err();
        assert!(
            unpublished
                .to_string()
                .contains("hub is running without a public URL")
        );
        assert!(relay.claims.lock().unwrap().is_empty());

        write_live_record(&paths, port, Some("https://workstation.tail.example/"));
        authorize_with_relay(&args, &test_cli(), &relay, &paths).unwrap();
        health.join().unwrap();

        let claims = relay.claims.lock().unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].0, "K7MW-4H2Q");
        assert_eq!(
            *relay.completed_hub_urls.lock().unwrap(),
            ["https://workstation.tail.example"]
        );
        assert_eq!(
            read_last_hub_url(&paths).unwrap().as_deref(),
            Some("https://workstation.tail.example")
        );
    }

    #[test]
    fn successful_explicit_origin_is_saved_to_project_hub_config() {
        let temp = tempfile::tempdir().unwrap();
        let paths = HubRuntimePaths::new(temp.path().join("hub"));
        let config_root = temp.path().join("cas");
        fs::create_dir_all(&config_root).unwrap();
        crate::config::Config::default()
            .save_toml(&config_root)
            .unwrap();
        let (port, health) = serve_ready_health_checks(1);
        write_live_record(&paths, port, None);
        let mut args = authorize_args("K7MW-4H2Q");
        args.hub_url = Some("configured.example".to_owned());

        authorize_with_relay_with_config(
            &args,
            &test_cli(),
            &RecordingRelay::default(),
            &paths,
            None,
            Some(&config_root),
        )
        .unwrap();
        let config = crate::config::Config::load(&config_root).unwrap();
        assert_eq!(
            config.hub.and_then(|hub| hub.public_url),
            Some("https://configured.example".to_owned())
        );
        health.join().unwrap();
    }

    #[test]
    fn hosted_relay_completion_uses_the_two_key_invitation_contract() {
        let temp = tempfile::tempdir().unwrap();
        let paths = HubRuntimePaths::new(temp.path().join("hub"));
        let (port, health) = serve_ready_health_checks(1);
        write_live_record(&paths, port, Some("https://workstation.tail.example/"));

        let relay = RecordingRelay::default();
        authorize_with_relay(&authorize_args("K7MW-4H2Q"), &test_cli(), &relay, &paths).unwrap();
        health.join().unwrap();

        let invitation_url = relay.completed_invitation_urls.lock().unwrap()[0].clone();
        assert!(!invitation_url.contains("&scopes="), "{invitation_url}");
        assert!(invitation_url.contains("#pair="), "{invitation_url}");
        assert!(invitation_url.contains("&hub="), "{invitation_url}");
        assert_eq!(invitation_url.matches('&').count(), 1, "{invitation_url}");
        validate_relay_invitation_url(&invitation_url).unwrap();

        let pair = URL_SAFE_NO_PAD.encode([7_u8; 32]);
        for invalid in [
            format!("https://commander.example/#pair={pair}&hub=machine&scopes=read"),
            format!("https://commander.example/#pair=short&hub=machine"),
            format!("https://commander.example/path#pair={pair}&hub=machine"),
            format!("https://commander.example/?query=1#pair={pair}&hub=machine"),
            format!(
                "https://commander.example/#pair={pair}&hub={}",
                "h".repeat(129)
            ),
        ] {
            assert!(
                validate_relay_invitation_url(&invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn invalid_invitation_reports_the_field_contract_and_sent_shape() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            let body = r#"{"error":"invalid_invitation","error_description":"malformed or has an invalid expiry"}"#;
            write!(
                stream,
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });

        let relay = RelayClient {
            endpoint,
            token: "test-token".to_owned(),
        };
        let scopes = Scope::default_read_only();
        let error = relay
            .complete(
                "authorization",
                "nonce",
                "https://workstation.tail.example",
                "machine",
                "https://commander.example/#pair=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&hub=machine-uuid",
                "2026-08-11T20:11:30Z".parse().unwrap(),
                &scopes,
            )
            .unwrap_err();
        let error = format!("{error:#}");
        server.join().unwrap();

        assert!(error.contains("invitation_url"), "{error}");
        assert!(error.contains("exactly two fragment keys"), "{error}");
        assert!(!error.contains("invalid expiry"), "{error}");
        assert!(
            error.contains("https://commander.example/#pair=<redacted>&hub=machine-uuid"),
            "{error}"
        );
    }

    #[test]
    fn same_machine_retry_reuses_nonce_and_resumes_its_claim() {
        let temp = tempfile::tempdir().unwrap();
        let paths = HubRuntimePaths::new(temp.path().join("hub"));
        let relay = RecordingRelay::failing_first_completion();
        let args = authorize_args("K7MW-4H2Q");
        let (port, health) = serve_ready_health_checks(2);
        write_live_record(&paths, port, Some("https://workstation.tail.example/"));

        let first = authorize_with_relay(&args, &test_cli(), &relay, &paths).unwrap_err();
        assert!(first.to_string().contains("deliberate completion failure"));
        authorize_with_relay(&args, &test_cli(), &relay, &paths).unwrap();
        health.join().unwrap();

        let claims = relay.claims.lock().unwrap();
        assert_eq!(claims.len(), 2);
        assert_eq!(claims[0].1, claims[1].1);
        assert_eq!(URL_SAFE_NO_PAD.decode(&claims[0].1).unwrap().len(), 32);
        assert_eq!(
            *relay.completed_hub_urls.lock().unwrap(),
            [
                "https://workstation.tail.example",
                "https://workstation.tail.example"
            ]
        );
    }

    #[test]
    fn code_claimed_message_distinguishes_missing_local_state() {
        let error = relay_error_from_parts(Some(409), "code_claimed", "safe text");
        let text = error.to_string();
        assert!(text.contains("different machine or an earlier local attempt"));
        assert!(text.contains("automatically resumes"));
    }
}
