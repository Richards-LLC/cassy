//! Verified account enrollment of a browser installation (cas-4634).
//!
//! The operator inbox cloud (contract §5.5) can vouch that an enrolled
//! account device is the browser that holds a given hub installation key:
//! the device asks the cloud for a `psc-op-enrollment+jwt`, naming this hub
//! and a one-use challenge the hub issued to its authenticated session. The
//! hub persists `AccountEnrollment::Enrolled` for that installation only
//! after every check below passes:
//!
//! 1. the issuer signature, against the cloud JWKS, with `typ` exactly
//!    `psc-op-enrollment+jwt` (checked by the caller, which owns the JWKS);
//! 2. `aud` is `cas-hub:<this hub's id>` and `hub` is this hub;
//! 3. `chl` is a challenge this hub issued to the same device and credential,
//!    unexpired (at most five minutes) and not used before; it is consumed
//!    here even when a later check fails, so it can never be replayed;
//! 4. `exp` has not passed;
//! 5. `ins_jkt` is the thumbprint of the key this device proves with DPoP,
//!    and `origin` is the device's paired origin;
//! 6. `acct` is the account this hub's own machine principal belongs to:
//!    a browser cannot bind this hub to some other account.
//!
//! A binding is keyed by device ID and installation-key thumbprint. A
//! credential refresh keeps it; a new installation key, a revoke or a
//! different account needs a new assertion. Display labels never decide
//! membership. No hub credential or DPoP proof is sent to the cloud.

use super::*;

/// One-use challenge lifetime (contract §16 Q3: at most five minutes).
pub const CHALLENGE_TTL_SECONDS: i64 = 300;
/// Outstanding challenges per device; older ones are dropped first.
const MAX_CHALLENGES_PER_DEVICE: usize = 4;
const MAX_CHALLENGES: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct AccountChallenge {
    challenge_hash: String,
    device_id: String,
    credential_id: String,
    expires_at: DateTime<Utc>,
}

/// A persisted, verified binding of one installation to one account device.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct AccountBinding {
    pub(super) device_id: String,
    pub(super) installation_jkt: String,
    pub(super) account_id: String,
    pub(super) relay_device_id: String,
    pub(super) grant_generation: String,
    pub(super) feed_generation: String,
    pub(super) epoch: String,
    pub(super) issuer_kid: String,
    pub(super) verified_at: DateTime<Utc>,
}

/// Claims of a `psc-op-enrollment+jwt` whose signature and `typ` the caller
/// has already verified against the issuer JWKS.
#[derive(Debug, Clone)]
pub struct EnrollmentAssertion {
    pub issuer_kid: String,
    pub aud: String,
    pub hub: String,
    pub account_id: String,
    pub relay_device_id: String,
    pub origin: String,
    pub installation_jkt: String,
    pub challenge: String,
    pub grant_generation: String,
    pub feed_generation: String,
    pub epoch: String,
    pub expires_at: DateTime<Utc>,
}

/// Why an assertion was refused; each is a closed, user-safe code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EnrollmentRefusal {
    #[error("challenge_unknown")]
    ChallengeUnknown,
    #[error("challenge_expired")]
    ChallengeExpired,
    #[error("wrong_hub")]
    WrongHub,
    #[error("assertion_expired")]
    AssertionExpired,
    #[error("installation_mismatch")]
    InstallationMismatch,
    #[error("origin_mismatch")]
    OriginMismatch,
    #[error("account_mismatch")]
    AccountMismatch,
    #[error("hub_not_enrolled")]
    HubNotEnrolled,
}

impl AccountBinding {
    fn enrollment(&self) -> AccountEnrollment {
        AccountEnrollment::Enrolled {
            account_id: self.account_id.clone(),
            relay_device_id: self.relay_device_id.clone(),
            grant_generation: self.grant_generation.clone(),
            feed_generation: self.feed_generation.clone(),
            epoch: self.epoch.clone(),
            verified_at: self.verified_at,
        }
    }
}

/// The installation's current enrollment: a binding for this exact device
/// and key, on a device that is not revoked; otherwise unenrolled.
pub(super) fn enrollment_for(state: &PersistedState, device: &DeviceSession) -> AccountEnrollment {
    if device.revoked_at.is_some() {
        return AccountEnrollment::Unenrolled;
    }
    state
        .account_bindings
        .iter()
        .find(|binding| {
            binding.device_id == device.device_id
                && binding.installation_jkt == device.public_key_thumbprint
        })
        .map(AccountBinding::enrollment)
        .unwrap_or(AccountEnrollment::Unenrolled)
}

