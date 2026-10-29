//! Rapid response mode (cas-1b4d): an operator-enabled, per-epic window with
//! a mandatory expiry in which some delivery gates scale down.
//!
//! While a window is open for a delivery's epic:
//! - independent per-task QA is not dispatched (the supervisor reviews
//!   screenshots instead; an explicit supervisor request still dispatches);
//! - close verification is sampled, one task in [`VERIFY_SAMPLE_EVERY`];
//! - the evidence gate requires only journeys whose owned files the delivery
//!   diff touched;
//! - the batch QA receipt wait is skipped, so each fix merges as it lands;
//! - the post-release check is an asynchronous watch, not a gate.
//!
//! The safety floor never reads this mode: build proofs, the touched-test
//! receipt, the evidence secret check and every other close gate keep
//! refusing. A delivery that touches a migration path never gets rapid
//! treatment, so migrations keep their full review, and the operator states a
//! rollback when opening the window.
//!
//! Every skipped check is appended to a back-fill ledger. When the window
//! closes (expiry or `disable`), the factory daemon turns the ledger into one
//! back-fill task under the epic and tells the supervisor ([`backfill_due`]).
//!
//! Only the operator opens or closes a window: the CLI refuses an agent
//! context, the PreToolUse hook refuses agent shells running it
//! (`operator_only_cas_invocation`), and the state file lives under
//! `.cas/operator/`, which agents may not write.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use cas_types::{Task, TaskType};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// State file, relative to the Cassy root (operator-written only).
pub const STATE_FILE: &str = "operator/rapid-response.toml";
/// Longest window the operator may open.
pub const MAX_WINDOW_HOURS: i64 = 12;
/// Close verification runs for one task in this many while a window is open.
pub const VERIFY_SAMPLE_EVERY: u64 = 4;

/// One operator-opened window for one epic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub epic: String,
    pub enabled_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub reason: String,
    pub rollback: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_at: Option<DateTime<Utc>>,
}

impl Window {
    /// Open until it expires or the operator disables it.
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        self.disabled_at.is_none() && now < self.expires_at
    }

    fn closed_at(&self) -> DateTime<Utc> {
        self.disabled_at.map_or(self.expires_at, |at| at.min(self.expires_at))
    }

    /// Stable key for this window's ledger and back-fill marker.
    fn key(&self) -> String {
        format!("{}-{}", self.epic, self.enabled_at.format("%Y%m%dT%H%M%SZ"))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub windows: Vec<Window>,
}

/// One skipped check, as written to the back-fill ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skip {
    pub at: DateTime<Utc>,
    pub task: String,
    pub check: String,
    pub detail: String,
}

/// What [`backfill_due`] did for one closed window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backfill {
    pub epic: String,
    pub skipped: usize,
    pub task: Option<String>,
}

fn state_path(cas_root: &Path) -> PathBuf {
    cas_root.join(STATE_FILE)
}

fn ledger_dir(cas_root: &Path) -> PathBuf {
    cas_root.join("rapid-response")
}

fn ledger_path(cas_root: &Path, window: &Window) -> PathBuf {
    ledger_dir(cas_root).join(format!("{}.jsonl", window.key()))
}

fn backfilled_marker(cas_root: &Path, window: &Window) -> PathBuf {
    ledger_dir(cas_root).join(format!("{}.backfilled", window.key()))
}

