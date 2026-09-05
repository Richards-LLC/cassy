use std::io;

use crate::cli::{Cli, ListArgs};
use crate::store::{find_cas_root_from, open_agent_store, open_prompt_queue_store};
use crate::ui::components::{Formatter, KeyValue, Renderable, StatusLine};
use crate::ui::factory::{SessionInfo, SessionManager, list_sessions};
use crate::ui::theme::ActiveTheme;
use anyhow::{Result, anyhow, bail};
use cas_factory::{DirectorData, SessionType};
use cas_types::{AgentStatus, Event};
use serde::Serialize;

/// List running factory sessions
pub fn execute_list(cli: &Cli, args: &ListArgs) -> Result<()> {
    let mut sessions = list_sessions()?;

    if args.running_only {
        sessions.retain(|s| s.is_running);
    }
    if args.attachable_only {
        sessions.retain(|s| s.can_attach());
    }
    if let Some(ref name) = args.name {
        sessions.retain(|s| &s.name == name);
    }
    if let Some(ref project_dir) = args.project_dir {
        let project_dir = project_dir.to_string_lossy();
        sessions.retain(|s| {
            s.metadata
                .project_dir
                .as_ref()
                .map(|p| p == project_dir.as_ref())
                .unwrap_or(false)
        });
    }

    if sessions.is_empty() {
        if cli.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&SessionListJson::new(vec![]))?
            );
            return Ok(());
        }

        let theme = ActiveTheme::default();
        let mut stdout = io::stdout();
        let mut fmt = Formatter::stdout(&mut stdout, theme);
        StatusLine::info("No factory sessions found.").render(&mut fmt)?;
        fmt.newline()?;
        fmt.info("Start a new session with: cas")?;
        return Ok(());
    }

    if cli.json {
        let json_sessions: Vec<SessionJson> = sessions
            .iter()
            .map(|s| SessionJson::from_session_info(s, cli.full))
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&SessionListJson::new(json_sessions))?
        );
        return Ok(());
    }

    let theme = ActiveTheme::default();
    let mut stdout = io::stdout();
    let mut fmt = Formatter::stdout(&mut stdout, theme);

    fmt.heading("Factory sessions")?;
    fmt.newline()?;

    let mut has_orphaned = false;

    for session in sessions {
        let is_orphaned = session.is_running && !session.socket_exists;
        if is_orphaned {
            has_orphaned = true;
        }

        let status_label = if session.can_attach() {
            "running"
        } else if is_orphaned {
            "orphaned"
        } else if session.is_running {
            "starting"
        } else {
            "stopped"
        };

        let summary = session.to_session_summary();
        let type_badge = session_type_badge_plain(summary.session_type);

        fmt.bullet(&format!(
            "{status_label} {type_badge} {} (workers: {}, pid: {})",
            session.name,
            session.worker_count(),
            session.metadata.daemon_pid
        ))?;

        if let Some(ref project_dir) = session.metadata.project_dir {
            let short_path: String = std::path::Path::new(project_dir)
                .iter()
                .rev()
                .take(2)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(|s| s.to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            fmt.write_muted(&format!("      {short_path}"))?;
            fmt.newline()?;
        }

        if let Some(ref epic) = session.metadata.epic_id {
            fmt.write_muted(&format!("      Epic: {epic}"))?;
            fmt.newline()?;
        }
    }

    fmt.newline()?;
    fmt.info("Attach with: cas attach <name>")?;
    fmt.info("Kill with:   cas kill <name>")?;
    fmt.info("Kill all:    cas kill-all")?;

    if has_orphaned {
        fmt.newline()?;
        StatusLine::warning("Orphaned sessions will be auto-cleaned on next `cas` start.")
            .render(&mut fmt)?;
    }

    Ok(())
}

pub(super) fn execute_sessions(cli: &Cli, attachable_only: bool) -> Result<()> {
    let mut sessions = list_sessions()?;
    if attachable_only {
        sessions.retain(|s| s.can_attach());
    }

    if cli.json {
        let json_sessions: Vec<SessionJson> = sessions
            .iter()
            .map(|s| SessionJson::from_session_info(s, cli.full))
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&SessionListJson::new(json_sessions))?
        );
        return Ok(());
    }

    execute_list(
        cli,
        &ListArgs {
            attachable_only,
            ..Default::default()
        },
    )
}

