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
    #[serde(default)]
    pub migration_reviews: Vec<MigrationReview>,
}

/// The operator's human review of one task's migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationReview {
    pub task: String,
    pub reviewed_at: DateTime<Utc>,
    pub note: String,
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

/// Under `.cas/operator/`, which agents may not write, so a ledger row or
/// back-fill marker cannot be edited away.
fn ledger_dir(cas_root: &Path) -> PathBuf {
    cas_root.join("operator").join("rapid-response")
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
/// full gates and needs the operator's review inside a window.
pub fn is_migration_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("migration")
        || lower.contains("db/migrate/")
        || lower.contains("alembic/versions/")
        || lower.ends_with("schema.prisma")
}

/// The open window for the epic `task` belongs to (or is), ignoring the
/// delivery's content.
fn window_for_task(cas_root: &Path, task: &Task) -> Option<Window> {
    if !state_path(cas_root).exists() {
        return None;
    }
    let epic = if task.task_type == TaskType::Epic {
        task.id.clone()
    } else {
        crate::store::open_task_store(cas_root).ok()?.get_parent_epic(&task.id).ok()??.id
    };
    active_window(cas_root, &epic, Utc::now())
}

/// A delivery's committed change: its paths and added lines.
struct Delivery {
    paths: Vec<String>,
    added: String,
}

/// Whether a change may relax gates: no migration path and no secret-shaped
/// added line (the QA-evidence secret patterns over the delivery diff).
fn floor_clear(delivery: &Delivery, extra_paths: &[String]) -> bool {
    if delivery.paths.iter().chain(extra_paths).any(|path| is_migration_path(path)) {
        return false;
    }
    if let Some(kind) = crate::qa_evidence::first_secret(&delivery.added) {
        tracing::warn!(kind, "rapid response: secret-shaped line in the delivery diff keeps every gate");
        return false;
    }
    true
}

/// The open window that scales down gates for `task`'s delivery, if any.
///
/// Fails closed: the delivery diff is read from Git (recorded start base to
/// parked anchor), and without a readable diff, with a migration path (in
/// that diff or in `changed`), or with a secret-shaped added line, no window
/// applies and every gate keeps refusing.
pub fn active_for_task_at(cas_root: &Path, task: &Task, changed: Option<&[String]>) -> Option<Window> {
    active_for_delivery(cas_root, cas_root.parent()?, task, changed)
}

/// [`active_for_task_at`] reading the delivery from `repo`, for gates that
/// know the delivery's repository (a Cassy root need not sit in it).
pub fn active_for_delivery(
    cas_root: &Path,
    repo: &Path,
    task: &Task,
    changed: Option<&[String]>,
) -> Option<Window> {
    let window = window_for_task(cas_root, task)?;
    let delivery = recorded_delivery(repo, task)?;
    floor_clear(&delivery, changed.unwrap_or(&[])).then_some(window)
}

fn git_text(repo: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git").arg("-C").arg(repo).args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn diff_between(repo: &Path, range: &[&str]) -> Option<Delivery> {
    let mut names = vec!["diff", "--name-only", "--no-renames"];
    names.extend_from_slice(range);
    let paths = git_text(repo, &names)?.lines().map(str::to_string).collect();
    let mut patch = vec!["diff", "--no-color", "--no-ext-diff", "--unified=0", "--no-renames"];
    patch.extend_from_slice(range);
    let added = git_text(repo, &patch)?
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .map(|line| &line[1..])
        .collect::<Vec<_>>()
        .join("\n");
    Some(Delivery { paths, added })
}

fn recorded_delivery(repo: &Path, task: &Task) -> Option<Delivery> {
    let base = task.deliverables.factory_branch_start.as_ref()?.base_sha.as_str();
    let head = task.deliverables.factory_branch_anchor.as_deref()?;
    diff_between(repo, &[base, head, "--"])
}

