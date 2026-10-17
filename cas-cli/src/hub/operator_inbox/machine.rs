//! The hub's machine principal on the operator inbox cloud (contract §5.4).
//!
//! A machine principal is one CAS hub installation (`hub_id` = the hub's
//! durable machine identity) of one PSC account. It holds two keys of its own,
//! neither of which is the hub's DPoP or credential key:
//!
//! - the relay signing key, which signs every PSC-PoP proof;
//! - the command encryption key, to which devices seal offline commands.
//!
//! Enrollment needs the account's PSC bearer (`cas login`) once, plus a PoP
//! proof by the new signing key. The bearer authorizes the account; it is
//! never stored here, never logged, and never sent on a relay route.
//! Afterwards the hub authenticates with its grant alone.
//!
//! The principal file is `~/.cas/hub/operator-inbox/machine.json`, mode 0600,
//! replaced atomically. Losing it means re-enrollment, which replaces the
//! machine atomically on the cloud (its unreserved commands expire).

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cas_operator_crypto::Zeroizing;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::jws::{
    IssuerKeys, JwsError, ProofBinding, ProofRequest, RelayKey, TYP_MACHINE_BINDING, claim_str,
};
use super::{AuthenticatedTransport, Failure, Request, Response, Role};

pub const PRINCIPAL_FILE: &str = "machine.json";
const HTTP_DEADLINE: Duration = Duration::from_secs(10);
const MAX_HTTP_RESPONSE: u64 = 4 * 1024 * 1024;
const EMPTY_BODY_DIGEST: &str = "47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU";

/// The persisted machine principal. Deliberately no Debug: it holds secrets.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachinePrincipal {
    pub wire_version: u8,
    /// PSC base URL, e.g. `https://petra-stella-cloud.vercel.app`.
    pub cloud_origin: String,
    pub account_id: String,
    pub machine_id: String,
    pub hub_id: String,
    pub grant_id: String,
    pub grant_generation: String,
    pub feed_generation: String,
    pub active_epoch: String,
    pub label: String,
    pub projects: Vec<String>,
    pub capabilities: Vec<String>,
    /// Base64url 32-byte relay signing scalar.
    pub signing_secret: String,
    /// Base64url 32-byte command encryption scalar.
    pub command_secret: String,
    /// Base64url 65-byte command encryption public key.
    pub command_public: String,
    /// `cmd_kid` of the verified machine binding.
    pub command_key_id: String,
    /// Highest epoch-manifest policy version accepted (§6.2).
    #[serde(default)]
    pub policy_version: Option<String>,
    pub enrolled_at: String,
}

impl MachinePrincipal {
    pub fn audience(&self) -> String {
        format!("{}/api/operator", self.cloud_origin)
    }

    pub fn relay_key(&self) -> Result<RelayKey, JwsError> {
        let secret = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(&self.signing_secret)
                .map_err(|_| JwsError::InvalidKey)?,
        );
        RelayKey::from_secret(&secret)
    }

    pub fn command_secret(&self) -> Option<Zeroizing<Vec<u8>>> {
        URL_SAFE_NO_PAD
            .decode(&self.command_secret)
            .ok()
            .map(Zeroizing::new)
    }
}

/// Where the principal lives; one per hub installation.
#[derive(Debug, Clone)]
pub struct PrincipalStore {
    dir: PathBuf,
}

