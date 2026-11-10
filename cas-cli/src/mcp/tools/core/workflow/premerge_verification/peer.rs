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
                "Peer approval sharing failed after confirmed publication: {error}. Publication remains confirmed. Eligible peers are attempted independently; retryable sends remain in the outbox."
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
        inherited_supervisor_call: None,
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
    // Existing dispatch ownership is checked atomically after authenticating
    // and validating this receipt. A stale admitted peer approval may be
    // superseded by its source's fresh review, never by rebinding old proof.
    let existing = cas_store::get_latest_verification_dispatch(root, &task.id)
        .map_err(ReceiptFailure::retry)?;
    if existing.as_ref().is_some_and(|dispatch| {
        dispatch.repository.as_ref().and_then(|proof| proof.peer_premerge.as_ref()).is_none()
            || !matches!(dispatch.state, VerificationDispatchState::Resolved | VerificationDispatchState::Invalidated)
    }) {
        return Err(ReceiptFailure::reject("an existing local verification cycle owns this task"));
    }
    if existing.is_none() && crate::store::open_verification_store(root)
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
    // Bind the current plan when admitting new authority. After admission,
    // release bookkeeping can change it without undoing the published review,
    // just as for a local promotion. Close still rechecks the task and Git proof.
    if portable_plan_digest(root, &task).map_err(ReceiptFailure::retry)? != receipt.plan_digest {
        return Err(ReceiptFailure::reject(
            "peer approval does not match the current task and delivery",
        ));
    }
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
        |error| {
            // Ownership conflicts are permanent. A store Parse here can be
            // its updated_at compare losing a race;
            // retry so the next pass can classify the now-current local cycle.
            let permanent = matches!(&error, cas_store::StoreError::VerificationDispatchConflict { .. })
                || matches!(&error, cas_store::StoreError::Database(rusqlite::Error::SqliteFailure(cause, _))
                    if cause.code == rusqlite::ErrorCode::ConstraintViolation);
            if permanent { ReceiptFailure::reject(error) } else { ReceiptFailure::retry(error) }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn repaired_delivery_replaces_only_superseded_source_peer_approval_cas_d7f9() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        const PROJECT: &str = "peer-repair-fixture";
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/api/agents"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"agents":[{
                "id":"source-supervisor", "name":"source", "agent_type":"primary", "status":"active",
                "machine_id":"source-machine", "last_heartbeat":chrono::Utc::now().to_rfc3339(), "active_tasks":0,
                "metadata":{"role":"supervisor", "canonical_id":PROJECT, "factory_session":"source-session"}
            }, {
                "id":"other-supervisor", "name":"other", "agent_type":"primary", "status":"active",
                "machine_id":"other-machine", "last_heartbeat":chrono::Utc::now().to_rfc3339(), "active_tasks":0,
                "metadata":{"role":"supervisor", "canonical_id":PROJECT, "factory_session":"other-session"}
            }]}))).mount(&server).await;
        let endpoint = server.uri();
        tokio::task::spawn_blocking(move || {
            let _env = crate::test_support::TestEnvGuard::temp_home();
            let home = _env.home();
            let origin = home.join("origin.git");
            let source = home.join("source");
            let receiver = home.join("receiver");
            git(home, &["init", "--bare", "-b", "main", origin.to_str().unwrap()]).unwrap();
            git(home, &["init", "-b", "main", source.to_str().unwrap()]).unwrap();
            git(&source, &["config", "user.name", "Review fixture"]).unwrap();
            git(&source, &["config", "user.email", "review@example.test"]).unwrap();
            git(&source, &["remote", "add", "origin", origin.to_str().unwrap()]).unwrap();
            std::fs::write(source.join(".gitignore"), ".cas/\n").unwrap();
            git(&source, &["add", "."]).unwrap();
            git(&source, &["commit", "-qm", "base"]).unwrap();
            let base = git(&source, &["rev-parse", "HEAD"]).unwrap();
            git(&source, &["switch", "-c", "factory/peer"]).unwrap();
            std::fs::write(source.join("delivery"), "old\n").unwrap();
            git(&source, &["add", "."]).unwrap();
            git(&source, &["commit", "-qm", "delivery"]).unwrap();
            let old_sha = git(&source, &["rev-parse", "HEAD"]).unwrap();
            git(&source, &["push", "origin", "HEAD:main"]).unwrap();
            git(home, &["clone", "--branch", "main", origin.to_str().unwrap(), receiver.to_str().unwrap()]).unwrap();
            let root = crate::store::init_cas_dir(&receiver).unwrap();
            crate::cloud::set_canonical_id_in_config_toml(&root, PROJECT).unwrap();
            crate::cloud::CloudConfig { endpoint, token: Some("fixture-token".into()), ..Default::default() }
                .save_to_cas_dir(&root).unwrap();
            let recipient = Agent::new_with_role("receiver-supervisor".into(), "receiver".into(), AgentRole::Supervisor);
            crate::store::open_agent_store(&root).unwrap().register(&recipient).unwrap();
            let tasks = crate::store::open_task_store_local(&root).unwrap();
            let mut task = Task::new("cas-d7f9-repair".into(), "Reviewed repair".into());
            task.status = cas_types::TaskStatus::AwaitingMerge;
            task.deliverables.parked_branch = Some("factory/peer".into());
            task.deliverables.factory_branch_anchor = Some(old_sha.clone());
            task.deliverables.work_target = Some(cas_types::WorkTarget {
                repo_selector: format!("project:{PROJECT}"), target_branch: "main".into(),
            });
            tasks.add(&task).unwrap();
            let receipt = |task: &Task, sha: &str, target: &str, cycle: &str| PeerPremergeReceipt {
                status: VerificationStatus::Approved, project_id: PROJECT.into(),
                supervisor_id: "source-supervisor".into(), task_id: task.id.clone(),
                source_branch: "factory/peer".into(), source_sha: sha.into(), target_branch: "main".into(),
                target_sha: target.into(), candidate_tree: git(&source, &["rev-parse", &format!("{sha}^{{tree}}")]).unwrap(),
                landed_sha: sha.into(), dispatch_id: format!("dispatch-{cycle}"), verdict_id: format!("ver-{cycle}"),
                plan_digest: portable_plan_digest(&root, task).unwrap(),
            };
            let message = |receipt: &PeerPremergeReceipt, age: i64| ClaimedPeerMessage {
                id: format!("message-{}", receipt.dispatch_id), project_id: PROJECT.into(),
                recipient_agent_id: recipient.id.clone(), sender_agent_id: "source-supervisor".into(),
                sender_name: "source".into(), sender_machine_id: Some("source-machine".into()),
                sender_session: Some("source-session".into()), sender_label: None,
                body: format!("{TAG}{}", serde_json::to_string(receipt).unwrap()), summary: None, in_reply_to: None,
                created_at: (chrono::Utc::now() - chrono::Duration::minutes(age)).to_rfc3339(),
                attempts: 1, scope: PeerScope::Project,
            };
            let old = receipt(&task, &old_sha, &base, "old");
            let old_message = message(&old, 2);
            assert!(matches!(ingest_peer_receipt(&root, &recipient, &old_message).unwrap(), PeerReceiptIngested::Imported));
            let old_dispatch = cas_store::get_latest_verification_dispatch(&root, &task.id).unwrap().unwrap();
            assert!(verify_peer_promotion(&root, &task, &old_dispatch, &old.verdict_id).unwrap());

            std::fs::write(source.join("delivery"), "repaired\n").unwrap();
            git(&source, &["commit", "-qam", "reviewed repair"]).unwrap();
            let repaired = git(&source, &["rev-parse", "HEAD"]).unwrap();
            git(&source, &["push", "origin", "HEAD:main"]).unwrap();
            task.deliverables.factory_branch_anchor = Some(repaired.clone());
            tasks.update(&task).unwrap();
            assert!(verify_peer_promotion(&root, &task, &old_dispatch, &old.verdict_id).is_err());
            let fresh = receipt(&task, &repaired, &old_sha, "repair");
            let fresh_message = message(&fresh, 1);
            // A source approval is required; an authenticated rejected verdict
            // and an envelope whose sender identity changed grant no recovery.
            let mut rejected = fresh.clone();
            rejected.status = VerificationStatus::Rejected;
            let mut foreign = fresh_message.clone();
            foreign.sender_machine_id = Some("foreign-machine".into());
            let mut rebound = fresh.clone();
            rebound.dispatch_id = old.dispatch_id.clone();
            rebound.verdict_id = old.verdict_id.clone();
            let mut other = fresh.clone();
            other.supervisor_id = "other-supervisor".into();
            let mut other_message = message(&other, 1);
            other_message.sender_agent_id = other.supervisor_id.clone();
            other_message.sender_machine_id = Some("other-machine".into());
            other_message.sender_session = Some("other-session".into());
            for invalid in [message(&rejected, 1), foreign, old_message.clone(), message(&rebound, 1), other_message] {
                assert!(matches!(ingest_peer_receipt(&root, &recipient, &invalid).unwrap(), PeerReceiptIngested::Rejected(_)));
                assert_eq!(cas_store::get_latest_verification_dispatch(&root, &task.id).unwrap().unwrap().id, old.dispatch_id);
            }
            let admitted = ingest_peer_receipt(&root, &recipient, &fresh_message).unwrap();
            assert!(matches!(admitted, PeerReceiptIngested::Imported), "{admitted:?}");
            let current = cas_store::get_latest_verification_dispatch(&root, &task.id).unwrap().unwrap();
            assert_eq!(current.id, fresh.dispatch_id);
            assert!(verify_peer_promotion(&root, &task, &current, &fresh.verdict_id).unwrap());
            assert_eq!(cas_store::get_verification_dispatch(&root, &old.dispatch_id).unwrap().state, VerificationDispatchState::Invalidated);
            assert!(matches!(ingest_peer_receipt(&root, &recipient, &old_message).unwrap(), PeerReceiptIngested::Rejected(_)));
            assert!(matches!(ingest_peer_receipt(&root, &recipient, &fresh_message).unwrap(), PeerReceiptIngested::Imported));
            let events = crate::store::open_event_store(&root).unwrap()
                .list_for_entity(cas_types::EventEntityType::Task, &task.id, 100).unwrap();
            let replacements: Vec<_> = events.iter().filter_map(|event| event.metadata.as_ref())
                .filter(|metadata| metadata["code"] == "peer_premerge_approval_replaced").collect();
            assert_eq!(replacements.len(), 1);
            assert_eq!(replacements[0]["old_dispatch_id"], old.dispatch_id);
            assert_eq!(replacements[0]["new_dispatch_id"], fresh.dispatch_id);
            assert_eq!(cas_store::get_verification_for_dispatch(&root, &old.dispatch_id).unwrap().unwrap().id, old.verdict_id);
            assert_eq!(cas_store::get_verification_dispatch(&root, &old.dispatch_id).unwrap()
                .repository.unwrap().peer_premerge.unwrap().receipt.source_sha, old_sha);
            let local = cas_store::create_verification_dispatch(&root, &task.id, &recipient.id, &recipient.id,
                chrono::Utc::now() + chrono::Duration::minutes(10)).unwrap();
            assert!(matches!(ingest_peer_receipt(&root, &recipient, &fresh_message).unwrap(), PeerReceiptIngested::Rejected(_)));
            assert_eq!(cas_store::get_latest_verification_dispatch(&root, &task.id).unwrap().unwrap().id, local.id);
        }).await.unwrap();
    }
}