pub(super) fn execute_agents(
    cli: &Cli,
    session_name: Option<&str>,
    project_dir: Option<&std::path::Path>,
    all: bool,
    cas_root_override: Option<&std::path::Path>,
) -> Result<()> {
    let project_dir = resolve_project_dir(project_dir)?;
    let session = resolve_session(session_name, &project_dir)?;
    let cas_root = cas_root_override
        .map(std::path::Path::to_path_buf)
        .map(Ok)
        .unwrap_or_else(|| cas_root_for_session(&session))?;

    let store = open_agent_store(&cas_root)?;
    let agents = store.list(Some(AgentStatus::Active))?;

    let allowed_names = session_agent_name_set(&session);
    let now = chrono::Utc::now();
    let mut out: Vec<AgentJson> = agents
        .into_iter()
        .filter(|a| all || allowed_names.contains(&a.name))
        .map(|a| AgentJson {
            id: a.id.clone(),
            name: a.name.clone(),
            role: format!("{:?}", a.role).to_lowercase(),
            status: format!("{:?}", a.status).to_lowercase(),
            last_heartbeat_rfc3339: a.last_heartbeat.to_rfc3339(),
            seconds_since_heartbeat: (now - a.last_heartbeat).num_seconds(),
            metadata: a.metadata.clone(),
        })
        .collect();

    out.sort_by(|a, b| a.name.cmp(&b.name));

    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&AgentsJson {
                schema_version: 1,
                session: SessionJson::from_session_info(&session, cli.full),
                agents: out,
            })?
        );
        return Ok(());
    }

    let theme = ActiveTheme::default();
    let mut stdout = io::stdout();
    let mut fmt = Formatter::stdout(&mut stdout, theme);

    fmt.heading(&format!("Agents for session: {}", session.name))?;
    fmt.field(
        "Project",
        &session.metadata.project_dir.clone().unwrap_or_default(),
    )?;
    fmt.newline()?;

    for agent in out {
        fmt.bullet(&format!(
            "{} ({}, heartbeat: {}s ago)",
            agent.name, agent.role, agent.seconds_since_heartbeat
        ))?;
    }

    Ok(())
}

pub(super) fn execute_activity(
    cli: &Cli,
    session_name: Option<&str>,
    project_dir: Option<&std::path::Path>,
    all: bool,
    limit: usize,
    cas_root_override: Option<&std::path::Path>,
) -> Result<()> {
    use cas_store::{EventStore, SqliteEventStore};

    let project_dir = resolve_project_dir(project_dir)?;
    let session = resolve_session(session_name, &project_dir)?;
    let cas_root = cas_root_override
        .map(std::path::Path::to_path_buf)
        .map(Ok)
        .unwrap_or_else(|| cas_root_for_session(&session))?;

    let event_store = SqliteEventStore::open(&cas_root)?;
    event_store.init()?;

    let mut events = event_store.list_recent(limit)?;
    if !all {
        let allowed_names = session_agent_name_set(&session);
        filter_events_for_session_agents(&mut events, &allowed_names);
    }

    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&ActivityJson {
                schema_version: 1,
                session: SessionJson::from_session_info(&session, cli.full),
                events,
            })?
        );
        return Ok(());
    }

    let theme = ActiveTheme::default();
    let mut stdout = io::stdout();
    let mut fmt = Formatter::stdout(&mut stdout, theme);

    fmt.heading(&format!("Activity for session: {}", session.name))?;
    fmt.field(
        "Project",
        &session.metadata.project_dir.clone().unwrap_or_default(),
    )?;
    fmt.newline()?;

    for event in events {
        fmt.bullet(&format!(
            "{} [{}] {}",
            event.created_at.to_rfc3339(),
            format!("{:?}", event.event_type).to_lowercase(),
            event.summary
        ))?;
    }

    Ok(())
}

pub(super) fn execute_targets(
    cli: &Cli,
    session_name: Option<&str>,
    project_dir: Option<&std::path::Path>,
) -> Result<()> {
    let project_dir = resolve_project_dir(project_dir)?;
    let session = resolve_session(session_name, &project_dir)?;

    let supervisor_actual = session.metadata.supervisor.name.clone();
    let workers: Vec<String> = session
        .metadata
        .workers
        .iter()
        .map(|w| w.name.clone())
        .collect();

    let mut aliases = std::collections::HashMap::new();
    aliases.insert("supervisor".to_string(), supervisor_actual.clone());
    aliases.insert("all_workers".to_string(), "all_workers".to_string());

    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&TargetsJson {
                schema_version: 1,
                session: SessionJson::from_session_info(&session, cli.full),
                supervisor: supervisor_actual,
                workers,
                aliases,
            })?
        );
        return Ok(());
    }

    let theme = ActiveTheme::default();
    let mut stdout = io::stdout();
    let mut fmt = Formatter::stdout(&mut stdout, theme);

    fmt.heading(&format!("Targets for session: {}", session.name))?;
    fmt.field(
        "Project",
        &session.metadata.project_dir.clone().unwrap_or_default(),
    )?;
    fmt.newline()?;
    fmt.field("supervisor", &supervisor_actual)?;
    fmt.field("all_workers", "all_workers")?;
    for worker in workers {
        fmt.bullet(&worker)?;
    }

    Ok(())
}

