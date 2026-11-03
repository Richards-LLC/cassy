//! `candidate_request`: the worker's typed request for a candidate binary. The
//! checks live here, at the request, so a refusal reaches the worker at once;
//! [`crate::factory_candidate_queue`] builds what passes.

use std::process::{Command, Stdio};

use super::factory_ops::{WorkerClonePathResolve, resolve_worker_clone_path};
use crate::factory_candidate_queue::{self as queue, CandidateRequest, Enqueued};
use crate::mcp::tools::service::imports::*;

fn refuse(reason: impl std::fmt::Display) -> McpError {
    CasService::error(
        ErrorCode::INVALID_PARAMS,
        format!("candidate_request refused: {reason}"),
    )
}

fn git_succeeds(checkout: &std::path::Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

impl CasService {
    pub(super) async fn factory_candidate_request(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        let task = req
            .task_id
            .as_deref()
            .ok_or_else(|| refuse("task_id must name your active task"))?;
        let revision = req
            .revision
            .as_deref()
            .filter(|revision| {
                revision.len() == 40
                    && revision
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
            .ok_or_else(|| refuse("revision must be a full 40-character commit SHA, not a ref"))?;
        let caller = self
            .server_caller()
            .ok_or_else(|| refuse("this session has no registered agent identity"))?;
        let holds_lease = self
            .inner
            .open_agent_store()?
            .list_renewable_task_leases(&caller.id)
            .map_err(|error| refuse(format!("could not read your task leases: {error}")))?
            .iter()
            .any(|lease| lease.task_id == task && lease.is_valid());
        if !holds_lease {
            return Err(refuse(format!(
                "{task} is not {}'s active task; start your own task and request its candidate",
                caller.name
            )));
        }
        let cas_root = &self.inner.cas_root;
        // A fixture's npm scripts run during the build, so only what this
        // worker's own checkout already contains may reach it.
        let checkout = match resolve_worker_clone_path(cas_root, &caller) {
            WorkerClonePathResolve::Ready(checkout) => checkout,
            WorkerClonePathResolve::NotOnDisk { candidate, .. } => {
                return Err(refuse(format!(
                    "no checkout of yours on disk at {}",
                    candidate.display()
                )));
            }
        };
        if !git_succeeds(&checkout, &["merge-base", "--is-ancestor", revision, "HEAD"]) {
            return Err(refuse(format!(
                "{revision} is not reachable from HEAD of your checkout {}; commit it there first",
                checkout.display()
            )));
        }
        let builder = format!("{revision}:scripts/qa-candidate-build.py");
        if !git_succeeds(&checkout, &["cat-file", "-e", &builder]) {
            return Err(refuse(format!(
                "{revision} has no scripts/qa-candidate-build.py; this revision has no candidate builder"
            )));
        }
        if !queue::runner_is_live(cas_root) {
            return Err(refuse(
                "no factory daemon with a candidate queue is running for this project; \
                 it may predate the queue, so ask the supervisor for this candidate",
            ));
        }
        if let Some(problem) = queue::sandbox_refusal(cas_root) {
            return Err(refuse(format!(
                "{problem}. Nothing was queued. {}",
                queue::SUPERVISOR_FALLBACK
            )));
        }
        let request = CandidateRequest {
            task: task.to_owned(),
            revision: revision.to_owned(),
            worker: caller.name.clone(),
            factory_session: caller.factory_session.clone(),
            requested_at: chrono::Utc::now(),
            admission_notice_sent: false,
        };
        let enqueued = queue::enqueue(cas_root, &request).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("could not queue the candidate request: {error:#}"),
            )
        })?;
        let status = match enqueued {
            Enqueued::AlreadyBuilding => format!("{revision} is already building for {task}."),
            Enqueued::Queued {
                superseded: Some(superseded),
            } => format!(
                "Queued {revision} for {task}, replacing queued {superseded}, which will not build."
            ),
            Enqueued::Queued { superseded: None } => format!("Queued {revision} for {task}."),
        };
        Ok(Self::success(format!(
            "{status} The factory daemon builds it and messages you the path, version, revision, \
             hash and log, or the exact error. Nothing to run or poll; do not invoke cargo for this."
        )))
    }
}