impl PrincipalStore {
    pub fn new(hub_state_dir: impl AsRef<Path>) -> Self {
        Self {
            dir: hub_state_dir.as_ref().join("operator-inbox"),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(PRINCIPAL_FILE)
    }

    pub fn load(&self) -> anyhow::Result<Option<MachinePrincipal>> {
        let path = self.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        anyhow::ensure!(
            metadata.file_type().is_file(),
            "operator inbox principal is not a regular file"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            anyhow::ensure!(
                metadata.mode() & 0o777 == 0o600,
                "operator inbox principal must have mode 0600"
            );
        }
        let principal: MachinePrincipal = serde_json::from_slice(&fs::read(&path)?)?;
        anyhow::ensure!(
            principal.wire_version == 1,
            "unsupported operator inbox principal"
        );
        Ok(Some(principal))
    }

    /// Write-then-rename, so a crash leaves the old or the new file, never half.
    pub fn save(&self, principal: &MachinePrincipal) -> anyhow::Result<()> {
        crate::hub::ensure_private_dir(&self.dir)?;
        let bytes = Zeroizing::new(serde_json::to_vec_pretty(principal)?);
        let temp = self.dir.join(format!(".{PRINCIPAL_FILE}.tmp"));
        let _ = fs::remove_file(&temp);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, self.path())?;
        if let Ok(dir) = fs::File::open(&self.dir) {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    pub fn remove(&self) -> anyhow::Result<()> {
        match fs::remove_file(self.path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

// ------------------------------------------------------------------ HTTP

pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    /// The server `Date` header, for clock-skew correction.
    pub date: Option<String>,
    /// Retry-After seconds on throttled operator routes.
    pub retry_after_s: Option<u32>,
}

/// A finite, synchronous HTTPS exchange. Tests substitute an in-process cloud.
pub trait HttpClient: Send + Sync {
    fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, String)],
        body: &[u8],
    ) -> Result<HttpResponse, Failure>;
}

/// Production client: ureq with a whole-request deadline, a capped body read
/// and no compression (operator routes refuse `Content-Encoding`, §4.2).
pub struct UreqHttp {
    agent: ureq::Agent,
}

impl Default for UreqHttp {
    fn default() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout(HTTP_DEADLINE)
                .redirects(0)
                .build(),
        }
    }
}

impl HttpClient for UreqHttp {
    fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, String)],
        body: &[u8],
    ) -> Result<HttpResponse, Failure> {
        let mut request = self.agent.request(method, url);
        for (name, value) in headers {
            request = request.set(name, value);
        }
        let result = if body.is_empty() {
            request.call()
        } else {
            request.send_bytes(body)
        };
        let response = match result {
            Ok(response) => response,
            Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(transport)) => {
                return Err(
                    if matches!(transport.kind(), ureq::ErrorKind::Io)
                        && transport.to_string().contains("timed out")
                    {
                        Failure::Deadline
                    } else {
                        Failure::Unavailable
                    },
                );
            }
        };
        let status = response.status();
        let date = response.header("Date").map(str::to_owned);
        let retry_after_s = response
            .header("Retry-After")
            .and_then(|value| value.parse().ok());
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(MAX_HTTP_RESPONSE + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Failure::Unavailable)?;
        if bytes.len() as u64 > MAX_HTTP_RESPONSE {
            return Err(Failure::Protocol("response size"));
        }
        Ok(HttpResponse {
            status,
            body: bytes,
            date,
            retry_after_s,
        })
    }
}

fn error_code(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_owned)
}

fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn skew_from(date: Option<&str>) -> Option<i64> {
    let parsed = chrono::DateTime::parse_from_rfc2822(date?).ok()?;
    Some(parsed.timestamp() - unix_now())
}

// ------------------------------------------------------------------ relay transport

/// The machine's PSC-PoP transport for `MachineRelay` and the command routes.
///
/// It signs each request with the relay key and the current grant generation.
/// Two recoveries are automatic and bounded to one retry each: a
/// `pop_expired` answered with a server `Date` re-signs with the observed
/// skew, and `grant_generation_stale` resyncs the generation through
/// `GET /grants/me` (the one route that accepts a stale `gen`, §4.1) and
/// persists it. Device-role requests are refused: a hub never reads the feed.
///
/// `ureq` is blocking, so each exchange runs on the blocking pool and is
/// bounded by the 10 s agent deadline. A dropped future can let an in-flight
/// request finish; that is safe because every machine mutation is idempotent
/// by its client-chosen ID (append by `event_id`, reserve by command, receipt
/// by `receipt_id`).
#[derive(Clone)]
pub struct MachineTransport {
    inner: Arc<TransportInner>,
}

struct TransportInner {
    http: Arc<dyn HttpClient>,
    store: Option<PrincipalStore>,
    principal: Mutex<MachinePrincipal>,
    key: RelayKey,
    skew: Mutex<i64>,
}

