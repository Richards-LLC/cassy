//! Reusable media capabilities; never a pairing credential or a logged URL.
use super::*;
use hmac::{Hmac, Mac};

const MEDIA_TTL_SECONDS: i64 = 300;
const DOMAIN: &[u8] = b"cas-media-v1.";

#[derive(Serialize, Deserialize)]
struct Claims {
    device: String,
    credential: String,
    epoch: u64,
    session: String,
    artifact: String,
    issued: i64,
    expires: i64,
}

#[derive(Serialize)]
pub(crate) struct MediaTicket {
    pub ticket: String,
    pub expires_at: DateTime<Utc>,
}

impl AuthStore {
    pub(crate) fn issue_media_ticket(
        &self,
        context: &AuthContext,
        session: &str,
        artifact: &str,
        now: DateTime<Utc>,
    ) -> Result<MediaTicket> {
        let state = self.read_lock().map_err(store_failure)?;
        Self::ensure_active_context_in_state(&state, context, now)?;
        anyhow::ensure!(
            context.has(Scope::SessionRead),
            "media authorization refused"
        );
        let device = state
            .devices
            .iter()
            .find(|device| device.device_id == context.device_id)
            .context("media authorization refused")?;
        let expiry = (now + Duration::seconds(MEDIA_TTL_SECONDS)).min(device.expires_at);
        let expires_at = DateTime::from_timestamp(expiry.timestamp(), 0)
            .context("media authorization refused")?;
        anyhow::ensure!(expires_at > now, "media authorization refused");
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&Claims {
            device: device.device_id.clone(),
            credential: device.credential_id.clone(),
            epoch: device.credential_generation,
            session: session.into(),
            artifact: artifact.into(),
            issued: now.timestamp(),
            expires: expires_at.timestamp(),
        })?);
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.0.media_ticket_key).expect("HMAC key length");
        mac.update(DOMAIN);
        mac.update(payload.as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        Ok(MediaTicket {
            ticket: format!("v1.{payload}.{signature}"),
            expires_at,
        })
    }

    pub(crate) fn authenticate_artifact_ticket(
        &self,
        ticket: &str,
        artifact: &str,
        now: DateTime<Utc>,
    ) -> Result<(String, AuthContext)> {
        // Bound all work before decoding attacker-controlled data. Never include
        // a token, its payload or its signature in an error or Debug output.
        anyhow::ensure!(ticket.len() <= 4096, "media ticket refused");
        let (payload, signature) = ticket
            .strip_prefix("v1.")
            .and_then(|value| value.split_once('.'))
            .context("media ticket refused")?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .context("media ticket refused")?;
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.0.media_ticket_key).expect("HMAC key length");
        mac.update(DOMAIN);
        mac.update(payload.as_bytes());
        mac.verify_slice(&signature)
            .map_err(|_| anyhow::anyhow!("media ticket refused"))?;
        let claims: Claims = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(payload)
                .context("media ticket refused")?,
        )
        .context("media ticket refused")?;
        anyhow::ensure!(
            claims.artifact == artifact
                && claims.issued <= now.timestamp()
                && claims.expires > now.timestamp()
                && claims.expires > claims.issued
                && claims.expires - claims.issued <= MEDIA_TTL_SECONDS,
            "media ticket refused"
        );
        let state = self.read_lock().map_err(store_failure)?;
        let device = state
            .devices
            .iter()
            .find(|device| {
                device.device_id == claims.device
                    && device.credential_id == claims.credential
                    && device.credential_generation == claims.epoch
            })
            .context("media ticket refused")?;
        let context = AuthContext {
            device_id: device.device_id.clone(),
            credential_id: device.credential_id.clone(),
            device_label: device.device_label.clone(),
            operator_label: device.operator_label.clone(),
            controller_origin: device.controller_origin.clone(),
            scopes: device.scopes.clone(),
            request_id: uuid::Uuid::new_v4().to_string(),
        };
        Self::ensure_active_context_in_state(&state, &context, now)?;
        anyhow::ensure!(context.has(Scope::SessionRead), "media ticket refused");
        Ok((claims.session, context))
    }
}
