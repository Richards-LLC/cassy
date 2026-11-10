//! Read-only post-release observations, persisted independently of publication.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub epic: String,
    pub sha: String,
    pub started_at: DateTime<Utc>,
    pub window_mins: u32,
    pub interval_mins: u32,
    pub runs: Vec<Run>,
    /// Display only (cas-0b3d): the release's affected live journeys, highest
    /// registry traffic share first, captured at start. Never a gate condition.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watch_first: Vec<RankedJourney>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankedJourney {
    pub id: String,
    pub name: String,
    pub share_pct: Option<f64>,
}

/// Affected live journeys of `sha` against its first parent, in the change
/// impact's order: highest traffic share first, unmeasured last.
pub fn rank_release_journeys(project_root: &std::path::Path, qa: &crate::config::QaConfig, sha: &str) -> anyhow::Result<Vec<RankedJourney>> {
    let base = crate::qa_impact::io::resolve_ref(project_root, &format!("{sha}^1"))?;
    let changes = crate::qa_impact::io::changes_for_diff(project_root, &base, Some(sha))?;
    let impact = crate::qa_impact::analyze(project_root, qa, &changes, Some(&base), Some(sha))?;
    Ok(impact
        .journeys
        .into_iter()
        .filter(|journey| journey.status == "live")
        .map(|journey| RankedJourney { id: journey.id, name: journey.name, share_pct: journey.share_pct })
        .collect())
}

