//! The hub-side verifier of the cloud enrollment assertion (contract §5.5,
//! cas-4634). This module checks the issuer signature and `typ` and decodes
//! the claims; `hub::auth::account` then checks them against the hub's own
//! challenge, device session and machine principal before persisting.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

use super::jws::{IssuerKeys, JwsError, TYP_ENROLLMENT};
use super::machine::{HttpClient, PrincipalStore, UreqHttp, issuer_keys};
use crate::hub::auth::EnrollmentAssertion;

fn text(claims: &serde_json::Map<String, Value>, name: &'static str) -> Result<String, JwsError> {
    claims
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or(JwsError::Malformed(name))
}

fn decimal(
    claims: &serde_json::Map<String, Value>,
    name: &'static str,
) -> Result<String, JwsError> {
    let value = text(claims, name)?;
    if super::wire::decimal(&value) {
        Ok(value)
    } else {
        Err(JwsError::Malformed(name))
    }
}

/// Verify an enrollment assertion's signature, exact `typ` and `exp`, and
/// decode the claims the hub binds. Every binding check is the caller's.
pub fn verify_enrollment_assertion(
    issuer: &IssuerKeys,
    token: &str,
    now: DateTime<Utc>,
) -> Result<EnrollmentAssertion, JwsError> {
    let verified = issuer.verify(token, TYP_ENROLLMENT, now.timestamp())?;
    let claims = &verified.claims;
    if claims.get("cak").and_then(Value::as_str) != Some(verified.kid.as_str()) {
        return Err(JwsError::Malformed("cak"));
    }
    let exp = claims
        .get("exp")
        .and_then(Value::as_i64)
        .and_then(|exp| Utc.timestamp_opt(exp, 0).single())
        .ok_or(JwsError::Malformed("exp"))?;
    Ok(EnrollmentAssertion {
        issuer_kid: verified.kid.clone(),
        aud: text(claims, "aud")?,
        hub: text(claims, "hub")?,
        account_id: text(claims, "acct")?,
        relay_device_id: text(claims, "sub")?,
        origin: text(claims, "origin")?,
        installation_jkt: text(claims, "ins_jkt")?,
        challenge: text(claims, "chl")?,
        grant_generation: decimal(claims, "gen")?,
        feed_generation: decimal(claims, "fgen")?,
        epoch: decimal(claims, "epoch")?,
        expires_at: exp,
    })
}

/// The hub's enrollment context: its machine principal's account and an
/// issuer key cache for that principal's cloud, shared by every request.
pub struct HubVerifier {
    store: PrincipalStore,
    http: Arc<dyn HttpClient>,
    issuers: Mutex<HashMap<String, Arc<IssuerKeys>>>,
}

impl HubVerifier {
    pub fn new(hub_state_dir: &std::path::Path, http: Arc<dyn HttpClient>) -> Self {
        Self {
            store: PrincipalStore::new(hub_state_dir),
            http,
            issuers: Mutex::new(HashMap::new()),
        }
    }

    /// One verifier per hub state directory for the process.
    pub fn shared(hub_state_dir: &std::path::Path) -> Arc<Self> {
        static SHARED: OnceLock<Mutex<HashMap<std::path::PathBuf, Arc<HubVerifier>>>> =
            OnceLock::new();
        let map = SHARED.get_or_init(|| Mutex::new(HashMap::new()));
        let mut map = map.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(map.entry(hub_state_dir.to_path_buf()).or_insert_with(|| {
            Arc::new(HubVerifier::new(
                hub_state_dir,
                Arc::new(UreqHttp::default()),
            ))
        }))
    }

    /// Verify a token for this hub and return it with this hub's own
    /// machine-principal account. Blocking: fetches the JWKS at most hourly.
    pub fn verify(
        &self,
        token: &str,
        now: DateTime<Utc>,
    ) -> Result<(EnrollmentAssertion, String), AssertionError> {
        let principal = self
            .store
            .load()
            .map_err(|_| AssertionError::PrincipalUnreadable)?
            .ok_or(AssertionError::HubNotEnrolled)?;
        let issuer = {
            let mut issuers = self
                .issuers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Arc::clone(
                issuers
                    .entry(principal.cloud_origin.clone())
                    .or_insert_with(|| {
                        Arc::new(issuer_keys(Arc::clone(&self.http), &principal.cloud_origin))
                    }),
            )
        };
        let assertion =
            verify_enrollment_assertion(&issuer, token, now).map_err(AssertionError::Token)?;
        Ok((assertion, principal.account_id))
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AssertionError {
    #[error("this hub is not enrolled in an operator inbox")]
    HubNotEnrolled,
    #[error("this hub's operator inbox principal is unreadable")]
    PrincipalUnreadable,
    #[error("assertion refused: {0}")]
    Token(JwsError),
}

impl AssertionError {
    /// A closed, user-safe code for the hub's HTTP refusal.
    pub fn code(&self) -> &'static str {
        match self {
            Self::HubNotEnrolled => "hub_not_enrolled",
            Self::PrincipalUnreadable => "hub_principal_unreadable",
            Self::Token(JwsError::WrongType(_)) => "assertion_wrong_type",
            Self::Token(JwsError::Expired) => "assertion_expired",
            Self::Token(JwsError::UnknownKey(_)) => "assertion_unknown_issuer_key",
            Self::Token(JwsError::BadSignature) => "assertion_bad_signature",
            Self::Token(JwsError::JwksUnavailable) => "issuer_unavailable",
            Self::Token(_) => "assertion_malformed",
        }
    }
}

#[cfg(test)]
#[path = "assertion_tests.rs"]
mod tests;
