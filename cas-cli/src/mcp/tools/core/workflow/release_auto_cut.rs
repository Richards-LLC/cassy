//! Auto-cut (cas-e060): Cassy cuts a green release train itself.
//!
//! The daemon's cut-notice poll calls [`attempt`] for each uncut train whose
//! exact tip holds a green train-suite receipt. When the tip carries
//! changelog fragments, no merge-queue run is in flight, the operator has not
//! paused auto-cut in Commander and the cadence allows it, Cassy freezes the
//! train (cas-2f4d) and runs the repository's own `scripts/release.sh` with
//! the supervisor's notes draft. That script already arms the landing and
//! publication reminders, and publication sends the post-release notice
//! that has the supervisor post the approved notes. Cassy never writes or posts
//! notes: without a draft it waits, and wakes the supervisor once per tip.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};

use super::release_freeze;
use crate::mcp::tools::core::task::release_train::is_release_train;
use crate::supervisor_notices::{ReleaseNoticePhase, release_action};
use crate::types::Task;

/// The operator's Commander pause; a synced label, like the freeze.
pub(crate) const PAUSED_LABEL: &str = "release-auto-cut-paused";
/// Records the started tip; only a corrected notes-lint failure can retry it.
const STARTED_LABEL: &str = "release-auto-cut:";
/// How long the supervisor has to write a missing notes draft.
const NOTES_DEADLINE_MINUTES: i64 = 30;
/// scripts/release.sh reserves this exit for its existing notes lint.
const NOTES_LINT_EXIT: i32 = 5;

/// What the poll does next for this train.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Cassy owns this tip: a cut started, is running, failed and was
    /// reported, or waits for notes. No "ready to cut" notice follows.
    Handled,
    /// Auto-cut does not apply; the ordinary notice names why.
    Declined(String),
}

pub(crate) fn paused(train: &Task) -> bool {
    train.labels.iter().any(|label| label == PAUSED_LABEL)
}

/// The supervisor-authored notes draft for a train's cut.
pub(crate) fn notes_draft(root: &Path, train_id: &str) -> Result<PathBuf> {
    Ok(train_dir(root, train_id)?.join("release-notes.md"))
}