/// One line naming what to watch first, or None when nothing was captured.
pub fn watch_first_line(journeys: &[RankedJourney]) -> Option<String> {
    const SHOWN: usize = 6;
    if journeys.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = journeys
        .iter()
        .take(SHOWN)
        .map(|j| match j.share_pct {
            Some(share) => format!("{} {} ({share}%)", j.id, j.name),
            None => format!("{} {} (unmeasured)", j.id, j.name),
        })
        .collect();
    if journeys.len() > SHOWN {
        parts.push(format!("+{} more", journeys.len() - SHOWN));
    }
    Some(format!("Watch first, by traffic share: {}", parts.join("; ")))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub at: DateTime<Utc>,
    pub output: Option<CheckOutput>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckOutput {
    pub ok: bool,
    pub alarms: Vec<Alarm>,
    pub checked_at: DateTime<Utc>,
    pub readback: Option<Readback>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alarm {
    pub id: String,
    pub summary: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Readback {
    pub journey: String,
    pub subject: String,
    pub invariants: Vec<String>,
    pub ok: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pending,
    Clean,
    Failed,
}

impl Receipt {
    pub fn expected_runs(&self) -> usize {
        (self.window_mins.div_ceil(self.interval_mins.max(1)) as usize) + 1
    }
    pub fn next_run_at(&self) -> Option<DateTime<Utc>> {
        if self.runs.len() >= self.expected_runs() {
            return None;
        }
        let offset = (self.runs.len() as u64 * u64::from(self.interval_mins))
            .min(u64::from(self.window_mins));
        Some(self.started_at + chrono::Duration::minutes(offset as i64))
    }
    fn has_failure(&self) -> bool {
        self.runs.iter().any(|run| {
            run.error.is_some()
                || run.output.as_ref().is_none_or(|o| {
                    !o.ok || !o.alarms.is_empty() || o.readback.as_ref().is_some_and(|r| !r.ok)
                })
        })
    }
    pub fn verdict(&self, now: DateTime<Utc>) -> Verdict {
        if self.has_failure() || self.interval_mins == 0 || self.window_mins == 0 {
            return Verdict::Failed;
        }
        if now < self.started_at + chrono::Duration::minutes(i64::from(self.window_mins)) {
            return Verdict::Pending;
        }
        if self.runs.len() != self.expected_runs()
            || !self.runs.iter().any(|run| {
                run.output
                    .as_ref()
                    .and_then(|o| o.readback.as_ref())
                    .is_some_and(|r| {
                        r.ok && !r.journey.is_empty()
                            && !r.subject.is_empty()
                            && !r.invariants.is_empty()
                    })
            })
        {
            return Verdict::Failed;
        }
        Verdict::Clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// cas-0b3d: the release's affected journeys come back in measured-share
    /// order (the change impact's ranking), unmeasured last, for display.
    #[test]
    fn release_journeys_rank_by_traffic_share() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git").arg("-C").arg(root).args(args).output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "qa@example.test"]);
        git(&["config", "user.name", "QA"]);
        let journey = |id: &str, file: &str, share: &str| {
            format!("  - id: {id}\n    name: Journey {id}\n    status: live\n    entry: {{url: /, trigger: Open}}\n    steps: [Go]\n    owners: ['{file}']\n    invariants: [FLOW]\n    traffic: {{share_pct: {share}, basis: counted actions}}\n")
        };
        let registry = format!(
            "version: 1\nupdated: '2026-10-26'\nsurfaces: ['src/**']\ninvariants:\n  FLOW: {{statement: Arrived, check: Read state}}\njourneys:\n{}{}{}",
            journey("rare", "src/a.ts", "10"),
            journey("unmeasured", "src/b.ts", "null"),
            journey("common", "src/c.ts", "60"),
        );
        fs::create_dir_all(root.join("docs/qa")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("docs/qa/journeys.yaml"), registry).unwrap();
        for file in ["a", "b", "c"] {
            fs::write(root.join(format!("src/{file}.ts")), "one").unwrap();
        }
        git(&["add", "."]);
        git(&["commit", "-qm", "base"]);
        for file in ["a", "b", "c"] {
            fs::write(root.join(format!("src/{file}.ts")), "two").unwrap();
        }
        git(&["commit", "-qam", "release"]);
        let sha = git(&["rev-parse", "HEAD"]);
        let ranked = rank_release_journeys(root, &crate::config::QaConfig::default(), &sha).unwrap();
        let ids: Vec<_> = ranked.iter().map(|j| j.id.as_str()).collect();
        assert_eq!(ids, ["common", "rare", "unmeasured"]);
        assert_eq!(
            watch_first_line(&ranked).unwrap(),
            "Watch first, by traffic share: common Journey common (60%); rare Journey rare (10%); unmeasured Journey unmeasured (unmeasured)"
        );
    }

    /// cas-4b22: a parked delivery already on origin/<target> gets one
    /// supervisor notice per uncovered set naming the exact batch_receipt call.
    #[tokio::test]
    async fn batch_receipt_offer_names_live_parked_deliveries_once_per_set_cas_3a25() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git").arg("-C").arg(project).args(args).output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "qa@example.test"]);
        git(&["config", "user.name", "QA"]);
        fs::write(project.join(".gitignore"), ".cas/\nartifacts/\nLEDGER.md\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "base"]);
        let base = git(&["rev-parse", "HEAD"]);
        let anchor = |name: &str| {
            fs::write(project.join(name), name).unwrap();
            git(&["add", name]);
            git(&["commit", "-qm", name]);
            git(&["rev-parse", "HEAD"])
        };
        let live = anchor("live.css");
        git(&["update-ref", "refs/remotes/origin/main", &live]);
        let unpushed = anchor("unpushed.md");
        let cas = crate::store::init_cas_dir(project).unwrap();
        env.set("CAS_ROOT", cas.to_str().unwrap());
        env.set("CAS_CLONE_PATH", project.to_str().unwrap());
        fs::write(cas.join("config.toml"), "[project]\ncanonical_id='batch-offer-fixture'\n[verification]\nenabled=true\n[qa]\nmode='batch'\n").unwrap();
        let mut supervisor = cas_types::Agent::new_with_role("sup-4b22".into(), "release-sup".into(), cas_types::AgentRole::Supervisor);
        supervisor.factory_session = Some("old-session".into());
        crate::store::open_agent_store(&cas).unwrap().register(&supervisor).unwrap();
        let mut unrelated = cas_types::Agent::new_with_role("unrelated".into(), "other-supervisor".into(), cas_types::AgentRole::Supervisor);
        unrelated.registered_at = supervisor.registered_at + chrono::Duration::seconds(1);
        unrelated.factory_session = Some("new-session".into());
        crate::store::open_agent_store(&cas).unwrap().register(&unrelated).unwrap();
        let worker = cas_types::Agent::new_with_role("delivery-worker".into(), "delivery-worker".into(), cas_types::AgentRole::Worker);
        crate::store::open_agent_store(&cas).unwrap().register(&worker).unwrap();
        git(&["branch", "factory/delivery-worker", &live]);
        let tasks = crate::store::open_task_store(&cas).unwrap();
        let mut epic = Task::new("cas-offer-epic".into(), "Offer owner".into());
        epic.task_type = TaskType::Epic;
        epic.epic_verification_owner = Some(supervisor.id.clone());
        tasks.add(&epic).unwrap();
        let unavailable_anchor = "0".repeat(40);
        for (id, head) in [("cas-live", &live), ("cas-unpushed", &unpushed), ("cas-unavailable", &unavailable_anchor)] {
            let mut task = Task::new(id.into(), id.into());
            task.status = cas_types::TaskStatus::AwaitingMerge;
            task.assignee = Some(worker.id.clone());
            task.risk = vec![cas_types::TaskRisk::None];
            task.deliverables.factory_branch_anchor = Some(head.clone());
            task.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
                work_target: cas_types::WorkTarget { repo_selector: "project:batch-offer-fixture".into(), target_branch: "main".into() },
                assignee: worker.id.clone(), branch: "factory/delivery-worker".into(), base_sha: base.clone(),
            });
            tasks.add(&task).unwrap();
            tasks.add_dependency(&cas_types::Dependency::new(id.into(), epic.id.clone(), cas_types::DependencyType::ParentChild)).unwrap();
            cas_store::defer_qa_batch_evidence(&cas, id, head).unwrap();
        }
        assert_eq!(offer_batch_receipts(&cas).unwrap(), 1);
        assert_eq!(offer_batch_receipts(&cas).unwrap(), 0);
        // cas-8863: unrelated integrations move the target, not its live task set.
        let mut target_tip = live.clone();
        for name in ["unrelated-one", "unrelated-two"] {
            git(&["checkout", "-qb", name, &target_tip]);
            anchor(&format!("{name}.md"));
            git(&["checkout", "-q", "--detach", &target_tip]);
            git(&["merge", "--no-ff", "-qm", name, name]);
            target_tip = git(&["rev-parse", "HEAD"]);
            git(&["update-ref", "refs/remotes/origin/main", &target_tip]);
            assert_eq!(offer_batch_receipts(&cas).unwrap(), 0);
        }
        // A delivery parked later at the already-noticed tip still gets its own offer.
        let mut late = Task::new("cas-late".into(), "late".into());
        late.status = cas_types::TaskStatus::AwaitingMerge;
        late.deliverables.factory_branch_anchor = Some(base.clone());
        tasks.add(&late).unwrap();
        tasks.add_dependency(&cas_types::Dependency::new(late.id.clone(), epic.id.clone(), cas_types::DependencyType::ParentChild)).unwrap();
        cas_store::defer_qa_batch_evidence(&cas, "cas-late", &base).unwrap();
        assert_eq!(offer_batch_receipts(&cas).unwrap(), 1);
        assert_eq!(offer_batch_receipts(&cas).unwrap(), 0);
        let rows = crate::store::open_supervisor_queue_store(&cas).unwrap().list_pending(&supervisor.id).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter().any(|row| row.payload.contains(&format!("task_ids=[cas-late,cas-live] staging_sha={target_tip}"))),
            "{:?}",
            rows.iter().map(|row| &row.payload).collect::<Vec<_>>()
        );
        let payload = &rows.iter().find(|row| row.payload.contains(&format!("task_ids=[cas-live] staging_sha={live}"))).unwrap().payload;
        assert!(
            payload.contains(&format!("verification action=batch_receipt task_ids=[cas-live] staging_sha={live}")),
            "{payload}"
        );
        assert!(rows.iter().all(|row| !row.payload.contains("cas-unpushed")), "{rows:?}");

        // cas-0f7b: an ownership transfer after queue selection invalidates
        // the old recipient at the actual transport claim, not just emission.
        let queue = crate::store::open_prompt_queue_store(&cas).unwrap();
        let old = queue.peek_all(10).unwrap();
        assert!(old.iter().all(|row| row.target == supervisor.name && row.factory_session == supervisor.factory_session));
        epic.epic_verification_owner = Some("remote-owner-not-registered-here".into());
        tasks.update(&epic).unwrap();
        assert_eq!(offer_batch_receipts(&cas).unwrap(), 0);
        use crate::ui::factory::daemon::runtime::prompt_queue_jobs::{run_claim_for_inject, ClaimOutcome};
        for row in &old {
            assert_eq!(run_claim_for_inject(&cas, "old-session", row.id, &supervisor.name, &supervisor.name, 1).outcome, ClaimOutcome::Stale);
            assert!(queue.queued_prompt(row.id).unwrap().unwrap().processed_at.is_some());
        }
        assert!(crate::store::open_supervisor_queue_store(&cas).unwrap().list_pending(&supervisor.id).unwrap().is_empty());
        epic.epic_verification_owner = Some(unrelated.id.clone());
        tasks.update(&epic).unwrap();
        assert_eq!(offer_batch_receipts(&cas).unwrap(), 1);
        assert_eq!(offer_batch_receipts(&cas).unwrap(), 0);
        let mut transferred = queue.peek_all(10).unwrap();
        assert_eq!(transferred.len(), 1);
        assert_eq!(transferred[0].target, unrelated.name);
        assert_eq!(transferred[0].factory_session, unrelated.factory_session);
        assert!(batch_notice_deliverable(&cas, queue.as_ref(), &transferred[0]).unwrap());

        // cas-b26e: a superseded episode can return to the same owner/session.
        for owner in [&supervisor, &unrelated] {
            epic.epic_verification_owner = Some(owner.id.clone());
            tasks.update(&epic).unwrap();
            let prior = transferred[0].id;
            for row in &transferred {
                assert!(!batch_notice_deliverable(&cas, queue.as_ref(), row).unwrap());
            }
            assert_eq!(offer_batch_receipts(&cas).unwrap(), 1);
            assert_eq!(offer_batch_receipts(&cas).unwrap(), 0);
            transferred = queue.peek_all(10).unwrap();
            assert_eq!(transferred.len(), 1);
            assert_ne!(transferred[0].id, prior);
            assert_eq!(transferred[0].target, owner.name);
            assert_eq!(transferred[0].factory_session, owner.factory_session);
            assert!(batch_notice_deliverable(&cas, queue.as_ref(), &transferred[0]).unwrap());
        }

        // GH #1656: both delivered tasks remain covered after their worker retires.
        let mut retired = tasks.get("cas-live").unwrap();
        retired.id = "cas-retired".into();
        tasks.add(&retired).unwrap();
        crate::store::open_agent_store(&cas).unwrap().unregister(&worker.id).unwrap();
        let ledger = project.join("LEDGER.md");
        fs::write(&ledger, crate::qa_ledger::scaffold(&tasks.get("cas-live").unwrap(), &[], "rev") + "\ncas-live PASS\ncas-retired PASS\n").unwrap();
        let store = cas_store::SqliteArtifactStore::open(&cas).unwrap();
        let context = crate::artifacts::PublishContext {
            store: &store,
            roots: crate::artifacts::paths::PublishRoots::new(&cas, &project.join("artifacts"), "cas-live"),
            task_id: "cas-live".into(), cloud: None,
        };
        let artifact = crate::artifacts::publish(&context, &ledger).unwrap().artifact.id;
        let core = crate::mcp::CasCore::with_daemon(cas.clone(), None, None);
        core.set_agent_id_for_testing(unrelated.id.clone());
        let service = crate::mcp::CasService::new(core, None);
        let request = |kind: &str| serde_json::from_value(serde_json::json!({
            "action": "batch_receipt", "verification_type": kind,
            "task_ids": ["cas-live", "cas-retired"], "staging_sha": target_tip,
            "ledger_artifact_id": artifact, "ledger_path": ledger, "ledger_author": "external-qa",
        })).unwrap();
        let text = |response: rmcp::model::CallToolResult| response.content.iter()
            .filter_map(|content| match &content.raw { rmcp::model::RawContent::Text(t) => Some(t.text.as_str()), _ => None })
            .collect::<Vec<_>>().join("\n");
        let qa = text(service.verification(rmcp::handler::server::wrapper::Parameters(request("task"))).await.unwrap());
        let pending_holds = || crate::store::open_supervisor_queue_store(&cas).unwrap()
            .list_pending(&unrelated.id).unwrap().into_iter().filter(|row| {
                serde_json::from_str::<serde_json::Value>(&row.payload).unwrap()["kind"] == "batch_close_hold"
            }).collect::<Vec<_>>();
        assert_eq!(pending_holds().len(), 2, "{qa}");
        let prompts = crate::store::open_prompt_queue_store(&cas).unwrap().peek_all(100).unwrap();
        let holds: Vec<_> = prompts.iter().filter(|row| row.summary.as_deref() == Some("batch_close_hold")).collect();
        assert_eq!(holds.len(), 2);
        assert!(holds.iter().all(|row| crate::supervisor_notices::is_action_envelope(&row.prompt)));
        let receipt = cas_store::list_qa_batch_receipts(&cas, "batch-offer-fixture").unwrap().remove(0);
        for id in ["cas-live", "cas-retired"] {
            let task = tasks.get(id).unwrap();
            assert_eq!(task.status, cas_types::TaskStatus::AwaitingMerge);
            assert_eq!(task.notes.matches(&format!("BATCH_CLOSE_OUTCOME {}:", receipt.id)).count(), 1);
            assert!(pending_holds().iter().any(|row| row.payload.contains(id)));
        }
        // A continuation replay retries ordinary gates without duplicating its notice or outcome.
        service.inner.close_release_covered_deliveries(&receipt).await;
        assert_eq!(pending_holds().len(), 2);
        for id in ["cas-live", "cas-retired"] {
            assert_eq!(tasks.get(id).unwrap().notes.matches(&format!("BATCH_CLOSE_OUTCOME {}:", receipt.id)).count(), 1);
        }
        // Re-recording the same QA payload creates audit evidence, not another action for the same gate.
        let qa = text(service.verification(rmcp::handler::server::wrapper::Parameters(request("task"))).await.unwrap());
        assert_eq!(pending_holds().len(), 2);
        // cas-809a: an epic takes no receipt of its own, so its status names the child receipts covering it.
        let mut train = Task::new("cas-train".into(), "train".into());
        train.task_type = cas_types::TaskType::Epic;
        tasks.add(&train).unwrap();
        tasks.add_dependency(&cas_types::Dependency::new("cas-live".into(), "cas-train".into(), cas_types::DependencyType::ParentChild)).unwrap();
        // cas-c041: once a post-merge reopen retires the active anchor, close reads the same receipt qa_status does.
        let mut reopened = tasks.get("cas-live").unwrap();
        let retired = reopened.deliverables.factory_branch_anchor.take().unwrap();
        reopened.deliverables.historical_factory_branch_anchors.push(retired);
        crate::qa_batch::ensure_deferred_close(&cas, &reopened, None, None).unwrap();
        let receipt_id = qa.split("qab-").nth(1).map(|rest| format!("qab-{}", rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>())).unwrap();
        let status = text(service.verification(rmcp::handler::server::wrapper::Parameters(serde_json::from_value(serde_json::json!({
            "action": "qa_status", "task_id": "cas-train",
        })).unwrap())).await.unwrap());
        assert!(status.contains("cas-live") && status.contains(&receipt_id), "{status}");
        // pin: a QA walk must name the remaining release-review gate rather than silently doing nothing.

        assert_eq!(tasks.get("cas-live").unwrap().status, cas_types::TaskStatus::AwaitingMerge);
        // The notice is obsolete once QA is covered, even though a different
        // close gate still holds this shipped task open.
        assert!(!batch_notice_deliverable(&cas, queue.as_ref(), &transferred[0]).unwrap());
        assert_eq!(offer_batch_receipts(&cas).unwrap(), 1, "only the uncovered late delivery needs a receipt");
        let rows = crate::store::open_supervisor_queue_store(&cas).unwrap().list_pending(&unrelated.id).unwrap();
        let latest = rows.iter().filter(|row| row.payload.contains("batch_receipt_offer"))
            .max_by_key(|row| row.id).unwrap();
        assert!(!latest.payload.contains("cas-live"), "covered anchor cannot be re-offered: {latest:?}");
        let reviewed = text(service.verification(rmcp::handler::server::wrapper::Parameters(request("release"))).await.unwrap());
        for id in ["cas-live", "cas-retired"] {
            assert_eq!(tasks.get(id).unwrap().status, cas_types::TaskStatus::Closed, "{reviewed}");
        }
        let verdict = cas_store::release_verdicts_for_task(&cas, "batch-offer-fixture", "cas-live").unwrap().remove(0);
        service.inner.close_release_covered_deliveries(&verdict).await;
        assert_eq!(pending_holds().len(), 2);
        assert_eq!(offer_batch_receipts(&cas).unwrap(), 0, "closed delivery has no further QA offer");

    }

    #[tokio::test]
    async fn legacy_batch_overdue_is_withheld_at_inbox_boundaries_cas_0f7b() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_AGENT_ROLE", "supervisor");
        env.set("CAS_AGENT_NAME", "batch-owner");
        env.set("CAS_FACTORY_SESSION", "batch-session");
        for hook in [false, true] {
            for delivered in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let root = crate::store::init_cas_dir(dir.path()).unwrap();
                env.set("CAS_ROOT", root.to_str().unwrap());
                let mut owner = cas_types::Agent::new_with_role("batch-owner-id".into(), "batch-owner".into(), cas_types::AgentRole::Supervisor);
                owner.factory_session = Some("batch-session".into());
                crate::store::open_agent_store(&root).unwrap().register(&owner).unwrap();
                let body = format!("obsolete-batch-{}", uuid::Uuid::new_v4());
                crate::supervisor_notices::enqueue(&root, &crate::supervisor_notices::Recipient::agent(&owner), "qa_batch_overdue", &body, "legacy").unwrap();
                let queue = crate::store::open_prompt_queue_store(&root).unwrap();
                let row = queue.peek_all(10).unwrap().remove(0);
                if delivered { queue.mark_transport_delivered(row.id).unwrap(); }
                if hook {
                    let input = cas_core::hooks::types::HookInput {
                        agent_role: Some("supervisor".into()),
                        hook_event_name: "UserPromptSubmit".into(),
                        user_prompt: Some(format!("CAS wake: message {} from supervisor", row.id)),
                        ..Default::default()
                    };
                    let output = crate::hooks::handlers::handlers_middle::factory_inbox::surface_factory_inbox(Some(&root), &input);
                    assert!(!output.is_some_and(|output| output.contains(&body)));
                } else {
                    let core = crate::mcp::CasCore::with_daemon(root.clone(), None, None);
                    core.set_agent_id_for_testing(owner.id.clone());
                    let service = crate::mcp::CasService::new(core, None);
                    let result = service.coordination(rmcp::handler::server::wrapper::Parameters(serde_json::from_value(json!({"action":"inbox_poll", "notification_id":row.id})).unwrap())).await.unwrap();
                    assert!(result.content.iter().all(|content| match &content.raw {
                        rmcp::model::RawContent::Text(text) => !text.text.contains(&body),
                        _ => true,
                    }));
                }
                assert!(queue.queued_prompt(row.id).unwrap().is_some());
                let pending = crate::store::open_supervisor_queue_store(&root).unwrap().list_pending(&owner.id).unwrap();
                assert!(pending.is_empty(), "hook={hook} delivered={delivered}: {pending:?}");
            }
        }
    }

    #[test]
    fn qa_post_release_verdict_sequences() {
        let start = DateTime::parse_from_rfc3339("2026-10-21T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let good = json!({"ok":true,"alarms":[],"checked_at":start,
            "readback":{"journey":"signup","subject":"opaque-1","invariants":["workspace"],"ok":true}});
        let no_readback = json!({"ok":true,"alarms":[],"checked_at":start});
        let mut alarm = good.clone();
        alarm["alarms"] = json!([{"id":"missing","summary":"Missing workspace","count":1}]);
        let mut bad_readback = good.clone();
        bad_readback["readback"]["ok"] = json!(false);
        let mut not_ok = good.clone();
        not_ok["ok"] = json!(false);
        for (name, minutes, outputs, expected) in [
            ("open", 10, vec![Some(good.clone())], Verdict::Pending),
            (
                "clean",
                30,
                vec![
                    Some(good.clone()),
                    Some(no_readback.clone()),
                    Some(good.clone()),
                ],
                Verdict::Clean,
            ),
            ("alarm", 1, vec![Some(alarm)], Verdict::Failed),
            ("error", 1, vec![None], Verdict::Failed),
            (
                "readback failed",
                1,
                vec![Some(bad_readback)],
                Verdict::Failed,
            ),
            ("not ok", 1, vec![Some(not_ok)], Verdict::Failed),
            (
                "missing readback",
                30,
                vec![Some(no_readback.clone()); 3],
                Verdict::Failed,
            ),
            ("missing run", 31, vec![Some(good)], Verdict::Failed),
        ] {
            let receipt = Receipt {
                epic: "cas-epic".into(),
                sha: "a".repeat(40),
                started_at: start,
                window_mins: 30,
                interval_mins: 15,
                watch_first: Vec::new(),
                runs: outputs
                    .into_iter()
                    .enumerate()
                    .map(|(i, output)| Run {
                        at: start + chrono::Duration::minutes(i as i64 * 15),
                        error: output.is_none().then(|| "timeout".into()),
                        output: output.map(|v| serde_json::from_value(v).unwrap()),
                    })
                    .collect(),
            };
            assert_eq!(
                receipt.verdict(start + chrono::Duration::minutes(minutes)),
                expected,
                "{name}"
            );
        }
        // Exercise the persisted start -> real bounded probe -> final receipt path.
        use cas_store::TaskStore;
        use std::os::unix::fs::PermissionsExt;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("QA_READ_TOKEN", "read-only-test-token");
        env.set("POST_RELEASE_PRIVATE", "must-not-inherit");
        let dir = tempfile::tempdir().unwrap();
        let cas = dir.path().join(".cas");
        fs::create_dir_all(&cas).unwrap();
        let tasks = crate::store::open_task_store_local(&cas).unwrap();
        tasks.init().unwrap();
        let mut epic = Task::new("cas-probe".into(), "Observe release".into());
        epic.task_type = TaskType::Epic;
        tasks.add(&epic).unwrap();
        // An unconfigured server must not spawn a task (no Tokio runtime here).
        spawn_runner(cas.clone());
        assert!(!cas.join("post-release").exists());
        let mut config = Config::default();
        for (key, value) in [
            ("command", "./check"),
            ("window_mins", "1"),
            ("interval_mins", "1"),
            ("timeout_secs", "2"),
            ("release_branches", "main"),
            ("env", "QA_READ_TOKEN"),
        ] {
            let key = format!("qa.post_release.{key}");
            config.set(&key, value).unwrap();
            assert_eq!(config.get(&key).as_deref(), Some(value));
            assert!(config.list().contains(&(key, value.into())));
        }
        config
            .set(
                "factory.artifacts_root",
                dir.path().join("artifacts").to_str().unwrap(),
            )
            .unwrap();
        config.save(&cas).unwrap();
        let lease = project_runner_lease(&cas).unwrap().unwrap();
        assert!(
            project_runner_lease(&cas).unwrap().is_none(),
            "only one project runner may own the lease"
        );
        drop(lease);
        assert!(
            project_runner_lease(&cas).unwrap().is_some(),
            "an exited owner releases the lease"
        );
        let observation_lock = lock(&cas, &epic.id).unwrap();
        assert!(
            lock_contention(&lock(&cas, &epic.id).unwrap_err()),
            "wrapped observation contention is a silent retry"
        );
        drop(observation_lock);
        let probe = dir.path().join("check");
        fs::write(&probe,format!("#!/bin/sh\n[ -z \"$POST_RELEASE_PRIVATE\" ] && [ \"$QA_READ_TOKEN\" = read-only-test-token ] || exit 1\nprintf '%s' '{}'\n",serde_json::json!({"ok":true,"alarms":[],"checked_at":start,"readback":{"journey":"signup","subject":"opaque-1","invariants":["workspace"],"ok":true}}))).unwrap();
        fs::set_permissions(&probe, fs::Permissions::from_mode(0o700)).unwrap();
        let sha = "b".repeat(40);
        env.set("CAS_ROOT", &cas);
        let cli = crate::cli::try_parse_from_with_wordmark([
            "cas",
            "qa",
            "post-release",
            "start",
            "--epic",
            &epic.id,
            "--sha",
            &sha,
        ])
        .unwrap();
        crate::cli::run(cli).unwrap();
        let started = load(&cas, &epic.id).unwrap().unwrap();
        assert_eq!(
            super::start(&cas, &epic.id, &sha).unwrap().started_at,
            started.started_at,
            "same SHA start is idempotent"
        );
        tick(&cas).unwrap();
        let mut receipt = load(&cas, &epic.id).unwrap().unwrap();
        assert_eq!(receipt.runs.len(), 1);
        assert_eq!(receipt.verdict(Utc::now()), Verdict::Pending);
        receipt.started_at = Utc::now() - chrono::Duration::minutes(1);
        save(&cas, &receipt).unwrap();
        tick(&cas).unwrap();
        let receipt = load(&cas, &epic.id).unwrap().unwrap();
        assert_eq!(receipt.verdict(Utc::now()), Verdict::Clean, "{receipt:?}");
        assert!(gate(&cas, &tasks.get(&epic.id).unwrap(), false).is_none());
        let cli = crate::cli::try_parse_from_with_wordmark([
            "cas",
            "qa",
            "post-release",
            "status",
            "--epic",
            &epic.id,
            "--json",
        ])
        .unwrap();
        crate::cli::run(cli).unwrap();
        let artifact_base = crate::config::resolved_factory_artifacts_root(
            config.factory().artifacts_root.as_deref(),
        );
        assert!(
            crate::config::project_factory_artifacts_root(&cas, &artifact_base)
                .join(&epic.id)
                .join("post-release.json")
                .exists()
        );
    }
}