impl MachineTransport {
    pub fn new(
        http: Arc<dyn HttpClient>,
        principal: MachinePrincipal,
        store: Option<PrincipalStore>,
    ) -> Result<Self, JwsError> {
        let key = principal.relay_key()?;
        Ok(Self {
            inner: Arc::new(TransportInner {
                http,
                store,
                principal: Mutex::new(principal),
                key,
                skew: Mutex::new(0),
            }),
        })
    }

    pub fn principal(&self) -> MachinePrincipal {
        self.inner
            .principal
            .lock()
            .map(|principal| principal.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    /// Synchronous signed exchange (used by the blocking pool and the CLI).
    pub fn exchange_blocking(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<HttpResponse, Failure> {
        self.inner.exchange(method, path, body)
    }
}

impl TransportInner {
    fn snapshot(&self) -> MachinePrincipal {
        self.principal
            .lock()
            .map(|principal| principal.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    fn signed(
        &self,
        principal: &MachinePrincipal,
        method: &str,
        path: &str,
        body: &[u8],
        body_digest: &str,
    ) -> Result<HttpResponse, Failure> {
        let now = unix_now() + self.skew.lock().map(|skew| *skew).unwrap_or(0);
        let proof = self.key.proof(&ProofRequest {
            audience: &principal.audience(),
            method,
            path_and_query: path,
            body_digest,
            binding: ProofBinding::Grant {
                grant_id: &principal.grant_id,
                generation: &principal.grant_generation,
            },
            now,
        });
        let mut headers = vec![
            ("Authorization", format!("PSC-PoP {}", principal.grant_id)),
            ("PSC-PoP-Proof", proof),
        ];
        if !body.is_empty() {
            headers.push(("Content-Type", "application/json".to_owned()));
        }
        self.http.send(
            method,
            &format!("{}{path}", principal.cloud_origin),
            &headers,
            body,
        )
    }

    fn exchange(&self, method: &str, path: &str, body: &[u8]) -> Result<HttpResponse, Failure> {
        let body_digest = if body.is_empty() {
            EMPTY_BODY_DIGEST.to_owned()
        } else {
            use sha2::Digest;
            URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(body))
        };
        let mut skew_retried = false;
        let mut generation_retried = false;
        loop {
            let principal = self.snapshot();
            let response = self.signed(&principal, method, path, body, &body_digest)?;
            if response.status != 401 {
                return Ok(response);
            }
            match error_code(&response.body).as_deref() {
                Some("pop_expired") if !skew_retried => {
                    skew_retried = true;
                    match skew_from(response.date.as_deref()) {
                        Some(skew) => {
                            if let Ok(mut current) = self.skew.lock() {
                                *current = skew;
                            }
                        }
                        None => return Ok(response),
                    }
                }
                Some("grant_generation_stale") if !generation_retried => {
                    generation_retried = true;
                    self.resync_generation()?;
                }
                _ => return Ok(response),
            }
        }
    }

    fn resync_generation(&self) -> Result<(), Failure> {
        let principal = self.snapshot();
        let response = self.signed(
            &principal,
            "GET",
            "/api/operator/grants/me",
            &[],
            EMPTY_BODY_DIGEST,
        )?;
        if response.status != 200 {
            return Err(Failure::Http {
                status: response.status,
                code: error_code(&response.body).unwrap_or_else(|| "unknown_error".into()),
                recovery: None,
            });
        }
        let value: Value =
            serde_json::from_slice(&response.body).map_err(|_| Failure::Protocol("grant shape"))?;
        let grant = &value["grant"];
        if grant["grant_id"].as_str() != Some(principal.grant_id.as_str())
            || grant["kind"].as_str() != Some("machine")
            || grant["status"].as_str() != Some("active")
        {
            return Err(Failure::Protocol("grant identity"));
        }
        let generation = grant["grant_generation"]
            .as_str()
            .filter(|value| super::wire::decimal(value))
            .ok_or(Failure::Protocol("grant generation"))?
            .to_owned();
        let updated = {
            let mut current = self
                .principal
                .lock()
                .map_err(|_| Failure::Protocol("principal lock"))?;
            current.grant_generation = generation;
            if let Some(projects) = grant["scopes"].as_array() {
                let granted: Vec<String> = projects
                    .iter()
                    .filter_map(|scope| scope["project_id"].as_str().map(str::to_owned))
                    .collect();
                if !granted.is_empty() {
                    current.projects = granted;
                }
            }
            current.clone()
        };
        if let Some(store) = &self.store {
            store
                .save(&updated)
                .map_err(|_| Failure::Protocol("principal persistence"))?;
        }
        Ok(())
    }
}

impl AuthenticatedTransport for MachineTransport {
    fn exchange(
        &self,
        request: Request,
    ) -> Pin<Box<dyn std::future::Future<Output = super::Result<Response>> + Send + '_>> {
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            if request.role != Role::Machine {
                return Err(Failure::Protocol("hub holds no device grant"));
            }
            let method = request.method.as_str();
            let max_response_bytes = request.max_response_bytes;
            let result = tokio::task::spawn_blocking(move || {
                inner.exchange(method, &request.path, &request.body)
            })
            .await
            .map_err(|_| Failure::Unavailable)??;
            if result.body.len() > max_response_bytes {
                return Err(Failure::Protocol("response size"));
            }
            Ok(Response {
                status: result.status,
                body: result.body,
            })
        })
    }
}

// ------------------------------------------------------------------ account authority (bearer)

/// The account authority: the operator's PSC bearer, read at call time from
/// the CLI's cloud config. Deliberately no Debug; never logged.
pub struct AccountAuthority {
    pub cloud_origin: String,
    bearer: Zeroizing<String>,
}

impl AccountAuthority {
    pub fn new(cloud_origin: &str, bearer: String) -> Self {
        Self {
            cloud_origin: cloud_origin.trim_end_matches('/').to_owned(),
            bearer: Zeroizing::new(bearer),
        }
    }

