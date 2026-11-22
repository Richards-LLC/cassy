//! Automatic predicted/delivered journey context and the G8 ledger check.
use super::close_ops;
use crate::mcp::tools::core::imports::*;
use crate::qa_impact::{self, Change};
use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use std::path::Path;

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}
fn cell(value: &str) -> String {
    value.replace(['\n', '\r'], " ").replace('|', "\\|")
}

/// Shared by the task note and independent review brief.
pub(crate) fn render(impact: &Value) -> String {
    let mut out = String::new();
    if let Some(warnings) = impact["warnings"].as_array() {
        for warning in warnings {
            out.push_str(&format!("> WARNING: {}\n", cell(text(warning))));
        }
    }
    if let Some(safety) = impact
        .get("safety")
        .filter(|value| value.is_object())
        .and_then(|value| serde_json::from_value::<crate::qa_registry::Safety>(value.clone()).ok())
    {
        out.push_str(&format!("> {}\n", cell(&safety.brief())));
    }
    out.push_str("\n| Journey | Status | Share | Invariants and evidence required |\n| --- | --- | --- | --- |\n");
    if let Some(journeys) = impact["journeys"].as_array() {
        for journey in journeys {
            let invariants = journey["invariants"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|i| {
                            format!(
                                "{}: {} — {}",
                                text(&i["id"]),
                                text(&i["statement"]),
                                text(&i["check"])
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .unwrap_or_default();
            let share = journey["share_pct"]
                .as_f64()
                .map(|n| format!("{n}%"))
                .unwrap_or_else(|| "unknown".into());
            out.push_str(&format!(
                "| {} | {} | {} | {} {} |\n",
                cell(text(&journey["id"])),
                cell(text(&journey["status"])),
                share,
                cell(&invariants),
                cell(text(&journey["evidence_required"]))
            ));
        }
    }
    if let Some(areas) = impact["areas"].as_array() {
        for area in areas {
            out.push_str(&format!(
                "\n- Walk the user-facing entry of CODEMAP area `{}`: {}\n",
                cell(text(&area["prefix"])),
                cell(text(&area["summary"]))
            ));
        }
    }
    if let Some(unowned) = impact["unowned"]
        .as_array()
        .filter(|items| !items.is_empty())
    {
        out.push_str(&format!(
            "\n{} unowned file(s); see state_patch.qa_impact for the full impact.\n",
            unowned.len()
        ));
    }
    out.push_str("\nRecord one ledger row per live journey: PASS, FAIL, NOT A ROUTE, or NOT EXERCISED: <reason>.\n");
    out
}

pub(crate) fn predicted_value(core: &CasCore, task: &Task) -> Result<Value> {
    let repo = super::super::repo_context::resolve_task_metadata_repo(&core.cas_root, task)
        .map_err(|error| anyhow!(error))?;
    Ok(serde_json::to_value(crate::qa_impact::predicted(&repo, &core.load_config().qa(), task)?)?)
}

pub(crate) fn predicted(core: &CasCore, task: &Task) -> String {
    let result = predicted_value(core, task);
    match result {
        Ok(report) => format!(
            "Predicted QA impact (paths named in title, description, design and proof targets; not a delivery diff):\n{}",
            render(&report)
        ),
        Err(error) => format!("Predicted QA impact unavailable (non-blocking): {error:#}"),
    }
}

impl CasCore {
    /// Cache the committed delivery using close's task attribution. A lane's
    /// later work and the caller's working-tree metadata are never selected.
    pub(crate) fn attach_delivery_qa_impact(
        &self,
        task: &Task,
        repo: &Path,
        target: &str,
        head: &str,
    ) -> Result<Value> {
        let store = self
            .open_task_store()
            .map_err(|e| anyhow!(e.message.to_string()))?;
        if let Some(state) = store.get_execution_state(&task.id)?
            && let Some(impact) = state.get("qa_impact")
            && impact["head_sha"].as_str() == Some(head)
        {
            return Ok(impact.clone());
        }
        let latest = cas_store::get_latest_worker_delivery(&self.cas_root, &task.id)?
            .map(|(r, _)| r.commit_sha);
        let history = self
            .open_agent_store()
            .ok()
            .and_then(|s| s.get_lease_history(&task.id, None).ok())
            .unwrap_or_default();
        let window = close_ops::resolve_task_commit_receipt_window(
            task.created_at,
            &history,
            close_ops::task_commit_identity_in_repo(repo, task, latest).map_err(|e| anyhow!(e))?,
        );
        let target = close_ops::freshest_target_ref(repo, target);
        let paths = close_ops::task_attribution::paths(repo, &target, &window, Some(head))
            .ok_or_else(|| anyhow!("delivery paths could not be attributed"))?;
        let base = close_ops::task_attribution::delivery_base(repo, &target, &window, Some(head));
        let mut changes = match base.as_deref() {
            Some(base) => qa_impact::io::changes_for_diff(repo, base, Some(head))?,
            None => Vec::new(),
        };
        changes.retain(|c| {
            paths.contains(&c.file) || c.old_file.as_ref().is_some_and(|p| paths.contains(p))
        });
        for path in paths {
            if !changes
                .iter()
                .any(|c| c.file == path || c.old_file.as_deref() == Some(path.as_str()))
            {
                changes.push(Change::modified(path));
            }
        }
        let report = qa_impact::analyze(
            repo,
            &self.load_config().qa(),
            &changes,
            base.as_deref(),
            Some(head),
        )?;
        let mut value = serde_json::to_value(report)?;
        value["head_sha"] = head.into();
        value["base_sha"] = base.into();
        store
            .patch_execution_state(&task.id, &serde_json::json!({"qa_impact": value}))
            .context("store authoritative QA impact")?;
        store.append_note(
            &task.id,
            &format!(
                "[{}] Authoritative QA impact @{head}:\n{}",
                chrono::Utc::now().format("%Y-%m-%d %H:%M"),
                render(&value)
            ),
        )?;
        Ok(value)
    }
}

/// G8 reads the park-time report, never a new diff. Legacy catalog adoption
/// remains advisory; enforcement arms when a YAML registry exists.
// gate-audit[forward]: G-118 The qa_record decision tags this pure renderer output.
pub(crate) fn ledger_refusal(impact: &Value, head: &str, ledger: &str) -> Option<String> {
    if impact["registry"]["present"] != true {
        return None;
    }
    let live: Vec<_> = impact["journeys"]
        .as_array()?
        .iter()
        .filter(|j| j["status"] == "live")
        .filter_map(|j| j["id"].as_str())
        .collect();
    if live.is_empty() {
        return None;
    }
    if impact["head_sha"].as_str() != Some(head) {
        return Some("QA RECORD REJECTED: journey impact is for a different delivery head; refresh the QA dispatch for this round before recording approval.".into());
    }
    let parsed = crate::qa_ledger::parse(ledger, 1);
    if !parsed.ok { return Some(format!("QA RECORD REJECTED: {}", parsed.refusal())); }
    let missing: Vec<_> = live.into_iter().filter(|id| {
        !parsed.rows.iter().any(|row| row.id == *id || row.cell == *id)
    }).collect();
    (!missing.is_empty()).then(|| format!("QA RECORD REJECTED: the ledger has no row for live journeys {}. Add one row per journey: PASS, FAIL, NOT A ROUTE, or NOT EXERCISED: <reason>.", missing.join(", ")))
}

#[cfg(test)]
mod checkout_tests {
    use super::*;
    use crate::mcp::tools::service::CasService;
    use rmcp::handler::server::wrapper::Parameters;

    fn git(repo: &Path, args: &[&str]) {
        let output = std::process::Command::new("git").arg("-C").arg(repo).args(args)
            .env("GIT_AUTHOR_NAME", "Fixture").env("GIT_AUTHOR_EMAIL", "fixture@example.test")
            .env("GIT_COMMITTER_NAME", "Fixture").env("GIT_COMMITTER_EMAIL", "fixture@example.test")
            .output().unwrap();
        assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
    }

    fn registry(repo: &Path, id: &str) {
        std::fs::create_dir_all(repo.join("docs/qa")).unwrap();
        std::fs::write(repo.join("docs/qa/journeys.yaml"), format!(
            "version: 1\nupdated: '2026-11-21'\nmain_journeys: [{id}]\nsurfaces: ['app/**']\ninvariants:\n  ARRIVED: {{statement: Arrived, check: Read state}}\njourneys:\n- id: {id}\n  status: live\n  entry: {{url: /feature, trigger: Open}}\n  steps: [Arrive]\n  owners: ['app/**']\n  invariants: [ARRIVED]\n"
        )).unwrap();
    }

    fn fixture(env: &mut crate::test_support::TestEnvGuard) -> (tempfile::TempDir, CasService, Task, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-qb", "main"]);
        std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
        git(repo, &["add", ".gitignore"]);
        git(repo, &["commit", "-qm", "base without metadata"]);
        let root = repo.join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("config.toml"), "[project]\ncanonical_id='worker-metadata-fixture'\n[verification]\nenabled=false\n").unwrap();
        let worker = root.join("worktrees/reader");
        std::fs::create_dir_all(worker.parent().unwrap()).unwrap();
        git(repo, &["worktree", "add", "-qb", "factory/reader", worker.to_str().unwrap()]);
        registry(&worker, "WORKER-J");
        std::fs::write(worker.join("docs/PRODUCT_OVERVIEW.md"), "# Worker product\n").unwrap();
        std::fs::create_dir_all(worker.join("app")).unwrap();
        std::fs::write(worker.join("app/feature.rs"), "pub fn feature() {}\n").unwrap();
        git(&worker, &["add", "."]);
        git(&worker, &["commit", "-qm", "worker metadata"]);
        env.set("CAS_ROOT", &root);
        env.set("CAS_CLONE_PATH", &worker);
        env.set("CAS_AGENT_NAME", "reader");
        env.set("CAS_AGENT_ROLE", "worker");
        let core = CasCore::with_daemon(root, None, None);
        let mut agent = cas_types::Agent::new_with_role("reader-session".into(), "reader".into(), cas_types::AgentRole::Worker);
        agent.metadata.insert("clone_path".into(), worker.display().to_string());
        agent.pid = Some(std::process::id());
        core.open_agent_store().unwrap().register(&agent).unwrap();
        core.set_agent_id_for_testing(agent.id.clone());
        let mut task = Task::new("cas-metadata".into(), "Inspect app/feature.rs".into());
        task.assignee = Some(agent.name);
        task.origin_project = Some("worker-metadata-fixture".into());
        task.acceptance_criteria = "The feature path uses its own registry".into();
        task.notes = "Repro: caller supplied fixture".into();
        task.labels = vec!["from-finding".into(), "severity:broken".into(), "reach:secondary".into()];
        task.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:worker-metadata-fixture".into(), target_branch: "main".into(),
        });
        core.open_task_store().unwrap().add(&task).unwrap();
        (dir, CasService::new(core, #[cfg(feature = "mcp-proxy")] None), task, worker)
    }

    #[tokio::test]
    async fn cas_598f_show_and_start_read_assigned_checkout_metadata() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let (dir, service, task, worker) = fixture(&mut env);
        for primary_has_registry in [false, true] {
            if primary_has_registry { registry(dir.path(), "PRIMARY-J"); }
            let answer = service.task(Parameters(serde_json::from_value(serde_json::json!({
                "action":"show", "id":task.id, "full":true,
            })).unwrap())).await.unwrap();
            let data = answer.structured_content.expect("current task metadata, independent of saved notes");
            assert_eq!(data["qa_impact"]["registry"]["present"], true);
            assert!(data["qa_impact"]["journeys"].as_array().unwrap().iter().any(|j| j["id"] == "WORKER-J"));
            assert!(!data["qa_impact"]["journeys"].as_array().unwrap().iter().any(|j| j["id"] == "PRIMARY-J"));
        }
        let started = service.task(Parameters(serde_json::from_value(serde_json::json!({
            "action":"start", "id":task.id, "brief":false,
        })).unwrap())).await.unwrap();
        assert_eq!(started.is_error, Some(false));
        assert!(serde_json::to_string(&started).unwrap().contains("WORKER-J"));
        // A genuinely missing worker registry must not borrow the primary's.
        std::fs::remove_file(worker.join("docs/qa/journeys.yaml")).unwrap();
        let answer = service.task(Parameters(serde_json::from_value(serde_json::json!({
            "action":"show", "id":task.id, "full":true,
        })).unwrap())).await.unwrap();
        assert_eq!(answer.structured_content.unwrap()["qa_impact"]["registry"]["present"], false);
        // Even a registered worker cannot substitute a different repository.
        let foreign = tempfile::tempdir().unwrap();
        git(foreign.path(), &["init", "-qb", "main"]);
        registry(foreign.path(), "PRIMARY-J");
        let agents = crate::store::open_agent_store(&dir.path().join(".cas")).unwrap();
        let mut agent = agents.get("reader-session").unwrap();
        agent.metadata.insert("clone_path".into(), foreign.path().display().to_string());
        agents.update(&agent).unwrap();
        let answer = service.task(Parameters(serde_json::from_value(serde_json::json!({
            "action":"show", "id":task.id, "full":true,
        })).unwrap())).await.unwrap();
        let data = answer.structured_content.unwrap();
        assert!(data["qa_impact"].is_null());
        assert!(data["qa_impact_error"].is_string());
    }

    #[tokio::test]
    async fn cas_598f_finding_labels_validate_the_assigned_registry() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let (dir, service, task, _worker) = fixture(&mut env);
        registry(dir.path(), "PRIMARY-J");
        let updated = service.task(Parameters(serde_json::from_value(serde_json::json!({
            "action":"update", "id":task.id, "labels":"severity:broken,reach:main,journey:WORKER-J",
        })).unwrap())).await.unwrap();
        assert_eq!(updated.is_error, Some(false));
        let store = crate::store::open_task_store(&dir.path().join(".cas")).unwrap();
        assert_eq!(store.get(&task.id).unwrap().priority.0, 1);
        let before = store.get(&task.id).unwrap();
        let error = service.task(Parameters(serde_json::from_value(serde_json::json!({
            "action":"update", "id":task.id, "labels":"reach:main,journey:PRIMARY-J",
        })).unwrap())).await.unwrap_err();
        assert_eq!(error.data.unwrap()["rule_id"], "finding_main_journey_required");
        assert_eq!(store.get(&task.id).unwrap().labels, before.labels);
        assert_eq!(store.get(&task.id).unwrap().priority, before.priority);
    }
}
