//! Portable, daemon-admitted proof. Mailbox prompts carry no close authority.
use super::*;
use crate::cloud::peer_mailbox::{ClaimedPeerMessage, PeerScope, PeerSend};
use crate::cloud::peers::{PeerDiscovery, discover_peers};
use cas_types::{
    Agent, AgentRole, PeerPremergeAttestation, PeerPremergeReceipt, Verification,
    VerificationDispatchState, VerificationProvenance,
};

const TAG: &str = "CAS_PEER_PREMERGE_V1 ";

impl Approval {
    /// Publication is already confirmed when this is called. Retrying a
    /// publication reuses the durable outbox's exact logical-send identity.
    pub(super) fn share_published(&self) {
        if let Err(error) = self.try_share_published() {
            tracing::warn!(task = %self.candidate.task_id, %error, "peer approval sharing failed after publication");
            let note = format!(
                "Peer approval sharing failed after confirmed publication: {error}. Publication remains confirmed; other peers were attempted and the outbox retains retryable sends."
            );
            let recorded = (|| -> Result<(), String> {
                let tasks = crate::store::open_task_store(&self.candidate.cas_root)
                    .map_err(|error| error.to_string())?;
                let task = tasks
                    .get(&self.candidate.task_id)
                    .map_err(|error| error.to_string())?;
                if !task.notes.contains(&note) {
                    tasks
                        .append_note(&task.id, &note)
                        .map_err(|error| error.to_string())?;
                }
                Ok(())
            })();
            if let Err(cause) = recorded {
                tracing::warn!(task = %self.candidate.task_id, %cause, "could not record peer sharing failure");
            }
        }
    }