pub fn load(cas_root: &Path) -> Result<State> {
    let path = state_path(cas_root);
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .with_context(|| format!("invalid rapid-response state {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(error) => Err(error.into()),
    }
}

fn save(cas_root: &Path, state: &State) -> Result<()> {
    let path = state_path(cas_root);
    let parent = path.parent().context("rapid-response state has no parent")?;
    std::fs::create_dir_all(parent)?;
    let text = format!(
        "# Rapid response windows (cas-1b4d). Written only by the operator's own\n\
         # `cas qa rapid-response`. Agents are refused when they try to write this file.\n{}",
        toml::to_string_pretty(state)?
    );
    let staging = parent.join(format!(".rapid-response.{}.tmp", std::process::id()));
    std::fs::write(&staging, text)?;
    std::fs::rename(&staging, &path)?;
    Ok(())
}

/// Parse `90m`, `2h` or `1h30m`.
pub fn parse_duration(text: &str) -> Result<Duration> {
    let text = text.trim();
    let mut total = 0i64;
    let mut digits = String::new();
    for ch in text.chars() {
        match ch {
            '0'..='9' => digits.push(ch),
            'h' | 'm' if !digits.is_empty() => {
                let value: i64 = digits.parse()?;
                total += if ch == 'h' { value * 60 } else { value };
                digits.clear();
            }
            _ => bail!("--for takes a duration such as 90m, 2h or 1h30m, not {text:?}"),
        }
    }
    ensure!(digits.is_empty() && total > 0, "--for takes a duration such as 90m, 2h or 1h30m, not {text:?}");
    Ok(Duration::minutes(total))
}

/// Open a window for `epic`. The caller has already established the operator.
pub fn enable(
    cas_root: &Path,
    store: &dyn cas_store::TaskStore,
    epic: &str,
    duration: Duration,
    reason: &str,
    rollback: &str,
    now: DateTime<Utc>,
) -> Result<Window> {
    let (reason, rollback) = (reason.trim(), rollback.trim());
    ensure!(!reason.is_empty(), "--reason is required: it is recorded on the epic");
    ensure!(!rollback.is_empty(), "--rollback is required: name how a bad release is undone");
    ensure!(
        duration > Duration::zero() && duration <= Duration::hours(MAX_WINDOW_HOURS),
        "--for must be between 1 minute and {MAX_WINDOW_HOURS} hours"
    );
    let task = store.get(epic).with_context(|| format!("epic {epic} not found"))?;
    ensure!(task.task_type == TaskType::Epic, "{epic} is not an epic");
    ensure!(!task.is_terminal(), "epic {epic} is closed");
    let mut state = load(cas_root)?;
    if let Some(open) = state.windows.iter().find(|w| w.epic == task.id && w.is_active(now)) {
        bail!(
            "a rapid-response window for {} is already open until {}; disable it first",
            task.id,
            open.expires_at.to_rfc3339()
        );
    }
    let window = Window {
        epic: task.id.clone(),
        enabled_at: now,
        expires_at: now + duration,
        reason: reason.to_string(),
        rollback: rollback.to_string(),
        disabled_at: None,
    };
    state.windows.push(window.clone());
    save(cas_root, &state)?;
    store.append_note(
        &task.id,
        &format!(
            "[{}] ✅ DECISION rapid response ON by operator until {} UTC (cas-1b4d). Reason: {reason}. Rollback: {rollback}. Skipped checks back-fill when the window closes.",
            now.format("%Y-%m-%d %H:%M"),
            window.expires_at.format("%Y-%m-%d %H:%M")
        ),
    )?;
    Ok(window)
}

/// Close the open window for `epic`; false when none was open.
pub fn disable(
    cas_root: &Path,
    store: &dyn cas_store::TaskStore,
    epic: &str,
    now: DateTime<Utc>,
) -> Result<bool> {
    let mut state = load(cas_root)?;
    let Some(window) = state.windows.iter_mut().find(|w| w.epic == epic && w.is_active(now)) else {
        return Ok(false);
    };
    window.disabled_at = Some(now);
    save(cas_root, &state)?;
    let _ = store.append_note(
        epic,
        &format!(
            "[{}] ✅ DECISION rapid response OFF by operator (cas-1b4d); back-fill runs next.",
            now.format("%Y-%m-%d %H:%M")
        ),
    );
    Ok(true)
}

/// The open window for `epic`, if any.
pub fn active_window(cas_root: &Path, epic: &str, now: DateTime<Utc>) -> Option<Window> {
    load(cas_root)
        .ok()?
        .windows
        .into_iter()
        .find(|w| w.epic == epic && w.is_active(now))
}

/// Whether a path is a schema or data migration, which always keeps its
/// full human review.
pub fn is_migration_path(path: &str) -> bool {
    path.to_ascii_lowercase().contains("migration")
}

/// The open window that scales down gates for `task`'s delivery, if any.
/// `changed` is the delivery diff when known; a migration path in it keeps
/// every gate.
pub fn active_for_task(
    cas_root: &Path,
    store: &dyn cas_store::TaskStore,
    task: &Task,
    changed: Option<&[String]>,
) -> Option<Window> {
    if changed.is_some_and(|paths| paths.iter().any(|path| is_migration_path(path))) {
        return None;
    }
    let epic = if task.task_type == TaskType::Epic {
        task.clone()
    } else {
        store.get_parent_epic(&task.id).ok()??
    };
    active_window(cas_root, &epic.id, Utc::now())
}

/// [`active_for_task`] opening the task store itself, for gates that hold
/// only the Cassy root. Without `changed`, the delivery diff is read from Git
/// (recorded start base to parked anchor); when a recorded delivery's diff
/// cannot be read, no window applies, so an unknown diff never skips the
/// migration floor.
pub fn active_for_task_at(cas_root: &Path, task: &Task, changed: Option<&[String]>) -> Option<Window> {
    if !state_path(cas_root).exists() {
        return None;
    }
    let measured;
    let changed = match changed {
        Some(paths) => paths,
        None => {
            measured = recorded_delivery_paths(cas_root, task)?;
            &measured
        }
    };
    let store = crate::store::open_task_store(cas_root).ok()?;
    active_for_task(cas_root, store.as_ref(), task, Some(changed))
}

fn recorded_delivery_paths(cas_root: &Path, task: &Task) -> Option<Vec<String>> {
    let Some(start) = task.deliverables.factory_branch_start.as_ref() else {
        // No recorded factory delivery: Cassy knows of no diff to review.
        return Some(Vec::new());
    };
    let base = &start.base_sha;
    let head = task.deliverables.factory_branch_anchor.as_deref()?;
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cas_root.parent()?)
        .args(["diff", "--name-only", "--no-renames", base, head, "--"])
        .output()
        .ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout).lines().map(str::to_string).collect()
    })
}