    fn headers(&self, proof: Option<String>, json_body: bool) -> Vec<(&'static str, String)> {
        let mut headers = vec![("Authorization", format!("Bearer {}", self.bearer.as_str()))];
        if let Some(proof) = proof {
            headers.push(("PSC-PoP-Proof", proof));
        }
        if json_body {
            headers.push(("Content-Type", "application/json".to_owned()));
        }
        headers
    }

    fn call(
        &self,
        http: &dyn HttpClient,
        method: &str,
        path: &str,
        body: Option<&Value>,
        proof: Option<&dyn Fn(&[u8]) -> String>,
    ) -> Result<Value, AuthorityError> {
        let bytes = match body {
            Some(value) => {
                serde_json::to_vec(value).map_err(|_| AuthorityError::Protocol("body"))?
            }
            None => Vec::new(),
        };
        let proof = proof.map(|sign| sign(&bytes));
        let headers = self.headers(proof, !bytes.is_empty());
        let response = http
            .send(
                method,
                &format!("{}{path}", self.cloud_origin),
                &headers,
                &bytes,
            )
            .map_err(AuthorityError::Transport)?;
        let value: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
        if !(200..300).contains(&response.status) {
            return Err(AuthorityError::Refused {
                status: response.status,
                code: value["error"]
                    .as_str()
                    .unwrap_or("unknown_error")
                    .to_owned(),
            });
        }
        Ok(value)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthorityError {
    #[error("cloud unreachable: {0}")]
    Transport(Failure),
    #[error("cloud refused ({status} {code})")]
    Refused { status: u16, code: String },
    #[error("cloud reply did not match the contract: {0}")]
    Protocol(&'static str),
    #[error("machine binding did not verify: {0}")]
    Binding(String),
    #[error("could not save the machine principal: {0}")]
    Persist(String),
}

pub struct EnrollRequest<'a> {
    pub hub_id: &'a str,
    pub label: &'a str,
    pub projects: &'a [String],
    pub presence: bool,
}

fn body_digest(bytes: &[u8]) -> String {
    use sha2::Digest;
    if bytes.is_empty() {
        EMPTY_BODY_DIGEST.to_owned()
    } else {
        URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(bytes))
    }
}

fn decimal_field(value: &Value, name: &'static str) -> Result<String, AuthorityError> {
    value[name]
        .as_str()
        .filter(|text| super::wire::decimal(text))
        .map(str::to_owned)
        .ok_or(AuthorityError::Protocol(name))
}

fn str_field(value: &Value, name: &'static str) -> Result<String, AuthorityError> {
    value[name]
        .as_str()
        .map(str::to_owned)
        .ok_or(AuthorityError::Protocol(name))
}

/// Enroll this hub as a machine of the bearer's account (§5.4) and verify the
/// returned machine binding before anything is persisted.
pub fn enroll_machine(
    http: &dyn HttpClient,
    authority: &AccountAuthority,
    issuer: &IssuerKeys,
    request: &EnrollRequest<'_>,
) -> Result<MachinePrincipal, AuthorityError> {
    let relay = RelayKey::generate();
    let command = cas_operator_crypto::generate_key_pair();
    let command_public = URL_SAFE_NO_PAD.encode(command.public);
    let challenge = authority.call(
        http,
        "POST",
        "/api/operator/machines/challenges",
        Some(&json!({
            "wire_version": 1,
            "hub_id": request.hub_id,
            "signing_jwk": relay.public_jwk(),
            "command_encryption_public_key": command_public,
        })),
        None,
    )?;
    let enrollment_id = str_field(&challenge, "enrollment_id")?;
    let check = &challenge["encryption_key_check"];
    let decode = |field: &'static str| {
        check[field]
            .as_str()
            .and_then(|text| URL_SAFE_NO_PAD.decode(text).ok())
            .ok_or(AuthorityError::Protocol("encryption_key_check"))
    };
    let opened = cas_operator_crypto::open_enrollment_check(
        &command.secret[..],
        &decode("enc")?,
        &decode("ct")?,
        &enrollment_id,
    )
    .map_err(|_| AuthorityError::Protocol("encryption_key_check"))?;

