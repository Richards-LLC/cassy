//! Promotion check (design art-e3dfbeb7, finding 14): the commit status
//! [`RELEASE_GATE_CONTEXT`] on the head of a PR into `qa.release.branches`.
//!
//! The head passes only when a sealed CLEAN pack walked it, or walked a
//! commit with the same whole tree (Q7: a merge commit whose tree equals the
//! walked SHA counts as walked), and the target environment serves the flag
//! values the evidence ran under. The flag diff (cas-93d6) runs here, once
//! per evaluation. A registered supervisor's decision note
//! `RELEASE-GATE OVERRIDE sha=<full sha>: <reason>` passes that head (or a
//! tree-equal one) for 6 hours, and each use is logged (Q1).
//!
//! While a gate run walks the integration branch, [`publish_freeze`] holds
//! open PRs into it with a pending [`FREEZE_CONTEXT`] status (Q3), so merges
//! from outside Cassy wait too. Statuses go through the existing
//! `qa_pass::github_gate` publisher; `cas qa release-gate verify --sha X`
//! answers the same question for projects without GitHub.
use super::flags;
use super::pack::{PACK_FILE, Pack, pack_dir};
use super::verdict::GateVerdict;
use super::{ReleaseConfig, printable, resolve_commit, short, trees_equal};
use crate::qa_pass::github_gate::{self, QaCommitState};
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use serde::Serialize;
use std::path::Path;

/// The commit status branch protection on a promotion branch requires.
pub const RELEASE_GATE_CONTEXT: &str = "cassy/release-gate";
/// The commit status that holds PRs into the integration branch during a run.
pub const FREEZE_CONTEXT: &str = "cassy/release-freeze";
/// Decision-note marker: `RELEASE-GATE OVERRIDE sha=<40-hex>: <reason>`.
pub const OVERRIDE_MARKER: &str = "RELEASE-GATE OVERRIDE";
/// How long a recorded override passes its commit.
pub const OVERRIDE_HOURS: i64 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PromotionState {
    Success,
    Failure,
    /// No sealed pack exists yet.
    Pending,
}

impl PromotionState {
    fn commit_state(self) -> QaCommitState {
        match self {
            Self::Success => QaCommitState::Success,
            Self::Failure => QaCommitState::Failure,
            Self::Pending => QaCommitState::Pending,
        }
    }
}

/// A supervisor override that passed a head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Override {
    pub task_id: String,
    pub sha: String,
    pub reason: String,
    pub expires_at: DateTime<Utc>,
}

/// The promotion verdict for one head.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Promotion {
    pub head: String,
    pub state: PromotionState,
    /// The SHA whose sealed pack was read.
    pub walked: Option<String>,
    /// Why the head does not pass on its own evidence.
    pub reasons: Vec<String>,
    pub overridden: Option<Override>,
}

impl Promotion {
    /// The status description: the override, the walked pack, or the reasons.
    pub fn description(&self) -> String {
        if let Some(granted) = &self.overridden {
            return format!(
                "{OVERRIDE_MARKER} until {}: {}",
                granted.expires_at.format("%Y-%m-%d %H:%MZ"),
                printable(&granted.reason)
            );
        }
        match (self.state, &self.walked) {
            (PromotionState::Success, Some(walked)) if *walked == self.head => {
                format!("CLEAN release gate pack for {}", short(walked))
            }
            (PromotionState::Success, Some(walked)) => {
                format!("CLEAN release gate pack for tree-equal {}", short(walked))
            }
            _ => self.reasons.join("; "),
        }
    }
}