/// Whether a release of `epic` may treat its post-release check as a watch:
/// the epic branch's change against every release branch it ships to (the
/// post-release `release_branches`, else the default branch, as the watch
/// itself arms) carries no migration and no secret-shaped line. The epic's
/// own work target is not used: it is often the epic lane itself, whose
/// `X...X` range is empty. An unreadable diff, or a release branch that is
/// the epic branch, keeps the gate.
fn epic_release_clear(cas_root: &Path, epic: &Task) -> bool {
    let (Some(branch), Some(repo)) = (epic.branch.as_deref(), cas_root.parent()) else {
        return false;
    };
    let mut targets = crate::qa_post_release::configured(cas_root)
        .map(|config| config.release_branches)
        .unwrap_or_default();
    if targets.is_empty() {
        targets.push(crate::worktree::GitOperations::new(repo.to_path_buf()).detect_default_branch());
    }
    targets.iter().all(|target| {
        target != branch
            && diff_between(repo, &[format!("{target}...{branch}").as_str(), "--"])
                .is_some_and(|delivery| floor_clear(&delivery, &[]))
    })
}

/// The open window that makes `epic`'s post-release check a watch, if any.
pub fn active_for_release(cas_root: &Path, epic: &Task, now: DateTime<Utc>) -> Option<Window> {
    let window = active_window(cas_root, &epic.id, now)?;
    epic_release_clear(cas_root, epic).then_some(window)
}

/// Close refusal for a migration delivery under an open window until the
/// operator records a human review (`cas qa rapid-response review-migration`).
/// Its gates were never relaxed; the review is the extra floor check.
pub fn migration_review_refusal(cas_root: &Path, task: &Task) -> Option<String> {
    window_for_task(cas_root, task)?;
    let delivery = recorded_delivery(cas_root.parent()?, task)?;
    let migration = delivery.paths.iter().find(|path| is_migration_path(path))?;
    let reviewed = load(cas_root).ok()?.migration_reviews.iter().any(|review| review.task == task.id);
    (!reviewed).then(|| {
        format!(
            "MIGRATION REVIEW REQUIRED (rapid response): {} changes {migration} while its epic is in a rapid-response window, so a human must review the migration before close. Ask the operator to run `cas qa rapid-response review-migration {} --note \"<what they checked>\"`, then retry the close.",
            task.id, task.id
        )
    })
}

