//! Cloud presence (cas-f707): an agent stays registered with Cassy Cloud.
//!
//! Peer discovery, peer messaging and claim refusal all read the cloud agent
//! registry, so an agent that is not registered there is invisible to its
//! peers. Registration used to be one-shot from SessionStart, with the
//! failure printed and forgotten. This module keeps it alive instead:
//!
//! - [`register_now`] pushes the registration and records the outcome.
//! - [`tick`] runs on every daemon heartbeat: it registers when the agent
//!   has no record, or its last attempt failed and the backoff ran out;
//!   otherwise it heartbeats with the agent's project, epic focus, branch
//!   and factory session in the body (merged by the cloud, so a changed
//!   focus or branch shows on the next beat), and a heartbeat the cloud
//!   refuses with 404 or 401 registers again.
//! - The outcome is persisted per agent under `.cas/cloud_presence/`, so
//!   `coordination action=peers`, `worker_status` and `cas status` show it
//!   from any process ([`render_line`]).

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cloud::coordinator::HeartbeatError;
use crate::cloud::{CloudConfig, CloudCoordinator};
use crate::types::Agent;

/// Directory under the project's `.cas` holding one record per agent.
pub const PRESENCE_DIR: &str = "cloud_presence";
/// Per-request cap for presence calls, so a slow cloud never holds a
/// heartbeat tick for the coordinator's default thirty seconds.
pub const PRESENCE_TIMEOUT: Duration = Duration::from_secs(10);
const SESSION_ENDED_CAUSE: &str = "session ended; cloud shutdown pending or completed";
/// Backoff after the second consecutive failure; it doubles from here.
pub const BACKOFF_BASE_SECS: i64 = 30;
/// Longest wait between two registration attempts.
pub const BACKOFF_MAX_SECS: i64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceState {
    /// The cloud accepted the last registration or heartbeat.
    Registered,
    /// The last attempt failed; see `cause`.
    Failing,
}

/// The last known cloud presence of one agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceRecord {
    pub agent_id: String,
    pub state: PresenceState,
    /// Why the last attempt failed. `None` while registered.
    #[serde(default)]
    pub cause: Option<String>,
    /// Consecutive failed attempts since the last success.
    #[serde(default)]
    pub failures: u32,
    pub last_attempt_at: DateTime<Utc>,
    /// When the cloud last accepted a registration.
    #[serde(default)]
    pub registered_at: Option<DateTime<Utc>>,
    /// Earliest time the next registration attempt is due while failing.
    #[serde(default)]
    pub next_retry_at: Option<DateTime<Utc>>,
    /// The epic focus carried by the last accepted registration or beat.
    #[serde(default)]
    pub focus: Option<String>,
    /// The branch carried by the last accepted registration or beat.
    #[serde(default)]
    pub branch: Option<String>,
}

/// What one [`tick`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TickAction {
    /// Registered (first time, retry, or focus change).
    Registered,
    /// The cloud refused a heartbeat as unknown; registered again.
    Reregistered,
    /// Heartbeat accepted (with the current focus and branch).
    Heartbeat,
    /// The attempt failed; the record carries the cause.
    Failed(String),
    /// Failing, and the backoff has not run out yet.
    Waiting,
}

/// How long to wait after `failures` consecutive failures: the first is
/// retried on the next heartbeat tick, then 30 s, 60 s, ... up to 5 min.
pub fn backoff_after(failures: u32) -> chrono::Duration {
    if failures <= 1 {
        return chrono::Duration::zero();
    }
    let shift = (failures - 2).min(16);
    chrono::Duration::seconds((BACKOFF_BASE_SECS << shift).min(BACKOFF_MAX_SECS))
}

fn record_path(cas_root: &Path, agent_id: &str) -> PathBuf {
    let file: String = agent_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    cas_root.join(PRESENCE_DIR).join(format!("{file}.json"))
}

/// The recorded presence of `agent_id`, if any attempt was ever made.
pub fn load(cas_root: &Path, agent_id: &str) -> Option<PresenceRecord> {
    let text = std::fs::read_to_string(record_path(cas_root, agent_id)).ok()?;
    serde_json::from_str::<PresenceRecord>(&text)
        .ok()
        .filter(|record| record.agent_id == agent_id)
}