pub(super) fn execute_status(
    cli: &Cli,
    session_name: Option<&str>,
    project_dir: Option<&std::path::Path>,
    activity_limit: usize,
    cas_root_override: Option<&std::path::Path>,
) -> Result<()> {
    let project_dir = resolve_project_dir(project_dir)?;
    let session = resolve_session(session_name, &project_dir)?;
    let cas_root = cas_root_override
        .map(std::path::Path::to_path_buf)
        .map(Ok)
        .unwrap_or_else(|| cas_root_for_session(&session))?;

    let allowed_names = session_agent_name_set(&session);

    let mut data = DirectorData::load_fast(&cas_root)?;
    data.agents.retain(|a| allowed_names.contains(&a.name));
    filter_events_for_session_agents(&mut data.activity, &allowed_names);

    if data.activity.len() > activity_limit {
        data.activity.truncate(activity_limit);
    }

    let queue = open_prompt_queue_store(&cas_root)?;
    let pending = queue.pending_count()?;
    let peek = queue.peek_all(10)?;
    let prompt_queue_peek: Vec<QueuedPromptJson> = peek
        .into_iter()
        .map(|p| QueuedPromptJson {
            id: p.id,
            source: p.source,
            target: p.target,
            created_at_rfc3339: p.created_at.to_rfc3339(),
        })
        .collect();

    let tasks_ready: Vec<TaskSummaryJson> = data
        .ready_tasks
        .into_iter()
        .map(|t| TaskSummaryJson {
            id: t.id,
            title: t.title,
            status: format!("{:?}", t.status).to_lowercase(),
            priority: t.priority.0,
            assignee: t.assignee,
            task_type: format!("{:?}", t.task_type).to_lowercase(),
            epic: t.epic,
            branch: t.branch,
        })
        .collect();
    let tasks_in_progress: Vec<TaskSummaryJson> = data
        .in_progress_tasks
        .into_iter()
        .map(|t| TaskSummaryJson {
            id: t.id,
            title: t.title,
            status: format!("{:?}", t.status).to_lowercase(),
            priority: t.priority.0,
            assignee: t.assignee,
            task_type: format!("{:?}", t.task_type).to_lowercase(),
            epic: t.epic,
            branch: t.branch,
        })
        .collect();
    let epics: Vec<TaskSummaryJson> = data
        .epic_tasks
        .into_iter()
        .map(|t| TaskSummaryJson {
            id: t.id,
            title: t.title,
            status: format!("{:?}", t.status).to_lowercase(),
            priority: t.priority.0,
            assignee: t.assignee,
            task_type: format!("{:?}", t.task_type).to_lowercase(),
            epic: t.epic,
            branch: t.branch,
        })
        .collect();

    let agents: Vec<AgentSummaryJson> = data
        .agents
        .into_iter()
        .map(|a| AgentSummaryJson {
            id: a.id,
            name: a.name,
            status: format!("{:?}", a.status).to_lowercase(),
            current_task: a.current_task,
            latest_activity: a
                .latest_activity
                .map(|(summary, ts)| AgentLatestActivityJson {
                    summary,
                    created_at_rfc3339: ts.to_rfc3339(),
                }),
            last_heartbeat_rfc3339: a.last_heartbeat.map(|ts| ts.to_rfc3339()),
        })
        .collect();

    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&StatusJson {
                schema_version: 1,
                session: SessionJson::from_session_info(&session, cli.full),
                prompt_queue_pending: pending,
                prompt_queue_peek,
                tasks_ready,
                tasks_in_progress,
                epics,
                agents,
                activity: data.activity,
            })?
        );
        return Ok(());
    }

    let theme = ActiveTheme::default();
    let mut stdout = io::stdout();
    let mut fmt = Formatter::stdout(&mut stdout, theme);

    fmt.heading(&format!("Status for session: {}", session.name))?;
    KeyValue::new()
        .add(
            "Project",
            session.metadata.project_dir.clone().unwrap_or_default(),
        )
        .add("Pending prompts", pending.to_string())
        .add("Ready tasks", tasks_ready.len().to_string())
        .add("In-progress tasks", tasks_in_progress.len().to_string())
        .add("Agents", agents.len().to_string())
        .add(
            "Actionable-idle minutes",
            session
                .metadata
                .actionable_idle_minutes_at(chrono::Utc::now())
                .to_string(),
        )
        .render(&mut fmt)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_message(
    cli: &Cli,
    session_name: Option<&str>,
    project_dir: Option<&std::path::Path>,
    target: &str,
    message: &str,
    from: &str,
    no_wrap: bool,
    wait_ack: bool,
    timeout_ms: u64,
    cas_root_override: Option<&std::path::Path>,
) -> Result<()> {
    use cas_store::{EventStore, SqliteEventStore};
    use cas_types::{EventEntityType, EventType};

    let project_dir = resolve_project_dir(project_dir)?;
    let session = resolve_session(session_name, &project_dir)?;
    let cas_root = cas_root_override
        .map(std::path::Path::to_path_buf)
        .map(Ok)
        .unwrap_or_else(|| cas_root_for_session(&session))?;

    let resolved_target = if target == "supervisor" {
        session.metadata.supervisor.name.clone()
    } else {
        target.to_string()
    };

    let queue = open_prompt_queue_store(&cas_root)?;
    let payload = if no_wrap {
        message.to_string()
    } else {
        let response_hint = format!(
            "To respond, use: coordination action=message target={} message=\"...\"\n\nDO NOT USE SENDMESSAGE.",
            from.trim()
        );
        format!("{}\n\n{}", message.trim_end(), response_hint)
    };
    let factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
    let message_id = if let Some(ref session) = factory_session {
        queue.enqueue_with_session(from, &resolved_target, &payload, session)?
    } else {
        queue.enqueue(from, &resolved_target, &payload)?
    };

    let mut ack_event_id: Option<i64> = None;
    if wait_ack {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        let store = SqliteEventStore::open(&cas_root)?;
        while std::time::Instant::now() < deadline {
            let recent = store.list_by_type(EventType::SupervisorInjected, 25)?;
            let found = recent.into_iter().find(|e| {
                e.metadata
                    .as_ref()
                    .and_then(|m| m.get("prompt_id"))
                    .and_then(|v| v.as_i64())
                    == Some(message_id)
                    && e.metadata
                        .as_ref()
                        .and_then(|m| m.get("status"))
                        .and_then(|v| v.as_str())
                        == Some("ok")
                    && (e.entity_type == EventEntityType::Agent)
            });
            if let Some(ev) = found {
                ack_event_id = Some(ev.id);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    if cli.json {
        #[derive(Serialize)]
        #[serde(rename_all = "snake_case")]
        struct MessageResult {
            schema_version: u32,
            session: String,
            target: String,
            enqueued: bool,
            message_id: i64,
            #[serde(skip_serializing_if = "Option::is_none")]
            ack_event_id: Option<i64>,
        }

        println!(
            "{}",
            serde_json::to_string_pretty(&MessageResult {
                schema_version: 1,
                session: session.name,
                target: resolved_target,
                enqueued: true,
                message_id,
                ack_event_id,
            })?
        );
        return Ok(());
    }

    let theme = ActiveTheme::default();
    let mut stdout = io::stdout();
    let mut fmt = Formatter::stdout(&mut stdout, theme);
    StatusLine::success(format!(
        "Enqueued message {} for {} (session: {})",
        message_id, resolved_target, session.name
    ))
    .render(&mut fmt)?;
    Ok(())
}

fn resolve_project_dir(project_dir: Option<&std::path::Path>) -> Result<std::path::PathBuf> {
    if let Some(path) = project_dir {
        return Ok(path.to_path_buf());
    }

    // When running from a git worktree (e.g., .cas/worktrees/<name>/), the CWD
    // won't match the factory session's registered project_dir. Use find_cas_root()
    // which already handles worktree detection via CAS_ROOT env and .git file parsing,
    // then derive the project root from the .cas directory.
    if let Ok(cas_root) = crate::store::find_cas_root() {
        if let Some(project_root) = cas_root.parent() {
            return Ok(project_root.to_path_buf());
        }
    }

    Ok(std::env::current_dir()?)
}

fn resolve_session(
    session_name: Option<&str>,
    project_dir: &std::path::Path,
) -> Result<SessionInfo> {
    let manager = SessionManager::new();

    if let Some(name) = session_name {
        return manager
            .find_session(Some(name))?
            .ok_or_else(|| anyhow!("Session '{name}' not found"));
    }

    let project_dir_str = project_dir.to_string_lossy().to_string();
    manager
        .find_session_for_project(None, &project_dir_str)?
        .ok_or_else(|| {
            anyhow!(
                "No running factory sessions found for project '{}'. Try `cas list`.",
                project_dir.display()
            )
        })
}

fn cas_root_for_session(session: &SessionInfo) -> Result<std::path::PathBuf> {
    let Some(project_dir) = session.metadata.project_dir.as_ref() else {
        bail!(
            "Session '{}' has no project_dir in metadata; cannot resolve Cassy root",
            session.name
        );
    };

    let project_path = std::path::PathBuf::from(project_dir);
    Ok(find_cas_root_from(&project_path)?)
}

fn session_agent_name_set(session: &SessionInfo) -> std::collections::HashSet<String> {
    let mut allowed = std::collections::HashSet::new();
    allowed.insert(session.metadata.supervisor.name.clone());
    for worker in &session.metadata.workers {
        allowed.insert(worker.name.clone());
    }
    allowed
}

fn filter_events_for_session_agents(
    events: &mut Vec<Event>,
    allowed_names: &std::collections::HashSet<String>,
) {
    events.retain(|e| {
        e.session_id
            .as_ref()
            .map(|sid| allowed_names.iter().any(|n| sid.contains(n)))
            .unwrap_or(false)
            || allowed_names.iter().any(|n| e.entity_id.contains(n))
    });
}

fn session_type_badge_plain(session_type: SessionType) -> &'static str {
    match session_type {
        SessionType::Factory => "[FAC]",
        SessionType::Managed => "[MAN]",
        SessionType::Recording => "[REC]",
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct SessionListJson {
    schema_version: u32,
    sessions: Vec<SessionJson>,
}

impl SessionListJson {
    fn new(sessions: Vec<SessionJson>) -> Self {
        Self {
            schema_version: 1,
            sessions,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct SessionJson {
    name: String,
    created_at: String,
    daemon_pid: u32,
    socket_path: String,
    ws_port: Option<u16>,
    project_dir: Option<String>,
    epic_id: Option<String>,
    supervisor: String,
    workers: Vec<String>,
    worker_count: usize,
    is_running: bool,
    socket_exists: bool,
    can_attach: bool,
    actionable_idle_minutes: u64,

    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
}

impl SessionJson {
    fn from_session_info(session: &SessionInfo, include_metadata: bool) -> Self {
        let workers = session.worker_names();
        Self {
            name: session.name.clone(),
            created_at: session.metadata.created_at.clone(),
            daemon_pid: session.metadata.daemon_pid,
            socket_path: session.metadata.socket_path.clone(),
            ws_port: session.metadata.ws_port,
            project_dir: session.metadata.project_dir.clone(),
            epic_id: session.metadata.epic_id.clone(),
            supervisor: session.metadata.supervisor.name.clone(),
            worker_count: workers.len(),
            workers,
            is_running: session.is_running,
            socket_exists: session.socket_exists,
            can_attach: session.can_attach(),
            actionable_idle_minutes: session
                .metadata
                .actionable_idle_minutes_at(chrono::Utc::now()),
            metadata: if include_metadata {
                serde_json::to_value(&session.metadata).ok()
            } else {
                None
            },
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct AgentsJson {
    schema_version: u32,
    session: SessionJson,
    agents: Vec<AgentJson>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct AgentJson {
    id: String,
    name: String,
    role: String,
    status: String,
    last_heartbeat_rfc3339: String,
    seconds_since_heartbeat: i64,
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    metadata: std::collections::HashMap<String, String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct ActivityJson {
    schema_version: u32,
    session: SessionJson,
    events: Vec<Event>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct TargetsJson {
    schema_version: u32,
    session: SessionJson,
    supervisor: String,
    workers: Vec<String>,
    aliases: std::collections::HashMap<String, String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct StatusJson {
    schema_version: u32,
    session: SessionJson,
    prompt_queue_pending: usize,
    prompt_queue_peek: Vec<QueuedPromptJson>,
    tasks_ready: Vec<TaskSummaryJson>,
    tasks_in_progress: Vec<TaskSummaryJson>,
    epics: Vec<TaskSummaryJson>,
    agents: Vec<AgentSummaryJson>,
    activity: Vec<Event>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct TaskSummaryJson {
    id: String,
    title: String,
    status: String,
    priority: i32,
    assignee: Option<String>,
    task_type: String,
    epic: Option<String>,
    branch: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct AgentSummaryJson {
    id: String,
    name: String,
    status: String,
    current_task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_activity: Option<AgentLatestActivityJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_heartbeat_rfc3339: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct AgentLatestActivityJson {
    summary: String,
    created_at_rfc3339: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct QueuedPromptJson {
    id: i64,
    source: String,
    target: String,
    created_at_rfc3339: String,
}
