//! Possession-proven installation rotations under the cross-process auth lock.
use super::*;

/// cas-4c78 owns verified enrollment. Display labels never identify accounts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum AccountEnrollment {
    Unenrolled,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct InstallationProof {
    pub operation_id: String,
    pub credential: String,
    pub device_id: Option<String>,
    pub expected_generation: u64,
    pub proof: String,
    pub previous_proof: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct InstallationAction {
    pub public_key_jwk: PublicJwk,
    pub operation_id: String,
    pub controller_origin: String,
    pub proof: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Phase {
    Prepared,
    Committed,
    Aborted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PendingInstallation {
    operation_id: String,
    transcript_hash: String,
    candidate: DeviceSession,
    prior: Option<DeviceSession>,
    phase: Phase,
    expires_at: DateTime<Utc>,
}

fn verify_transcript(key: &PublicJwk, proof: &str, transcript: &serde_json::Value) -> Result<()> {
    let bytes = URL_SAFE_NO_PAD
        .decode(proof)
        .context("installation proof refused")?;
    let signature =
        Signature::from_slice(&bytes).map_err(|_| anyhow::anyhow!("installation proof refused"))?;
    key.validate()?
        .verify(&serde_json::to_vec(transcript)?, &signature)
        .map_err(|_| anyhow::anyhow!("installation proof refused"))
}

impl AuthStore {
    pub(super) fn prepare_installation(
        &self,
        exchange: PairingExchange,
        now: DateTime<Utc>,
    ) -> Result<DeviceCredential> {
        validate_origin(&exchange.controller_origin)?;
        anyhow::ensure!(
            exchange.hub_id == self.0.machine_id,
            "installation hub refused"
        );
        let proof = exchange
            .installation
            .as_ref()
            .context("installation proof required")?;
        uuid::Uuid::parse_str(&proof.operation_id).context("invalid installation operation")?;
        let secret = URL_SAFE_NO_PAD
            .decode(&proof.credential)
            .context("invalid candidate credential")?;
        anyhow::ensure!(
            secret.len() == 32 && proof.credential.len() == 43,
            "invalid candidate credential"
        );
        let thumbprint = exchange.public_key_jwk.thumbprint()?;
        let token_hash = hash_b64(exchange.token.as_bytes());
        let credential_hash = hash_b64(proof.credential.as_bytes());
        let transcript = serde_json::json!([
            "cassy-installation-v1",
            exchange.hub_id,
            exchange.controller_origin,
            token_hash,
            proof.operation_id,
            proof.device_id,
            proof.expected_generation,
            thumbprint,
            credential_hash,
            exchange
                .requested_scopes
                .iter()
                .map(|s| s.as_wire())
                .collect::<Vec<_>>(),
            exchange.device_label,
            exchange.operator_label
        ]);
        verify_transcript(&exchange.public_key_jwk, &proof.proof, &transcript)?;
        let transcript_hash = hash_b64(&serde_json::to_vec(&transcript)?);
        let mut state = self.lock()?;
        anyhow::ensure!(
            !state
                .aborted_installations
                .contains_key(&proof.operation_id),
            "installation was cancelled"
        );
        if let Some(pending) = state
            .installations
            .iter()
            .find(|p| p.operation_id == proof.operation_id)
        {
            anyhow::ensure!(
                pending.transcript_hash == transcript_hash
                    && pending.phase != Phase::Aborted
                    && pending.expires_at >= now,
                "installation replay refused"
            );
            return Ok(installation_credential(
                &pending.candidate,
                proof.credential.clone(),
            ));
        }
        state.installations.retain(|p| p.expires_at >= now);
        state
            .source_attempts
            .retain(|a| a.at > now - Duration::hours(1));
        anyhow::ensure!(
            state
                .source_attempts
                .iter()
                .filter(|a| a.source == exchange.source && a.at > now - Duration::minutes(1))
                .count()
                < 5,
            "installation rate limited"
        );
        state.source_attempts.push(SourceAttempt {
            source: exchange.source.clone(),
            at: now,
        });
        let invitation = state.pairings.iter().position(|p| {
            constant_time_eq(&p.token_hash, &token_hash)
                && p.hub_id == exchange.hub_id
                && p.controller_origin == exchange.controller_origin
                && p.consumed_at.is_none()
                && p.expires_at >= now
                && exchange.requested_scopes.is_subset(&p.max_scopes)
        });
        let invitation = invitation.context("pairing exchange refused")?;
        let matches: Vec<_> = state
            .devices
            .iter()
            .filter(|d| {
                d.controller_origin == exchange.controller_origin
                    && d.public_key_thumbprint == thumbprint
                    && d.revoked_at.is_none()
            })
            .collect();
        let prior = if let Some(id) = &proof.device_id {
            let old = state
                .devices
                .iter()
                .find(|d| &d.device_id == id)
                .context("installation refused")?;
            anyhow::ensure!(
                old.controller_origin == exchange.controller_origin && old.revoked_at.is_none(),
                "installation refused"
            );
            anyhow::ensure!(
                old.credential_generation == proof.expected_generation,
                "installation generation conflict"
            );
            if old.public_key_thumbprint != thumbprint {
                verify_transcript(
                    &old.public_key,
                    proof
                        .previous_proof
                        .as_deref()
                        .context("previous key proof required")?,
                    &transcript,
                )?;
            }
            Some(old.clone())
        } else {
            anyhow::ensure!(
                matches.len() <= 1,
                "select an explicit installation before repairing legacy duplicates"
            );
            let old = matches.first().map(|d| (*d).clone());
            anyhow::ensure!(
                old.as_ref()
                    .map_or(proof.expected_generation == 0, |d| d.credential_generation
                        == proof.expected_generation),
                "installation generation conflict"
            );
            old
        };
        let device_id = prior
            .as_ref()
            .map(|d| d.device_id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        anyhow::ensure!(
            !state
                .installations
                .iter()
                .any(|p| p.candidate.device_id == device_id && p.phase == Phase::Prepared),
            "installation already rotating"
        );
        let generation = state
            .generation_highwater
            .get(&device_id)
            .copied()
            .unwrap_or_else(|| prior.as_ref().map_or(0, |d| d.credential_generation))
            + 1;
        state
            .generation_highwater
            .insert(device_id.clone(), generation);
        let candidate = DeviceSession {
            device_id,
            credential_id: uuid::Uuid::new_v4().to_string(),
            credential_generation: generation,
            device_label: sanitize_label(&exchange.device_label),
            operator_label: sanitize_label(&exchange.operator_label),
            controller_origin: exchange.controller_origin,
            scopes: exchange.requested_scopes,
            issued_at: prior.as_ref().map_or(now, |d| d.issued_at),
            last_used_at: now,
            expires_at: now + Duration::days(CREDENTIAL_ABSOLUTE_DAYS),
            revoked_at: None,
            credential_hash,
            public_key: exchange.public_key_jwk,
            public_key_thumbprint: thumbprint,
        };
        state.pairings[invitation].consumed_at = Some(now);
        state.installations.push(PendingInstallation {
            operation_id: proof.operation_id.clone(),
            transcript_hash,
            candidate: candidate.clone(),
            prior,
            phase: Phase::Prepared,
            expires_at: now + Duration::minutes(10),
        });
        self.persist(&state)?;
        Ok(installation_credential(
            &candidate,
            proof.credential.clone(),
        ))
    }

    /// Idempotent actions bind their purpose. Prepared credentials cannot authorize.
    pub fn installation_action(
        &self,
        action: InstallationAction,
        commit: bool,
        now: DateTime<Utc>,
    ) -> Result<()> {
        validate_origin(&action.controller_origin)?;
        let mut state = self.lock()?;
        uuid::Uuid::parse_str(&action.operation_id).context("invalid installation operation")?;
        let transcript = serde_json::json!([
            if commit {
                "cassy-installation-commit-v1"
            } else {
                "cassy-installation-abort-v1"
            },
            self.0.machine_id,
            action.controller_origin,
            action.operation_id
        ]);
        verify_transcript(&action.public_key_jwk, &action.proof, &transcript)?;
        let index = state
            .installations
            .iter()
            .position(|p| p.operation_id == action.operation_id);
        let Some(index) = index else {
            anyhow::ensure!(!commit, "unknown installation operation");
            state
                .aborted_installations
                .retain(|_, expires| *expires >= now);
            anyhow::ensure!(
                state.aborted_installations.len() < 1000,
                "too many pending cancellations"
            );
            state
                .aborted_installations
                .insert(action.operation_id, now + Duration::minutes(10));
            self.persist(&state)?;
            return Ok(());
        };
        let pending = state.installations[index].clone();
        anyhow::ensure!(
            pending.candidate.controller_origin == action.controller_origin
                && pending.candidate.public_key_thumbprint == action.public_key_jwk.thumbprint()?,
            "installation origin/key refused"
        );
        if commit {
            anyhow::ensure!(
                pending.phase != Phase::Aborted && pending.expires_at >= now,
                "installation commit refused"
            );
            if pending.phase == Phase::Committed {
                return Ok(());
            }
            let current = state
                .devices
                .iter()
                .position(|d| d.device_id == pending.candidate.device_id);
            match (&pending.prior, current) {
                (Some(old), Some(i)) => {
                    let active = &state.devices[i];
                    anyhow::ensure!(
                        active.credential_id == old.credential_id
                            && active.credential_hash == old.credential_hash
                            && active.scopes == old.scopes
                            && active.revoked_at.is_none(),
                        "installation generation conflict"
                    );
                    state.devices[i] = pending.candidate.clone();
                }
                (None, None) => state.devices.push(pending.candidate.clone()),
                _ => anyhow::bail!("installation generation conflict"),
            }
            state.installations[index].phase = Phase::Committed;
        } else {
            if pending.phase == Phase::Aborted {
                return Ok(());
            }
            if pending.phase == Phase::Committed {
                let i = state
                    .devices
                    .iter()
                    .position(|d| d.device_id == pending.candidate.device_id)
                    .context("installation rollback fenced")?;
                let active = &state.devices[i];
                anyhow::ensure!(
                    active.credential_id == pending.candidate.credential_id
                        && active.credential_generation == pending.candidate.credential_generation
                        && active.scopes == pending.candidate.scopes
                        && active.revoked_at.is_none(),
                    "installation rollback fenced"
                );
                if let Some(old) = pending.prior {
                    state.devices[i] = old;
                } else {
                    state.devices.remove(i);
                }
            }
            state.installations[index].phase = Phase::Aborted;
        }
        self.persist(&state)?;
        Ok(())
    }
}

fn installation_credential(device: &DeviceSession, credential: String) -> DeviceCredential {
    DeviceCredential {
        device_id: device.device_id.clone(),
        credential_id: device.credential_id.clone(),
        credential_generation: device.credential_generation,
        account_enrollment: AccountEnrollment::Unenrolled,
        credential,
        expires_at: device.expires_at,
        scopes: device.scopes.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::{SigningKey, signature::Signer};
    use p256::elliptic_curve::rand_core::OsRng;

    fn jwk(key: &SigningKey) -> PublicJwk {
        let point = key.verifying_key().to_encoded_point(false);
        PublicJwk {
            kty: "EC".into(),
            crv: "P-256".into(),
            x: URL_SAFE_NO_PAD.encode(point.x().unwrap()),
            y: URL_SAFE_NO_PAD.encode(point.y().unwrap()),
        }
    }
    fn signed(key: &SigningKey, value: serde_json::Value) -> String {
        let signature: Signature = key.sign(&serde_json::to_vec(&value).unwrap());
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    }
    fn prepare(
        store: &AuthStore,
        key: &SigningKey,
        old: Option<&DeviceCredential>,
        now: DateTime<Utc>,
    ) -> (PairingExchange, DeviceCredential) {
        let origin = "https://hub.example";
        let scopes = Scope::default_read_only();
        let invitation = store.mint_pairing(origin, scopes.clone(), now).unwrap();
        let mut exchange =
            PairingExchange::test_fixture(invitation.token, "test-hub", origin, scopes);
        exchange.public_key_jwk = jwk(key);
        let mut installation = InstallationProof {
            operation_id: uuid::Uuid::new_v4().to_string(),
            credential: URL_SAFE_NO_PAD.encode([7_u8; 32]),
            device_id: old.map(|c| c.device_id.clone()),
            expected_generation: old.map_or(0, |c| c.credential_generation),
            proof: String::new(),
            previous_proof: None,
        };
        installation.credential = URL_SAFE_NO_PAD.encode(
            uuid::Uuid::new_v4()
                .as_bytes()
                .iter()
                .chain(uuid::Uuid::new_v4().as_bytes())
                .copied()
                .collect::<Vec<_>>(),
        );
        installation.proof = signed(
            key,
            serde_json::json!([
                "cassy-installation-v1",
                "test-hub",
                origin,
                hash_b64(exchange.token.as_bytes()),
                installation.operation_id,
                installation.device_id,
                installation.expected_generation,
                exchange.public_key_jwk.thumbprint().unwrap(),
                hash_b64(installation.credential.as_bytes()),
                exchange
                    .requested_scopes
                    .iter()
                    .map(|s| s.as_wire())
                    .collect::<Vec<_>>(),
                exchange.device_label,
                exchange.operator_label
            ]),
        );
        exchange.installation = Some(installation);
        let credential = store.exchange_pairing(exchange.clone(), now).unwrap();
        (exchange, credential)
    }
    fn action(
        store: &AuthStore,
        key: &SigningKey,
        exchange: &PairingExchange,
        commit: bool,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let op = &exchange.installation.as_ref().unwrap().operation_id;
        store.installation_action(
            InstallationAction {
                public_key_jwk: jwk(key),
                operation_id: op.clone(),
                controller_origin: exchange.controller_origin.clone(),
                proof: signed(
                    key,
                    serde_json::json!([
                        if commit {
                            "cassy-installation-commit-v1"
                        } else {
                            "cassy-installation-abort-v1"
                        },
                        "test-hub",
                        exchange.controller_origin,
                        op
                    ]),
                ),
            },
            commit,
            now,
        )
    }
    fn context(c: &DeviceCredential) -> AuthContext {
        AuthContext {
            device_id: c.device_id.clone(),
            credential_id: c.credential_id.clone(),
            device_label: "test".into(),
            operator_label: "test".into(),
            controller_origin: "https://hub.example".into(),
            scopes: c.scopes.clone(),
            request_id: "test".into(),
        }
    }

    #[test]
    fn five_repairs_keep_one_device_and_refuse_previous_generations() {
        let root = tempfile::tempdir().unwrap();
        let store = AuthStore::open(root.path(), "test-hub").unwrap();
        let key = SigningKey::random(&mut OsRng);
        let now = Utc::now();
        let mut prior: Option<DeviceCredential> = None;
        for generation in 1..=5 {
            let (exchange, candidate) = prepare(
                &store,
                &key,
                prior.as_ref(),
                now + Duration::minutes(generation),
            );
            assert!(
                store
                    .ensure_active_context(&context(&candidate), now)
                    .is_err()
            );
            action(
                &store,
                &key,
                &exchange,
                true,
                now + Duration::minutes(generation),
            )
            .unwrap();
            assert_eq!(candidate.credential_generation, generation as u64);
            if let Some(old) = prior {
                assert_eq!(old.device_id, candidate.device_id);
                assert!(store.ensure_active_context(&context(&old), now).is_err());
            }
            assert_eq!(store.list_devices().unwrap().len(), 1);
            prior = Some(candidate);
        }
    }

    #[test]
    fn cancelled_committed_repair_restores_exact_prior_and_fences_late_abort() {
        let root = tempfile::tempdir().unwrap();
        let store = AuthStore::open(root.path(), "test-hub").unwrap();
        let key = SigningKey::random(&mut OsRng);
        let now = Utc::now();
        let (first, old) = prepare(&store, &key, None, now);
        action(&store, &key, &first, true, now).unwrap();
        let row = store.lock().unwrap().devices[0].clone();
        let (second, _) = prepare(&store, &key, Some(&old), now + Duration::minutes(1));
        action(&store, &key, &second, true, now + Duration::minutes(1)).unwrap();
        action(&store, &key, &second, false, now + Duration::minutes(1)).unwrap();
        assert_eq!(store.lock().unwrap().devices[0], row);
        assert!(store.ensure_active_context(&context(&old), now).is_ok());
        let (third, third_credential) =
            prepare(&store, &key, Some(&old), now + Duration::minutes(2));
        action(&store, &key, &third, true, now + Duration::minutes(2)).unwrap();
        assert_eq!(third_credential.credential_generation, 3);
        let (fourth, _) = prepare(
            &store,
            &key,
            Some(&third_credential),
            now + Duration::minutes(3),
        );
        action(&store, &key, &fourth, true, now + Duration::minutes(3)).unwrap();
        assert!(action(&store, &key, &third, false, now + Duration::minutes(3)).is_err());
    }

    #[test]
    fn substituted_body_refused_and_same_label_different_keys_remain_distinct() {
        let root = tempfile::tempdir().unwrap();
        let store = AuthStore::open(root.path(), "test-hub").unwrap();
        let a = SigningKey::random(&mut OsRng);
        let b = SigningKey::random(&mut OsRng);
        let now = Utc::now();
        let (exchange, _) = prepare(&store, &a, None, now);
        let mut tampered = exchange.clone();
        tampered.public_key_jwk = jwk(&b);
        assert!(store.exchange_pairing(tampered, now).is_err());
        action(&store, &a, &exchange, true, now).unwrap();
        let (second, _) = prepare(&store, &b, None, now);
        action(&store, &b, &second, true, now).unwrap();
        assert_eq!(store.list_devices().unwrap().len(), 2);
    }
}