/// Record the operator's migration review. The caller has established the
/// operator.
pub fn record_migration_review(
    cas_root: &Path,
    store: &dyn cas_store::TaskStore,
    task_id: &str,
    note: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    let note = note.trim();
    ensure!(!note.is_empty(), "--note is required: say what you checked");
    let task = store.get(task_id).with_context(|| format!("task {task_id} not found"))?;
    let mut state = load(cas_root)?;
    state.migration_reviews.retain(|review| review.task != task.id);
    state.migration_reviews.push(MigrationReview { task: task.id.clone(), reviewed_at: now, note: note.to_string() });
    save(cas_root, &state)?;
    store.append_note(
        &task.id,
        &format!("[{}] ✅ DECISION operator migration review (rapid response, cas-1b4d): {note}", now.format("%Y-%m-%d %H:%M")),
    )?;
    Ok(())
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
/// and check (dispatch and close both see the same skip). Returns whether the
/// row is on record: a caller relaxes its gate only when it is, so a skip
/// that cannot be recorded keeps the gate.
#[must_use]
pub fn record_skip(cas_root: &Path, window: &Window, task: &str, check: &str, detail: &str) -> bool {
    if ledger(cas_root, window).iter().any(|skip| skip.task == task && skip.check == check) {
        return true;
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
        file.sync_data()?;
        Ok(())
    })();
    match written {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(%error, task, check, "rapid-response ledger row not written; the gate stays");
            false
        }
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
    if !dropped.is_empty() && record_skip(cas_root, window, task, "journeys", &dropped.join(",")) {
        journeys.retain(|id| owned.contains(id));
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

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.email=t@example.com", "-c", "user.name=T", "-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Commit `file` on the epic branch and record it as `task`'s delivery.
    fn deliver(dir: &Path, store: &dyn cas_store::TaskStore, task: &mut Task, file: &str, body: &str) {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        git(dir, &["add", file]);
        git(dir, &["commit", "-q", "-m", file]);
        task.deliverables.factory_branch_anchor = Some(git(dir, &["rev-parse", "HEAD"]));
        store.update(task).unwrap();
    }

    /// An epic on branch `epic/hot` with one delivery of `src/fix.rs`.
    pub(crate) fn epic_fixture() -> (tempfile::TempDir, PathBuf, std::sync::Arc<dyn cas_store::TaskStore>, Task) {
        let dir = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
        let store = crate::store::open_task_store(&cas_dir).unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("README.md"), "seed\n").unwrap();
        git(dir.path(), &["add", "README.md"]);
        git(dir.path(), &["commit", "-q", "-m", "seed"]);
        let base = git(dir.path(), &["rev-parse", "HEAD"]);
        git(dir.path(), &["checkout", "-q", "-b", "epic/hot"]);
        let mut epic = Task::new("cas-ep1c".to_string(), "Hotfix epic".to_string());
        epic.task_type = TaskType::Epic;
        epic.branch = Some("epic/hot".to_string());
        store.add(&epic).unwrap();
        let mut delivery = Task::new("cas-de11".to_string(), "Fix".to_string());
        delivery.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
            work_target: cas_types::WorkTarget { repo_selector: "project:test".into(), target_branch: "epic/hot".into() },
            assignee: "worker".into(),
            branch: "factory/worker".into(),
            base_sha: base,
        });
        store.add(&delivery).unwrap();
        store
            .add_dependency(&cas_types::Dependency::new(
                delivery.id.clone(),
                epic.id.clone(),
                cas_types::DependencyType::ParentChild,
            ))
            .unwrap();
        deliver(dir.path(), store.as_ref(), &mut delivery, "src/fix.rs", "pub fn fix() {}\n");
        (dir, cas_dir, store, delivery)
    }

    fn open(cas: &Path, store: &dyn cas_store::TaskStore) -> Window {
        enable(cas, store, "cas-ep1c", Duration::hours(1), "outage", "revert", Utc::now()).unwrap()
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
        assert!(active_for_task_at(&cas, &delivery, None).is_some());
        assert!(record_skip(&cas, &window, &delivery.id, "independent-qa", "journeys:HUB-J1"));
        assert!(backfill_due(&cas, store.as_ref(), now).unwrap().is_empty(), "nothing back-fills while open");
        let later = window.expires_at + Duration::minutes(1);
        assert!(active_window(&cas, "cas-ep1c", later).is_none(), "the window turns off on its own");
        let done = backfill_due(&cas, store.as_ref(), later).unwrap();
        assert_eq!((done.len(), done[0].skipped), (1, 1));
        let task = store.get(done[0].task.as_deref().unwrap()).unwrap();
        assert!(task.description.contains("cas-de11 independent-qa"));
        assert_eq!(store.get_parent_epic(&task.id).unwrap().unwrap().id, "cas-ep1c");
        assert!(backfill_due(&cas, store.as_ref(), later).unwrap().is_empty(), "back-fill runs once");
    }

    /// Safety floor: a secret-shaped added line or an unreadable delivery diff
    /// keeps every gate inside a window.
    #[test]
    fn secrets_and_unknown_diffs_keep_every_gate() {
        let (dir, cas, store, mut delivery) = epic_fixture();
        open(&cas, store.as_ref());
        let token = format!("gh{}_{}", "p", "a".repeat(24));
        deliver(dir.path(), store.as_ref(), &mut delivery, "src/client.rs", &format!("const T: &str = \"{token}\";\n"));
        assert!(active_for_task_at(&cas, &delivery, None).is_none(), "secret-shaped line");
        let mut unknown = delivery.clone();
        unknown.deliverables.factory_branch_start = None;
        assert!(active_for_task_at(&cas, &unknown, None).is_none(), "no recorded delivery diff");
    }

    /// Safety floor: a migration keeps every gate, and its close waits for
    /// the operator's human review.
    #[test]
    fn migrations_keep_every_gate_and_need_an_operator_review() {
        let (dir, cas, store, mut delivery) = epic_fixture();
        open(&cas, store.as_ref());
        assert!(migration_review_refusal(&cas, &delivery).is_none(), "no migration, no review");
        for path in ["db/migrate/001_users.rb", "alembic/versions/a1.py", "prisma/schema.prisma"] {
            assert!(is_migration_path(path), "{path}");
        }
        deliver(dir.path(), store.as_ref(), &mut delivery, "db/migrate/001_users.rb", "class AddUsers; end\n");
        assert!(active_for_task_at(&cas, &delivery, None).is_none());
        let refusal = migration_review_refusal(&cas, &delivery).expect("review required");
        assert!(refusal.contains("review-migration cas-de11"), "{refusal}");
        record_migration_review(&cas, store.as_ref(), &delivery.id, "checked the down step", Utc::now()).unwrap();
        assert!(migration_review_refusal(&cas, &delivery).is_none());
    }

    /// Evidence is required only for journeys whose owned files were touched.
    #[test]
    fn window_narrows_required_journeys_to_touched_routes() {
        let (_dir, cas, store, delivery) = epic_fixture();
        let window = open(&cas, store.as_ref());
        let mut journeys = vec!["HUB-J1".to_string(), "HUB-J4".to_string()];
        narrow_journeys(&cas, &window, &delivery.id, &mut journeys, &["HUB-J4".to_string()].into());
        assert_eq!(journeys, ["HUB-J4"]);
        assert_eq!(ledger(&cas, &window)[0].detail, "HUB-J1");
    }

    /// The batch receipt wait is skipped in a window, so each fix closes as it lands.
    #[test]
    fn window_skips_the_batch_receipt_wait() {
        let (_dir, cas, store, delivery) = epic_fixture();
        cas_store::defer_qa_batch_evidence(&cas, &delivery.id, delivery.deliverables.factory_branch_anchor.as_deref().unwrap()).unwrap();
        assert!(crate::qa_batch::ensure_deferred_close(&cas, &delivery, None, None).is_err());
        let window = open(&cas, store.as_ref());
        assert!(crate::qa_batch::ensure_deferred_close(&cas, &delivery, None, None).is_ok());
        assert_eq!(ledger(&cas, &window)[0].check, "batch-qa-receipt");
    }

    /// A release cut in a window keeps the post-release run as a watch, not a
    /// gate, unless the epic's change carries a migration.
    #[test]
    fn window_makes_the_post_release_check_a_watch() {
        let (dir, cas, store, mut delivery) = epic_fixture();
        std::fs::write(
            cas.join("config.toml"),
            "[qa.post_release]\ncommand = \"echo healthy\"\nrelease_branches = [\"main\"]\n",
        )
        .unwrap();
        // The epic's own work target is its lane, as factory epics record it.
        let mut epic = store.get("cas-ep1c").unwrap();
        epic.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:test".into(),
            target_branch: "epic/hot".into(),
        });
        store.update(&epic).unwrap();
        assert!(crate::qa_post_release::gate(&cas, &epic, true).is_some());
        let window = open(&cas, store.as_ref());
        assert!(crate::qa_post_release::gate(&cas, &epic, true).is_none());
        assert_eq!(ledger(&cas, &window)[0].check, "post-release-gate");
        deliver(dir.path(), store.as_ref(), &mut delivery, "migrations/002.sql", "alter table t add c int;\n");
        assert!(crate::qa_post_release::gate(&cas, &epic, true).is_some(), "a migration keeps the gate");
    }
}