impl AuthStore {
    /// Issue a one-use challenge to an authenticated device session.
    pub fn issue_account_challenge(
        &self,
        context: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<(String, DateTime<Utc>)> {
        let mut state = self.lock()?;
        Self::ensure_active_context_in_state(&state, context, now)?;
        state
            .account_challenges
            .retain(|entry| entry.expires_at > now);
        let mine: Vec<usize> = state
            .account_challenges
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.device_id == context.device_id)
            .map(|(index, _)| index)
            .collect();
        if mine.len() >= MAX_CHALLENGES_PER_DEVICE {
            let oldest = mine[0];
            state.account_challenges.remove(oldest);
        }
        if state.account_challenges.len() >= MAX_CHALLENGES {
            state.account_challenges.remove(0);
        }
        let challenge = random_secret();
        let expires_at = now + Duration::seconds(CHALLENGE_TTL_SECONDS);
        state.account_challenges.push(AccountChallenge {
            challenge_hash: hash_b64(challenge.as_bytes()),
            device_id: context.device_id.clone(),
            credential_id: context.credential_id.clone(),
            expires_at,
        });
        self.persist(&state)?;
        Ok((challenge, expires_at))
    }

    /// Bind the authenticated installation to the asserted account device.
    /// `hub_account` is this hub's own machine-principal account; `None`
    /// means the hub is not enrolled, so nothing can be verified against it.
    pub fn bind_account(
        &self,
        context: &AuthContext,
        assertion: &EnrollmentAssertion,
        hub_account: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<std::result::Result<AccountEnrollment, EnrollmentRefusal>> {
        let mut state = self.lock()?;
        Self::ensure_active_context_in_state(&state, context, now)?;
        // Consume the challenge first: whatever happens next, it is spent.
        let hash = hash_b64(assertion.challenge.as_bytes());
        let position = state.account_challenges.iter().position(|entry| {
            bool::from(entry.challenge_hash.as_bytes().ct_eq(hash.as_bytes()))
                && entry.device_id == context.device_id
                && entry.credential_id == context.credential_id
        });
        let Some(position) = position else {
            return Ok(Err(EnrollmentRefusal::ChallengeUnknown));
        };
        let challenge = state.account_challenges.remove(position);
        self.persist(&state)?;
        let refusal = if challenge.expires_at <= now {
            Some(EnrollmentRefusal::ChallengeExpired)
        } else if assertion.aud != format!("cas-hub:{}", self.0.machine_id)
            || assertion.hub != self.0.machine_id
        {
            Some(EnrollmentRefusal::WrongHub)
        } else if assertion.expires_at <= now {
            Some(EnrollmentRefusal::AssertionExpired)
        } else {
            None
        };
        if let Some(refusal) = refusal {
            return Ok(Err(refusal));
        }
        let device = state
            .devices
            .iter()
            .find(|device| device.device_id == context.device_id)
            .context("authorization refused")?
            .clone();
        if !bool::from(
            device
                .public_key_thumbprint
                .as_bytes()
                .ct_eq(assertion.installation_jkt.as_bytes()),
        ) {
            return Ok(Err(EnrollmentRefusal::InstallationMismatch));
        }
        if device.controller_origin != assertion.origin {
            return Ok(Err(EnrollmentRefusal::OriginMismatch));
        }
        let Some(hub_account) = hub_account else {
            return Ok(Err(EnrollmentRefusal::HubNotEnrolled));
        };
        if hub_account != assertion.account_id {
            return Ok(Err(EnrollmentRefusal::AccountMismatch));
        }
        let binding = AccountBinding {
            device_id: device.device_id.clone(),
            installation_jkt: device.public_key_thumbprint.clone(),
            account_id: assertion.account_id.clone(),
            relay_device_id: assertion.relay_device_id.clone(),
            grant_generation: assertion.grant_generation.clone(),
            feed_generation: assertion.feed_generation.clone(),
            epoch: assertion.epoch.clone(),
            issuer_kid: assertion.issuer_kid.clone(),
            verified_at: now,
        };
        state
            .account_bindings
            .retain(|existing| existing.device_id != device.device_id);
        state.account_bindings.push(binding.clone());
        self.persist(&state)?;
        drop(state);
        self.audit(
            Some(context),
            "allowed",
            "account_enrollment",
            None,
            None,
            now,
        )?;
        Ok(Ok(binding.enrollment()))
    }
}

#[cfg(test)]
#[path = "account_tests.rs"]
mod tests;
