//! QA output is independent of native process/turn liveness. Read only on a
//! worker-status request or the existing director snapshot/delivery jobs.
use cas_types::{TaskStatus, TaskType};
use chrono::{DateTime, Utc};
use std::{
    collections::HashSet,
    io::Read,
    path::{Path, PathBuf},
};

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
        self.lines
            .iter()
            .cloned()
            .chain(self.advisories.iter().map(|a| a.text.clone()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Failure is uncertainty, never an empty/healthy roster. Callers surface it.
pub(crate) fn read(
    cas_dir: &Path,
    session: Option<&str>,
    now: DateTime<Utc>,
) -> anyhow::Result<Report> {
    let Some(session) = session else {
        return Ok(Report::default());
    };
    let config = crate::config::Config::load(cas_dir)?;
    let project = crate::cloud::resolve_canonical_id(cas_dir);
    let data = cas_factory::DirectorData::load_with_stores_for_project(
        cas_dir,
        None,
        false,
        None,
        project.as_deref(),
    )?;
    let tasks = crate::store::open_task_store(cas_dir)?;
    let agents = crate::store::open_agent_store(cas_dir)?.list(None)?;
    let focus =
        crate::ui::factory::preferred_epic_focus_from_session_metadata_named(session).epic_id;
    let held = crate::ui::factory::session_progress_named(session).held_workers;
    let candidates: Vec<_> = data
        .ready_tasks
        .iter()
        .chain(&data.in_progress_tasks)
        .filter(|task| task.task_type != TaskType::Epic)
        .map(|task| tasks.get(&task.id))
        .collect::<Result<_, _>>()?;
    let qa_ids: HashSet<_> = candidates
        .iter()
        .filter(|task| {
            task.labels
                .iter()
                .any(|label| matches!(label.as_str(), "qa" | "qa-pass" | "qa-walk"))
        })
        .map(|task| task.id.clone())
        .collect();
    let workers: Vec<_> = data
        .agents
        .iter()
        .filter(|summary| {
            agents.iter().any(|agent| {
                agent.id == summary.id
                    && agent.is_alive()
                    && agent.role == cas_types::AgentRole::Worker
                    && agent.visible_to_factory_session(Some(session))
            })
        })
        .collect();
    let mut report = Report::default();
    let base =
        crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
    let root = crate::config::project_factory_artifacts_root(cas_dir, &base);
    for task in data
        .in_progress_tasks
        .iter()
        .filter(|task| task.status == TaskStatus::InProgress && qa_ids.contains(&task.id))
    {
        let Some(worker) = workers.iter().find(|agent| {
            task.assignee
                .as_deref()
                .is_some_and(|id| id == agent.id || id == agent.name)
        }) else {
            continue;
        };
        let dir = root.join(&task.id);
        let Some(dir) = crate::config::canonical_factory_task_artifact_dir(&base, &dir) else {
            report.lines.push(format!(
                "QA output: {} / {}: ledger unavailable",
                worker.name, task.id
            ));
            continue;
        };
        let files = ledgers(&dir);
        if files.is_empty() {
            report.lines.push(format!(
                "QA output: {} / {}: ledger unavailable",
                worker.name, task.id
            ));
        }
        for file in files {
            let name = file
                .strip_prefix(&dir)
                .unwrap_or(&file)
                .display()
                .to_string();
            match progress(&file, now) {
                Some((stamp, age)) if age >= config.factory().stall_threshold_secs as i64 => {
                    let advisory = Advisory {
                        key: format!("qa-flat:{}:{name}:{stamp}", task.id),
                        text: format!("QA output flat: {} / {} / {name}: no recorded result for {age}s (threshold {}s). Native execution liveness is separate; inspect the walk or split independent surfaces.", worker.name, task.id, config.factory().stall_threshold_secs),
                    };
                    if held.as_ref().is_none_or(|held| held.contains(&worker.name) || held.contains(&worker.id)) {
                        report.lines.push(advisory.text);
                    } else { report.advisories.push(advisory); }
                }
                Some((_, age)) => report.lines.push(format!("QA output: {} / {} / {name}: last recorded result {age}s ago", worker.name, task.id)),
                None => report.lines.push(format!("QA output: {} / {} / {name}: progress unknown (missing, legacy or invalid progress_at)", worker.name, task.id)),
            }
        }
    }
    let unfinished: Vec<_> = data
        .ready_tasks
        .iter()
        .chain(&data.in_progress_tasks)
        .filter(|task| qa_ids.contains(&task.id) && task.status != TaskStatus::AwaitingMerge)
        .collect();
    let scoped: Vec<_> = unfinished
        .iter()
        .copied()
        .filter(|task| {
            focus
                .as_deref()
                .is_some_and(|id| task.epic.as_deref() == Some(id))
        })
        .collect();
    let active_worker = match scoped.as_slice() {
        [task] if task.status == TaskStatus::InProgress => workers.iter().find(|worker| {
            task.assignee
                .as_deref()
                .is_some_and(|id| id == worker.id || id == worker.name)
        }),
        _ => None,
    };
    let idle = held.as_ref().filter(|held| active_worker.is_some_and(|worker|
        !held.contains(&worker.name) && !held.contains(&worker.id)))
        .and_then(|held| workers.iter().find(|worker| {
        !held.contains(&worker.name) && !held.contains(&worker.id)
            && worker.pending_messages == 0 && !worker.qa_reviewer_between_rounds
            && crate::store::open_prompt_queue_store(cas_dir).ok().is_some_and(|queue|
                queue.peek_for_targets(&[&worker.name, &worker.id], Some(session), 1).is_ok_and(|rows| rows.is_empty()))
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
    if let ([task], Some(idle)) = (scoped.as_slice(), idle) {
        report.advisories.push(Advisory {
            key: format!("qa-split:{}:{}", task.id, idle.name),
            text: format!("QA critical path: only {} remains; {} is idle. Consider splitting independent surfaces or cases onto that worker with separate personas, profiles and ledgers; keep dependent steps together.", task.id, idle.name),
        });
    }
    Ok(report)
}

// Bounded, task-owned discovery only. Do not follow symlinks or external_ref.
fn ledgers(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![(root.to_path_buf(), 0)];
    let mut budget = 256;
    while let Some((dir, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if budget == 0 || found.len() == 32 {
                return found;
            }
            budget -= 1;
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            if path
                .canonicalize()
                .ok()
                .is_none_or(|path| !path.starts_with(root))
            {
                continue;
            }
            if kind.is_dir() && depth < 3 {
                pending.push((path, depth + 1));
            } else if kind.is_file()
                && entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case("LEDGER.md")
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

fn progress(file: &Path, now: DateTime<Utc>) -> Option<(String, i64)> {
    let metadata = std::fs::symlink_metadata(file).ok()?;
    if !metadata.is_file() || metadata.len() > 128 * 1024 {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(file)
        .ok()?
        .take(128 * 1024 + 1)
        .read_to_string(&mut text)
        .ok()?;
    if text.len() > 128 * 1024 {
        return None;
    }
    let mut stamps = text
        .lines()
        .filter_map(|line| line.strip_prefix("progress_at: "));
    let stamp = stamps.next()?;
    if stamps.next().is_some() {
        return None;
    }
    let at = DateTime::parse_from_rfc3339(stamp)
        .ok()?
        .with_timezone(&Utc);
    let age = (now - at).num_seconds();
    (at <= now).then(|| (stamp.to_string(), age))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qa_output_flat_while_native_execution_continues_then_record_clears_it() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = tmp.path().join("LEDGER.md");
        let now: DateTime<Utc> = "2026-11-06T12:08:00Z".parse().unwrap();
        std::fs::write(
            &ledger,
            "progress_at: 2026-11-06T12:00:00.000Z\ncells=1; PASS=0; FAIL=1\n",
        )
        .unwrap();
        let transcript = tmp.path().join("busy.jsonl");
        std::fs::write(
            &transcript,
            serde_json::json!({"timestamp": now, "type": "event_msg",
            "payload": {"type": "turn_started"}})
            .to_string(),
        )
        .unwrap();
        let busy = crate::mcp::tools::service::worker_liveness::observe(
            cas_mux::SupervisorCli::Codex,
            Some(&transcript),
            crate::mcp::tools::service::worker_liveness::ProcessEvidence {
                alive: Some(true),
                cpu_busy: Some(true),
                detail: "native tool execution".into(),
            },
            now,
            300,
        );
        assert_eq!(progress(&ledger, now).unwrap().1, 480);
        // Reformatting/replacing the file (fresh mtime) is not a result.
        std::fs::write(
            &ledger,
            "# reformatted\nprogress_at: 2026-11-06T12:00:00.000Z\ncells=1; PASS=0; FAIL=1\n",
        )
        .unwrap();
        assert_eq!(progress(&ledger, now).unwrap().1, 480);
        std::fs::write(
            &ledger,
            "progress_at: 2026-11-06T12:08:00.000Z\ncells=1; PASS=0; FAIL=1\n",
        )
        .unwrap();
        assert_eq!(progress(&ledger, now).unwrap().1, 0);
        assert!(busy.summary("walk").contains("liveness: executing"));
        for invalid in [
            "cells=1",
            "progress_at: unknown",
            "progress_at: 2026-11-06T12:09:00Z",
        ] {
            std::fs::write(&ledger, invalid).unwrap();
            assert!(progress(&ledger, now).is_none());
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(tmp.path(), tmp.path().join("escape")).unwrap();
            assert_eq!(
                ledgers(&tmp.path().canonicalize().unwrap()),
                vec![ledger.canonicalize().unwrap()]
            );
        }
    }

    #[test]
    fn qa_critical_path_requires_one_remaining_scoped_lane_and_eligible_idle_worker() {
        use cas_store::TaskStore;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let project = tempfile::tempdir().unwrap();
        let cas_dir = crate::store::init_cas_dir(project.path()).unwrap();
        crate::config::Config::default().save(&cas_dir).unwrap();
        let session = "qa-progress-fixture";
        env.set("CAS_FACTORY_SESSION", session);
        let now: DateTime<Utc> = "2026-11-06T12:08:00Z".parse().unwrap();
        let tasks = cas_store::SqliteTaskStore::open(&cas_dir).unwrap();
        let agents = crate::store::open_agent_store(&cas_dir).unwrap();
        let mut epic = cas_types::Task::new("cas-epic".into(), "QA epic".into());
        epic.task_type = TaskType::Epic;
        tasks.add(&epic).unwrap();
        let mut lane = cas_types::Task::new("cas-walk".into(), "edit lane".into());
        lane.assignee = Some("walker".into());
        lane.labels.push("qa".into());
        let mut second = cas_types::Task::new("cas-second".into(), "second QA lane".into());
        second.labels.push("qa".into());
        let mut reserved =
            cas_types::Task::new("cas-reserved".into(), "assigned non-QA work".into());
        reserved.assignee = Some("helper".into());
        for task in [&lane, &second, &reserved] {
            tasks.add(task).unwrap();
            tasks
                .add_dependency(&cas_types::Dependency::new(
                    task.id.clone(),
                    epic.id.clone(),
                    cas_types::DependencyType::ParentChild,
                ))
                .unwrap();
        }
        for name in ["walker", "helper"] {
            let mut agent = cas_types::Agent::new(name.into(), name.into());
            agent.role = cas_types::AgentRole::Worker;
            agent.factory_session = Some(session.into());
            agents.register(&agent).unwrap();
        }
        let account = cas_dir.join("account");
        let sessions = account.join("sessions/2026/11/06");
        std::fs::create_dir_all(&sessions).unwrap();
        let id = "019f84af-3121-7950-ba14-b01db2dad6c7";
        let clone = project.path().canonicalize().unwrap();
        let meta = serde_json::json!({"timestamp": now, "type": "session_meta", "payload": {"session_id":id, "cwd":clone, "originator":"codex-tui", "source":"cli"}});
        let done = serde_json::json!({"timestamp": now, "type":"event_msg", "payload":{"type":"task_complete"}});
        std::fs::write(
            sessions.join(format!("rollout-2026-11-06T12-08-00-{id}.jsonl")),
            format!("{meta}\n{done}\n"),
        )
        .unwrap();
        let mut helper = agents.get("helper").unwrap();
        helper.cc_session_id = Some(id.into());
        helper.machine_id = Some(cas_types::Agent::get_or_generate_machine_id());
        helper.metadata.insert("worker_cli".into(), "codex".into());
        helper
            .metadata
            .insert("clone_path".into(), clone.to_str().unwrap().into());
        helper.metadata.insert(
            "worker_account_dir".into(),
            account.to_str().unwrap().into(),
        );
        let mut metadata = crate::ui::factory::create_metadata(
            session,
            1,
            "supervisor",
            &["walker".into(), "helper".into()],
            None,
            None,
            None,
        );
        let path = crate::ui::factory::metadata_path(session);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        for (case, status, expected) in [
            ("one", TaskStatus::InProgress, true),
            ("multiple", TaskStatus::InProgress, false),
            ("pending", TaskStatus::Open, false),
            ("blocked", TaskStatus::Blocked, false),
            ("foreign epic", TaskStatus::InProgress, false),
            ("unfocused", TaskStatus::InProgress, false),
            ("held helper", TaskStatus::InProgress, false),
            ("held lane", TaskStatus::InProgress, false),
            ("assigned helper", TaskStatus::InProgress, false),
            ("foreign helper", TaskStatus::InProgress, false),
            ("pending message", TaskStatus::InProgress, false),
        ] {
            lane.status = status;
            second.status = if case == "multiple" {
                TaskStatus::Open
            } else {
                TaskStatus::Closed
            };
            reserved.status = if case == "assigned helper" {
                TaskStatus::Open
            } else {
                TaskStatus::Closed
            };
            for task in [&lane, &second, &reserved] {
                tasks.update(task).unwrap();
            }
            metadata.pinned_epic_id = match case {
                "unfocused" => None,
                "foreign epic" => Some("cas-other".into()),
                _ => Some(epic.id.clone()),
            };
            metadata.held_workers = match case {
                "held helper" => vec!["helper".into()],
                "held lane" => vec!["walker".into()],
                _ => vec![],
            };
            std::fs::write(&path, serde_json::to_vec(&metadata).unwrap()).unwrap();
            helper.factory_session = Some(
                if case == "foreign helper" {
                    "other-session"
                } else {
                    session
                }
                .into(),
            );
            agents.update(&helper).unwrap();
            if case == "pending message" {
                crate::store::open_prompt_queue_store(&cas_dir)
                    .unwrap()
                    .enqueue_with_session("supervisor", "helper", "reserved work", session)
                    .unwrap();
            }
            let report = read(&cas_dir, Some(session), now).unwrap();
            assert_eq!(
                report.advisories.len(),
                usize::from(expected),
                "{case}: {report:?}"
            );
            if expected {
                assert!(report.render().contains(&helper.name));
            }
        }
    }
}
