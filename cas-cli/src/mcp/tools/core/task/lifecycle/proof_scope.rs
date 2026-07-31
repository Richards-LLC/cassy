//! Immutable task scope while an exact verification/delivery proof is active.
//!
//! Notes are an append-only progress surface and intentionally remain writable.
//! Every other public task-update field can change what is being reviewed, how
//! it closes, or who owns/routes it, so it is rejected until the exact proof
//! cycle reaches a terminal delivery state.

use std::path::Path;

use cas_types::{Task, TaskStatus, VerificationDispatchState, WorkerDeliveryState};

use crate::mcp::tools::TaskUpdateRequest;

#[derive(Debug, Clone, Copy)]
pub(crate) enum ProofScopeOperation<'a> {
    CompletionReceipt,
    TaskUpdate {
        request: &'a TaskUpdateRequest,
        target_repo_supplied: bool,
        target_branch_supplied: bool,
    },
}

impl ProofScopeOperation<'_> {
    fn locked_fields(self) -> Vec<&'static str> {
        let Self::TaskUpdate {
            request,
            target_repo_supplied,
            target_branch_supplied,
        } = self
        else {
            return Vec::new();
        };
        let mut fields = Vec::new();
        macro_rules! supplied {
            ($field:ident) => {
                if request.$field.is_some() {
                    fields.push(stringify!($field));
                }
            };
        }
        supplied!(title);
        supplied!(priority);
        supplied!(labels);
        supplied!(description);
        supplied!(design);
        supplied!(acceptance_criteria);
        supplied!(demo_statement);
        supplied!(execution_note);
        supplied!(external_ref);
        supplied!(assignee);
        supplied!(status);
        supplied!(epic);
        supplied!(epic_verification_owner);
        supplied!(depth);
        if target_repo_supplied {
            fields.push("target_repo");
        }
        if target_branch_supplied {
            fields.push("target_branch");
        }
        fields
    }
}

fn delivery_state_locks_scope(state: WorkerDeliveryState) -> bool {
    matches!(
        state,
        WorkerDeliveryState::AwaitingVerification
            | WorkerDeliveryState::AwaitingMerge
            | WorkerDeliveryState::MergeAuthorized
            | WorkerDeliveryState::Merged
            | WorkerDeliveryState::CloseReady
    )
}

fn exact_proof_locks_scope(cas_root: &Path, task: &Task) -> Result<bool, String> {
    // The task projection is itself authoritative enough to freeze semantic
    // mutation. Missing/corrupt backing proof must not turn PSR into a writable
    // legacy escape hatch.
    if matches!(
        task.status,
        TaskStatus::PendingSupervisorReview | TaskStatus::AwaitingMerge
    ) {
        return Ok(true);
    }

    let delivery = cas_store::get_latest_worker_delivery(cas_root, &task.id)
        .map_err(|error| format!("exact delivery lookup failed: {error}"))?;
    let dispatch = cas_store::get_latest_verification_dispatch(cas_root, &task.id)
        .map_err(|error| format!("exact dispatch lookup failed: {error}"))?;
    let Some(dispatch) = dispatch else {
        return match delivery {
            Some((_, transaction)) if delivery_state_locks_scope(transaction.state) => {
                Err("active delivery transaction has no exact verification dispatch".to_string())
            }
            _ => Ok(false),
        };
    };
    match (
        dispatch.receipt_id.as_deref(),
        dispatch.delivery_transaction_id.as_deref(),
    ) {
        (None, None) => {
            if let Some((_, transaction)) = delivery
                && delivery_state_locks_scope(transaction.state)
            {
                return Err(
                    "active delivery transaction is not bound to the exact verification dispatch"
                        .to_string(),
                );
            }
            Ok(matches!(
                dispatch.state,
                VerificationDispatchState::Pending
                    | VerificationDispatchState::Claimed
                    | VerificationDispatchState::TimedOut
            ))
        }
        (Some(receipt_id), Some(transaction_id)) => {
            let Some((receipt, transaction)) =
                cas_store::get_worker_delivery_by_receipt(cas_root, receipt_id)
                    .map_err(|error| format!("exact delivery lookup failed: {error}"))?
            else {
                return Err(
                    "exact dispatch names a delivery receipt that is unavailable".to_string(),
                );
            };
            if receipt.task_id != task.id
                || transaction.task_id != task.id
                || transaction.receipt_id != receipt_id
                || transaction.id != transaction_id
            {
                return Err(
                    "exact dispatch and delivery receipt/transaction boundary disagree".to_string(),
                );
            }
            if let Some((latest_receipt, latest_transaction)) = delivery
                && (latest_receipt.id != receipt.id || latest_transaction.id != transaction.id)
                && delivery_state_locks_scope(latest_transaction.state)
            {
                return Err(
                    "latest active delivery transaction is not the exact dispatched proof"
                        .to_string(),
                );
            }
            Ok(delivery_state_locks_scope(transaction.state))
        }
        _ => Err(
            "exact dispatch has a partial delivery proof boundary and is not safe to mutate"
                .to_string(),
        ),
    }
}