/// Persist `record` as its agent's presence (written atomically).
pub fn save(cas_root: &Path, record: &PresenceRecord) {
    let path = record_path(cas_root, &record.agent_id);
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap_or(cas_root))?;
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(record)?)?;
        std::fs::rename(&tmp, &path)
    };
    if let Err(error) = write() {
        tracing::debug!(agent = %record.agent_id, %error, "could not record cloud presence");
    }
}

/// The epic focus `agent` registers with: its factory session's pinned (or
/// default) epic, read fresh so a `focus_epic` shows on the next push.
pub fn current_focus(agent: &Agent) -> Option<String> {
    agent
        .factory_session
        .as_deref()
        .and_then(crate::ui::factory::preferred_epic_id_from_session_metadata_named)
}

/// Publish the focused epic's title beside its id (cas-8522), so peers can
/// read what this supervisor owns. Empty clears a previous title.
fn publish_focus_title(cas_root: &Path, agent: &mut Agent, focus: Option<&str>) {
    let title = focus
        .and_then(|id| crate::store::open_task_store_local(cas_root).ok()?.get(id).ok())
        .map(|epic| epic.title.trim().chars().take(120).collect::<String>())
        .unwrap_or_default();
    agent
        .metadata
        .insert(crate::cloud::peers::META_FOCUS_TITLE.into(), title);
}

/// The directory `agent` works in: its registered clone path, else the
/// project root that holds `cas_root`.
fn work_dir(cas_root: &Path, agent: &Agent) -> PathBuf {
    agent
        .metadata
        .get("clone_path")
        .map(|path| path.trim())
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| cas_root.parent().unwrap_or(cas_root).to_path_buf())
}