    fn try_share_published(&self) -> Result<(), String> {
        let root = &self.candidate.cas_root;
        if !crate::cloud::CloudConfig::load_from_cas_dir(root)
            .map_err(|error| error.to_string())?
            .is_logged_in()
        {
            return Ok(());
        }
        let current = cas_store::get_latest_verification_dispatch(root, &self.candidate.task_id)
            .map_err(|error| error.to_string())?
            .ok_or("published dispatch missing")?;
        if current.id != self.dispatch_id || current.state != VerificationDispatchState::Resolved {
            return Err("published approval is no longer the current proof cycle".into());
        }
        let verdict = cas_store::get_verification_for_dispatch(root, &self.dispatch_id)
            .map_err(|error| error.to_string())?
            .ok_or("published verdict missing")?;
        if verdict.id != self.verdict_id
            || verdict.status != VerificationStatus::Approved
            || !matches!(
                verdict.provenance,
                VerificationProvenance::SupervisorDirect | VerificationProvenance::TaskVerifier
            )
        {
            return Err("only an original approved verdict can be shared".into());
        }
        let sender = verdict
            .issuer_agent_id
            .as_deref()
            .ok_or("verifying supervisor missing")?;
        let agents = crate::store::open_agent_store(root).map_err(|error| error.to_string())?;
        if agents.get(sender).map_err(|error| error.to_string())?.role != AgentRole::Supervisor {
            return Err("peer proof sender is not a registered verifying supervisor".into());
        }
        let PeerDiscovery::Found {
            canonical_id,
            peers,
            ..
        } = discover_peers(root, Some(sender)).map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        let record = self.promotion()?.ok_or("published mapping missing")?;
        let task = crate::store::open_task_store(root)
            .map_err(|error| error.to_string())?
            .get(&self.candidate.task_id)
            .map_err(|error| error.to_string())?;
        if context_digest(root, &task)? != self.candidate.context_digest {
            return Err("published approval context changed before peer attestation".into());
        }
        let receipt = PeerPremergeReceipt {
            project_id: canonical_id.clone(),
            supervisor_id: sender.into(),
            task_id: task.id.clone(),
            source_branch: self.candidate.source_branch.clone(),
            source_sha: self.candidate.source_sha.clone(),
            target_branch: self.candidate.target_branch.clone(),
            target_sha: self.candidate.target_sha.clone(),
            candidate_tree: self.candidate.tree.clone(),
            landed_sha: record.landed_sha,
            dispatch_id: self.dispatch_id.clone(),
            verdict_id: self.verdict_id.clone(),
            status: verdict.status,
            plan_digest: portable_plan_digest(root, &task)?,
        };
        let body = format!(
            "{TAG}{}",
            serde_json::to_string(&receipt).map_err(|error| error.to_string())?
        );
        let mut errors = Vec::new();
        for peer in peers
            .iter()
            .filter(|peer| peer.live && peer.session.is_some())
        {
            if let Err(error) = crate::cloud::peer_mailbox::send_premerge_receipt(
                root,
                PeerSend {
                    project_id: canonical_id.clone(),
                    sender_agent_id: sender.into(),
                    recipient_agent_id: peer.agent_id.clone(),
                    body: body.clone(),
                    summary: Some(format!("Published approval for {}", task.id)),
                    in_reply_to: None,
                    dedupe_key: format!("premerge:{}:{}", self.dispatch_id, self.verdict_id),
                    scope: PeerScope::Project,
                    recipient_project_id: None,
                },
                &format!("{}@{}", peer.name, peer.machine),
            ) {
                errors.push(format!("{}@{}: {error}", peer.name, peer.machine));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

#[derive(Debug)]
pub(crate) enum PeerReceiptIngested {
    NotAReceipt,
    Imported,
    Rejected(String),
}

#[derive(Debug)]
enum ReceiptFailure {
    Rejected(String),
    Retry(String),
}
impl ReceiptFailure {
    fn reject(cause: impl ToString) -> Self {
        Self::Rejected(cause.to_string())
    }
    fn retry(cause: impl ToString) -> Self {
        Self::Retry(cause.to_string())
    }
    fn cause(self) -> String {
        match self {
            Self::Rejected(cause) | Self::Retry(cause) => cause,
        }
    }
}

fn validate_receipt(
    root: &Path,
    task: &Task,
    repo: &Path,
    receipt: &PeerPremergeReceipt,
) -> Result<RepositoryProofBoundary, ReceiptFailure> {
    let canonical = crate::cloud::resolve_canonical_id(root)
        .ok_or_else(|| ReceiptFailure::retry("project identity missing"))?;
    if receipt.status != VerificationStatus::Approved {
        return Err(ReceiptFailure::reject(
            "peer receipt is not an approved verdict",
        ));
    }
    if !crate::cloud::project_ids_match_with_aliases(
        &receipt.project_id,
        &canonical,
        &crate::cloud::project_aliases_from_config_toml(root),
    ) || receipt.task_id != task.id
        || task.is_terminal()
        || task.deliverables.factory_branch_anchor.as_deref() != Some(receipt.source_sha.as_str())
        || task.deliverables.parked_branch.as_deref() != Some(receipt.source_branch.as_str())
        || portable_plan_digest(root, task).map_err(ReceiptFailure::retry)? != receipt.plan_digest
        || task
            .deliverables
            .work_target
            .as_ref()
            .is_none_or(|target| target.target_branch != receipt.target_branch)
    {
        return Err(ReceiptFailure::reject(
            "peer approval does not match the current task and delivery",
        ));
    }
    for sha in [
        &receipt.source_sha,
        &receipt.target_sha,
        &receipt.candidate_tree,
        &receipt.landed_sha,
    ] {
        if sha.len() != 40 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ReceiptFailure::reject(
                "peer approval has an invalid Git object identity",
            ));
        }
    }
    if receipt.dispatch_id.is_empty()
        || receipt.verdict_id.is_empty()
        || receipt.supervisor_id.is_empty()
    {
        return Err(ReceiptFailure::reject("peer approval identity missing"));
    }
    // Fetch is the retryable boundary. Once the confirmed target is available,
    // an invalid object, candidate or landing is a permanent proof refusal.
    let operations = crate::worktree::GitOperations::new(repo.to_path_buf());
    let target = operations
        .fetch_merge_target(&receipt.target_branch)
        .map_err(ReceiptFailure::retry)?
        .ok_or_else(|| ReceiptFailure::retry("peer publication requires a remote target"))?;
    let merged = git(
        repo,
        &[
            "merge-tree",
            "--write-tree",
            &receipt.target_sha,
            &receipt.source_sha,
        ],
    )
    .map_err(ReceiptFailure::reject)?;
    if merged.lines().next() != Some(receipt.candidate_tree.as_str()) {
        return Err(ReceiptFailure::reject(
            "peer candidate tree does not match the source and reviewed base",
        ));
    }
    let proof = repository_proof::capture_repository_proof_at_target(
        repo,
        &receipt.landed_sha,
        vec![receipt.source_sha.clone()],
    )
    .map_err(ReceiptFailure::reject)?;
    let candidate = Candidate {
        cas_root: root.into(),
        task_id: task.id.clone(),
        repository: repo.into(),
        source_branch: receipt.source_branch.clone(),
        source_sha: receipt.source_sha.clone(),
        target_branch: receipt.target_branch.clone(),
        target_sha: receipt.target_sha.clone(),
        tree: receipt.candidate_tree.clone(),
        context_digest: String::new(),
        proof: proof.clone(),
        preparation_ms: 0,
    };
    candidate
        .verify_landing(&receipt.landed_sha)
        .map_err(ReceiptFailure::reject)?;
    git(
        repo,
        &["merge-base", "--is-ancestor", &receipt.landed_sha, &target],
    )
    .map_err(ReceiptFailure::reject)?;
    repository_proof::verify_repository_proof(&proof).map_err(ReceiptFailure::reject)?;
    Ok(proof)
}

/// Only authenticated daemon admission can mint peer authority. Permanent
/// refusals are explicit terminal outcomes; only availability errors retry.
pub(crate) fn ingest_peer_receipt(
    root: &Path,
    recipient: &Agent,
    message: &ClaimedPeerMessage,
) -> Result<PeerReceiptIngested, String> {
    let Some(body) = message.body.strip_prefix(TAG) else {
        return Ok(PeerReceiptIngested::NotAReceipt);
    };
    match import_receipt(root, recipient, message, body) {
        Ok(()) => Ok(PeerReceiptIngested::Imported),
        Err(ReceiptFailure::Rejected(why)) => Ok(PeerReceiptIngested::Rejected(why)),
        Err(ReceiptFailure::Retry(error)) => Err(error),
    }
}

fn import_receipt(
    root: &Path,
    recipient: &Agent,
    message: &ClaimedPeerMessage,
    body: &str,
) -> Result<(), ReceiptFailure> {
    let receipt: PeerPremergeReceipt =
        serde_json::from_str(body).map_err(ReceiptFailure::reject)?;
    if receipt.status != VerificationStatus::Approved {
        return Err(ReceiptFailure::reject(
            "peer receipt is not an approved verdict",
        ));
    }
    if recipient.role != AgentRole::Supervisor
        || message.scope != PeerScope::Project
        || message.sender_agent_id != receipt.supervisor_id
        || message.id.is_empty()
        || message
            .sender_machine_id
            .as_deref()
            .is_none_or(str::is_empty)
        || message.sender_session.as_deref().is_none_or(str::is_empty)
    {
        return Err(ReceiptFailure::reject(
            "unauthorized peer pre-merge envelope",
        ));
    }
    let at = chrono::DateTime::parse_from_rfc3339(&message.created_at)
        .map_err(ReceiptFailure::reject)?
        .with_timezone(&chrono::Utc);
    let canonical = crate::cloud::resolve_canonical_id(root)
        .ok_or_else(|| ReceiptFailure::retry("project identity missing"))?;
    if !crate::cloud::project_ids_match_with_aliases(
        &message.project_id,
        &canonical,
        &crate::cloud::project_aliases_from_config_toml(root),
    ) {
        return Err(ReceiptFailure::reject(
            "peer proof came from a different project",
        ));
    }
    let task = crate::store::open_task_store(root)
        .map_err(ReceiptFailure::retry)?
        .get(&receipt.task_id)
        .map_err(ReceiptFailure::retry)?;
    if task.is_terminal() {
        return Err(ReceiptFailure::reject(
            "peer approval task is already complete",
        ));
    }
    // A newer local cycle or an existing local verdict permanently owns this
    // slot. Do not keep an old receipt leased while waiting on the network.
    let existing = cas_store::get_latest_verification_dispatch(root, &task.id)
        .map_err(ReceiptFailure::retry)?;
    if let Some(existing) = existing {
        let same = existing.state == VerificationDispatchState::Resolved
            && existing
                .repository
                .as_ref()
                .and_then(|proof| proof.peer_premerge.as_ref())
                .is_some_and(|attestation| {
                    attestation.receipt == receipt
                        && attestation.message_id == message.id
                        && Some(&attestation.sender_machine_id)
                            == message.sender_machine_id.as_ref()
                        && Some(&attestation.sender_session) == message.sender_session.as_ref()
                });
        if !same {
            return Err(ReceiptFailure::reject(
                "an existing local verification cycle owns this task",
            ));
        }
    } else if crate::store::open_verification_store(root)
        .map_err(ReceiptFailure::retry)?
        .get_latest_for_task(&task.id)
        .map_err(ReceiptFailure::retry)?
        .is_some()
    {
        return Err(ReceiptFailure::reject(
            "an existing local verdict owns this task",
        ));
    }
    let PeerDiscovery::Found { peers, ended, .. } =
        discover_peers(root, Some(&recipient.id)).map_err(ReceiptFailure::retry)?
    else {
        return Err(ReceiptFailure::retry(
            "peer supervisor identity unavailable",
        ));
    };
    if !peers.iter().chain(&ended).any(|peer| {
        peer.agent_id == message.sender_agent_id
            && peer.machine_id == message.sender_machine_id
            && peer.session == message.sender_session
    }) {
        return Err(ReceiptFailure::reject(
            "peer proof sender is not an authenticated project supervisor",
        ));
    }
    let repo = root
        .parent()
        .ok_or_else(|| ReceiptFailure::retry("repository root missing"))?;
    let mut proof = validate_receipt(root, &task, repo, &receipt)?;
    proof.peer_premerge = Some(PeerPremergeAttestation {
        receipt: receipt.clone(),
        message_id: message.id.clone(),
        sender_machine_id: message.sender_machine_id.clone().unwrap(),
        sender_session: message.sender_session.clone().unwrap(),
    });
    let dispatch = VerificationDispatch {
        id: receipt.dispatch_id.clone(),
        task_id: task.id.clone(),
        receipt_id: None,
        delivery_transaction_id: None,
        repository: Some(proof),
        requester_agent_id: receipt.supervisor_id.clone(),
        owner_agent_id: recipient.id.clone(),
        verifier_agent_id: None,
        capability_id: None,
        state: VerificationDispatchState::Resolved,
        requested_at: at,
        deadline_at: at,
        resolved_at: Some(at),
        recovery_action: Default::default(),
    };
    let mut verdict = Verification::new(receipt.verdict_id.clone(), task.id.clone());
    verdict.status = receipt.status;
    verdict.provenance = VerificationProvenance::PeerAttested;
    verdict.dispatch_id = Some(dispatch.id.clone());
    verdict.issuer_agent_id = Some(receipt.supervisor_id.clone());
    verdict.agent_id = Some(receipt.supervisor_id);
    verdict.created_at = at;
    verdict.summary = format!("Peer-attested published approval via {}", message.id);
    cas_store::import_peer_premerge_verification(root, &task, &dispatch, &verdict).map_err(
        |error| match error {
            cas_store::StoreError::Parse(_)
            | cas_store::StoreError::VerificationDispatchConflict { .. } => {
                ReceiptFailure::reject(error)
            }
            _ => ReceiptFailure::retry(error),
        },
    )?;
    Ok(())
}

pub(super) fn verify_peer_promotion(
    root: &Path,
    task: &Task,
    dispatch: &VerificationDispatch,
    verdict_id: &str,
) -> Result<bool, String> {
    let Some(proof) = dispatch.repository.as_ref() else {
        return Ok(false);
    };
    let Some(attestation) = proof.peer_premerge.as_ref() else {
        return Ok(false);
    };
    let receipt = &attestation.receipt;
    let verdict = cas_store::get_verification_for_dispatch(root, &dispatch.id)
        .map_err(|error| error.to_string())?
        .ok_or("peer verdict missing")?;
    if dispatch.id != receipt.dispatch_id
        || dispatch.task_id != task.id
        || dispatch.state != VerificationDispatchState::Resolved
        || verdict.id != verdict_id
        || verdict.id != receipt.verdict_id
        || verdict.task_id != task.id
        || verdict.status != VerificationStatus::Approved
        || verdict.provenance != VerificationProvenance::PeerAttested
        || verdict.issuer_agent_id.as_deref() != Some(receipt.supervisor_id.as_str())
        || attestation.message_id.is_empty()
        || attestation.sender_machine_id.is_empty()
        || attestation.sender_session.is_empty()
    {
        return Err("peer verdict does not match its admitted proof".into());
    }
    let current = validate_receipt(root, task, Path::new(&proof.repository_root), receipt)
        .map_err(ReceiptFailure::cause)?;
    if current.head_commit != proof.head_commit
        || current.state_digest != proof.state_digest
        || current.anchor_commits != proof.anchor_commits
    {
        return Err("peer approval repository binding changed".into());
    }
    Ok(true)
}