/// Whether close verification still runs for `task_id` in a window: a
/// stable one-in-[`VERIFY_SAMPLE_EVERY`] sample by task id.
pub fn verification_sampled(task_id: &str) -> bool {
    let hash = task_id
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3));
    hash % VERIFY_SAMPLE_EVERY == 0
}

/// Append one skipped check to the window's back-fill ledger, once per task
/// and check (dispatch and close both see the same skip).
pub fn record_skip(cas_root: &Path, window: &Window, task: &str, check: &str, detail: &str) {
    if ledger(cas_root, window).iter().any(|skip| skip.task == task && skip.check == check) {
        return;
    }
    let row = Skip {
        at: Utc::now(),
        task: task.to_string(),
        check: check.to_string(),
        detail: detail.to_string(),
    };
    let path = ledger_path(cas_root, window);
    let written = (|| -> Result<()> {
        use std::io::Write;
        std::fs::create_dir_all(ledger_dir(cas_root))?;
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(file, "{}", serde_json::to_string(&row)?)?;
        Ok(())
    })();
    if let Err(error) = written {
        tracing::warn!(%error, task, check, "rapid-response ledger row not written");
    }
}

/// Registry journeys whose owner globs match one of `paths`; `None` when the
/// journey registry cannot be read, so nothing is narrowed.
pub fn owned_journeys(
    repo: &Path,
    qa: &crate::config::QaConfig,
    paths: &[String],
) -> Option<std::collections::BTreeSet<String>> {
    let changes: Vec<_> = paths.iter().map(|path| crate::qa_impact::Change::modified(path.clone())).collect();
    let impact = crate::qa_impact::analyze(repo, qa, &changes, None, None).ok()?;
    impact.registry.present.then(|| {
        impact
            .journeys
            .into_iter()
            .filter(|journey| !journey.matches.is_empty())
            .map(|journey| journey.id)
            .collect()
    })
}