/// Evaluates `head` for promotion at `now`. Never publishes.
pub fn evaluate(
    project_root: &Path,
    cas_root: &Path,
    config: &ReleaseConfig,
    head: &str,
    now: DateTime<Utc>,
) -> Promotion {
    let mut promotion = Promotion {
        head: head.to_ascii_lowercase(),
        state: PromotionState::Failure,
        walked: None,
        reasons: Vec::new(),
        overridden: None,
    };
    let Some(commit) = resolve_commit(project_root, head) else {
        promotion
            .reasons
            .push(format!("head {} is not a local commit; fetch it", printable(head)));
        return promotion;
    };
    promotion.head = commit.clone();
    match judge(project_root, cas_root, config, &commit) {
        Ok(walked) => {
            promotion.walked = Some(walked);
            promotion.state = PromotionState::Success;
        }
        Err(Judged { state, walked, reasons }) => {
            promotion.state = state;
            promotion.walked = walked;
            promotion.reasons = reasons;
            if let Some(granted) = recorded_override(project_root, cas_root, &commit, now) {
                promotion.state = PromotionState::Success;
                promotion.overridden = Some(granted);
            }
        }
    }
    promotion
}

struct Judged {
    state: PromotionState,
    walked: Option<String>,
    reasons: Vec<String>,
}

fn failed(walked: Option<String>, reasons: Vec<String>) -> Judged {
    Judged { state: PromotionState::Failure, walked, reasons }
}

/// The walked SHA when the head's evidence passes, else why not.
fn judge(project_root: &Path, cas_root: &Path, config: &ReleaseConfig, head: &str) -> Result<String, Judged> {
    let pack = find_pack(project_root, cas_root, head)?;
    let walked = Some(pack.sha.clone());
    match pack.gate.verdict {
        GateVerdict::Clean => {}
        GateVerdict::Failed => {
            return Err(failed(walked, vec![format!("FAILED rows: {}", pack.gate.failed.join(", "))]));
        }
        GateVerdict::Owed => {
            let owed: Vec<String> =
                pack.gate.owed.iter().map(|o| format!("{} ({})", o.subject, o.cause)).collect();
            return Err(failed(walked, vec![format!("OWED: {}", owed.join("; "))]));
        }
    }
    let journeys = walked_journeys(project_root, cas_root, &pack).map_err(|e| failed(walked.clone(), vec![e]))?;
    let target = flags::read(project_root, config, &config.target_env, head, &journeys)
        .map_err(|e| failed(walked.clone(), vec![format!("FLAGS UNKNOWN on {}: {e}", printable(&config.target_env))]))?;
    let mismatches = flags::compare(&pack.flags.clone().unwrap_or_default(), &target, &journeys);
    if !mismatches.is_empty() {
        return Err(failed(walked, mismatches.iter().map(ToString::to_string).collect()));
    }
    Ok(pack.sha)
}

/// The registry journeys the pack's rows walked; their declared flags are
/// the ones the evidence depends on.
fn walked_journeys(
    project_root: &Path,
    cas_root: &Path,
    pack: &Pack,
) -> Result<Vec<crate::qa_registry::Journey>, String> {
    let qa = crate::config::Config::load(cas_root).map(|c| c.qa()).unwrap_or_default();
    let path = project_root.join(&qa.registry.path);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let registry = crate::qa_registry::load(&path)
        .map_err(|e| format!("journey registry unreadable, so flag drift cannot be checked: {e:#}"))?;
    Ok(registry
        .journeys
        .into_iter()
        .filter(|journey| pack.rows.iter().any(|row| row.journey == journey.id))
        .collect())
}

/// The sealed pack for `head`, else a CLEAN (or the newest) pack whose
/// walked commit has the same whole tree.
fn find_pack(project_root: &Path, cas_root: &Path, head: &str) -> Result<Pack, Judged> {
    let exact = pack_dir(cas_root, head).map_err(|e| failed(None, vec![e.to_string()]))?;
    if exact.join(PACK_FILE).exists() {
        return Pack::load(&exact)
            .map_err(|e| failed(Some(head.into()), vec![format!("pack for {} is invalid: {e}", short(head))]));
    }
    let release = exact.parent().expect("pack dir has a parent");
    let mut sealed: Vec<(std::time::SystemTime, String)> = std::fs::read_dir(release)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let modified = entry.path().join(PACK_FILE).metadata().ok()?.modified().ok()?;
            (name.len() == 40 && name.bytes().all(|b| b.is_ascii_hexdigit())).then_some((modified, name))
        })
        .collect();
    sealed.sort_by(|a, b| b.cmp(a));
    let Some((_, latest)) = sealed.first() else {
        return Err(Judged {
            state: PromotionState::Pending,
            walked: None,
            reasons: vec![format!("no sealed release gate pack walked {} yet", short(head))],
        });
    };
    let mut equal: Vec<Pack> = sealed
        .iter()
        .filter(|(_, sha)| trees_equal(project_root, sha, head, &[]) == Some(true))
        .filter_map(|(_, sha)| Pack::load(&release.join(sha)).ok())
        .collect();
    if let Some(clean) = equal.iter().position(|p| p.gate.verdict == GateVerdict::Clean) {
        return Ok(equal.swap_remove(clean));
    }
    if !equal.is_empty() {
        return Ok(equal.swap_remove(0));
    }
    Err(failed(
        Some(latest.clone()),
        vec![format!(
            "head {} was not walked: the latest pack walked {} and their trees differ",
            short(head),
            short(latest)
        )],
    ))
}