/// Opt-in project-owned read-only probe. Environment values never enter receipts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PostReleaseConfig {
    pub command: String,
    pub window_mins: u32,
    pub interval_mins: u32,
    pub timeout_secs: u32,
    pub release_branches: Vec<String>,
    pub env: Vec<String>,
}
impl Default for PostReleaseConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            window_mins: 30,
            interval_mins: 15,
            timeout_secs: 120,
            release_branches: Vec::new(),
            env: Vec::new(),
        }
    }
}

use crate::config::Config;
use anyhow::{Context, Result, ensure};
use cas_types::{Task, TaskType};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

pub fn configured(cas_root: &Path) -> Option<PostReleaseConfig> {
    Config::load(cas_root)
        .ok()?
        .qa()
        .post_release
        .filter(|c| !c.command.trim().is_empty())
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
fn receipt_path(cas_root: &Path, epic: &str) -> Result<PathBuf> {
    ensure!(valid_id(epic), "Invalid epic id");
    Ok(cas_root.join("post-release").join(format!("{epic}.json")))
}
pub fn load(cas_root: &Path, epic: &str) -> Result<Option<Receipt>> {
    match fs::read(receipt_path(cas_root, epic)?) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn lock(cas_root: &Path, epic: &str) -> Result<File> {
    let path = receipt_path(cas_root, epic)?.with_extension("lock");
    fs::create_dir_all(path.parent().unwrap())?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    fs2::FileExt::try_lock_exclusive(&file)
        .context("Post-release check is being updated; retry")?;
    Ok(file)
}
fn save(cas_root: &Path, receipt: &Receipt) -> Result<()> {
    let config = Config::load(cas_root)?;
    let base =
        crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
    let artifact = crate::config::project_factory_artifacts_root(cas_root, &base)
        .join(&receipt.epic)
        .join("post-release.json");
    let mut value = serde_json::to_value(receipt)?;
    value["verdict"] = serde_json::to_value(receipt.verdict(Utc::now()))?;
    value["next_run_at"] = serde_json::to_value(receipt.next_run_at())?;
    let bytes = serde_json::to_vec_pretty(&value)?;
    for path in [receipt_path(cas_root, &receipt.epic)?, artifact] {
        fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
        fs::write(&tmp, &bytes)?;
        fs::rename(tmp, path)?;
    }
    Ok(())
}
pub fn start(cas_root: &Path, epic: &str, sha: &str) -> Result<Receipt> {
    let config = configured(cas_root).context("No qa.post_release.command configured")?;
    ensure!(
        config.interval_mins > 0 && config.window_mins > 0 && config.timeout_secs > 0,
        "Post-release window, interval and timeout must be positive"
    );
    ensure!(
        sha.len() == 40 && sha.bytes().all(|c| c.is_ascii_hexdigit()),
        "--sha must be a full 40-character commit SHA"
    );
    let store = crate::store::open_task_store(cas_root)?;
    ensure!(
        store.get(epic)?.task_type == TaskType::Epic,
        "--epic must name an epic"
    );
    let _lock = lock(cas_root, epic)?;
    if let Some(current) = load(cas_root, epic)? {
        if current.sha.eq_ignore_ascii_case(sha) {
            return Ok(current);
        }
        let previous =
            receipt_path(cas_root, epic)?.with_file_name(format!("{epic}-{}.history", current.sha));
        fs::write(previous, serde_json::to_vec_pretty(&current)?)?;
    }
    // Best effort and display only: a release outside a Git checkout, or
    // with no registry, simply names nothing to watch first.
    let watch_first = cas_root
        .parent()
        .and_then(|root| {
            let qa = Config::load(cas_root).ok()?.qa();
            rank_release_journeys(root, &qa, sha).ok()
        })
        .unwrap_or_default();
    let receipt = Receipt {
        epic: epic.into(),
        sha: sha.to_ascii_lowercase(),
        started_at: Utc::now(),
        window_mins: config.window_mins,
        interval_mins: config.interval_mins,
        runs: Vec::new(),
        watch_first,
    };
    save(cas_root, &receipt)?;
    let _ = fs::remove_file(receipt_path(cas_root, epic)?.with_extension("armed"));
    store.append_note(epic,&format!("[{}] POST-RELEASE START {}: {} runs over {} minutes; background job owned by cas serve.",Utc::now().format("%Y-%m-%d %H:%M"),receipt.sha,receipt.expected_runs(),receipt.window_mins))?;
    Ok(receipt)
}
pub const OVERRIDE_MARKER: &str = "POST-RELEASE OVERRIDE:";
fn recent_override(notes: &str, now: DateTime<Utc>) -> Option<&str> {
    notes.lines().rev().find_map(|line| {
        let (stamp, body) = line.trim_start().strip_prefix('[')?.split_once(']')?;
        let body = body.trim_start();
        let body = body
            .strip_prefix("✅ DECISION")
            .or_else(|| body.strip_prefix("DECISION:"))?
            .trim_start();
        let reason = body.strip_prefix(OVERRIDE_MARKER)?.trim();
        if reason.is_empty() {
            return None;
        }
        let when = chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M").ok()?;
        let age = now.naive_utc() - when;
        (age >= chrono::Duration::minutes(-5) && age <= chrono::Duration::hours(6))
            .then_some(reason)
    })
}
/// Shared by Violet hook/CLI and epic close. Publish/deploy/merge never call it.
pub fn gate(cas_root: &Path, epic: &Task, release_notes: bool) -> Option<String> {
    configured(cas_root)?;
    let now = Utc::now();
    let waiting = fs::read(
        receipt_path(cas_root, &epic.id)
            .ok()?
            .with_extension("armed"),
    )
    .ok()
    .and_then(|bytes| serde_json::from_slice::<ArmedRelease>(&bytes).ok());
    let (verdict, progress) = match load(cas_root, &epic.id) {
        Ok(Some(r)) if waiting.as_ref().is_some_and(|w| w.sha != r.sha) => (
            Verdict::Pending,
            "0 runs; awaiting the new release SHA".into(),
        ),
        Ok(Some(r)) => (
            r.verdict(now),
            format!(
                "{}/{} runs, next {}; last result {}",
                r.runs.len(),
                r.expected_runs(),
                r.next_run_at()
                    .map(format_ct)
                    .unwrap_or_else(|| "complete".into()),
                r.runs
                    .last()
                    .map(run_summary)
                    .unwrap_or_else(|| "not run".into())
            ),
        ),
        Ok(None) => (Verdict::Pending, "0 runs; check has not started".into()),
        Err(_) => (Verdict::Failed, "receipt unreadable".into()),
    };
    if verdict == Verdict::Clean {
        return None;
    }
    if let Some(reason) = recent_override(&epic.notes, now) {
        if let Ok(store) = crate::store::open_task_store(cas_root) {
            if store.append_note(&epic.id,&format!("[{}] DECISION: post-release gate overridden for {} by recorded operator reason: {reason}",now.format("%Y-%m-%d %H:%M"),if release_notes {"release notes"} else {"epic close"})).is_ok() { return None; }
        }
    }
    let state = if verdict == Verdict::Pending {
        "pending"
    } else {
        "failed"
    };
    let watch = load(cas_root, &epic.id)
        .ok()
        .flatten()
        .and_then(|receipt| watch_first_line(&receipt.watch_first))
        .map(|line| format!(" {line}."))
        .unwrap_or_default();
    Some(format!(
        "{}: epic {} check {progress}. Run `cas qa post-release start --epic {} --sha <release-sha>` and wait for clean, or record an operator decision note `{OVERRIDE_MARKER} <operator's words>` (valid 6 hours).{watch}",
        if release_notes {
            format!("RELEASE NOTES BLOCKED (post_release_{state})")
        } else {
            format!("POST-RELEASE CHECK {state}")
        },
        epic.id,
        epic.id
    ))
}
fn run_summary(run: &Run) -> String {
    if let Some(error) = &run.error {
        return format!("ERROR {error}");
    }
    match run.output.as_ref() {
        Some(output) if !output.alarms.is_empty() => format!(
            "ALARM {}",
            output
                .alarms
                .iter()
                .map(|a| a
                    .id
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(80)
                    .collect::<String>())
                .collect::<Vec<_>>()
                .join(",")
        ),
        Some(output) if output.ok && output.readback.as_ref().is_none_or(|r| r.ok) => "ok".into(),
        _ => "ALARM failed invariant or read-back".into(),
    }
}
fn run_check(root: &Path, config: &PostReleaseConfig) -> Run {
    let at = Utc::now();
    let result = (|| -> Result<CheckOutput> {
        let mut words = config.command.split_whitespace();
        let program = words.next().context("Empty check command")?;
        ensure!(
            !Path::new(program).is_absolute(),
            "Check executable must be project-relative"
        );
        let path = root
            .join(program)
            .canonicalize()
            .context("Check executable missing")?;
        ensure!(
            path.starts_with(root.canonicalize()?),
            "Check executable must remain inside the project"
        );
        let mut command = std::process::Command::new(path);
        command
            .args(words)
            .current_dir(root)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("LANG", "C.UTF-8");
        for name in &config.env {
            ensure!(
                !name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'),
                "Invalid environment allow-list name"
            );
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let timeout = std::time::Duration::from_secs(u64::from(config.timeout_secs));
        let output = crate::bounded_process::run_command(
            &mut command,
            crate::bounded_process::Deadline::after(timeout),
            timeout,
        )
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        ensure!(output.status.success(), "Command exited {}", output.status);
        // Never persist raw stdout/stderr on errors: a broken probe may print credentials.
        ensure!(
            output.stdout.len() <= 1024 * 1024,
            "Check JSON exceeds 1 MiB"
        );
        serde_json::from_slice(&output.stdout).context("Invalid check JSON")
    })();
    match result {
        Ok(output) => Run {
            at,
            output: Some(output),
            error: None,
        },
        Err(error) => Run {
            at,
            output: None,
            error: Some(error.to_string()),
        },
    }
}
/// Each receipt is locked across serve processes. Interrupted windows fail closed.
pub(crate) fn tick(cas_root: &Path) -> Result<()> {
    let Some(config) = configured(cas_root) else {
        return Ok(());
    };
    start_landed(cas_root)?;
    let Ok(entries) = fs::read_dir(cas_root.join("post-release")) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Some(epic) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(_lock) = lock(cas_root, epic) else {
            continue;
        };
        let Some(mut receipt) = load(cas_root, epic)? else {
            continue;
        };
        let now = Utc::now();
        // Allow the final observation at the window boundary (the gate stays pending until then).
        let Some(due) = receipt.next_run_at() else {
            save(cas_root, &receipt)?;
            continue;
        };
        if due > now {
            continue;
        }
        // Missing an entire interval is an error, never backfill several observations at once.
        if now - due > chrono::Duration::minutes(i64::from(receipt.interval_mins)) {
            receipt.runs.push(Run {
                at: now,
                output: None,
                error: Some("missed observation interval".into()),
            });
        } else {
            receipt.runs.push(run_check(
                cas_root.parent().context("Missing project root")?,
                &config,
            ));
        }
        let mut summary = run_summary(receipt.runs.last().unwrap());
        if receipt.runs.len() == receipt.expected_runs()
            && !receipt.has_failure()
            && receipt.verdict(Utc::now()) == Verdict::Failed
        {
            summary.push_str("; ERROR no successful journey read-back in the completed window");
        }
        save(cas_root, &receipt)?;
        crate::store::open_task_store(cas_root)?.append_note(
            &receipt.epic,
            &format!(
                "[{}] POST-RELEASE {}/{} {}: {summary}",
                now.format("%Y-%m-%d %H:%M"),
                receipt.runs.len(),
                receipt.expected_runs(),
                receipt.sha
            ),
        )?;
    }
    Ok(())
}
/// One OS lease owns the configured project's runner across all serve processes.
/// Unconfigured projects and contenders spawn no task and produce no warning.
fn project_runner_lease(cas_root: &Path) -> Result<Option<File>> {
    if configured(cas_root).is_none() && !batch_offers_pending(cas_root) {
        return Ok(None);
    }
    let directory = cas_root.join("post-release");
    fs::create_dir_all(&directory)?;
    let lease = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("runner.lock"))?;
    match fs2::FileExt::try_lock_exclusive(&lease) {
        Ok(()) => Ok(Some(lease)),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn lock_contention(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::WouldBlock)
    })
}