    let audience = format!("{}/api/operator", authority.cloud_origin);
    let path = "/api/operator/machines";
    let sign = |bytes: &[u8]| {
        relay.proof(&ProofRequest {
            audience: &audience,
            method: "POST",
            path_and_query: path,
            body_digest: &body_digest(bytes),
            binding: ProofBinding::Enrollment {
                enrollment_id: &enrollment_id,
            },
            now: unix_now(),
        })
    };
    let mut body = json!({
        "wire_version": 1,
        "enrollment_id": enrollment_id,
        "encryption_key_check": URL_SAFE_NO_PAD.encode(&opened[..]),
        "label": request.label,
        "projects": request.projects,
    });
    if request.presence {
        body["capabilities"] = json!(["presence:report"]);
    }
    let completed = authority.call(http, "POST", path, Some(&body), Some(&sign))?;

    let machine_id = str_field(&completed, "machine_id")?;
    let account_id = str_field(&completed, "account_id")?;
    let grant = &completed["grant"];
    let grant_id = str_field(grant, "grant_id")?;
    let generation = decimal_field(grant, "grant_generation")?;
    if grant_id != machine_id || grant["kind"].as_str() != Some("machine") {
        return Err(AuthorityError::Protocol("grant"));
    }
    let binding = completed["machine_binding"]
        .as_str()
        .ok_or(AuthorityError::Protocol("machine_binding"))?;
    let verified = issuer
        .verify(binding, TYP_MACHINE_BINDING, unix_now())
        .map_err(|error| AuthorityError::Binding(error.to_string()))?;
    let claim = |name: &'static str| {
        claim_str(&verified, name)
            .map(str::to_owned)
            .map_err(|error| AuthorityError::Binding(error.to_string()))
    };
    let signing_jkt = relay.thumbprint();
    let bound = [
        ("acct", account_id.as_str()),
        ("mch", machine_id.as_str()),
        ("hub", request.hub_id),
        ("sig_jkt", signing_jkt.as_str()),
        ("cmd_pk", command_public.as_str()),
        ("gen", generation.as_str()),
    ];
    for (name, expected) in bound {
        if claim(name)? != expected {
            return Err(AuthorityError::Binding(format!("{name} does not match")));
        }
    }
    let command_key_id = claim("cmd_kid")?;
    let projects: Vec<String> = verified
        .claims
        .get("projects")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Ok(MachinePrincipal {
        wire_version: 1,
        cloud_origin: authority.cloud_origin.clone(),
        account_id,
        machine_id,
        hub_id: request.hub_id.to_owned(),
        grant_id,
        grant_generation: generation,
        feed_generation: decimal_field(&completed, "feed_generation")?,
        active_epoch: decimal_field(&completed, "active_epoch")?,
        label: request.label.to_owned(),
        projects,
        capabilities: grant["capabilities"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        signing_secret: URL_SAFE_NO_PAD.encode(&relay.secret()[..]),
        command_secret: URL_SAFE_NO_PAD.encode(&command.secret[..]),
        command_public,
        command_key_id,
        policy_version: None,
        enrolled_at: chrono::Utc::now().to_rfc3339(),
    })
}

/// Device command scope granted at approval (§5.2): exact routing IDs.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CommandScope {
    pub hub_id: String,
    pub project_id: String,
    pub session_id: Option<String>,
    pub operations: Vec<String>,
}