fn train_dir(root: &Path, train_id: &str) -> Result<PathBuf> {
    Ok(crate::train_ci::directory(root, train_id)?
        .parent()
        .context("train artifact directory")?
        .to_path_buf())
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").arg("-C").arg(repo).args(args).output()?;
    ensure!(output.status.success(), "git {}: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim());
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Whether auto-cut can act on this train at all: it is on, not paused, and
/// the repository cuts with its own `scripts/release.sh`.
pub(crate) fn enabled(root: &Path, train: &Task) -> bool {
    declined_setup(root, train).is_none()
}

fn declined_setup(root: &Path, train: &Task) -> Option<String> {
    let config = crate::config::Config::load(root).ok()?;
    if config.release.clone().unwrap_or_default().auto_cut_max_per_day == 0 {
        return Some("off (release.auto_cut_max_per_day = 0)".into());
    }
    if paused(train) {
        return Some("paused by the operator in Commander".into());
    }
    if config.factory().release_command.is_some() {
        return Some("this project cuts with its own factory.release_command".into());
    }
    let repo = crate::train_guardrails::train_repo_root(root, train).ok()?;
    if !repo.join("scripts/release.sh").is_file() {
        return Some("no scripts/release.sh in this repository".into());
    }
    None
}

/// The changelog fragments at `tip`: what makes the cut worth shipping.
fn fragments(repo: &Path, tip: &str) -> usize {
    git(repo, &["ls-tree", "--name-only", tip, "changelog.d/"])
        .unwrap_or_default()
        .lines()
        .filter(|path| path.ends_with(".md") && !path.ends_with("/README.md"))
        .count()
}

/// The cadence guard: freezes of other trains are the cut history, on every
/// machine, because the freeze label syncs with the train row.
fn cadence(root: &Path, train: &Task, now: DateTime<Utc>) -> Result<Option<String>> {
    let release = crate::config::Config::load(root)?.release.unwrap_or_default();
    let cuts: Vec<DateTime<Utc>> = crate::store::open_task_store(root)?
        .list(None)?
        .iter()
        .filter(|other| other.id != train.id && is_release_train(other))
        .filter_map(release_freeze::recorded)
        .map(|freeze| freeze.at)
        .collect();
    let day = cuts.iter().filter(|at| **at > now - Duration::hours(24)).count();
    if day >= release.auto_cut_max_per_day as usize {
        return Ok(Some(format!("{day} release freezes in the last 24 h reach release.auto_cut_max_per_day")));
    }
    let interval = Duration::hours(i64::from(release.auto_cut_min_interval_hours));
    if let Some(last) = cuts.iter().max().filter(|last| **last > now - interval) {
        return Ok(Some(format!(
            "the last release freeze was at {}; release.auto_cut_min_interval_hours allows the next after {}",
            crate::cli::release_report::eastern_time(*last),
            crate::cli::release_report::eastern_time(*last + interval)
        )));
    }
    Ok(None)
}

/// One poll for a train whose exact tip `tip` is green. `queue_busy` is true
/// while any merge-queue run is in flight.
pub(crate) fn attempt(root: &Path, train: &Task, lane: &str, tip: &str, queue_busy: bool, now: DateTime<Utc>) -> Result<Outcome> {
    if let Some(reason) = declined_setup(root, train) {
        return Ok(Outcome::Declined(reason));
    }
    let started = format!("{STARTED_LABEL}{tip}");
    let draft = notes_draft(root, &train.id)?;
    let log = train_dir(root, &train.id)?.join("auto-cut.log");
    let digest = std::fs::read(&draft).ok().map(|bytes| format!("{:x}", Sha256::digest(bytes)));
    // A changed draft can retry only a completed notes-lint refusal. Every
    // ordinary admission guard below still applies; other failures stay held.
    let attempted = std::fs::read_to_string(log.with_extension("notes")).ok();
    let notes_retry = release_exit(&log) == Some(NOTES_LINT_EXIT)
        && digest.as_ref().zip(attempted.as_ref())
            .is_some_and(|(current, attempted)| current != attempted);
    if train.labels.contains(&started) && !notes_retry {
        report_failure(root, train, lane, tip)?;
        return Ok(Outcome::Handled);
    }
    let agents = crate::store::open_agent_store(root)?.list(None)?;
    let owner = train.epic_verification_owner.as_deref();
    if !owner.is_some_and(|owner| agents.iter().any(|agent| agent.id == owner || agent.name == owner)) {
        return Ok(Outcome::Declined("the train's supervisor is not on this machine".into()));
    }
    let repo = crate::train_guardrails::train_repo_root(root, train)?;
    let count = fragments(&repo, tip);
    if count == 0 {
        return Ok(Outcome::Declined("no changelog fragments at the tip".into()));
    }
    if queue_busy {
        return Ok(Outcome::Declined("a merge-queue run is in flight".into()));
    }
    if release_freeze::active(root, train).is_none() {
        if let Some(reason) = cadence(root, train, now)? {
            return Ok(Outcome::Declined(reason));
        }
        release_freeze::freeze(
            root,
            &train.id,
            "Cassy auto-cut",
            Some(format!("auto-cut: green train suite at {tip}, {count} changelog fragments").as_str()),
        )?;
    }
    if !draft.is_file() {
        let deadline = crate::cli::release_report::eastern_time(now + Duration::minutes(NOTES_DEADLINE_MINUTES));
        release_action(
            root,
            train,
            ReleaseNoticePhase::CutReady,
            &format!("auto-cut-notes:{tip}"),
            &format!(
                "Release train {lane} ({}) is green at {tip} with {count} changelog fragments and is frozen for its \
                 auto-cut. Write the release-notes draft at {} by {deadline} (docs/release-notes/RUBRIC.md; \
                 `{{{{VERSION}}}}` becomes the version). Check it with `cas release notes check --train {}`. \
                 Cassy cuts once it exists, and never writes or posts notes.",
                train.id,
                draft.display(),
                train.id
            ),
        )?;
        return Ok(Outcome::Handled);
    }
    std::fs::write(log.with_extension("notes"), digest.context("release-notes draft unreadable")?)?;
    start_release(root, &repo, lane, tip, &draft, &log, &train.id)?;
    let store = crate::store::open_task_store(root)?;
    let mut task = store.get(&train.id)?;
    task.labels.retain(|label| !label.starts_with(STARTED_LABEL));
    task.labels.push(started);
    task.updated_at = Utc::now();
    store.update(&task)?;
    store.append_note(
        &train.id,
        &format!("AUTO-CUT: started scripts/release.sh --notes {} at {tip}; log {}", draft.display(), log.display()),
    )?;
    Ok(Outcome::Handled)
}

/// Check out the lane in a dedicated worktree and run `scripts/release.sh`
/// detached; the worktree is removed when the script exits.
fn start_release(root: &Path, repo: &Path, lane: &str, tip: &str, draft: &Path, log: &Path, train_id: &str) -> Result<()> {
    let checkout = train_dir(root, train_id)?.join("cut-checkout");
    std::fs::create_dir_all(checkout.parent().context("checkout parent")?)?;
    if checkout.exists() {
        let _ = git(repo, &["worktree", "remove", "--force", &checkout.to_string_lossy()]);
        let _ = std::fs::remove_dir_all(&checkout);
    }
    let _ = git(repo, &["worktree", "prune"]);
    git(repo, &["worktree", "add", "--quiet", &checkout.to_string_lossy(), lane])?;
    let release = r#"cd "$1" || exit 1
scripts/release.sh --notes "$2"; rc=$?
cd / && git -C "$3" worktree remove --force "$1"
printf '\nRELEASE_EXIT=%s\n' "$rc"
exit "$rc""#;
    let status = Command::new("sh")
        .arg("-c")
        .arg(r#"log=$1; shift; nohup bash -c "$CAS_AUTO_CUT_SCRIPT" auto-cut "$@" >"$log" 2>&1 </dev/null &"#)
        .arg("sh")
        .arg(log)
        .arg(&checkout)
        .arg(draft)
        .arg(repo)
        .env("CAS_AUTO_CUT_SCRIPT", release)
        .env("CAS_ROOT", root)
        .env("CAS_RELEASE_TRAIN_TIP", tip)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    ensure!(status.success(), "could not start scripts/release.sh");
    Ok(())
}

fn release_exit(log: &Path) -> Option<i32> {
    std::fs::read_to_string(log).ok()?.lines().rev()
        .find_map(|line| line.strip_prefix("RELEASE_EXIT=")?.trim().parse().ok())
}

/// A started cut that exited nonzero wakes the supervisor once, with its log.
fn report_failure(root: &Path, train: &Task, lane: &str, tip: &str) -> Result<()> {
    let log = train_dir(root, &train.id)?.join("auto-cut.log");
    let Some(code) = release_exit(&log) else {
        return Ok(());
    };
    if code == 0 {
        return Ok(());
    }
    let repair = if code == NOTES_LINT_EXIT {
        format!("Correct the draft at {} and check it with `cas release notes check --train {}`. \
            Cassy stays armed and retries once when the draft changes.", notes_draft(root, &train.id)?.display(), train.id)
    } else {
        format!("Repair it, then on {lane} run `scripts/release.sh --notes {}`.", notes_draft(root, &train.id)?.display())
    };
    let attempted = std::fs::read_to_string(log.with_extension("notes")).unwrap_or_default();
    release_action(
        root,
        train,
        ReleaseNoticePhase::CutReady,
        &format!("auto-cut-failed:{tip}:{attempted}"),
        &format!(
            "Auto-cut of {lane} ({}) at {tip} failed: scripts/release.sh exited {}. Complete log: {}. \
             The train stays frozen. {repair}",
            train.id,
            code,
            log.display()
        ),
    )?;
    Ok(())
}

/// The operator's Commander pause or resume.
pub(crate) fn set_paused(root: &Path, train_id: &str, actor: &str, pause: bool) -> Result<String> {
    let store = crate::store::open_task_store(root)?;
    let mut train = store.get(train_id).with_context(|| format!("{train_id} not found"))?;
    ensure!(is_release_train(&train) && !train.is_terminal(), "{train_id} is not an open release train");
    if paused(&train) == pause {
        return Ok(format!("Auto-cut is already {} on {train_id}.", if pause { "paused" } else { "on" }));
    }
    train.labels.retain(|label| label != PAUSED_LABEL);
    if pause {
        train.labels.push(PAUSED_LABEL.into());
    }
    train.updated_at = Utc::now();
    store.update(&train)?;
    let verb = if pause { "paused" } else { "resumed" };
    store.append_note(train_id, &format!("RELEASE AUTO-CUT {}: {actor} {verb} auto-cut on {train_id}", verb.to_uppercase()))?;
    Ok(format!("Auto-cut {verb} on {train_id}."))
}

#[cfg(test)]
#[path = "release_auto_cut_tests.rs"]
mod tests;