pub(crate) fn spawn_runner(cas_root: PathBuf) {
    let lease = match project_runner_lease(&cas_root) {
        Ok(Some(lease)) => lease,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(%error,"could not acquire post-release project runner lease");
            return;
        }
    };
    tokio::spawn(async move {
        let _lease = lease;
        loop {
            let root = cas_root.clone();
            let result = tokio::task::spawn_blocking(move || {
                let batch = batch_mode(&root);
                if configured(&root).is_none() && !batch {
                    return Ok(false);
                }
                if batch && let Err(error) = offer_batch_receipts(&root) {
                    tracing::warn!(%error, "batch receipt offer failed");
                }
                tick(&root).map(|()| true)
            })
            .await
            .unwrap_or_else(|error| Err(error.into()));
            match result {
                Ok(false) => return,
                Err(error) if !lock_contention(&error) => {
                    tracing::warn!(%error,"post-release background tick failed")
                }
                _ => {}
            }
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    });
}

fn batch_mode(cas_root: &Path) -> bool {
    Config::load(cas_root).is_ok_and(|config| config.qa().mode == crate::config::QaMode::Batch)
}

/// A batch-mode project starts the runner only once a deferral waits on a receipt.
fn batch_offers_pending(cas_root: &Path) -> bool {
    batch_mode(cas_root)
        && cas_store::pending_qa_batch_deferrals(cas_root).is_ok_and(|ids| !ids.is_empty())
}