/// Approve (or deny) a device challenge by its user code (§5.2). The consent
/// block is the contract's fixed policy; the server refuses anything else.
pub fn decide_device(
    http: &dyn HttpClient,
    authority: &AccountAuthority,
    user_code: &str,
    approve: bool,
    manage: bool,
    scopes: &[CommandScope],
) -> Result<Value, AuthorityError> {
    let mut capabilities = vec!["feed:read"];
    if manage {
        capabilities.push("account:manage");
    }
    authority.call(
        http,
        "POST",
        "/api/operator/enrollments/approve",
        Some(&json!({
            "wire_version": 1,
            "user_code": user_code,
            "decision": if approve { "approve" } else { "deny" },
            "capabilities": capabilities,
            "scopes": scopes,
            "consent": {
                "custody": "cloud_account_permission",
                "not_end_to_end_acknowledged": true,
                "retention_days": 90
            }
        })),
        None,
    )
}

/// `GET /principals` as the account authority (§5.6).
pub fn list_principals(
    http: &dyn HttpClient,
    authority: &AccountAuthority,
) -> Result<Value, AuthorityError> {
    authority.call(http, "GET", "/api/operator/principals", None, None)
}

/// Revoke a device (rotates the epoch) or a machine (§5.6). Idempotent.
pub fn revoke_principal(
    http: &dyn HttpClient,
    authority: &AccountAuthority,
    kind: PrincipalKind,
    id: &str,
) -> Result<Value, AuthorityError> {
    let segment = match kind {
        PrincipalKind::Device => "devices",
        PrincipalKind::Machine => "machines",
    };
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(AuthorityError::Protocol("principal id"));
    }
    authority.call(
        http,
        "POST",
        &format!("/api/operator/{segment}/{id}/revoke"),
        Some(&json!({"wire_version": 1})),
        None,
    )
}

/// Replace the machine's project grants (§5.6; the CLI adds paired projects).
pub fn set_machine_projects(
    http: &dyn HttpClient,
    authority: &AccountAuthority,
    machine_id: &str,
    projects: &[String],
) -> Result<Value, AuthorityError> {
    authority.call(
        http,
        "PUT",
        &format!("/api/operator/machines/{machine_id}/grants"),
        Some(&json!({"wire_version": 1, "projects": projects})),
        None,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalKind {
    Device,
    Machine,
}

/// The issuer JWKS through `GET /api/operator/jwks` (public route).
pub fn issuer_keys(http: Arc<dyn HttpClient>, cloud_origin: &str) -> IssuerKeys {
    let url = format!("{}/api/operator/jwks", cloud_origin.trim_end_matches('/'));
    IssuerKeys::new(Box::new(move || {
        let response = http
            .send("GET", &url, &[], &[])
            .map_err(|_| JwsError::JwksUnavailable)?;
        if response.status != 200 {
            return Err(JwsError::JwksUnavailable);
        }
        serde_json::from_slice(&response.body).map_err(|_| JwsError::JwksUnavailable)
    }))
}

#[cfg(test)]
#[path = "machine_tests.rs"]
mod tests;