/// The newest recorded override for `head`, or a tree-equal commit, still in
/// its window. Each use is logged once per head on the note's task.
fn recorded_override(project_root: &Path, cas_root: &Path, head: &str, now: DateTime<Utc>) -> Option<Override> {
    let store = crate::store::open_task_store(cas_root).ok()?;
    let mut found: Option<Override> = None;
    for task in store.list(None).ok()? {
        if !task.notes.contains(OVERRIDE_MARKER) {
            continue;
        }
        for line in task.notes.lines() {
            let Some((at, sha, reason)) = parse_override(line) else { continue };
            let expires_at = at + Duration::hours(OVERRIDE_HOURS);
            let fresh = at - Duration::minutes(5) <= now && now <= expires_at;
            let newer = found.as_ref().is_none_or(|f| expires_at > f.expires_at);
            if fresh && newer && (sha == head || trees_equal(project_root, &sha, head, &[]) == Some(true)) {
                found = Some(Override { task_id: task.id.clone(), sha, reason, expires_at });
            }
        }
    }
    let granted = found?;
    let log = format!(
        "release gate overridden for {head} by the supervisor override for {} (expires {}): {}",
        granted.sha,
        granted.expires_at.format("%Y-%m-%d %H:%MZ"),
        printable(&granted.reason)
    );
    let logged = store.get(&granted.task_id).is_ok_and(|task| task.notes.contains(&log));
    if !logged {
        let _ = store.append_note(&granted.task_id, &format!("[{}] ✅ DECISION {log}", now.format("%Y-%m-%d %H:%M")));
    }
    Some(granted)
}

/// Whether any line of `text` names a concrete override
/// (`RELEASE-GATE OVERRIDE sha=<40-hex>:`). A placeholder does not.
pub fn names_override(text: &str) -> bool {
    text.match_indices(OVERRIDE_MARKER).any(|(at, _)| {
        text[at + OVERRIDE_MARKER.len()..].strip_prefix(" sha=").is_some_and(|rest| {
            let hex = rest.bytes().take_while(u8::is_ascii_hexdigit).count();
            hex == 40 && rest[hex..].starts_with(':')
        })
    })
}

/// Whether a decision-note body is a well-formed override:
/// `RELEASE-GATE OVERRIDE sha=<40-hex>: <reason>`.
pub fn is_override_note(body: &str) -> bool {
    parse_override(&format!("[2000-01-01 00:00] ✅ DECISION {}", body.trim())).is_some()
}

/// `[YYYY-MM-DD HH:MM] ✅ DECISION RELEASE-GATE OVERRIDE sha=<40-hex>: <reason>`.
/// Callers other than a live registered supervisor's decision note cannot
/// write the marker (`caller_note_marker_rejection`).
fn parse_override(line: &str) -> Option<(DateTime<Utc>, String, String)> {
    let (stamp, body) = line.trim_start().strip_prefix('[')?.split_once("] ✅ DECISION ")?;
    let at = NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M").ok()?.and_utc();
    let (sha, reason) = body.strip_prefix(OVERRIDE_MARKER)?.strip_prefix(" sha=")?.split_once(':')?;
    let reason = reason.trim();
    let full = sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit());
    (full && !reason.is_empty()).then(|| (at, sha.to_ascii_lowercase(), reason.to_owned()))
}

