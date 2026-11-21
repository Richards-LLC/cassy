//! Append-only transfer transitions. The authenticated service supplies fresh
//! settlement checks; Git serializes changes across machines.
use super::*;

pub(crate) trait TransferChecks {
    /// Includes armed PRs, queue entries and the entire release chain.
    fn external_clear(&self, snapshot: &Snapshot) -> Result<serde_json::Value, String>;
    /// Positive coverage of all prior publishers; heartbeat loss is insufficient.
    fn publishers_stopped(&self, snapshot: &Snapshot) -> Result<serde_json::Value, String>;
}

fn conflict() -> String {
    "TRAIN_TRANSFER_CONFLICT: transfer parameters or authority changed".into()
}

impl Snapshot {
    fn coherent(&self, repo: &Path) -> Result<Self, String> {
        let live = Self::read(repo, &self.record.train)?;
        if live.tip != self.tip {
            return Err(conflict());
        }
        if live.observed_target(repo)? != live.record.target_sha {
            return Err(
                "TRAIN_TRANSFER_TARGET_DIVERGED: operator adoption of the exact target is required"
                    .into(),
            );
        }
        Ok(live)
    }

    fn require_actor(&self, actor: &Identity) -> Result<(), String> {
        if self.record.identity != *actor {
            return Err("TRAIN_OWNER_GENERATION_CHANGED: owner incarnation changed".into());
        }
        Ok(())
    }

    fn append(&self, repo: &Path, record: Record) -> Result<Self, String> {
        self.coherent(repo)?;
        let tip = record_commit(repo, Some(&self.tip), &record)?;
        let lease = format!("--force-with-lease={}:{}", self.reference, self.tip);
        let update = format!("{tip}:{}", self.reference);
        let args = ["push", "--atomic", "--porcelain", &lease, "origin", &update];
        let mut probe = args.to_vec();
        probe.push("--dry-run");
        command(repo, &probe, None)?;
        if let Err(error) = command(repo, &args, None) {
            // A timeout may follow a committed push. Reconcile the exact
            // commit rather than minting another transfer or generation.
            let live = Self::read(repo, &record.train)?;
            if live.tip != tip {
                return Err(if live.tip != self.tip {
                    conflict()
                } else {
                    error
                });
            }
        }
        let confirmed = Self::read(repo, &record.train)?;
        let git = GitOperations::new(repo.to_path_buf());
        if !git.is_ancestor(&tip, &confirmed.tip) {
            return Err(conflict());
        }
        if confirmed.observed_target(repo)? != confirmed.record.target_sha {
            return Err("TRAIN_TRANSFER_TARGET_DIVERGED: transition readback disagrees".into());
        }
        Ok(confirmed)
    }

    pub(crate) fn prepare(
        &self,
        repo: &Path,
        actor: &Identity,
        successor: &Identity,
        id: &str,
    ) -> Result<Self, String> {
        let live = self.coherent(repo)?;
        live.require_actor(actor)?;
        if id.trim().is_empty()
            || successor.owner.trim().is_empty()
            || successor.incarnation.trim().is_empty()
            || successor.owner == actor.owner
        {
            return Err(conflict());
        }
        if matches!(live.record.phase, Phase::Draining | Phase::Quiesced) {
            return if live
                .record
                .transfer
                .as_ref()
                .is_some_and(|t| t.id == id && t.successor == *successor)
            {
                Ok(live)
            } else {
                Err(conflict())
            };
        }
        if live.record.phase != Phase::Active {
            return Err(conflict());
        }
        let mut next = live.record.clone();
        next.phase = Phase::Draining;
        next.transfer = Some(Transfer {
            id: id.into(),
            successor: successor.clone(),
            prior_generation: next.generation.clone(),
        });
        next.quiescence = None;
        next.accepted = None;
        next.operation = format!("prepare:{id}");
        live.append(repo, next)
    }

    pub(crate) fn quiesce(
        &self,
        repo: &Path,
        actor: &Identity,
        checks: &impl TransferChecks,
    ) -> Result<Self, String> {
        let live = self.coherent(repo)?;
        live.require_actor(actor)?;
        if live.record.phase != Phase::Draining || live.record.transfer.is_none() {
            return Err(conflict());
        }
        let external = checks.external_clear(&live)?;
        let publishers = checks.publishers_stopped(&live)?;
        if external.is_null() || publishers.is_null() {
            return Err("TRAIN_TRANSFER_QUIESCENCE_REQUIRED: missing positive evidence".into());
        }
        let mut next = live.record.clone();
        next.phase = Phase::Quiesced;
        next.operation = "quiesce".into();
        next.quiescence = Some(
            serde_json::json!({"external": external, "publishers": publishers, "authority": live.tip, "target": next.target_sha}),
        );
        let confirmed = live.append(repo, next)?;
        checks.external_clear(&confirmed)?;
        Ok(confirmed)
    }

    pub(crate) fn accept(
        &self,
        repo: &Path,
        actor: &Identity,
        id: &str,
        prior_generation: &str,
        checks: &impl TransferChecks,
    ) -> Result<Self, String> {
        let live = Self::read(repo, &self.record.train)?;
        if live.observed_target(repo)? != live.record.target_sha {
            return Err("TRAIN_TRANSFER_TARGET_DIVERGED: operator adoption required".into());
        }
        // Always check, including idempotent projection repair.
        checks.external_clear(&live)?;
        if live.record.phase == Phase::Active
            && live.record.accepted.as_ref().is_some_and(|a| {
                a.transfer_id == id
                    && a.prior_generation == prior_generation
                    && a.successor == *actor
                    && a.generation == live.record.generation
                    && live.record.identity == *actor
            })
        {
            checks.external_clear(&live)?;
            return live.coherent(repo);
        }
        if live.tip != self.tip {
            return Err(conflict());
        }
        let transfer = live.record.transfer.as_ref().ok_or_else(conflict)?;
        if transfer.id != id
            || transfer.prior_generation != prior_generation
            || transfer.successor != *actor
        {
            return Err(conflict());
        }
        if live.record.phase != Phase::Quiesced || live.record.quiescence.is_none() {
            return Err(
                "TRAIN_TRANSFER_QUIESCENCE_REQUIRED: prior publishers have not settled".into(),
            );
        }
        let mut next = live.record.clone();
        next.identity = actor.clone();
        next.phase = Phase::Active;
        next.epoch = next.epoch.checked_add(1).ok_or_else(conflict)?;
        next.generation = uuid::Uuid::new_v4().to_string();
        next.accepted = Some(Acceptance {
            transfer_id: id.into(),
            prior_generation: prior_generation.into(),
            generation: next.generation.clone(),
            successor: actor.clone(),
        });
        next.operation = format!("accept:{id}");
        let confirmed = live.append(repo, next)?;
        checks.external_clear(&confirmed)?;
        Ok(confirmed)
    }

    pub(crate) fn abort(
        &self,
        repo: &Path,
        actor: &Identity,
        checks: &impl TransferChecks,
    ) -> Result<Self, String> {
        let live = self.coherent(repo)?;
        live.require_actor(actor)?;
        if !matches!(live.record.phase, Phase::Draining | Phase::Quiesced) {
            return Err(conflict());
        }
        checks.external_clear(&live)?;
        checks.publishers_stopped(&live)?;
        let mut next = live.record.clone();
        next.phase = Phase::Active;
        next.accepted = None;
        next.quiescence = None;
        next.operation = format!("abort:{}", next.transfer.as_ref().ok_or_else(conflict)?.id);
        let confirmed = live.append(repo, next)?;
        checks.external_clear(&confirmed)?;
        Ok(confirmed)
    }
}