/// Keep only `owned` journeys (touched routes) and ledger the rest.
pub fn narrow_journeys(
    cas_root: &Path,
    window: &Window,
    task: &str,
    journeys: &mut Vec<String>,
    owned: &std::collections::BTreeSet<String>,
) {
    let dropped: Vec<String> = journeys.iter().filter(|id| !owned.contains(*id)).cloned().collect();
    if !dropped.is_empty() {
        journeys.retain(|id| owned.contains(id));
        record_skip(cas_root, window, task, "journeys", &dropped.join(","));
    }
}

/// The skipped checks recorded for a window.
pub fn ledger(cas_root: &Path, window: &Window) -> Vec<Skip> {
    std::fs::read_to_string(ledger_path(cas_root, window))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// For each window that has closed and not been back-filled, create one
/// back-fill task under its epic listing every skipped check, and mark it
/// done. The factory daemon calls this on its refresh tick and tells the
/// supervisor about each result.
pub fn backfill_due(
    cas_root: &Path,
    store: &dyn cas_store::TaskStore,
    now: DateTime<Utc>,
) -> Result<Vec<Backfill>> {
    let mut done = Vec::new();
    for window in load(cas_root)?.windows {
        let marker = backfilled_marker(cas_root, &window);
        if window.is_active(now) || marker.exists() {
            continue;
        }
        let skips = ledger(cas_root, &window);
        let task = if skips.is_empty() {
            None
        } else {
            Some(create_backfill_task(store, &window, &skips)?)
        };
        std::fs::create_dir_all(ledger_dir(cas_root))?;
        std::fs::write(&marker, task.as_deref().unwrap_or("none"))?;
        done.push(Backfill { epic: window.epic.clone(), skipped: skips.len(), task });
    }
    Ok(done)
}

fn create_backfill_task(
    store: &dyn cas_store::TaskStore,
    window: &Window,
    skips: &[Skip],
) -> Result<String> {
    let id = store.generate_id()?;
    let mut task = Task::new(
        id.clone(),
        format!("Rapid-response back-fill: run the {} check(s) skipped in the window", skips.len()),
    );
    task.task_type = TaskType::Chore;
    task.priority = cas_types::Priority::HIGH;
    task.risk = vec![cas_types::TaskRisk::None];
    task.labels = vec!["rapid-response-backfill".to_string()];
    let rows = skips
        .iter()
        .map(|skip| format!("- {} {}: {}", skip.task, skip.check, skip.detail))
        .collect::<Vec<_>>()
        .join("\n");
    task.description = format!(
        "The rapid-response window for {} closed at {} UTC (opened {}; reason: {}; rollback: {}). \
         Run each check it skipped, and report each result in the close reason:\n{rows}",
        window.epic,
        window.closed_at().format("%Y-%m-%d %H:%M"),
        window.enabled_at.format("%Y-%m-%d %H:%M"),
        window.reason,
        window.rollback,
    );
    task.acceptance_criteria = "Every listed check has run against the merged work; each failure has a fix task.".to_string();
    store.add(&task)?;
    store.add_dependency(&cas_types::Dependency::new(
        id.clone(),
        window.epic.clone(),
        cas_types::DependencyType::ParentChild,
    ))?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn epic_fixture() -> (tempfile::TempDir, PathBuf, std::sync::Arc<dyn cas_store::TaskStore>, Task) {
        let dir = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
        let store = crate::store::open_task_store(&cas_dir).unwrap();
        let mut epic = Task::new("cas-ep1c".to_string(), "Hotfix epic".to_string());
        epic.task_type = TaskType::Epic;
        store.add(&epic).unwrap();
        let delivery = Task::new("cas-de11".to_string(), "Fix".to_string());
        store.add(&delivery).unwrap();
        store
            .add_dependency(&cas_types::Dependency::new(
                delivery.id.clone(),
                epic.id.clone(),
                cas_types::DependencyType::ParentChild,
            ))
            .unwrap();
        (dir, cas_dir, store, delivery)
    }

    /// The window needs an expiry, turns off on its own, and its skipped
    /// checks become one back-fill task once it has closed.
    #[test]
    fn window_expires_on_its_own_and_backfills_its_ledger() {
        let (_dir, cas, store, delivery) = epic_fixture();
        let now = Utc::now();
        assert!(enable(&cas, store.as_ref(), "cas-ep1c", Duration::hours(13), "outage", "revert", now).is_err());
        assert!(parse_duration("").is_err());
        let window = enable(&cas, store.as_ref(), "cas-ep1c", parse_duration("90m").unwrap(), "outage", "revert the merge", now).unwrap();
        assert!(active_for_task(&cas, store.as_ref(), &delivery, Some(&["src/lib.rs".into()])).is_some());
        assert!(
            active_for_task(&cas, store.as_ref(), &delivery, Some(&["cas-cli/src/migration/m200.rs".into()])).is_none(),
            "a migration keeps every gate"
        );
        record_skip(&cas, &window, &delivery.id, "independent-qa", "journeys:HUB-J1");
        assert!(backfill_due(&cas, store.as_ref(), now).unwrap().is_empty(), "nothing back-fills while open");
        let later = window.expires_at + Duration::minutes(1);
        assert!(active_window(&cas, "cas-ep1c", later).is_none(), "the window turns off on its own");
        let done = backfill_due(&cas, store.as_ref(), later).unwrap();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].skipped, 1);
        let task = store.get(done[0].task.as_deref().unwrap()).unwrap();
        assert!(task.description.contains("cas-de11 independent-qa"));
        assert_eq!(store.get_parent_epic(&task.id).unwrap().unwrap().id, "cas-ep1c");
        assert!(backfill_due(&cas, store.as_ref(), later).unwrap().is_empty(), "back-fill runs once");
    }

    /// Evidence is required only for journeys whose owned files were touched.
    #[test]
    fn window_narrows_required_journeys_to_touched_routes() {
        let (_dir, cas, store, delivery) = epic_fixture();
        let window = enable(&cas, store.as_ref(), "cas-ep1c", Duration::hours(1), "outage", "revert", Utc::now()).unwrap();
        let mut journeys = vec!["HUB-J1".to_string(), "HUB-J4".to_string()];
        narrow_journeys(&cas, &window, &delivery.id, &mut journeys, &["HUB-J4".to_string()].into());
        assert_eq!(journeys, ["HUB-J4"]);
        assert_eq!(ledger(&cas, &window)[0].detail, "HUB-J1");
    }

    /// The batch receipt wait is skipped in a window, so each fix closes as it lands.
    #[test]
    fn window_skips_the_batch_receipt_wait() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let (_dir, cas, store, delivery) = epic_fixture();
        env.set("CAS_ROOT", &cas);
        cas_store::defer_qa_batch_evidence(&cas, &delivery.id, "0123456789abcdef0123456789abcdef01234567").unwrap();
        assert!(crate::qa_batch::ensure_deferred_close(&cas, &delivery, None, None).is_err());
        let window = enable(&cas, store.as_ref(), "cas-ep1c", Duration::hours(1), "outage", "revert", Utc::now()).unwrap();
        assert!(crate::qa_batch::ensure_deferred_close(&cas, &delivery, None, None).is_ok());
        assert_eq!(ledger(&cas, &window)[0].check, "batch-qa-receipt");
    }

    /// A release cut in a window keeps the post-release run as a watch, not a gate.
    #[test]
    fn window_makes_the_post_release_check_a_watch() {
        let (_dir, cas, store, _delivery) = epic_fixture();
        std::fs::write(cas.join("config.toml"), "[qa.post_release]\ncommand = \"echo healthy\"\n").unwrap();
        let epic = store.get("cas-ep1c").unwrap();
        assert!(crate::qa_post_release::gate(&cas, &epic, true).is_some());
        let window = enable(&cas, store.as_ref(), "cas-ep1c", Duration::hours(1), "outage", "revert", Utc::now()).unwrap();
        assert!(crate::qa_post_release::gate(&cas, &epic, true).is_none());
        assert_eq!(ledger(&cas, &window)[0].check, "post-release-gate");
    }
}