/// Explicit verification ownership never falls back to an unrelated local
/// supervisor when that owner is on another machine or no longer live.
fn batch_owner(root: &Path, task: &Task, agents: &[cas_types::Agent]) -> Result<Option<crate::supervisor_notices::Recipient>> {
    let tasks = crate::store::open_task_store_local(root)?;
    let epic = tasks.get_parent_epic(&task.id)?;
    let live = || agents.iter().filter(|agent| crate::supervisor_notices::is_live(agent));
    let candidates: Vec<_> = if let Some(owner) = epic.as_ref().and_then(|epic| epic.epic_verification_owner.as_deref()) {
        live().filter(|agent| agent.id == owner || agent.name == owner || agent.factory_session.as_deref() == Some(owner)).collect()
    } else {
        // Legacy ownerless deliveries may use their lane's supervisor. With
        // no lane association, only a sole local supervisor is unambiguous.
        let worker = task.assignee.as_deref().or_else(|| task.deliverables.factory_branch_start.as_ref().map(|start| start.assignee.as_str()));
        let session = agents.iter().find(|agent| worker.is_some_and(|worker| agent.id == worker || agent.name == worker))
            .and_then(|agent| agent.factory_session.as_deref());
        live().filter(|agent| session.is_none() || agent.factory_session.as_deref() == session).collect()
    };
    Ok((candidates.len() == 1).then(|| crate::supervisor_notices::Recipient::agent(candidates[0])))
}