/// Publishes `promotion` as [`RELEASE_GATE_CONTEXT`] on its head.
pub fn publish(project_root: &Path, promotion: &Promotion) -> Result<(), String> {
    github_gate::publish_status(
        project_root,
        &promotion.head,
        RELEASE_GATE_CONTEXT,
        promotion.state.commit_state(),
        &promotion.description(),
    )
}

/// Evaluates and publishes the head of every open PR into
/// `qa.release.branches`; call after a pack is sealed. Returns the number
/// published. Off unless `qa.github_status` is on.
pub fn publish_promotions(cas_root: &Path, project_root: &Path, config: &ReleaseConfig) -> Result<usize, String> {
    if !github_gate::github_status_enabled(cas_root) {
        return Ok(0);
    }
    let mut published = 0;
    for branch in &config.branches {
        for (_, head) in github_gate::open_pr_heads(project_root, branch)? {
            publish(project_root, &evaluate(project_root, cas_root, config, &head, Utc::now()))?;
            published += 1;
        }
    }
    Ok(published)
}

/// Holds (`frozen`) or releases every open PR into the integration branch
/// with [`FREEZE_CONTEXT`]: pending when a run starts, success at seal or
/// abort. Returns the number published. Off unless `qa.github_status` is on.
pub fn publish_freeze(
    cas_root: &Path,
    project_root: &Path,
    config: &ReleaseConfig,
    frozen: bool,
) -> Result<usize, String> {
    if !github_gate::github_status_enabled(cas_root) || config.integration_branch.is_empty() {
        return Ok(0);
    }
    let (state, description) = if frozen {
        (
            QaCommitState::Pending,
            format!("Release gate is walking {}; merges wait for its seal", printable(&config.integration_branch)),
        )
    } else {
        (QaCommitState::Success, "No release gate run holds this branch".to_string())
    };
    let heads = github_gate::open_pr_heads(project_root, &config.integration_branch)?;
    for (_, head) in &heads {
        github_gate::publish_status(project_root, head, FREEZE_CONTEXT, state, &description)?;
    }
    Ok(heads.len())
}

/// `cas qa release-gate verify --sha X`: evaluates the head, publishes the
/// status when `qa.github_status` is on, and fails unless it passes.
pub fn verify_cli(cas_root: &Path, sha: &str, json: bool) -> anyhow::Result<()> {
    let config = super::configured(cas_root)
        .ok_or_else(|| anyhow::anyhow!("No qa.release.adapter configured; set qa.release.adapter first"))?;
    let toplevel = super::git(Path::new("."), &["rev-parse", "--show-toplevel"])
        .filter(|output| output.status.success())
        .map(|output| std::path::PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()));
    let project_root = match &toplevel {
        Some(root) => root.as_path(),
        None => cas_root.parent().ok_or_else(|| anyhow::anyhow!("Cassy root has no project directory"))?,
    };
    let promotion = evaluate(project_root, cas_root, &config, sha, Utc::now());
    let published = github_gate::github_status_enabled(cas_root).then(|| publish(project_root, &promotion));
    if json {
        let mut value = serde_json::to_value(&promotion)?;
        value["description"] = promotion.description().into();
        value["published"] = match &published {
            Some(Ok(())) => true.into(),
            Some(Err(error)) => error.clone().into(),
            None => false.into(),
        };
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        let word = match promotion.state {
            PromotionState::Success => "PASS",
            PromotionState::Failure => "FAIL",
            PromotionState::Pending => "PENDING",
        };
        println!("{RELEASE_GATE_CONTEXT} {word} {}: {}", short(&promotion.head), promotion.description());
        if let Some(Err(error)) = &published {
            println!("[WARN] status not published: {error}");
        }
    }
    anyhow::ensure!(
        promotion.state == PromotionState::Success,
        "release gate does not pass {}",
        short(&promotion.head)
    );
    Ok(())
}
