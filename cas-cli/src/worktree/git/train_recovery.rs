//! Enrollment and explicit recovery; authentication belongs to the shared
//! service. Operator claims in Git are audit data, never signatures.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OperatorClaim {
    pub operator: String,
    pub reason: String,
    pub evidence: String,
    pub stopped_or_fenced: String,
}

impl OperatorClaim {
    fn validate(&self) -> Result<(), String> {
        if [
            &self.operator,
            &self.reason,
            &self.evidence,
            &self.stopped_or_fenced,
        ]
        .iter()
        .any(|s| s.trim().is_empty())
        {
            return Err(
                "TRAIN_TRANSFER_QUIESCENCE_REQUIRED: explicit operator attestation is incomplete"
                    .into(),
            );
        }
        Ok(())
    }
    fn audit(&self) -> serde_json::Value {
        serde_json::json!({"unauthenticated_operator_claim": self})
    }
}

impl Snapshot {
    pub(crate) fn enroll(
        repo: &Path,
        project: &str,
        train: &str,
        target: &str,
        actor: &Identity,
        head: &str,
        expected_target: Option<&str>,
        checks: &impl TransferChecks,
    ) -> Result<Self, String> {
        if project.trim().is_empty()
            || !target.starts_with("epic/")
            || actor.owner.trim().is_empty()
            || actor.incarnation.trim().is_empty()
        {
            return Err(
                "TRAIN_TRANSFER_FENCE_UNAVAILABLE: enrollment identity is incomplete".into(),
            );
        }
        let reference = reference(train)?;
        // Explicit initialization must inspect origin directly: the new task
        // already records enrollment before its first expected-absent push.
        // Ordinary publisher discovery separately refuses missing authority.
        if Self::all(repo)?.iter().any(|snapshot| snapshot.record.target == target) {
            return Err("TRAIN_TRANSFER_CONFLICT: target already has an enrolled authority".into());
        }
        let target_ref = format!("refs/heads/{target}");
        command(repo, &["check-ref-format", &target_ref], None)?;
        let refs = command(
            repo,
            &["ls-remote", "--refs", "origin", &reference, &target_ref],
            None,
        )?;
        let refs: std::collections::BTreeMap<&str, &str> = refs
            .lines()
            .filter_map(|l| l.split_once('\t').map(|(sha, name)| (name, sha)))
            .collect();
        if refs.contains_key(reference.as_str())
            || refs.get(target_ref.as_str()).copied() != expected_target
        {
            return Err(
                "TRAIN_TRANSFER_CONFLICT: enrollment requires exact target and absent authority"
                    .into(),
            );
        }
        let git = GitOperations::new(repo.to_path_buf());
        let head = git
            .resolve_commit(head)
            .ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: enrollment source missing")?;
        if expected_target.is_some_and(|old| old != head) {
            return Err(
                "TRAIN_TRANSFER_TARGET_DIVERGED: enrollment cannot advance an existing target"
                    .into(),
            );
        }
        let mut snapshot = Self {
            reference,
            tip: String::new(),
            transition_audit: None,
            record: Record {
                protocol: 1,
                project: project.into(),
                train: train.into(),
                target: target.into(),
                identity: actor.clone(),
                epoch: 1,
                generation: uuid::Uuid::new_v4().to_string(),
                phase: Phase::Active,
                target_sha: head.clone(),
                operation: "enroll".into(),
                transfer: None,
                quiescence: None,
                accepted: None,
                operator_claim: None,
                audit: None,
                external_operations: Vec::new(),
                publication_sequence: 0,
            },
        };
        let external = checks.external_clear(&snapshot)?;
        let publishers = checks.publishers_stopped(&snapshot)?;
        if external.is_null() || publishers.is_null() {
            return Err(
                "TRAIN_TRANSFER_QUIESCENCE_REQUIRED: enrollment coverage is unknown".into(),
            );
        }
        snapshot.record.quiescence = Some(
            serde_json::json!({"enrollment": {"external": external, "publishers": publishers}}),
        );
        let tip = record_commit(repo, None, &snapshot.record)?;
        let update = AtomicUpdate {
            reference: snapshot.reference.clone(),
            expected: String::new(),
            next: tip.clone(),
        };
        let _outcome = git.publish_pinned_source_with_lease(
            target,
            &head,
            Some(expected_target.unwrap_or("")),
            false,
            Some(&update),
            false,
            TIMEOUT,
        );
        let confirmed = Self::read(repo, train)?;
        if confirmed.tip != tip {
            return Err(
                "TRAIN_TRANSFER_CONFLICT: atomic enrollment did not publish the exact authority"
                    .into(),
            );
        }
        if confirmed.observed_target(repo)? != head {
            return Err("TRAIN_TRANSFER_TARGET_DIVERGED: enrollment readback disagrees".into());
        }
        confirmed.external_clear_checked(checks)?;
        Ok(confirmed)
    }