/// Reject mutations that could change a terminal task or an active exact proof.
///
/// This guard must run immediately after loading the task, before receipt
/// parsing, repository resolution, hook execution, dependency writes, or task
/// projection updates.
pub(crate) fn guard_task_proof_scope(
    cas_root: &Path,
    task: &Task,
    operation: ProofScopeOperation<'_>,
) -> Result<(), String> {
    if matches!(operation, ProofScopeOperation::CompletionReceipt)
        && super::stale_close_guard::is_terminal_closed(task.status)
    {
        return Err(format!(
            "DELIVERY RECEIPT REJECTED: task {} is already Closed. A terminal task cannot accept or replay a completion receipt; task, delivery, dispatch, verdict, and event state were left unchanged.",
            task.id
        ));
    }

    let locked_fields = operation.locked_fields();
    if locked_fields.is_empty() {
        return Ok(());
    }
    match exact_proof_locks_scope(cas_root, task) {
        Ok(false) => Ok(()),
        Ok(true) => Err(format!(
            "DELIVERY PROOF SCOPE LOCKED: task {} has an active exact verification/delivery proof boundary. Refusing review-relevant update fields [{}]. Append progress with notes only; complete or explicitly recover the exact proof cycle before changing task scope.",
            task.id,
            locked_fields.join(", ")
        )),
        Err(reason) => Err(format!(
            "DELIVERY PROOF SCOPE LOCKED: task {} exact proof state is inconsistent ({reason}). Refusing review-relevant update fields [{}] rather than reusing or invalidating ambiguous proof.",
            task.id,
            locked_fields.join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_types::{VerificationProofBoundary, WorkerCompletionReceiptInput};
    use tempfile::TempDir;

    fn empty_update() -> TaskUpdateRequest {
        TaskUpdateRequest {
            id: "cas-scope".to_string(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: None,
            status: None,
            epic: None,
            epic_verification_owner: None,
            depth: None,
        }
    }

    #[test]
    fn notes_are_the_only_update_field_exempt_from_scope_locking() {
        let mut notes = empty_update();
        notes.notes = Some("progress".to_string());
        assert!(
            ProofScopeOperation::TaskUpdate {
                request: &notes,
                target_repo_supplied: false,
                target_branch_supplied: false,
            }
            .locked_fields()
            .is_empty()
        );

        let mut all = empty_update();
        all.title = Some("title".into());
        all.priority = Some(1);
        all.labels = Some("label".into());
        all.description = Some("description".into());
        all.design = Some("design".into());
        all.acceptance_criteria = Some("acceptance".into());
        all.demo_statement = Some("demo".into());
        all.execution_note = Some("test-first".into());
        all.external_ref = Some("ref".into());
        all.assignee = Some("worker".into());
        all.status = Some("open".into());
        all.epic = Some("cas-epic".into());
        all.epic_verification_owner = Some("supervisor".into());
        all.depth = Some("light".into());
        assert_eq!(
            ProofScopeOperation::TaskUpdate {
                request: &all,
                target_repo_supplied: true,
                target_branch_supplied: true,
            }
            .locked_fields(),
            [
                "title",
                "priority",
                "labels",
                "description",
                "design",
                "acceptance_criteria",
                "demo_statement",
                "execution_note",
                "external_ref",
                "assignee",
                "status",
                "epic",
                "epic_verification_owner",
                "depth",
                "target_repo",
                "target_branch",
            ]
        );
    }

    fn receipt_input(task_id: &str) -> WorkerCompletionReceiptInput {
        WorkerCompletionReceiptInput {
            task_id: task_id.to_string(),
            worker_agent_id: "worker-session".into(),
            repo_selector: "remote:github.com/org/repo".into(),
            source_branch: "factory/worker".into(),
            commit_sha: "a".repeat(40),
            merge_base_sha: "b".repeat(40),
            target_branch: "main".into(),
            target_sha: "c".repeat(40),
            proof_reference: "proof:scope".into(),
            scope_summary: "locked delivery scope".into(),
        }
    }

    #[test]
    fn exact_delivery_locks_scope_through_close_ready_but_not_after_delivery() {
        for state in [
            WorkerDeliveryState::AwaitingVerification,
            WorkerDeliveryState::AwaitingMerge,
            WorkerDeliveryState::MergeAuthorized,
            WorkerDeliveryState::Merged,
            WorkerDeliveryState::CloseReady,
        ] {
            let root = TempDir::new().unwrap();
            let mut task = Task::new(format!("cas-{state}"), "scope".into());
            task.status = TaskStatus::AwaitingMerge;
            let receipt = cas_store::build_worker_completion_receipt(
                &receipt_input(&task.id),
                "worker",
                chrono::Utc::now(),
            );
            cas_store::create_worker_delivery_with_dispatch(
                root.path(),
                &receipt,
                state,
                "worker-session",
                "supervisor-session",
                chrono::Utc::now() + chrono::Duration::minutes(10),
            )
            .unwrap();
            let mut update = empty_update();
            update.title = Some("changed".into());
            let error = guard_task_proof_scope(
                root.path(),
                &task,
                ProofScopeOperation::TaskUpdate {
                    request: &update,
                    target_repo_supplied: false,
                    target_branch_supplied: false,
                },
            )
            .unwrap_err();
            assert!(error.contains("DELIVERY PROOF SCOPE LOCKED"));
        }

        let root = TempDir::new().unwrap();
        let mut delivered = Task::new("cas-delivered".into(), "delivered".into());
        delivered.status = TaskStatus::Closed;
        let receipt = cas_store::build_worker_completion_receipt(
            &receipt_input(&delivered.id),
            "worker",
            chrono::Utc::now(),
        );
        cas_store::create_worker_delivery_with_dispatch(
            root.path(),
            &receipt,
            WorkerDeliveryState::Delivered,
            "worker-session",
            "supervisor-session",
            chrono::Utc::now() + chrono::Duration::minutes(10),
        )
        .unwrap();
        let mut update = empty_update();
        update.title = Some("reopened scope".into());
        assert!(
            guard_task_proof_scope(
                root.path(),
                &delivered,
                ProofScopeOperation::TaskUpdate {
                    request: &update,
                    target_repo_supplied: false,
                    target_branch_supplied: false,
                },
            )
            .is_ok(),
            "a terminal delivery must not prevent the existing authorized reopen path"
        );
        assert!(
            guard_task_proof_scope(
                root.path(),
                &delivered,
                ProofScopeOperation::CompletionReceipt,
            )
            .unwrap_err()
            .contains("already Closed"),
            "terminal receipt replay remains forbidden even after Delivered"
        );
    }

    #[test]
    fn task_only_exact_dispatch_also_freezes_review_scope() {
        let root = TempDir::new().unwrap();
        let task = Task::new("cas-task-proof".into(), "task proof".into());
        cas_store::create_verification_dispatch_bound(
            root.path(),
            &task.id,
            "requester",
            "owner",
            &VerificationProofBoundary::task(),
            chrono::Utc::now() + chrono::Duration::minutes(10),
            false,
        )
        .unwrap();
        let mut update = empty_update();
        update.acceptance_criteria = Some("changed".into());
        assert!(
            guard_task_proof_scope(
                root.path(),
                &task,
                ProofScopeOperation::TaskUpdate {
                    request: &update,
                    target_repo_supplied: false,
                    target_branch_supplied: false,
                },
            )
            .unwrap_err()
            .contains("DELIVERY PROOF SCOPE LOCKED")
        );
    }

    #[test]
    fn active_delivery_without_its_exact_dispatch_fails_closed() {
        let root = TempDir::new().unwrap();
        let task = Task::new("cas-orphan-delivery".into(), "orphan delivery".into());
        let receipt = cas_store::build_worker_completion_receipt(
            &receipt_input(&task.id),
            "worker",
            chrono::Utc::now(),
        );
        cas_store::create_worker_delivery(
            root.path(),
            &receipt,
            WorkerDeliveryState::AwaitingVerification,
            "supervisor-session",
        )
        .unwrap();
        let mut update = empty_update();
        update.description = Some("changed".into());
        let error = guard_task_proof_scope(
            root.path(),
            &task,
            ProofScopeOperation::TaskUpdate {
                request: &update,
                target_repo_supplied: false,
                target_branch_supplied: false,
            },
        )
        .unwrap_err();
        assert!(error.contains("DELIVERY PROOF SCOPE LOCKED"));
        assert!(error.contains("has no exact verification dispatch"));
    }
}
