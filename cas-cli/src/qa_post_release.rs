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
        .qa?
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
    let receipt = Receipt {
        epic: epic.into(),
        sha: sha.to_ascii_lowercase(),
        started_at: Utc::now(),
        window_mins: config.window_mins,
        interval_mins: config.interval_mins,
        runs: Vec::new(),
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
    Some(format!(
        "{}: epic {} check {progress}. Run `cas qa post-release start --epic {} --sha <release-sha>` and wait for clean, or record an operator decision note `{OVERRIDE_MARKER} <operator's words>` (valid 6 hours).",
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
    if configured(cas_root).is_none() {
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
                if configured(&root).is_none() {
                    return Ok(false);
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
