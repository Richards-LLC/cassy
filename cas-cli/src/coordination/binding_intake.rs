//! Local, durable legacy-intake admission. A pause has no automatic resume path.
//!
//! File locks serialize short admission/policy decisions, never network work.
//! The daemon lifecycle separately drains work admitted before the pause.
use super::binding_reconciliation::{
    ReconciliationCode, ReconciliationRequest, ReconciliationResponse,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntakeScope {
    pub team_id: String,
    pub canonical_project_id: String,
    pub channel_id: String,
}

impl IntakeScope {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if Uuid::parse_str(&self.team_id).is_err()
            || self.canonical_project_id.trim().is_empty()
            || self.canonical_project_id.len() > 256
            || !self.channel_id.starts_with(['C', 'G'])
            || !self.channel_id.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            return Err("invalid binding intake scope".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PauseRecord {
    pub operation_id: Uuid,
    pub scope: IntakeScope,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseOrigin {
    LegacyIntakeActive,
}

/// A missing classification never downgrades a positive managed observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChannelDisposition {
    Paused {
        /// Only a definite refusal establishes a quiet window across retries.
        /// Older journals and pauses that never submitted have no origin.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<PauseOrigin>,
    },
    /// One attempt's request may have left. Uncertain like `Unknown`, but
    /// attributable: only a definite answer to this same attempt settles it.
    Submitting {
        operation_id: Uuid,
        attempt_id: Uuid,
    },
    Unknown,
    Managed {
        operation_id: Uuid,
        completion: Box<ReconciliationResponse>,
    },
    LegacyAllowed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelPolicy {
    pub scope: IntakeScope,
    pub disposition: ChannelDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntakeState {
    pub schema_version: u32,
    pub generation: u64,
    pub pauses: Vec<PauseRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<ChannelPolicy>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub conservative_unclassified: bool,
}

impl Default for IntakeState {
    fn default() -> Self {
        Self {
            schema_version: 1,
            generation: 0,
            pauses: Vec::new(),
            policies: Vec::new(),
            conservative_unclassified: false,
        }
    }
}

impl IntakeState {
    /// v1 claims name a project, not a channel. Any paused or uncertain
    /// channel therefore closes this host's whole project claim lane.
    pub fn legacy_claims_allowed(&self) -> bool {
        let gated = |scope: &IntakeScope| {
            !matches!(
                self.policies
                    .iter()
                    .find(|policy| &policy.scope == scope)
                    .map(|policy| &policy.disposition),
                Some(ChannelDisposition::Managed { .. } | ChannelDisposition::LegacyAllowed)
            )
        };
        !self.pauses.iter().any(|pause| gated(&pause.scope))
            && !self.policies.iter().any(|policy| gated(&policy.scope))
    }

    pub fn channel_legacy_allowed(&self, channel: &str) -> bool {
        if let Some(policy) = self
            .policies
            .iter()
            .find(|policy| policy.scope.channel_id == channel)
        {
            return matches!(policy.disposition, ChannelDisposition::LegacyAllowed);
        }
        !self.conservative_unclassified
            && !self
                .pauses
                .iter()
                .any(|pause| pause.scope.channel_id == channel)
    }
}

const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// An absent journal is legacy-compatible. Any present unreadable, invalid or
/// future journal refuses admission; it is never reset or silently repaired.
pub struct IntakeJournal {
    directory: PathBuf,
}
impl IntakeJournal {
    pub fn new(cas_dir: &Path) -> Self {
        Self {
            directory: cas_dir.join("coordination/binding-intake"),
        }
    }
    pub fn path(&self) -> PathBuf {
        self.directory.join("state.json")
    }
    fn lock(&self) -> Result<File, String> {
        fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.directory.join("state.lock"))
            .map_err(|e| e.to_string())?;
        // Readers (the daemon tick, drain replies) hold the lock briefly; wait
        // for them rather than failing a settle/release and stranding a pause.
        let deadline = std::time::Instant::now() + LOCK_WAIT;
        loop {
            match lock.try_lock_exclusive() {
                Ok(()) => return Ok(lock),
                Err(e) if std::time::Instant::now() >= deadline => {
                    return Err(format!("binding intake journal busy/unavailable: {e}"));
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        }
    }
    fn read_locked(&self) -> Result<IntakeState, String> {
        let bytes = match fs::read(self.path()) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(IntakeState::default()),
            Err(e) => return Err(e.to_string()),
        };
        let state: IntakeState = serde_json::from_slice(&bytes)
            .map_err(|e| format!("invalid binding intake journal: {e}"))?;
        if state.schema_version != 1 || state.generation != state.pauses.len() as u64 {
            return Err("unsupported or inconsistent binding intake journal".into());
        }
        let mut operations = std::collections::HashSet::new();
        for (index, pause) in state.pauses.iter().enumerate() {
            pause.scope.validate()?;
            if pause.operation_id.is_nil()
                || pause.generation != index as u64 + 1
                || !operations.insert(pause.operation_id)
            {
                return Err("invalid binding intake journal generation/operation".into());
            }
        }
        let mut channels = std::collections::HashSet::new();
        let root_scope = state
            .pauses
            .first()
            .map(|p| &p.scope)
            .or_else(|| state.policies.first().map(|p| &p.scope));
        for scope in state
            .pauses
            .iter()
            .map(|p| &p.scope)
            .chain(state.policies.iter().map(|p| &p.scope))
        {
            scope.validate()?;
            if root_scope.is_some_and(|root| {
                root.team_id != scope.team_id
                    || root.canonical_project_id != scope.canonical_project_id
            }) {
                return Err(
                    "binding intake journal contains conflicting project/team scopes".into(),
                );
            }
        }
        for policy in &state.policies {
            if !channels.insert(&policy.scope.channel_id) {
                return Err("duplicate channel intake policy".into());
            }
            match &policy.disposition {
                ChannelDisposition::Managed {
                    operation_id,
                    completion,
                } => {
                    if !state.pauses.iter().any(|pause| {
                        pause.operation_id == *operation_id && pause.scope == policy.scope
                    }) {
                        return Err("managed evidence lacks its submission operation".into());
                    }
                    let request = ReconciliationRequest::new(
                        &policy.scope.team_id,
                        &policy.scope.canonical_project_id,
                        &policy.scope.channel_id,
                    )
                    .map_err(|e| e.to_string())?;
                    let bytes = serde_json::to_vec(completion).map_err(|e| e.to_string())?;
                    if request
                        .parse_response(200, &bytes)
                        .map_err(|e| e.to_string())?
                        .code
                        != ReconciliationCode::Completed
                        || !state.conservative_unclassified
                    {
                        return Err("managed policy lacks strict completion/safety latch".into());
                    }
                }
                ChannelDisposition::Submitting { operation_id, .. } => {
                    if !state.pauses.iter().any(|pause| {
                        pause.operation_id == *operation_id && pause.scope == policy.scope
                    }) {
                        return Err("submitting channel lacks its operation".into());
                    }
                }
                ChannelDisposition::Paused { .. } | ChannelDisposition::Unknown => {
                    if !state.pauses.iter().any(|p| p.scope == policy.scope) {
                        return Err("channel gate lacks its original pause".into());
                    }
                }
                ChannelDisposition::LegacyAllowed => {}
            }
        }
        Ok(state)
    }
    fn write_locked(&self, state: &IntakeState) -> Result<(), String> {
        let mut temporary =
            tempfile::NamedTempFile::new_in(&self.directory).map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut temporary, &state).map_err(|e| e.to_string())?;
        temporary.flush().map_err(|e| e.to_string())?;
        temporary.as_file().sync_all().map_err(|e| e.to_string())?;
        temporary.persist(self.path()).map_err(|e| e.to_string())?;
        // Persist the rename and newly created journal ancestors before ACK.
        for directory in [
            Some(self.directory.as_path()),
            self.directory.parent(),
            self.directory.parent().and_then(Path::parent),
        ]
        .into_iter()
        .flatten()
        {
            File::open(directory)
                .and_then(|f| f.sync_all())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub fn read(&self) -> Result<IntakeState, String> {
        // A project that never gated a channel has no journal directory; do
        // not create one (and its lock file) on every daemon tick.
        if !self.directory.exists() {
            return Ok(IntakeState::default());
        }
        let _lock = self.lock()?;
        self.read_locked()
    }
    /// Project-wide v1 claims cannot select channels. Any pause closes that
    /// admission path. C will refine direct reads without opening v1 claims.
    pub fn legacy_allowed(&self) -> Result<bool, String> {
        let state = self.read()?;
        Ok(state.legacy_claims_allowed()
            && (!state.conservative_unclassified
                || state
                    .policies
                    .iter()
                    .any(|p| matches!(p.disposition, ChannelDisposition::LegacyAllowed))))
    }
    /// Must be durable before the first ordinary POST byte can leave. There is
    /// deliberately no negative-GET/timer API that clears uncertainty; only a
    /// definite answer to a later identical attempt does (`settle_refusal`).
    /// Returns the new attempt, or `None` when managed evidence stands.
    pub fn mark_submitting(&self, operation_id: Uuid) -> Result<Option<Uuid>, String> {
        let _lock = self.lock()?;
        let mut state = self.read_locked()?;
        let scope = state
            .pauses
            .iter()
            .find(|pause| pause.operation_id == operation_id)
            .ok_or("submission requires an existing durable pause")?
            .scope
            .clone();
        let attempt_id = Uuid::new_v4();
        // An identical retry subsumes earlier uncertainty: if an earlier attempt
        // committed, Cloud answers this one already_managed; a definite refusal
        // of this attempt is the exit from Unknown.
        let submitting = ChannelDisposition::Submitting {
            operation_id,
            attempt_id,
        };
        if let Some(policy) = state.policies.iter_mut().find(|p| p.scope == scope) {
            if matches!(policy.disposition, ChannelDisposition::Managed { .. }) {
                return Ok(None);
            }
            policy.disposition = submitting;
        } else {
            state.policies.push(ChannelPolicy {
                scope,
                disposition: submitting,
            });
        }
        self.write_locked(&state)?;
        Ok(Some(attempt_id))
    }

    /// The attempt's outcome is unknown (transport, parser, no-store). It stays
    /// uncertain until a later identical attempt is answered definitely.
    pub(super) fn settle_uncertain(&self, attempt_id: Uuid) -> Result<(), String> {
        let _lock = self.lock()?;
        let mut state = self.read_locked()?;
        if let Some(policy) = state.policies.iter_mut().find(|policy| {
            matches!(policy.disposition, ChannelDisposition::Submitting { attempt_id: a, .. } if a == attempt_id)
        }) {
            policy.disposition = ChannelDisposition::Unknown;
            return self.write_locked(&state);
        }
        Ok(())
    }

    /// Settle THIS attempt from a strict, definite refusal. Only the attempt
    /// that still owns the channel settles: legacy intake still active stays
    /// paused so the host can go quiet; any other definite refusal of an
    /// unmanaged binding proves no effect and restores legacy intake. An
    /// unknown outcome, or a refusal reporting a managed binding, stays
    /// uncertain. Returns what the channel holds afterwards.
    pub(super) fn settle_refusal(
        &self,
        operation_id: Uuid,
        attempt_id: Uuid,
        response: &ReconciliationResponse,
    ) -> Result<ChannelDisposition, String> {
        use super::binding_reconciliation::ReconciliationReason as Reason;
        let _lock = self.lock()?;
        let mut state = self.read_locked()?;
        let scope = state
            .pauses
            .iter()
            .find(|pause| pause.operation_id == operation_id)
            .ok_or("refusal requires its original pause")?
            .scope
            .clone();
        let policy = state
            .policies
            .iter_mut()
            .find(|policy| policy.scope == scope)
            .ok_or("refusal requires durable submitting state")?;
        // Another attempt or managed evidence is never relaxed here.
        if policy.disposition
            != (ChannelDisposition::Submitting {
                operation_id,
                attempt_id,
            })
        {
            return Ok(policy.disposition.clone());
        }
        let request = ReconciliationRequest::new(
            &policy.scope.team_id,
            &policy.scope.canonical_project_id,
            &policy.scope.channel_id,
        )
        .map_err(|e| e.to_string())?;
        let (status, _, _) = response.reason.contract();
        let bytes = serde_json::to_vec(response).map_err(|e| e.to_string())?;
        let response = request
            .parse_response(status, &bytes)
            .map_err(|e| e.to_string())?;
        let reports_managed = response
            .binding
            .as_ref()
            .is_some_and(|binding| binding.mode == super::binding_reconciliation::BindingMode::Managed);
        policy.disposition = match (response.code, response.reason) {
            (ReconciliationCode::Completed, _) | (_, Reason::ActivationOutcomeUnknown) => {
                ChannelDisposition::Unknown
            }
            _ if reports_managed => ChannelDisposition::Unknown,
            (_, Reason::LegacyIntakeActive) => ChannelDisposition::Paused {
                origin: Some(PauseOrigin::LegacyIntakeActive),
            },
            _ => ChannelDisposition::LegacyAllowed,
        };
        let settled = policy.disposition.clone();
        self.write_locked(&state)?;
        Ok(settled)
    }

    /// No request left for this channel: a pause that never reached Cloud has
    /// no remote effect, so a blocked update must not keep project claims shut.
    /// A refusal-backed quiet window belongs to a previous submitted attempt.
    pub fn release_unsubmitted(&self, scope: &IntakeScope) -> Result<(), String> {
        let _lock = self.lock()?;
        let mut state = self.read_locked()?;
        if let Some(policy) = state.policies.iter_mut().find(|policy| &policy.scope == scope)
            && matches!(
                policy.disposition,
                ChannelDisposition::Paused { origin: None }
            )
        {
            policy.disposition = ChannelDisposition::LegacyAllowed;
            return self.write_locked(&state);
        }
        Ok(())
    }

    pub(super) fn store_invocation(
        &self,
        invocation: super::binding_repair::CompletedInvocation,
    ) -> Result<(), String> {
        self.persist_completion(invocation.operation_id(), invocation.response(), true)
    }

    #[cfg(test)]
    fn store_completed(
        &self,
        operation_id: Uuid,
        response: &ReconciliationResponse,
    ) -> Result<(), String> {
        self.persist_completion(operation_id, response, false)
    }

    fn persist_completion(
        &self,
        operation_id: Uuid,
        response: &ReconciliationResponse,
        from_http: bool,
    ) -> Result<(), String> {
        let _lock = self.lock()?;
        let mut state = self.read_locked()?;
        let scope = state
            .pauses
            .iter()
            .find(|pause| pause.operation_id == operation_id)
            .ok_or("completion requires its original pause")?
            .scope
            .clone();
        let request = ReconciliationRequest::new(
            &scope.team_id,
            &scope.canonical_project_id,
            &scope.channel_id,
        )
        .map_err(|e| e.to_string())?;
        let bytes = serde_json::to_vec(response).map_err(|e| e.to_string())?;
        let response = request
            .parse_response(200, &bytes)
            .map_err(|e| e.to_string())?;
        if response.code != ReconciliationCode::Completed {
            return Err("only strict evidenced completion permits managed handoff".into());
        }
        let existing = state
            .policies
            .iter()
            .find(|policy| policy.scope == scope)
            .ok_or("completion requires durable submitting state")?;
        match &existing.disposition {
            ChannelDisposition::Unknown | ChannelDisposition::Submitting { .. } => {}
            ChannelDisposition::Managed {
                completion: previous,
                ..
            } => {
                if previous.binding != response.binding
                    || previous.fence_receipt != response.fence_receipt
                {
                    return Err(
                        "immutable managed tuple/fence changed; explicit operator repair required"
                            .into(),
                    );
                }
                if from_http && previous.activation_id == response.activation_id {
                    return Err("ordinary invocation reused an earlier attempt receipt; managed evidence retained".into());
                }
                if previous.as_ref() == &response {
                    return Ok(());
                }
            }
            _ => return Err("completion requires durable submitting state".into()),
        }
        state
            .policies
            .retain(|policy| policy.scope.channel_id != scope.channel_id);
        state.policies.push(ChannelPolicy {
            scope,
            disposition: ChannelDisposition::Managed {
                operation_id,
                completion: Box::new(response),
            },
        });
        state.conservative_unclassified = true;
        self.write_locked(&state)
    }

    /// The project was re-linked to another team or canonical project. A gate
    /// that never reached Cloud (Paused/LegacyAllowed) carries no authority:
    /// archive it beside the journal and start over. A channel Cloud may hold
    /// under the old scope is named, with what to do, instead.
    fn retire_relinked(&self, state: &IntakeState) -> Result<(), String> {
        let held: Vec<String> = state
            .policies
            .iter()
            .filter_map(|policy| {
                let label = match policy.disposition {
                    ChannelDisposition::Paused { .. } | ChannelDisposition::LegacyAllowed => {
                        return None;
                    }
                    ChannelDisposition::Managed { .. } => "managed",
                    ChannelDisposition::Submitting { .. } | ChannelDisposition::Unknown => "uncertain",
                };
                Some(format!("{} ({label})", policy.scope.channel_id))
            })
            .collect();
        if let Some(old) = state.pauses.first().map(|pause| &pause.scope)
            && !held.is_empty()
        {
            return Err(format!(
                "project was re-linked since {} reconciled under team {} / project {}; Cloud may still hold those bindings. Restore that link with `cas cloud team set` and rerun `cas update`, or retire the old bindings in Cloud and move {} aside",
                held.join(", "),
                old.team_id,
                old.canonical_project_id,
                self.path().display()
            ));
        }
        let archive = self.directory.join(format!(
            "state.retired-{}.json",
            chrono::Utc::now().format("%Y%m%dT%H%M%S%.fZ")
        ));
        fs::rename(self.path(), &archive).map_err(|e| e.to_string())?;
        tracing::warn!(archive = %archive.display(), "binding intake journal retired after project re-link");
        Ok(())
    }

    pub fn pause(
        &self,
        scope: IntakeScope,
        operation_id: Uuid,
        expected_generation: u64,
    ) -> Result<PauseRecord, String> {
        scope.validate()?;
        if operation_id.is_nil() {
            return Err("missing intake operation identity".into());
        }
        let _lock = self.lock()?;
        let mut state = self.read_locked()?;
        let mut retired = false;
        if state.pauses.first().is_some_and(|p| {
            p.scope.team_id != scope.team_id
                || p.scope.canonical_project_id != scope.canonical_project_id
        }) {
            self.retire_relinked(&state)?;
            state = IntakeState::default();
            retired = true;
        }
        if let Some(existing) = state.pauses.iter().find(|p| p.operation_id == operation_id) {
            if existing.scope != scope {
                return Err("operation belongs to another scope".into());
            }
            let existing = existing.clone();
            // A released (never submitted) pause is re-armed by its operation.
            if let Some(policy) = state.policies.iter_mut().find(|p| p.scope == scope)
                && policy.disposition == ChannelDisposition::LegacyAllowed
            {
                policy.disposition = ChannelDisposition::Paused { origin: None };
                self.write_locked(&state)?;
            }
            return Ok(existing);
        }
        if !retired && expected_generation != state.generation {
            return Err("binding intake generation changed; reread status".into());
        }
        state.generation = state
            .generation
            .checked_add(1)
            .ok_or("intake generation exhausted")?;
        let pause = PauseRecord {
            operation_id,
            scope,
            generation: state.generation,
        };
        state.pauses.push(pause.clone());
        if let Some(policy) = state.policies.iter_mut().find(|p| p.scope == pause.scope) {
            if matches!(policy.disposition, ChannelDisposition::LegacyAllowed) {
                policy.disposition = ChannelDisposition::Paused { origin: None };
            }
        } else {
            state.policies.push(ChannelPolicy {
                scope: pause.scope.clone(),
                disposition: ChannelDisposition::Paused { origin: None },
            });
        }
        self.write_locked(&state)?;
        Ok(pause)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cas_9021_pause_is_durable_and_corrupt_or_busy_state_never_admits() {
        let temp = tempfile::tempdir().unwrap();
        let journal = IntakeJournal::new(temp.path());
        let scope = IntakeScope {
            team_id: Uuid::new_v4().to_string(),
            canonical_project_id: "project".into(),
            channel_id: "G123".into(),
        };
        assert!(journal.legacy_allowed().unwrap());
        let operation = Uuid::new_v4();
        let pause = journal.pause(scope.clone(), operation, 0).unwrap();
        assert!(
            !IntakeJournal::new(temp.path()).legacy_allowed().unwrap(),
            "a restart must retain the pause"
        );
        assert_eq!(journal.pause(scope.clone(), operation, 0).unwrap(), pause);
        assert!(journal.pause(scope.clone(), Uuid::new_v4(), 0).is_err());
        // A reader holding the lock briefly is waited for, not a failure that
        // strands a settle or release.
        let (held, holding) = std::sync::mpsc::channel();
        let reader = {
            let journal = IntakeJournal::new(temp.path());
            std::thread::spawn(move || {
                let _lock = journal.lock().unwrap();
                held.send(()).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(300));
            })
        };
        holding.recv().unwrap();
        journal.release_unsubmitted(&scope).unwrap();
        reader.join().unwrap();
        assert!(journal.legacy_allowed().unwrap(), "the waited-for release took effect");
        journal.pause(scope.clone(), operation, 0).unwrap();
        let before = fs::read(journal.path()).unwrap();
        {
            let _held = journal.lock().unwrap();
            assert!(journal.legacy_allowed().is_err(), "a lock held past the bound still refuses");
        }
        assert_eq!(fs::read(journal.path()).unwrap(), before);
        for invalid in [
            b"{".as_slice(),
            br#"{"schema_version":2,"generation":0,"pauses":[]}"#,
        ] {
            fs::write(journal.path(), invalid).unwrap();
            assert!(journal.legacy_allowed().is_err());
            assert_eq!(fs::read(journal.path()).unwrap(), invalid);
        }
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn cas_9021_unknown_survives_restart_until_identical_evidenced_completion() {
        let temp = tempfile::tempdir().unwrap();
        let journal = IntakeJournal::new(temp.path());
        let scope = IntakeScope {
            team_id: Uuid::new_v4().to_string(),
            canonical_project_id: "github.com/example/project".into(),
            channel_id: "C0123456789".into(),
        };
        let operation = Uuid::new_v4();
        journal.pause(scope.clone(), operation, 0).unwrap();
        journal.mark_submitting(operation).unwrap().unwrap();
        let restarted = IntakeJournal::new(temp.path());
        assert!(
            matches!(
                restarted
                    .read()
                    .unwrap()
                    .policies
                    .first()
                    .map(|p| &p.disposition),
                Some(ChannelDisposition::Submitting { .. })
            ),
            "an uncertain POST must survive restart"
        );
        assert!(!restarted.legacy_allowed().unwrap());
        let mut body: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/binding_reconciliation/v1/completed.json"
        ))
        .unwrap();
        body["scope"]["team_id"] = scope.team_id.clone().into();
        body["binding"]["team_id"] = scope.team_id.clone().into();
        let request = ReconciliationRequest::new(
            &scope.team_id,
            &scope.canonical_project_id,
            &scope.channel_id,
        )
        .unwrap();
        let completed = request
            .parse_response(200, &serde_json::to_vec(&body).unwrap())
            .unwrap();
        restarted.store_completed(operation, &completed).unwrap();
        let evidence = std::fs::read(restarted.path()).unwrap();
        restarted.mark_submitting(operation).unwrap();
        assert_eq!(
            std::fs::read(restarted.path()).unwrap(),
            evidence,
            "a later retry cannot downgrade prior managed evidence"
        );
        let state = restarted.read().unwrap();
        assert!(!state.channel_legacy_allowed(&scope.channel_id));
        assert!(!state.channel_legacy_allowed("CUNCLASSIFIED"));
        let mut foreign = completed.clone();
        foreign.scope.as_mut().unwrap().workspace_id = "T9999999999".into();
        foreign.binding.as_mut().unwrap().workspace_id = "T9999999999".into();
        request
            .parse_response(200, &serde_json::to_vec(&foreign).unwrap())
            .unwrap();
        assert!(restarted.store_completed(operation, &foreign).is_err());
        assert_eq!(std::fs::read(restarted.path()).unwrap(), evidence);
    }
}

#[cfg(test)]
mod settle_tests {
    use super::*;
    use crate::coordination::binding_reconciliation::ReconciliationRequest;

    /// A definite refusal settles only the attempt that still owns the channel;
    /// a second attempt over an uncertain one can never relax it.
    #[test]
    fn cas_9021_definite_refusal_settles_only_its_own_attempt() {
        let temp = tempfile::tempdir().unwrap();
        let journal = IntakeJournal::new(temp.path());
        let scope = IntakeScope {
            team_id: Uuid::new_v4().to_string(),
            canonical_project_id: "github.com/example/project".into(),
            channel_id: "C0123456789".into(),
        };
        let request =
            ReconciliationRequest::new(&scope.team_id, &scope.canonical_project_id, &scope.channel_id)
                .unwrap();
        let mut body: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/binding_reconciliation/v1/legacy-intake-active.json"
        ))
        .unwrap();
        body["scope"]["team_id"] = scope.team_id.clone().into();
        body["binding"]["team_id"] = scope.team_id.clone().into();
        let legacy_active = request.parse_response(409, &serde_json::to_vec(&body).unwrap()).unwrap();
        for (key, value) in [("code", "channel_unavailable"), ("reason", "not_member"), ("next_action", "invite_violet")] {
            body[key] = value.into();
        }
        for key in ["binding", "activation_id", "latest_attempt", "blockers", "retry_after_secs"] {
            body[key] = serde_json::Value::Null;
        }
        let not_member = request.parse_response(409, &serde_json::to_vec(&body).unwrap()).unwrap();
        let disposition = || journal.read().unwrap().policies[0].disposition.clone();

        let operation = Uuid::new_v4();
        let attempt = || journal.mark_submitting(operation).unwrap().unwrap();
        journal.pause(scope.clone(), operation, 0).unwrap();
        let first = attempt();
        assert_eq!(
            journal.settle_refusal(operation, first, &legacy_active).unwrap(),
            ChannelDisposition::Paused {
                origin: Some(PauseOrigin::LegacyIntakeActive),
            }
        );
        assert!(!journal.legacy_allowed().unwrap(), "the host must go quiet before a retry");
        let second = attempt();
        assert_eq!(journal.settle_refusal(operation, second, &not_member).unwrap(), ChannelDisposition::LegacyAllowed);
        assert!(journal.legacy_allowed().unwrap(), "a definite refusal had no effect");

        journal.pause(scope.clone(), operation, 1).unwrap();
        assert_eq!(
            disposition(),
            ChannelDisposition::Paused { origin: None },
            "the same operation re-arms its pause"
        );
        let overtaken = attempt();
        let latest = attempt();
        assert!(
            matches!(journal.settle_refusal(operation, overtaken, &not_member).unwrap(), ChannelDisposition::Submitting { attempt_id, .. } if attempt_id == latest),
            "a superseded attempt cannot settle the channel"
        );
        journal.release_unsubmitted(&scope).unwrap();
        assert!(!journal.legacy_allowed().unwrap(), "a submitted attempt is never released");
    }
}