/// A shipped anchor alone is not missing QA. A valid QA receipt suppresses
/// the offer even while a release-verdict or another close gate holds the task.
fn batch_offer_evidence(root: &Path, task: &Task) -> Result<Option<(String, String)>> {
    use crate::qa_batch::{ancestor, commit, resolved_delivery_anchor, task_context};
    if task.status != cas_types::TaskStatus::AwaitingMerge { return Ok(None); }
    let (repo, target) = task_context(root, task).map_err(anyhow::Error::msg)?;
    let anchor = resolved_delivery_anchor(&repo, task, &target).context("batch delivery anchor unavailable")?;
    let tip = commit(&repo, &format!("origin/{target}")).context("batch target unavailable")?;
    if crate::qa_batch::satisfying_receipt(root, task, &repo, &target, &anchor)
        .map_err(anyhow::Error::msg)?.is_some() { return Ok(None); }
    match ancestor(&repo, &anchor, &tip) {
        Some(true) => Ok(Some((target, tip))),
        Some(false) => Ok(None),
        None => anyhow::bail!("batch delivery integration unreadable"),
    }
}

/// Recheck CAS-authored batch notices at each read/transport boundary. Old
/// hourly overdue notices are obsolete; current offers need both current
/// ownership and outstanding evidence. Unknown state holds rather than routes.
pub(crate) fn batch_notice_deliverable(root: &Path, queue: &dyn cas_store::PromptQueueStore, row: &cas_store::QueuedPrompt) -> Result<bool> {
    if row.origin != Some(cas_store::QueueOrigin::Daemon)
        || !row.source.starts_with("supervisor-notice:")
    { return Ok(true); }
    let id = row.source.strip_prefix("supervisor-notice:").and_then(|id| id.parse::<i64>().ok())
        .context("batch notice identity unavailable")?;
    let notices = crate::store::open_supervisor_queue_store(root)?;
    let notice = notices.get(id)?.context("batch notice unavailable")?;
    let payload: serde_json::Value = serde_json::from_str(&notice.payload)?;
    let current = if payload["kind"] == "qa_batch_overdue" {
        false
    } else if payload["kind"] == "batch_receipt_offer" {
        if let Some(ids) = payload.get("context").and_then(|context| context.get("task_ids")).and_then(|ids| ids.as_array()) {
            let pending = cas_store::pending_qa_batch_deferrals(root)?;
            let tasks = crate::store::open_task_store_local(root)?;
            let agents = crate::store::open_agent_store(root)?.list(None)?;
            let mut current = !ids.is_empty();
            for id in ids {
                let id = id.as_str().context("invalid batch task identity")?;
                if !pending.iter().any(|pending| pending == id) { current = false; break; }
                let task = tasks.get(id)?;
                let owner = batch_owner(root, &task, &agents)?;
                if !owner.is_some_and(|owner| owner.id == notice.supervisor_id && owner.name == row.target && owner.session == row.factory_session)
                    || batch_offer_evidence(root, &task)?.is_none()
                { current = false; break; }
            }
            current
        } else {
            false // Pre-upgrade offers have no verifiable task scope.
        }
    } else { return Ok(true); };
    if !current {
        // A transported row retains its transport history; skipping its body
        // must not manufacture acknowledgement or rewrite delivered evidence.
        if queue.queued_prompt(row.id)?.is_some_and(|fresh| fresh.processed_at.is_none()) {
            queue.mark_superseded(row.id, "Batch notice superseded by current ownership or evidence")?;
        }
        notices.ack(id)?;
    }
    Ok(current)
}

