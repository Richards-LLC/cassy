//! QA output is independent of native process/turn liveness. Read only on a
//! worker-status request or the existing director snapshot/delivery jobs.
use std::{collections::HashSet, io::Read, path::{Path, PathBuf}};
use chrono::{DateTime, Utc};
use cas_types::{TaskStatus, TaskType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Advisory {
    pub key: String,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Report {
    pub lines: Vec<String>,
    pub advisories: Vec<Advisory>,
}
impl Report {
    pub fn render(&self) -> String {
        self.lines.iter().cloned().chain(self.advisories.iter().map(|a| a.text.clone()))
            .collect::<Vec<_>>().join("\n")
    }
}

/// Failure is uncertainty, never an empty/healthy roster. Callers surface it.
pub(crate) fn read(cas_dir: &Path, session: Option<&str>, now: DateTime<Utc>) -> anyhow::Result<Report> {
    let Some(session) = session else { return Ok(Report::default()); };
    let config = crate::config::Config::load(cas_dir)?;
    let project = crate::cloud::resolve_canonical_id(cas_dir);
    let data = cas_factory::DirectorData::load_with_stores_for_project(cas_dir, None, false, None, project.as_deref())?;
    let tasks = crate::store::open_task_store(cas_dir)?;
    let agents = crate::store::open_agent_store(cas_dir)?.list(None)?;
    let focus = crate::ui::factory::app::preferred_epic_focus_from_session_metadata_named(session).epic_id;
    let held = crate::ui::factory::app::session_progress_named(session).held_workers;
    let scoped = |task: &&cas_factory::TaskSummary| focus.as_deref().is_some_and(|id| task.epic.as_deref() == Some(id));
    let qa_ids: HashSet<_> = data.ready_tasks.iter().chain(&data.in_progress_tasks)
        .filter(|task| task.task_type != TaskType::Epic)
        .filter_map(|task| tasks.get(&task.id).ok())
        .filter(|task| task.labels.iter().any(|label| matches!(label.as_str(), "qa" | "qa-pass" | "qa-walk")))
        .map(|task| task.id).collect();
    let workers: Vec<_> = data.agents.iter().filter(|summary| agents.iter().any(|agent|
        agent.id == summary.id && agent.role == cas_types::AgentRole::Worker
            && agent.visible_to_factory_session(Some(session)))).collect();
    let mut report = Report::default();
    let base = crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
    let root = crate::config::project_factory_artifacts_root(cas_dir, &base);
    for task in data.in_progress_tasks.iter().filter(|task| qa_ids.contains(&task.id)) {
        let Some(worker) = workers.iter().find(|agent| task.assignee.as_deref()
            .is_some_and(|id| id == agent.id || id == agent.name)) else { continue; };
        let dir = root.join(&task.id);
        let Some(dir) = crate::config::canonical_factory_task_artifact_dir(&base, &dir) else {
            report.lines.push(format!("QA output: {} / {}: ledger unavailable", worker.name, task.id));
            continue;
        };
        let files = ledgers(&dir);
        if files.is_empty() { report.lines.push(format!("QA output: {} / {}: ledger unavailable", worker.name, task.id)); }
        for file in files {
            let name = file.strip_prefix(&dir).unwrap_or(&file).display().to_string();
            match progress(&file, now) {
                Some((stamp, age)) if age >= config.factory().stall_threshold_secs as i64 => {
                    report.advisories.push(Advisory {
                        key: format!("qa-flat:{}:{name}:{stamp}", task.id),
                        text: format!("QA output flat: {} / {} / {name}: no recorded result for {age}s (threshold {}s). Native execution liveness is separate; inspect the walk or split independent surfaces.", worker.name, task.id, config.factory().stall_threshold_secs),
                    });
                }
                Some((_, age)) => report.lines.push(format!("QA output: {} / {} / {name}: last recorded result {age}s ago", worker.name, task.id)),
                None => report.lines.push(format!("QA output: {} / {} / {name}: progress unknown (missing, legacy or invalid progress_at)", worker.name, task.id)),
            }
        }
    }
    let unfinished: Vec<_> = data.ready_tasks.iter().chain(&data.in_progress_tasks)
        .filter(scoped).filter(|task| qa_ids.contains(&task.id)).collect();
    let idle = held.as_ref().and_then(|held| workers.iter().find(|worker| {
        !held.contains(&worker.name) && !held.contains(&worker.id)
            && worker.pending_messages == 0 && !worker.qa_reviewer_between_rounds
            && worker.current_task.is_none() && worker.active_lease.is_none()
            && !data.ready_tasks.iter().chain(&data.in_progress_tasks).any(|task|
                task.assignee.as_deref().is_some_and(|id| id == worker.id || id == worker.name))
            && agents.iter().find(|agent| agent.id == worker.id).is_some_and(|agent| {
                let observed = crate::mcp::tools::service::factory_ops::worker_liveness_for_agent(
                    cas_dir, agent, now, config.factory().stall_threshold_secs as i64);
                matches!(observed.state, crate::mcp::tools::service::worker_liveness::Liveness::WaitingForInput
                    | crate::mcp::tools::service::worker_liveness::Liveness::AwaitingFirstPrompt)
            })
    }));
    if let Some(hint) = split_hint(&unfinished, idle.map(|worker| worker.name.as_str())) {
        report.advisories.push(hint);
    }
    Ok(report)
}

fn split_hint(tasks: &[&cas_factory::TaskSummary], idle: Option<&str>) -> Option<Advisory> {
    let [task] = tasks else { return None; };
    let idle = idle?;
    (task.status == TaskStatus::InProgress).then(|| Advisory {
        key: format!("qa-split:{}:{idle}", task.id),
        text: format!("QA critical path: only {} remains; {idle} is idle. Consider splitting independent surfaces or cases onto that worker with separate personas, profiles and ledgers; keep dependent steps together.", task.id),
    })
}

// Bounded, task-owned discovery only. Do not follow symlinks or external_ref.
fn ledgers(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![(root.to_path_buf(), 0)];
    let mut budget = 256;
    while let Some((dir, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(dir) else { continue; };
        for entry in entries.flatten() {
            if budget == 0 || found.len() == 32 { return found; }
            budget -= 1;
            let Ok(kind) = entry.file_type() else { continue; };
            if kind.is_symlink() { continue; }
            let path = entry.path();
            if kind.is_dir() && depth < 3 { pending.push((path, depth + 1)); }
            else if kind.is_file() && entry.file_name().to_string_lossy().eq_ignore_ascii_case("LEDGER.md") { found.push(path); }
        }
    }
    found.sort();
    found
}

fn progress(_file: &Path, _now: DateTime<Utc>) -> Option<(String, i64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qa_output_flat_while_native_execution_continues_then_record_clears_it() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = tmp.path().join("LEDGER.md");
        let now: DateTime<Utc> = "2026-11-06T12:08:00Z".parse().unwrap();
        std::fs::write(&ledger, "progress_at: 2026-11-06T12:00:00.000Z\ncells=1; PASS=0; FAIL=1\n").unwrap();
        let busy = crate::mcp::tools::service::worker_liveness::Observation {
            state: crate::mcp::tools::service::worker_liveness::Liveness::Executing,
            evidence: "fresh native tool execution".into(),
        };
        assert_eq!(progress(&ledger, now).unwrap().1, 480);
        // Reformatting/replacing the file (fresh mtime) is not a result.
        std::fs::write(&ledger, "# reformatted\nprogress_at: 2026-11-06T12:00:00.000Z\ncells=1; PASS=0; FAIL=1\n").unwrap();
        assert_eq!(progress(&ledger, now).unwrap().1, 480);
        std::fs::write(&ledger, "progress_at: 2026-11-06T12:08:00.000Z\ncells=1; PASS=0; FAIL=1\n").unwrap();
        assert_eq!(progress(&ledger, now).unwrap().1, 0);
        assert!(busy.summary("walk").contains("liveness: executing"));
        for invalid in ["cells=1", "progress_at: unknown", "progress_at: 2026-11-06T12:09:00Z"] {
            std::fs::write(&ledger, invalid).unwrap();
            assert!(progress(&ledger, now).is_none());
        }
        #[cfg(unix)] {
            std::os::unix::fs::symlink(tmp.path(), tmp.path().join("escape")).unwrap();
            assert_eq!(ledgers(tmp.path()), vec![ledger]);
        }
    }
}