    /// Only the service's authenticated mailbox predecessor check may call
    /// this; matching names, heartbeat age and a model claim never suffice.
    pub(crate) fn refresh_incarnation(
        &self,
        repo: &Path,
        prior: &Identity,
        successor: &Identity,
    ) -> Result<Self, String> {
        let live = self.coherent(repo)?;
        live.require_actor(prior)?;
        if prior.owner != successor.owner || successor.incarnation.trim().is_empty() {
            return Err(crate::gates::tag(
                "G-113",
                "TRAIN_OWNER_GENERATION_CHANGED: restart cannot change logical owner",
            ));
        }
        if prior == successor {
            return Ok(live);
        }
        let mut next = live.record.clone();
        next.identity = successor.clone();
        next.operation = "refresh-incarnation".into();
        live.append(repo, next)
    }

    pub(crate) fn force_quiesce(
        &self,
        repo: &Path,
        successor: &Identity,
        id: &str,
        claim: &OperatorClaim,
        checks: &impl TransferChecks,
    ) -> Result<Self, String> {
        claim.validate()?;
        let live = self.coherent(repo)?;
        if id.trim().is_empty()
            || successor.owner.trim().is_empty()
            || successor.incarnation.trim().is_empty()
            || successor.owner == live.record.identity.owner
        {
            return Err("TRAIN_TRANSFER_CONFLICT: invalid successor".into());
        }
        if matches!(live.record.phase, Phase::Draining | Phase::Quiesced)
            && !live
                .record
                .transfer
                .as_ref()
                .is_some_and(|t| t.id == id && t.successor == *successor)
        {
            return Err("TRAIN_TRANSFER_CONFLICT: draining successor is immutable".into());
        }
        let external = live.external_clear_checked(checks)?;
        if external.is_null() {
            return Err("TRAIN_TRANSFER_EXTERNAL_PUBLICATION_PENDING: observation missing".into());
        }
        let mut next = live.record.clone();
        next.transfer = Some(Transfer {
            id: id.into(),
            successor: successor.clone(),
            prior_generation: next.generation.clone(),
        });
        next.phase = Phase::Quiesced;
        next.accepted = None;
        next.operator_claim = Some(claim.audit());
        next.quiescence = Some(
            serde_json::json!({"external": external, "publishers": claim.audit(), "authority": live.tip, "target": next.target_sha}),
        );
        next.operation = format!("force-quiesce:{id}");
        let confirmed = live.append(repo, next)?;
        confirmed.external_clear_checked(checks)?;
        Ok(confirmed)
    }

    pub(crate) fn adopt_target(
        &self,
        repo: &Path,
        exact_target: &str,
        claim: &OperatorClaim,
        checks: &impl TransferChecks,
    ) -> Result<Self, String> {
        claim.validate()?;
        let mut live = Self::read(repo, &self.record.train)?;
        live.transition_audit = self.transition_audit.clone();
        if live.tip != self.tip {
            return Err("TRAIN_TRANSFER_CONFLICT: expected authority moved".into());
        }
        let observed = live.observed_target(repo)?;
        if observed != exact_target || observed == live.record.target_sha {
            return Err(
                "TRAIN_TRANSFER_CONFLICT: adoption must name the exact diverged target".into(),
            );
        }
        live.external_clear_checked(checks)?;
        let mut next = live.record.clone();
        next.target_sha = observed.clone();
        next.epoch = next
            .epoch
            .checked_add(1)
            .ok_or("TRAIN_TRANSFER_CONFLICT: epoch overflow")?;
        next.generation = uuid::Uuid::new_v4().to_string();
        next.phase = Phase::RecoveryHeld;
        next.transfer = None;
        next.accepted = None;
        next.quiescence = None;
        next.operator_claim = Some(
            serde_json::json!({"claim": claim.audit(), "prior_target": live.record.target_sha, "adopted_target": observed}),
        );
        next.operation = "adopt-target".into();
        let confirmed = live.append_observing(repo, next, exact_target)?;
        confirmed.external_clear_checked(checks)?;
        Ok(confirmed)
    }
}