/// Inbox drains stamp receipt/transport evidence before returning rows. Retire
/// obsolete notices before that transaction, not only before rendering bodies.
/// The supervisor outbox scopes this scan to this recipient's own notices.
pub(crate) fn refresh_batch_notices(root: &Path, queue: &dyn cas_store::PromptQueueStore, recipient: &str, session: Option<&str>) -> Result<()> {
    let agents = crate::store::open_agent_store(root)?.list(None)?;
    let notices = crate::store::open_supervisor_queue_store(root)?;
    for agent in agents.iter().filter(|agent| agent.role == cas_types::AgentRole::Supervisor && (agent.name == recipient || agent.id == recipient)) {
        for notice in notices.list_pending(&agent.id)? {
            if notice.event_type != "supervisor_notice" { continue; }
            let payload: serde_json::Value = serde_json::from_str(&notice.payload)?;
            if payload["kind"] != "qa_batch_overdue" && payload["kind"] != "batch_receipt_offer" { continue; }
            if let Some(row) = queue.latest_from_source_to_targets(&format!("supervisor-notice:{}", notice.id), &[&agent.name], session)? {
                batch_notice_deliverable(root, queue, &row)?;
            }
        }
    }
    Ok(())
}

/// cas-4b22: offer, never record, the batch QA receipt for parked deliveries
/// whose anchor is already on origin/<target>. A receipt needs a supervisor
/// recorder and an independent ledger author, so Cassy sends the supervisor
/// one notice per distinct uncovered delivery set naming the exact call with
/// the current target tip. Advancing the tip alone stays quiet. Returns notices sent.
pub(crate) fn offer_batch_receipts(cas_root: &Path) -> Result<usize> {
    use sha2::{Digest, Sha256};
    let pending = cas_store::pending_qa_batch_deferrals(cas_root)?;
    if pending.is_empty() {
        return Ok(0);
    }
    let tasks = crate::store::open_task_store(cas_root)?;
    let agents = crate::store::open_agent_store(cas_root)?.list(None)?;
    let mut live: std::collections::BTreeMap<(String, String, String, String, Option<String>), Vec<String>> = Default::default();
    for id in pending {
        let Ok(task) = tasks.get(&id) else { continue };
        let Some(recipient) = batch_owner(cas_root, &task, &agents)? else { continue };
        let (target, tip) = match batch_offer_evidence(cas_root, &task) {
            Ok(Some(evidence)) => evidence,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(task_id = id, %error, "batch offer evidence unavailable");
                continue;
            }
        };
        live.entry((target, tip, recipient.id, recipient.name, recipient.session)).or_default().push(id);
    }
    let notices = crate::store::open_supervisor_queue_store(cas_root)?;
    let queue = crate::store::open_prompt_queue_store(cas_root)?;
    let mut sent = 0;
    'offers: for ((target, tip, id, name, session), mut ids) in live {
        ids.sort_unstable();
        ids.dedup();
        // cas-8863: the target tip is payload evidence, not offer identity.
        let identity = format!("{:x}", Sha256::digest(serde_json::to_vec(&("owner-v1", &target, &id, &name, &session, &ids))?));
        let mut occurrence = identity.clone();
        loop {
            let key = format!("supervisor-notice:batch_receipt_offer:{occurrence}:{id}");
            let Some(notice) = notices.get_by_transition_key(&key)? else { break; };
            let source = format!("supervisor-notice:{}", notice.id);
            let Some(row) = queue.latest_from_source_to_targets(&source, &[&name], session.as_deref())? else {
                if notice.prompt_delivered_at.is_some() { continue 'offers; }
                break; // Repair a committed outbox row's missing prompt with the same key.
            };
            if !queue.message_delivery_report(row.id)?.is_some_and(|status| {
                status.pending_reason == Some(cas_store::PendingReason::SupersededStale)
            }) {
                continue 'offers;
            }
            // cas-b26e: a retired notice starts a new ownership episode. Its
            // durable ID keeps concurrent producers and crash retries idempotent.
            occurrence = format!("{identity}:after:{}", notice.id);
        }
        let detail = format!(
            "{} parked deliver{} already live on origin/{target} at {tip}: {}. {batch_close} \
             Initialize with cas qa ledger init --task <qa-task> --covers {covered}; keep every delivery ID in its covers: header. Before publishing, run cas qa ledger check <path> --task <qa-task> --covers {covered}. Walk and publish the ledger, then record: verification action=batch_receipt task_ids=[{}] staging_sha={tip} ledger_artifact_id=<id> ledger_path=<path> ledger_author=<walker>. \
             Cassy did not record it: a receipt needs a supervisor recorder and an independent ledger author.",
            ids.len(),
            if ids.len() == 1 { "y is" } else { "ies are" },
            ids.join(", "),
            ids.join(","),
            covered = ids.join(","),
            batch_close = crate::mcp::tools::core::task::lifecycle::close_ops::gate_text::BATCH_CLOSE_RULE,
        );
        let recipient = crate::supervisor_notices::Recipient { id, name, session };
        crate::supervisor_notices::enqueue_with_context(cas_root, &recipient, "batch_receipt_offer", &detail, &occurrence, Some(serde_json::json!({"task_ids": ids})))?;
        sent += 1;
    }
    Ok(sent)
}