/// The branch checked out in `dir`, read from git's `HEAD` without running
/// git (a worktree's `.git` file is followed). `None` when `dir` is not a
/// checkout or `HEAD` is detached.
pub fn checked_out_branch(dir: &Path) -> Option<String> {
    let dot_git = dir.join(".git");
    let git_dir = if dot_git.is_file() {
        let pointer = std::fs::read_to_string(&dot_git).ok()?;
        let target = pointer.trim().strip_prefix("gitdir:")?.trim();
        let target = PathBuf::from(target);
        if target.is_absolute() {
            target
        } else {
            dir.join(target)
        }
    } else {
        dot_git
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    head.trim()
        .strip_prefix("ref:")
        .map(str::trim)
        .map(|reference| reference.strip_prefix("refs/heads/").unwrap_or(reference))
        .filter(|branch| !branch.is_empty())
        .map(str::to_string)
}

/// The branch `agent` registers with (cas-f707).
pub fn current_branch(cas_root: &Path, agent: &Agent) -> Option<String> {
    checked_out_branch(&work_dir(cas_root, agent))
}

/// The heartbeat body's metadata: project binding, focus, branch, factory
/// session and machine label (so a row registered before the cloud's
/// project binding heals on its next beat). Focus and branch are always sent (empty when none) so a cleared
/// one clears in the cloud too; the project binding and session are sent
/// only when known, since omitting them keeps what the cloud has.
pub fn heartbeat_metadata(
    coordinator: &CloudCoordinator,
    agent: &Agent,
    focus: Option<&str>,
    branch: Option<&str>,
) -> serde_json::Map<String, serde_json::Value> {
    use crate::cloud::peers::{
        META_BRANCH, META_CANONICAL_ID, META_FACTORY_SESSION, META_FOCUS, META_HOSTNAME,
    };
    let mut metadata = serde_json::Map::new();
    metadata.insert(META_HOSTNAME.into(), coordinator.machine_label().into());
    if let Some(canonical_id) = coordinator.canonical_id() {
        metadata.insert(META_CANONICAL_ID.into(), canonical_id.into());
    }
    metadata.insert(META_FOCUS.into(), focus.unwrap_or_default().into());
    metadata.insert(META_BRANCH.into(), branch.unwrap_or_default().into());
    for key in [
        "clone_path",
        "scratch_root",
        "scratch_scope",
        "peer_prs_v1",
        "peer_splits_v1",
        crate::cloud::peers::META_FOCUS_TITLE,
        crate::cloud::peers::META_HARNESS_AUTH_FAILED,
    ] {
        metadata.insert(key.into(), agent.metadata.get(key).cloned().unwrap_or_default().into());
    }
    if let Some(session) = agent.factory_session.as_deref().filter(|s| !s.is_empty()) {
        metadata.insert(META_FACTORY_SESSION.into(), session.into());
    }
    metadata
}

fn failing(
    previous: Option<&PresenceRecord>,
    agent_id: &str,
    cause: String,
    now: DateTime<Utc>,
) -> PresenceRecord {
    let failures = previous
        .filter(|record| record.state == PresenceState::Failing)
        .map(|record| record.failures)
        .unwrap_or(0)
        .saturating_add(1);
    PresenceRecord {
        agent_id: agent_id.to_string(),
        state: PresenceState::Failing,
        cause: Some(cause),
        failures,
        last_attempt_at: now,
        registered_at: previous.and_then(|record| record.registered_at),
        next_retry_at: Some(now + backoff_after(failures)),
        focus: previous.and_then(|record| record.focus.clone()),
        branch: previous.and_then(|record| record.branch.clone()),
    }
}

/// Push `agent`'s registration (with its current focus) and record the
/// outcome.
pub fn register_now(
    coordinator: &CloudCoordinator,
    cas_root: &Path,
    agent: &Agent,
    now: DateTime<Utc>,
) -> PresenceRecord {
    register_with_shutdown(
        coordinator,
        cas_root,
        agent,
        now,
        &mut super::shutdown::Snapshot::load(cas_root),
    )
}

fn register_with_shutdown(
    coordinator: &CloudCoordinator,
    cas_root: &Path,
    agent: &Agent,
    now: DateTime<Utc>,
    shutdown: &mut super::shutdown::Snapshot,
) -> PresenceRecord {
    let _lease = match super::shutdown::presence_lease(cas_root) {
        Ok(Some(lease)) => lease,
        Ok(None) => {
            return failing(
                load(cas_root, &agent.id).as_ref(),
                &agent.id,
                "cloud presence busy; retry on next heartbeat".into(),
                now,
            );
        }
        Err(error) => {
            return failing(
                load(cas_root, &agent.id).as_ref(),
                &agent.id,
                format!("cloud presence lease: {error}"),
                now,
            );
        }
    };
    shutdown.refresh_after_lease(cas_root);
    let cause = match shutdown.is_ended(cas_root, agent, now) {
        Ok(false) => None,
        Ok(true) => Some(SESSION_ENDED_CAUSE.into()),
        Err(error) => Some(error),
    };
    if let Some(cause) = cause {
        let record = failing(None, &agent.id, cause, now);
        save(cas_root, &record);
        return record;
    }
    let previous =
        load(cas_root, &agent.id).filter(|record| record.last_attempt_at >= agent.registered_at);
    let focus = current_focus(agent);
    let branch = current_branch(cas_root, agent);
    let mut coordinator = coordinator.clone().with_timeout(PRESENCE_TIMEOUT).with_factory_slot_id(
        crate::config::Config::load(cas_root).ok().and_then(|config| config.factory().slot_id),
    );
    let mut published_agent = super::peers_detail::presence_agent(cas_root, agent);
    publish_focus_title(cas_root, &mut published_agent, focus.as_deref());
    let record =
        match coordinator.register_with_presence(&published_agent, focus.as_deref(), branch.as_deref()) {
            Ok(_) => PresenceRecord {
                agent_id: agent.id.clone(),
                state: PresenceState::Registered,
                cause: None,
                failures: 0,
                last_attempt_at: now,
                registered_at: Some(now),
                next_retry_at: None,
                focus,
                branch,
            },
            Err(error) => failing(previous.as_ref(), &agent.id, error.to_string(), now),
        };
    save(cas_root, &record);
    record
}

/// Register `agent` unless it is already registered with its current focus.
/// SessionStart calls this, so a replayed SessionStart does not re-push.
pub fn ensure_registered(
    coordinator: &CloudCoordinator,
    cas_root: &Path,
    agent: &Agent,
    now: DateTime<Utc>,
) -> PresenceRecord {
    match load(cas_root, &agent.id) {
        Some(record)
            if record.state == PresenceState::Registered
                && record
                    .registered_at
                    .is_some_and(|at| at >= agent.registered_at)
                && record.focus == current_focus(agent)
                && record.branch == current_branch(cas_root, agent) =>
        {
            record
        }
        _ => register_now(coordinator, cas_root, agent, now),
    }
}

/// Make a failing registration due now (cas-66c7): the login it failed
/// with has changed, so waiting out the backoff would only delay recovery.
pub fn retry_now(cas_root: &Path, agent_id: &str) {
    if let Some(mut record) = load(cas_root, agent_id)
        && record.state == PresenceState::Failing
        && record.next_retry_at.is_some()
    {
        record.next_retry_at = None;
        save(cas_root, &record);
    }
}

/// One heartbeat-tick of presence upkeep for `agent`. See the module docs.
pub fn tick(
    coordinator: &CloudCoordinator,
    cas_root: &Path,
    agent: &Agent,
    now: DateTime<Utc>,
) -> TickAction {
    tick_with_shutdown(
        coordinator,
        cas_root,
        agent,
        now,
        &mut super::shutdown::Snapshot::load(cas_root),
    )
}

pub(crate) fn tick_with_shutdown(
    coordinator: &CloudCoordinator,
    cas_root: &Path,
    agent: &Agent,
    now: DateTime<Utc>,
    shutdown: &mut super::shutdown::Snapshot,
) -> TickAction {
    match shutdown.is_ended(cas_root, agent, now) {
        Ok(true) => return TickAction::Waiting,
        Ok(false) => {}
        Err(cause) => {
            save(
                cas_root,
                &failing(
                    load(cas_root, &agent.id).as_ref(),
                    &agent.id,
                    cause.clone(),
                    now,
                ),
            );
            return TickAction::Failed(cause);
        }
    }
    let outcome = |record: PresenceRecord, ok: TickAction| match record.state {
        PresenceState::Registered => ok,
        PresenceState::Failing if record.cause.as_deref() == Some(SESSION_ENDED_CAUSE) => {
            TickAction::Waiting
        }
        PresenceState::Failing => TickAction::Failed(record.cause.unwrap_or_default()),
    };
    let Some(record) = load(cas_root, &agent.id) else {
        return outcome(
            register_with_shutdown(coordinator, cas_root, agent, now, shutdown),
            TickAction::Registered,
        );
    };
    if record.last_attempt_at < agent.registered_at
        || (record
            .registered_at
            .is_none_or(|at| at < agent.registered_at)
            && record.state == PresenceState::Registered)
    {
        return outcome(
            register_with_shutdown(coordinator, cas_root, agent, now, shutdown),
            TickAction::Registered,
        );
    }
    if record.state == PresenceState::Failing {
        if record.next_retry_at.is_some_and(|at| at > now) {
            return TickAction::Waiting;
        }
        return outcome(
            register_with_shutdown(coordinator, cas_root, agent, now, shutdown),
            TickAction::Registered,
        );
    }
    let focus = current_focus(agent);
    let branch = current_branch(cas_root, agent);
    let mut published_agent = super::peers_detail::presence_agent(cas_root, agent);
    publish_focus_title(cas_root, &mut published_agent, focus.as_deref());
    let metadata = heartbeat_metadata(coordinator, &published_agent, focus.as_deref(), branch.as_deref());
    let heartbeat = {
        let _lease = match super::shutdown::presence_lease(cas_root) {
            Ok(Some(lease)) => lease,
            Ok(None) => return TickAction::Waiting,
            Err(error) => return TickAction::Failed(format!("cloud presence lease: {error}")),
        };
        shutdown.refresh_after_lease(cas_root);
        match shutdown.is_ended(cas_root, agent, now) {
            Ok(true) => return TickAction::Waiting,
            Ok(false) => {}
            Err(cause) => return TickAction::Failed(cause),
        }
        coordinator.heartbeat_presence(&agent.id, Some(&metadata))
    };
    match heartbeat {
        Ok(_) => {
            if record.focus != focus || record.branch != branch {
                save(
                    cas_root,
                    &PresenceRecord {
                        focus,
                        branch,
                        last_attempt_at: now,
                        ..record
                    },
                );
            }
            TickAction::Heartbeat
        }
        Err(HeartbeatError::NotRegistered { .. }) => outcome(
            register_with_shutdown(coordinator, cas_root, agent, now, shutdown),
            TickAction::Reregistered,
        ),
        Err(HeartbeatError::Failed(error)) => {
            let record = failing(Some(&record), &agent.id, format!("heartbeat: {error}"), now);
            save(cas_root, &record);
            TickAction::Failed(record.cause.unwrap_or_default())
        }
    }
}

/// Whether the project explicitly opted out of cloud sync
/// (`cloud.auto_sync = false`). Then its agents are not registered: agent
/// names, hostnames, branches and focus never leave the machine. Unset means
/// on (the default).
pub fn opted_out(cas_root: &Path) -> bool {
    crate::config::Config::load(cas_root)
        .ok()
        .and_then(|config| config.cloud)
        .is_some_and(|cloud| !cloud.auto_sync)
}

/// The presence lines for a project that opted out (wrapped).
pub fn disabled_lines() -> String {
    wrap_line(
        "Cloud presence: disabled (cloud.auto_sync=false): this project's agents \
         are not registered with Cassy Cloud.",
    )
}

/// The project's cloud coordinator (its own cloud config, scoped to its
/// canonical id), or `None` when the project is not logged in or opted out
/// of cloud sync.
pub fn project_coordinator(cas_root: &Path) -> Option<CloudCoordinator> {
    if opted_out(cas_root) {
        return None;
    }
    let config = CloudConfig::load_from_cas_dir(cas_root).ok()?;
    if !config.is_logged_in() {
        return None;
    }
    Some(
        CloudCoordinator::new(config)
            .ok()?
            .with_canonical_id(crate::cloud::resolve_canonical_id(cas_root))
            .with_machine_label(Some(crate::cloud::peers::machine_label_for(cas_root))),
    )
}

/// Re-push `agent_id`'s registration now, e.g. after `focus_epic` changed
/// what it registers. `None` when the project is not logged in or the
/// agent is unknown.
pub fn push_agent(cas_root: &Path, agent_id: &str) -> Option<PresenceRecord> {
    let coordinator = project_coordinator(cas_root)?;
    let agent = crate::store::open_agent_store(cas_root)
        .ok()?
        .get(agent_id)
        .ok()?;
    Some(register_now(&coordinator, cas_root, &agent, Utc::now()))
}

fn age(secs: i64) -> String {
    match secs.max(0) {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// Status lines are wrapped to this many columns, so `cas status` (which
/// indents them) still fits an 80-column terminal.
pub const WRAP_COLUMNS: usize = 72;

/// `text` word-wrapped to [`WRAP_COLUMNS`], continuation lines indented by
/// two spaces, ending in a newline. A single word longer than the width is
/// kept whole on its own line.
pub fn wrap_line(text: &str) -> String {
    let mut out = String::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let indent = if out.is_empty() { 0 } else { 2 };
        if !line.is_empty()
            && indent + line.chars().count() + 1 + word.chars().count() > WRAP_COLUMNS
        {
            if !out.is_empty() {
                out.push_str("  ");
            }
            out.push_str(&line);
            out.push('\n');
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        if !out.is_empty() {
            out.push_str("  ");
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// The presence lines for `coordination action=peers`, `worker_status` and
/// `cas status`: "Cloud presence: registered ..." or "Cloud presence:
/// failing: <cause>" plus the retry state, wrapped to [`WRAP_COLUMNS`].
pub fn render_line(record: Option<&PresenceRecord>, now: DateTime<Utc>) -> String {
    let Some(record) = record else {
        return wrap_line(
            "Cloud presence: not registered yet (no registration attempt recorded for \
             this agent)",
        );
    };
    match record.state {
        PresenceState::Registered => wrap_line(&format!(
            "Cloud presence: registered (focus {}, branch {}, registered {} ago)",
            record.focus.as_deref().unwrap_or("-"),
            record.branch.as_deref().unwrap_or("-"),
            age(record
                .registered_at
                .map(|at| (now - at).num_seconds())
                .unwrap_or_default()),
        )),
        PresenceState::Failing => {
            let retry = match record.next_retry_at {
                Some(at) if at > now => format!("next retry in {}", age((at - now).num_seconds())),
                _ => "retrying on the next heartbeat".to_string(),
            };
            format!(
                "{}{}{}",
                wrap_line(&format!(
                    "Cloud presence: failing: {}",
                    record.cause.as_deref().unwrap_or("unknown error")
                )),
                // Short by construction ("  12 failed attempts, next retry in 5m.").
                format!(
                    "  {} failed attempt{}, {retry}.\n",
                    record.failures,
                    if record.failures == 1 { "" } else { "s" },
                ),
                "  Peers cannot see this agent until it registers.\n",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cas_f707_backoff_is_bounded_and_retries_the_first_failure_next_tick() {
        let secs = |n| backoff_after(n).num_seconds();
        assert_eq!(secs(1), 0);
        assert_eq!(secs(2), 30);
        assert_eq!(secs(3), 60);
        assert_eq!(secs(4), 120);
        assert_eq!(secs(5), 240);
        assert_eq!(secs(6), 300);
        assert_eq!(secs(60), 300);
    }

    #[test]
    fn cas_f707_render_line_names_the_failure_cause() {
        let now = Utc::now();
        let record = PresenceRecord {
            agent_id: "sup-1".into(),
            state: PresenceState::Failing,
            cause: Some("Failed to register agent (503): registry unavailable".into()),
            failures: 2,
            last_attempt_at: now,
            registered_at: None,
            next_retry_at: Some(now + chrono::Duration::seconds(30)),
            focus: None,
            branch: None,
        };
        let line = render_line(Some(&record), now);
        assert!(
            line.starts_with("Cloud presence: failing: Failed to register agent (503)"),
            "{line}"
        );
        assert!(
            line.contains("2 failed attempts, next retry in 30s."),
            "{line}"
        );
        assert!(
            line.contains("Peers cannot see this agent until it registers."),
            "{line}"
        );
        assert!(
            line.lines().all(|l| l.chars().count() <= WRAP_COLUMNS + 2),
            "{line}"
        );
        let registered = PresenceRecord {
            state: PresenceState::Registered,
            cause: None,
            failures: 0,
            registered_at: Some(now),
            next_retry_at: None,
            focus: Some("cas-571d".into()),
            branch: Some("epic/x".into()),
            ..record
        };
        assert!(
            render_line(Some(&registered), now)
                .starts_with("Cloud presence: registered (focus cas-571d, branch epic/x"),
        );
    }

    #[test]
    fn cas_f707_wrap_line_fits_the_width_and_indents_continuations() {
        let text = "Cloud presence: failing: Failed to register agent (503): registry \
                    unavailable because the upstream registry returned an error page";
        let wrapped = wrap_line(text);
        let lines: Vec<&str> = wrapped.lines().collect();
        assert!(lines.len() > 1, "{wrapped}");
        assert!(
            lines.iter().all(|l| l.chars().count() <= WRAP_COLUMNS),
            "{wrapped}"
        );
        assert!(lines[1..].iter().all(|l| l.starts_with("  ")), "{wrapped}");
        let rejoined: Vec<&str> = wrapped.split_whitespace().collect();
        assert_eq!(rejoined, text.split_whitespace().collect::<Vec<_>>());
        assert_eq!(wrap_line("short"), "short\n");
    }

    #[test]
    fn cas_f707_branch_is_read_from_head_and_follows_a_worktree_pointer() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/epic/same-repo\n").unwrap();
        assert_eq!(checked_out_branch(&repo).as_deref(), Some("epic/same-repo"));

        let worktree = dir.path().join("wt");
        let gitdir = repo.join(".git/worktrees/wt");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            gitdir.join("HEAD"),
            "ref: refs/heads/factory/fierce-cheetah-73\n",
        )
        .unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", gitdir.display()),
        )
        .unwrap();
        assert_eq!(
            checked_out_branch(&worktree).as_deref(),
            Some("factory/fierce-cheetah-73")
        );

        std::fs::write(
            gitdir.join("HEAD"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .unwrap();
        assert_eq!(
            checked_out_branch(&worktree),
            None,
            "detached HEAD has no branch"
        );
        assert_eq!(checked_out_branch(dir.path()), None, "not a checkout");
    }
}

#[cfg(test)]
mod round5_tests {
    use super::*;
    use crate::store::init_cas_dir;
    use crate::test_support::TestEnvGuard;
    use wiremock::matchers::any;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn late_fence(mode: &str) {
        late_fence_case(mode, false, false).await;
    }

    async fn late_fence_case(mode: &str, equal_mtime: bool, replace_unrelated: bool) {
        let env = TestEnvGuard::temp_home();
        let root = init_cas_dir(env.home()).unwrap();
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
        let mut agent = Agent::new("late-fence".into(), "late-fence".into());
        agent.cc_session_id = Some("same".into());
        let now = Utc::now();
        if mode == "heartbeat" {
            save(
                &root,
                &PresenceRecord {
                    agent_id: agent.id.clone(),
                    state: PresenceState::Registered,
                    cause: None,
                    failures: 0,
                    last_attempt_at: now,
                    registered_at: Some(now),
                    next_retry_at: None,
                    focus: None,
                    branch: None,
                },
            );
        }
        // Start with an existing directory generation: enqueue must exercise
        // the changed-generation comparison, rather than the None fallback.
        drop(
            super::super::shutdown::presence_lease(&root)
                .unwrap()
                .unwrap(),
        );
        let dir = root.join("cloud_shutdown");
        if replace_unrelated {
            let mut unrelated = Agent::new("unrelated".into(), "unrelated".into());
            unrelated.cc_session_id = Some("other".into());
            super::super::shutdown::enqueue(&root, &unrelated, "other", &server.uri()).unwrap();
        }
        let intent_files = || {
            std::fs::read_dir(&dir)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|file| file.extension().is_some_and(|ext| ext == "json"))
                .collect::<Vec<_>>()
        };
        let original_intents = intent_files();
        let original_mtime = std::fs::metadata(&dir).unwrap().modified().unwrap();
        let mut snapshot = super::super::shutdown::Snapshot::load(&root);
        if replace_unrelated {
            // Model another drainer retiring an older intent in the same tick.
            std::fs::remove_file(&original_intents[0]).unwrap();
        }
        super::super::shutdown::enqueue(&root, &agent, "same", &server.uri()).unwrap();
        if equal_mtime {
            std::fs::File::open(&dir)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(original_mtime))
                .unwrap();
            assert_eq!(
                std::fs::metadata(&dir).unwrap().modified().unwrap(),
                original_mtime
            );
        }
        if replace_unrelated {
            assert_eq!(
                intent_files().len(),
                original_intents.len(),
                "entry count alone cannot detect replacement"
            );
        }
        let endpoint = server.uri();
        let mode = mode.to_string();
        let (action, record) = tokio::task::spawn_blocking(move || {
            let client = CloudCoordinator::new(CloudConfig {
                endpoint,
                token: Some("test-token".into()),
                ..Default::default()
            })
            .unwrap();
            if mode == "register" {
                (
                    None,
                    Some(register_with_shutdown(
                        &client,
                        &root,
                        &agent,
                        now,
                        &mut snapshot,
                    )),
                )
            } else {
                (
                    Some(tick_with_shutdown(
                        &client,
                        &root,
                        &agent,
                        now,
                        &mut snapshot,
                    )),
                    None,
                )
            }
        })
        .await
        .unwrap();
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "stale snapshot must not resurrect Cloud presence"
        );
        if let Some(action) = action {
            assert_eq!(action, TickAction::Waiting);
        }
        if let Some(record) = record {
            assert_eq!(record.state, PresenceState::Failing);
            assert!(record.cause.unwrap().contains("session ended"));
        }
    }

    #[tokio::test]
    async fn cas_4ac8_round5_late_fence_blocks_first_registration() {
        late_fence("tick").await;
    }
    #[tokio::test]
    async fn cas_4ac8_round5_late_fence_blocks_heartbeat() {
        late_fence("heartbeat").await;
    }
    #[tokio::test]
    async fn cas_4ac8_round5_late_fence_blocks_direct_registration() {
        late_fence("register").await;
    }

    #[tokio::test]
    async fn cas_12e7_equal_mtime_late_intent_fences_heartbeat() {
        late_fence_case("heartbeat", true, false).await;
    }

    #[tokio::test]
    async fn cas_12e7_equal_mtime_and_count_replacement_fences_heartbeat() {
        late_fence_case("heartbeat", true, true).await;
    }

    #[tokio::test]
    async fn cas_12e7_equal_mtime_late_intent_blocks_registration() {
        late_fence_case("register", true, false).await;
    }
}
