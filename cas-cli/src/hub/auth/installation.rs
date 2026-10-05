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
    pub pairing_token_hash: String,
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

#[derive(Debug, thiserror::Error)]
#[error("installation generation conflict")]
pub(super) struct InstallationConflict;
fn generation_matches(matches: bool) -> Result<()> {
    if matches {
        Ok(())
    } else {
        Err(InstallationConflict.into())
    }
}

/// Keep active rollback and at most 1000 prepared operations. An expired
/// prepare cannot commit, but its receipt still proves a late abort after an
/// outage. Superseded/revoked commits need only the ten-minute retry window.
fn prune_installations(state: &mut PersistedState, now: DateTime<Utc>) {
    state.installations.retain(|p| {
        p.phase == Phase::Prepared
            || p.expires_at >= now
            || (p.phase == Phase::Committed
                && state.devices.iter().any(|d| {
                    d.device_id == p.candidate.device_id
                        && d.credential_id == p.candidate.credential_id
                        && d.revoked_at.is_none()
                }))
    });
    state
        .aborted_installations
        .retain(|_, expires| *expires >= now);
}

impl AuthStore {
    pub fn installation_protocol_matches(
        &self,
        token_hash: &str,
        origin: &str,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        Ok(self.lock()?.pairings.iter().any(|p| {
            constant_time_eq(&p.token_hash, token_hash)
                && p.hub_id == self.0.machine_id
                && p.controller_origin == origin
                && p.consumed_at.is_none()
                && p.expires_at >= now
        }))
    }
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
        prune_installations(&mut state, now);
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
        state
            .source_attempts
            .retain(|a| a.at > now - Duration::hours(1));
        if state
            .source_attempts
            .iter()
            .filter(|a| a.source == exchange.source && a.at > now - Duration::minutes(1))
            .count()
            >= 5
        {
            return Err(PairingExchangeError::Throttled {
                retry_after_seconds: 60,
            }
            .into());
        }
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
            generation_matches(old.credential_generation == proof.expected_generation)?;
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
            let old = matches.first().map(|d| (**d).clone());
            generation_matches(old.as_ref().map_or(proof.expected_generation == 0, |d| {
                d.credential_generation == proof.expected_generation
            }))?;
            old
        };
        let device_id = prior
            .as_ref()
            .map(|d| d.device_id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        generation_matches(!state.installations.iter().any(|p| {
            p.phase == Phase::Prepared
                && p.expires_at >= now
                && (p.candidate.device_id == device_id
                    || (p.candidate.controller_origin == exchange.controller_origin
                        && p.candidate.public_key_thumbprint == thumbprint))
        }))?;
        anyhow::ensure!(
            state
                .installations
                .iter()
                .filter(|p| p.phase == Phase::Prepared)
                .count()
                < 1000,
            "too many pending installation operations"
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

    /// Inventory actions recheck the authorizing version inside the mutation lock.
    pub fn revoke_installation(
        &self,
        context: &AuthContext,
        device_id: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let mut state = self.lock()?;
        Self::ensure_active_context_in_state(&state, context, now)?;
        anyhow::ensure!(
            context.device_id == device_id || context.has(Scope::HubAdmin),
            "installation revoke refused"
        );
        let device = state
            .devices
            .iter_mut()
            .find(|d| d.device_id == device_id)
            .context("device not found")?;
        device.revoked_at = Some(now);
        prune_installations(&mut state, now);
        state.leases.retain(|_, lease| lease.device_id != device_id);
        self.persist(&state)?;
        let _ = self.0.revocations.send(device_id.to_owned());
        drop(state);
        self.audit(
            Some(context),
            "allowed",
            "installation_revoke",
            Some(Scope::MachineRead),
            None,
            now,
        )
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
        prune_installations(&mut state, now);
        uuid::Uuid::parse_str(&action.operation_id).context("invalid installation operation")?;
        let transcript = serde_json::json!([
            if commit {
                "cassy-installation-commit-v1"
            } else {
                "cassy-installation-abort-v1"
            },
            self.0.machine_id,
            action.controller_origin,
            action.operation_id,
            action.pairing_token_hash.clone()
        ]);
        verify_transcript(&action.public_key_jwk, &action.proof, &transcript)?;
        let index = state
            .installations
            .iter()
            .position(|p| p.operation_id == action.operation_id);
        let Some(index) = index else {
            anyhow::ensure!(!commit, "unknown installation operation");
            if state
                .aborted_installations
                .contains_key(&action.operation_id)
            {
                return Ok(());
            }
            // A missing operation may be a pruned, fenced commit. Only an
            // unconsumed invitation proves that prepare never committed. Its
            // signed hash prevents substitution; lock ordering prevents a
            // delayed prepare overtaking this cancellation tombstone.
            let token_hash = &action.pairing_token_hash;
            let invitation_expiry = state
                .pairings
                .iter()
                .find(|p| {
                    constant_time_eq(&p.token_hash, token_hash)
                        && p.hub_id == self.0.machine_id
                        && p.controller_origin == action.controller_origin
                        && p.consumed_at.is_none()
                })
                .context("installation rollback cannot be confirmed")?
                .expires_at;
            state
                .aborted_installations
                .retain(|_, expires| *expires >= now);
            anyhow::ensure!(
                state.aborted_installations.len() < 1000,
                "too many pending cancellations"
            );
            state
                .aborted_installations
                .insert(action.operation_id, invitation_expiry);
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
            if pending.phase == Phase::Committed {
                generation_matches(state.devices.iter().any(|d| {
                    d.device_id == pending.candidate.device_id
                        && d.credential_id == pending.candidate.credential_id
                        && d.revoked_at.is_none()
                }))?;
                return Ok(());
            }
            anyhow::ensure!(
                pending.phase != Phase::Aborted && pending.expires_at >= now,
                "installation commit refused"
            );
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
    fn exchange(
        store: &AuthStore,
        key: &SigningKey,
        old: Option<&DeviceCredential>,
        now: DateTime<Utc>,
    ) -> PairingExchange {
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
        exchange
    }
    fn prepare(
        store: &AuthStore,
        key: &SigningKey,
        old: Option<&DeviceCredential>,
        now: DateTime<Utc>,
    ) -> (PairingExchange, DeviceCredential) {
        let exchange = exchange(store, key, old, now);
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
                pairing_token_hash: hash_b64(exchange.token.as_bytes()),
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
                        op,
                        hash_b64(exchange.token.as_bytes())
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

    fn dpop(
        key: &SigningKey,
        c: &DeviceCredential,
        method: &str,
        path: &str,
        now: DateTime<Utc>,
    ) -> String {
        let header = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(
                &serde_json::json!({"typ":"dpop+jwt", "alg":"ES256", "jwk":jwk(key)}),
            )
            .unwrap(),
        );
        let claims = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&serde_json::json!({"htm":method, "htu":path, "iat":now.timestamp(), "jti":uuid::Uuid::new_v4().to_string(), "ath":hash_b64(c.credential.as_bytes())})).unwrap());
        let input = format!("{header}.{claims}");
        let signature: Signature = key.sign(input.as_bytes());
        format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
    }
    fn authenticate(
        store: &AuthStore,
        key: &SigningKey,
        c: &DeviceCredential,
        now: DateTime<Utc>,
    ) -> Result<AuthContext> {
        let proof = dpop(key, c, "GET", "/v1/machine", now);
        store.authenticate_dpop(
            &format!("DPoP {}", c.credential),
            &proof,
            "https://hub.example",
            "GET",
            "/v1/machine",
            now,
        )
    }

    #[test]
    fn five_repairs_keep_one_device_and_refuse_previous_generations() {
        let root = tempfile::tempdir().unwrap();
        let store = AuthStore::open(root.path().join("hub"), "test-hub").unwrap();
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
            assert!(authenticate(&store, &key, &candidate, now).is_err());
            let ticket = prior.as_ref().map(|old| {
                store
                    .issue_ws_ticket(&context(old), "session", "/v1/attach", now)
                    .unwrap()
            });
            action(
                &store,
                &key,
                &exchange,
                true,
                now + Duration::minutes(generation),
            )
            .unwrap();
            assert_eq!(candidate.credential_generation, generation as u64);
            assert!(authenticate(&store, &key, &candidate, now).is_ok());
            if let Some(old) = prior {
                assert_eq!(old.device_id, candidate.device_id);
                assert!(store.ensure_active_context(&context(&old), now).is_err());
                assert!(authenticate(&store, &key, &old, now).is_err());
                assert!(
                    store
                        .consume_ws_ticket(
                            &ticket.unwrap().ticket,
                            "https://hub.example",
                            "session",
                            "/v1/attach",
                            now
                        )
                        .is_err()
                );
            }
            assert_eq!(store.list_devices().unwrap().len(), 1);
            prior = Some(candidate);
        }
    }

    #[test]
    fn cancelled_committed_repair_restores_exact_prior_and_fences_late_abort() {
        let root = tempfile::tempdir().unwrap();
        let store = AuthStore::open(root.path().join("hub"), "test-hub").unwrap();
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
        let store = AuthStore::open(root.path().join("hub"), "test-hub").unwrap();
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

    #[test]
    fn abort_before_prepare_is_idempotent_and_refuses_delayed_prepare() {
        let root = tempfile::tempdir().unwrap();
        let store = AuthStore::open(root.path().join("hub"), "test-hub").unwrap();
        let key = SigningKey::random(&mut OsRng);
        let now = Utc::now();
        let request = exchange(&store, &key, None, now);
        action(&store, &key, &request, false, now).unwrap();
        action(&store, &key, &request, false, now).unwrap();
        assert!(store.exchange_pairing(request, now).is_err());
        assert!(store.list_devices().unwrap().is_empty());
    }

    #[test]
    fn expired_prepare_cannot_commit_but_outage_recovery_can_still_abort() {
        let root = tempfile::tempdir().unwrap();
        let store = AuthStore::open(root.path().join("hub"), "test-hub").unwrap();
        let key = SigningKey::random(&mut OsRng);
        let now = Utc::now();
        let (first, old) = prepare(&store, &key, None, now);
        action(&store, &key, &first, true, now).unwrap();
        let (pending, _) = prepare(&store, &key, Some(&old), now);
        let later = now + Duration::days(1);
        assert!(action(&store, &key, &pending, true, later).is_err());
        action(&store, &key, &pending, false, later).unwrap();
        assert!(authenticate(&store, &key, &old, later).is_ok());
    }

    #[test]
    fn superseded_commit_is_pruned_without_falsely_confirming_late_rollback() {
        let root = tempfile::tempdir().unwrap();
        let store = AuthStore::open(root.path().join("hub"), "test-hub").unwrap();
        let key = SigningKey::random(&mut OsRng);
        let now = Utc::now();
        let (first, old) = prepare(&store, &key, None, now);
        action(&store, &key, &first, true, now).unwrap();
        let later = now + Duration::minutes(11);
        let (second, current) = prepare(&store, &key, Some(&old), later);
        action(&store, &key, &second, true, later).unwrap();
        assert!(action(&store, &key, &first, false, later).is_err());
        assert!(action(&store, &key, &first, true, later).is_err());
        assert_eq!(store.lock().unwrap().installations.len(), 1);
        assert!(authenticate(&store, &key, &current, later).is_ok());
        // The active candidate keeps its rollback even beyond the retry window.
        action(&store, &key, &second, false, later + Duration::hours(1)).unwrap();
        assert!(store.ensure_active_context(&context(&old), later).is_ok());
    }

    #[test]
    fn simultaneous_initial_prepares_cannot_enroll_two_rows_for_the_same_key() {
        let root = tempfile::tempdir().unwrap();
        let a = AuthStore::open(root.path().join("hub"), "test-hub").unwrap();
        let b = AuthStore::open(root.path().join("hub"), "test-hub").unwrap();
        let key = SigningKey::random(&mut OsRng);
        let now = Utc::now();
        let first = exchange(&a, &key, None, now);
        let second = exchange(&b, &key, None, now);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let other_barrier = barrier.clone();
        let other = std::thread::spawn(move || {
            other_barrier.wait();
            b.exchange_pairing(second, now)
        });
        barrier.wait();
        let own = a.exchange_pairing(first, now);
        let peer = other.join().unwrap();
        assert_ne!(own.is_ok(), peer.is_ok());
        assert_eq!(a.lock().unwrap().installations.len(), 1);
        assert!(a.list_devices().unwrap().is_empty());
    }

    #[test]
    fn rekey_requires_old_key_and_origin_body_changes_refuse() {
        let root = tempfile::tempdir().unwrap();
        let store = AuthStore::open(root.path().join("hub"), "test-hub").unwrap();
        let old_key = SigningKey::random(&mut OsRng);
        let new_key = SigningKey::random(&mut OsRng);
        let now = Utc::now();
        let (first, old) = prepare(&store, &old_key, None, now);
        action(&store, &old_key, &first, true, now).unwrap();
        let request = exchange(&store, &new_key, Some(&old), now);
        assert!(store.exchange_pairing(request.clone(), now).is_err());
        let mut altered = request.clone();
        altered.controller_origin = "https://other.example".into();
        assert!(store.exchange_pairing(altered, now).is_err());
        let mut altered = request.clone();
        altered.operator_label.push('x');
        assert!(store.exchange_pairing(altered, now).is_err());
        let proof = request.installation.as_ref().unwrap();
        let transcript = serde_json::json!([
            "cassy-installation-v1",
            "test-hub",
            request.controller_origin,
            hash_b64(request.token.as_bytes()),
            proof.operation_id,
            proof.device_id,
            proof.expected_generation,
            request.public_key_jwk.thumbprint().unwrap(),
            hash_b64(proof.credential.as_bytes()),
            request
                .requested_scopes
                .iter()
                .map(|s| s.as_wire())
                .collect::<Vec<_>>(),
            request.device_label,
            request.operator_label
        ]);
        let mut authorized = request;
        authorized.installation.as_mut().unwrap().previous_proof =
            Some(signed(&old_key, transcript));
        let rotated = store.exchange_pairing(authorized.clone(), now).unwrap();
        action(&store, &new_key, &authorized, true, now).unwrap();
        assert!(authenticate(&store, &new_key, &rotated, now).is_ok());
        assert!(authenticate(&store, &old_key, &rotated, now).is_err());
        action(&store, &new_key, &authorized, false, now).unwrap();
        assert!(authenticate(&store, &old_key, &old, now).is_ok());
    }

    #[test]
    fn revoked_and_refreshed_candidates_fence_abort_and_cross_device_revoke_needs_admin() {
        let root = tempfile::tempdir().unwrap();
        let store = AuthStore::open(root.path().join("hub"), "test-hub").unwrap();
        let key = SigningKey::random(&mut OsRng);
        let other_key = SigningKey::random(&mut OsRng);
        let now = Utc::now();
        let (first, old) = prepare(&store, &key, None, now);
        action(&store, &key, &first, true, now).unwrap();
        let (second, other) = prepare(&store, &other_key, None, now);
        action(&store, &other_key, &second, true, now).unwrap();
        assert!(
            store
                .revoke_installation(&context(&old), &other.device_id, now)
                .is_err()
        );
        let refreshed = store
            .refresh_device_credential(
                &format!("DPoP {}", old.credential),
                &dpop(&key, &old, "POST", "/v1/auth/refresh", now),
                "https://hub.example",
                "POST",
                "/v1/auth/refresh",
                now,
            )
            .unwrap();
        assert!(action(&store, &key, &first, false, now).is_err());
        assert!(
            store
                .ensure_active_context(&context(&refreshed), now)
                .is_ok()
        );
        store
            .revoke_installation(&context(&other), &other.device_id, now)
            .unwrap();
        assert!(action(&store, &other_key, &second, false, now).is_err());
        assert!(store.ensure_active_context(&context(&other), now).is_err());
    }

    #[test]
    fn refresh_wins_against_prepared_repair_and_abort_preserves_refresh() {
        let root = tempfile::tempdir().unwrap();
        let store = AuthStore::open(root.path().join("hub"), "test-hub").unwrap();
        let key = SigningKey::random(&mut OsRng);
        let now = Utc::now();
        let (first, old) = prepare(&store, &key, None, now);
        action(&store, &key, &first, true, now).unwrap();
        let (repair, _) = prepare(&store, &key, Some(&old), now);
        let refreshed = store
            .refresh_device_credential(
                &format!("DPoP {}", old.credential),
                &dpop(&key, &old, "POST", "/v1/auth/refresh", now),
                "https://hub.example",
                "POST",
                "/v1/auth/refresh",
                now,
            )
            .unwrap();
        assert!(action(&store, &key, &repair, true, now).is_err());
        action(&store, &key, &repair, false, now).unwrap();
        assert!(authenticate(&store, &key, &refreshed, now).is_ok());
        assert!(authenticate(&store, &key, &old, now).is_err());
    }
}