// US Central switches on March's second Sunday / November's first Sunday.
// Computing the offset avoids process-global TZ mutations.
fn format_ct(at: DateTime<Utc>) -> String {
    use chrono::{Datelike, TimeZone};
    let year = at.year();
    let sunday = |month: u32, ordinal: u32| {
        let first = chrono::NaiveDate::from_ymd_opt(year, month, 1).unwrap();
        1 + (7 - first.weekday().num_days_from_sunday()) % 7 + (ordinal - 1) * 7
    };
    let begin = Utc
        .with_ymd_and_hms(year, 3, sunday(3, 2), 8, 0, 0)
        .unwrap();
    let end = Utc
        .with_ymd_and_hms(year, 11, sunday(11, 1), 7, 0, 0)
        .unwrap();
    let hours = if at >= begin && at < end { 5 } else { 6 };
    at.with_timezone(&chrono::FixedOffset::west_opt(hours * 3600).unwrap())
        .format("at %Y-%m-%d %H:%M CT")
        .to_string()
}
#[derive(Serialize, Deserialize)]
struct ArmedRelease {
    epic: String,
    sha: String,
    branches: Vec<String>,
}
fn git_output(root: &Path, args: &[&str]) -> Result<String> {
    let mut command = std::process::Command::new("git");
    command.current_dir(root).args(args);
    let cap = std::time::Duration::from_secs(2);
    let output = crate::bounded_process::run_command(
        &mut command,
        crate::bounded_process::Deadline::after(cap),
        cap,
    )
    .map_err(|e| anyhow::anyhow!("Git probe: {e:?}"))?;
    ensure!(output.status.success(), "Git probe did not succeed");
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}
/// Durable branch-contained fallback, armed at the epic's merge request.
/// Best effort: arming never holds a merge or publication.
pub(crate) fn arm_on_merge(cas_root: &Path, epic: &Task, owner: &str) -> Result<()> {
    let Some(config) = configured(cas_root) else {
        return Ok(());
    };
    let Some(branch) = &epic.branch else {
        return Ok(());
    };
    let project = cas_root.parent().context("Missing project root")?;
    let sha = git_output(
        project,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{branch}^{{commit}}"),
        ],
    )?;
    if load(cas_root, &epic.id)?.is_some_and(|r| {
        r.sha == sha || git_output(project, &["merge-base", "--is-ancestor", &sha, &r.sha]).is_ok()
    }) {
        return Ok(());
    }
    let branches = if config.release_branches.is_empty() {
        vec![crate::worktree::GitOperations::new(project.to_path_buf()).detect_default_branch()]
    } else {
        config.release_branches
    };
    let _lock = lock(cas_root, &epic.id)?;
    let path = receipt_path(cas_root, &epic.id)?.with_extension("armed");
    let watch = ArmedRelease {
        epic: epic.id.clone(),
        sha,
        branches,
    };
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&tmp, serde_json::to_vec(&watch)?)?;
    fs::rename(tmp, path)?;
    let reminders = crate::store::open_reminder_store(cas_root)?;
    let pending = reminders.list_all_pending()?;
    for target in &watch.branches {
        let filter = serde_json::json!({"commit":watch.sha,"target_branch":target});
        if pending.iter().any(|r| {
            r.task_id.as_deref() == Some(&epic.id)
                && r.message.starts_with("POST-RELEASE AUTO START")
                && r.trigger_filter.as_ref() == Some(&filter)
        }) {
            continue;
        }
        reminders.create_with_scope(
            owner,
            None,
            "POST-RELEASE AUTO START: observe the landed epic before release notes and close",
            cas_store::ReminderTriggerType::Event,
            None,
            Some("branch_contained_in"),
            Some(&filter),
            7 * 24 * 3600,
            None,
            None,
            true,
            Some(&epic.id),
        )?;
    }
    Ok(())
}
/// Before a reminder is marked fired, so a failed start retries.
pub(crate) fn start_from_reminder(cas_root: &Path, reminder: &cas_store::Reminder) -> Result<()> {
    if reminder.trigger_event.as_deref() != Some("branch_contained_in")
        || !reminder.message.starts_with("POST-RELEASE AUTO START")
    {
        return Ok(());
    }
    let Some(epic) = reminder.task_id.as_deref() else {
        return Ok(());
    };
    let path = receipt_path(cas_root, epic)?.with_extension("armed");
    let Ok(bytes) = fs::read(path) else {
        return Ok(());
    };
    let watch: ArmedRelease = serde_json::from_slice(&bytes)?;
    if reminder
        .trigger_filter
        .as_ref()
        .and_then(|v| v.get("commit"))
        .and_then(serde_json::Value::as_str)
        == Some(&watch.sha)
    {
        start(cas_root, epic, &watch.sha)?;
    }
    Ok(())
}
fn start_landed(cas_root: &Path) -> Result<()> {
    let Ok(entries) = fs::read_dir(cas_root.join("post-release")) else {
        return Ok(());
    };
    let project = cas_root.parent().context("Missing project root")?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "armed") {
            continue;
        }
        let watch: ArmedRelease = serde_json::from_slice(&fs::read(path)?)?;
        if watch.branches.iter().any(|branch| {
            git_output(
                project,
                &[
                    "merge-base",
                    "--is-ancestor",
                    &watch.sha,
                    &format!("origin/{branch}"),
                ],
            )
            .is_ok()
        }) {
            start(cas_root, &watch.epic, &watch.sha)?;
        }
    }
    Ok(())
}
