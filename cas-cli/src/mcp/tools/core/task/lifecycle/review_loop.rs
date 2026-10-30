//! Review findings do not become work unless they clear the spawn threshold.
//!
//! A follow-up that originates from a verifier, QA or reviewer finding must say
//! how to reproduce the wrong behaviour and who acts on it today. Every such task
//! carries [`FINDING_LABEL`], and the epic's loop gain counts them against its
//! completed reviews: at 0.5 or more, reviews are generating work about as fast as
//! they finish it, and the supervisor hears about it once per crossing.

use std::path::Path;

use cas_types::{DependencyType, QaPassState, TaskType, VerificationStatus};
use chrono::{DateTime, Utc};

pub(crate) const FINDING_LABEL: &str = "from-finding";
/// Declares that text which looks like a finding is not one.
const NOT_A_FINDING: &str = "none";
const LOOP_GAIN_WINDOW: usize = 10;
const LOOP_GAIN_LIMIT: f64 = 0.5;

pub(crate) struct SpawnRequest<'a> {
    pub(crate) origin: Option<&'a str>,
    pub(crate) repro: Option<&'a str>,
    pub(crate) affected: Option<&'a str>,
    pub(crate) title: &'a str,
    pub(crate) description: &'a str,
    pub(crate) priority: u8,
    pub(crate) task_type: TaskType,
    /// The caller is itself reviewing: a verifier, or a worker on a QA task.
    pub(crate) reviewer_origin: Option<&'a str>,
    /// Factory workers are never the operator or supervisor observing directly.
    pub(crate) is_worker: bool,
}

/// Text that names a review finding as its source. A P0/P1 from the operator or a
/// supervisor can mention these words about its own observation.
fn origin_in_text(text: &str) -> Option<String> {
    let text = text.to_ascii_lowercase();
    if let Some(start) = text.find("ver-") {
        let id: String = text[start + 4..].chars().take_while(char::is_ascii_hexdigit).collect();
        if id.len() >= 6 {
            return Some(format!("verification:ver-{id}"));
        }
    }
    ["found in qa of", "(found in qa", "verifier finding", "verifier warning", "non-blocking verifier"]
        .into_iter()
        .find(|phrase| text.contains(phrase))
        .map(|phrase| format!("text:{phrase}"))
}

/// The origin to record, or `None` when this is not a follow-up from a finding.
pub(crate) fn spawn_gate(request: &SpawnRequest<'_>) -> Result<Option<String>, String> {
    let explicit = request.origin.map(str::trim).filter(|origin| !origin.is_empty());
    let origin = match (request.reviewer_origin, explicit) {
        (Some(reviewer), _) => reviewer.to_string(),
        (None, Some(origin)) if origin.eq_ignore_ascii_case(NOT_A_FINDING) => return Ok(None),
        (None, Some(origin)) => origin.to_string(),
        (None, None) => {
            let own_observation =
                !request.is_worker && request.task_type == TaskType::Bug && request.priority <= 1;
            let text = format!("{}\n{}", request.title, request.description);
            match origin_in_text(&text).filter(|_| !own_observation) {
                Some(origin) => origin,
                None => return Ok(None),
            }
        }
    };
    let filled = |field: Option<&str>| field.is_some_and(|value| !value.trim().is_empty());
    if filled(request.repro) && filled(request.affected) {
        return Ok(Some(origin));
    }
    Err(format!(
        "SPAWN THRESHOLD: this follow-up originates from a review finding ({origin}). A finding becomes a task only when it is a reproduced, observable defect affecting someone today. Pass `repro` (the steps or command, plus the wrong output you observed) and `affected` (who acts on that output today), or leave it in the verdict. If this is not a review finding, pass finding_origin=\"{NOT_A_FINDING}\"."
    ))
}

/// The durable record of why this follow-up cleared the threshold.
pub(crate) fn origin_note(origin: &str, repro: &str, affected: &str) -> String {
    format!("Finding origin: {origin}\nRepro: {}\nAffected: {}", repro.trim(), affected.trim())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LoopGain {
    pub(crate) followups: usize,
    pub(crate) reviews: usize,
}

impl LoopGain {
    pub(crate) fn ratio(self) -> f64 {
        self.followups as f64 / self.reviews as f64
    }

    pub(crate) fn tripped(self) -> bool {
        self.ratio() >= LOOP_GAIN_LIMIT
    }

    pub(crate) fn status_line(self) -> String {
        let warning = if self.tripped() { " — findings are being filed as work; apply the spawn threshold" } else { "" };
        format!(
            "Review loop gain: {:.2} ({} follow-ups from findings over the last {} reviews){warning}\n",
            self.ratio(),
            self.followups,
            self.reviews
        )
    }
}

/// Follow-ups created since the oldest of the last ten completed reviews.
fn loop_gain_from(mut reviews: Vec<DateTime<Utc>>, followups: &[DateTime<Utc>]) -> Option<LoopGain> {
    reviews.sort_unstable();
    let window = &reviews[reviews.len().saturating_sub(LOOP_GAIN_WINDOW)..];
    let start = *window.first()?;
    Some(LoopGain {
        followups: followups.iter().filter(|created| **created >= start).count(),
        reviews: window.len(),
    })
}

pub(crate) fn epic_loop_gain(cas_root: &Path, epic: &str) -> Option<LoopGain> {
    let tasks = crate::store::open_task_store(cas_root).ok()?;
    let verifications = crate::store::open_verification_store(cas_root).ok()?;
    let mut reviews = Vec::new();
    let mut followups = Vec::new();
    for dependency in tasks.get_dependents(epic).ok()? {
        if dependency.dep_type != DependencyType::ParentChild {
            continue;
        }
        let Ok(task) = tasks.get(&dependency.from_id) else {
            continue;
        };
        if task.labels.iter().any(|label| label == FINDING_LABEL) {
            followups.push(task.created_at);
        }
        reviews.extend(
            verifications
                .get_for_task(&task.id)
                .unwrap_or_default()
                .into_iter()
                .filter(|verdict| matches!(verdict.status, VerificationStatus::Approved | VerificationStatus::Rejected))
                .map(|verdict| verdict.created_at),
        );
        reviews.extend(
            cas_store::list_qa_passes(cas_root, &task.id)
                .unwrap_or_default()
                .into_iter()
                .filter(|pass| !pass.is_withdrawn() && matches!(pass.state, QaPassState::Passed | QaPassState::Failed))
                .map(|pass| pass.requested_at),
        );
    }
    loop_gain_from(reviews, &followups)
}

/// Called after a follow-up from a finding joins an epic. Tells the supervisor
/// only when this follow-up crossed the limit, so a tripped epic does not repeat.
pub(crate) fn notify_on_crossing(cas_root: &Path, epic: &str) {
    let Some(gain) = epic_loop_gain(cas_root, epic).filter(|gain| gain.tripped()) else {
        return;
    };
    let before = LoopGain { followups: gain.followups.saturating_sub(1), ..gain };
    if before.tripped() {
        return;
    }
    let notice = format!(
        "review loop gain {:.2}; findings are being filed as work; apply the spawn threshold",
        gain.ratio()
    );
    let Ok(queue) = crate::store::open_prompt_queue_store(cas_root) else {
        return;
    };
    let session = std::env::var("CAS_FACTORY_SESSION").ok().filter(|session| !session.trim().is_empty());
    let source = format!("review-loop-gain:{epic}:{}", gain.followups);
    if let Err(error) = queue.enqueue_idempotent(
        &source,
        "supervisor",
        &notice,
        session.as_deref(),
        Some(&notice),
        Some(cas_store::NotificationPriority::Normal),
        &source,
        Some(&cas_store::QueueOrigin::Daemon),
    ) {
        tracing::warn!(%epic, %error, "review loop gain notice not queued");
    }
    crate::ui::factory::daemon::runtime::delivery::wake_daemon_after_enqueue(cas_root);
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_types::{Dependency, Task, Verification};

    fn request<'a>() -> SpawnRequest<'a> {
        SpawnRequest {
            origin: None,
            repro: None,
            affected: None,
            title: "Tighten a check",
            description: "Non-blocking verifier findings on cas-4d4f (ver-7e87d441000d).",
            priority: 3,
            task_type: TaskType::Task,
            reviewer_origin: None,
            is_worker: false,
        }
    }

    #[test]
    fn a_follow_up_from_a_finding_needs_repro_and_affected() {
        let refusal = spawn_gate(&request()).unwrap_err();
        assert!(refusal.contains("SPAWN THRESHOLD"), "{refusal}");
        let accepted = SpawnRequest { repro: Some("cas x prints y"), affected: Some("the supervisor"), ..request() };
        assert_eq!(spawn_gate(&accepted).unwrap().as_deref(), Some("verification:ver-7e87d441000d"));

        let reviewer = SpawnRequest {
            description: "A plain description.",
            origin: Some("none"),
            reviewer_origin: Some("qa:cas-q"),
            ..request()
        };
        assert!(spawn_gate(&reviewer).is_err(), "a reviewer cannot declare its way out");
        let declared = SpawnRequest { origin: Some("none"), ..request() };
        assert_eq!(spawn_gate(&declared).unwrap(), None);
    }

    /// A supervisor's own P1 bug may mention a verifier; a worker's may not, and
    /// naming a finding as its origin removes the exemption.
    #[test]
    fn own_observed_p0_p1_bugs_are_exempt() {
        let own = SpawnRequest { priority: 1, task_type: TaskType::Bug, ..request() };
        assert_eq!(spawn_gate(&own).unwrap(), None);
        let worker = SpawnRequest { priority: 1, task_type: TaskType::Bug, is_worker: true, ..request() };
        assert!(spawn_gate(&worker).is_err());
        let named = SpawnRequest { priority: 1, task_type: TaskType::Bug, origin: Some("verification:ver-1"), ..request() };
        assert!(spawn_gate(&named).is_err());
    }

    #[test]
    fn the_breaker_notifies_once_when_loop_gain_crosses_the_limit() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        env.set_current_dir(dir.path());
        let cas = crate::store::init_cas_dir(dir.path()).unwrap();
        let tasks = crate::store::open_task_store(&cas).unwrap();
        let verifications = crate::store::open_verification_store(&cas).unwrap();
        let queue = crate::store::open_prompt_queue_store(&cas).unwrap();
        let mut epic = Task::new("cas-epic".to_string(), "Epic".to_string());
        epic.task_type = TaskType::Epic;
        tasks.add(&epic).unwrap();
        let add = |id: &str, finding: bool| {
            let mut task = Task::new(id.to_string(), id.to_string());
            if finding {
                task.labels.push(FINDING_LABEL.to_string());
            }
            tasks.add(&task).unwrap();
            tasks
                .add_dependency(&Dependency::new(id.to_string(), "cas-epic".to_string(), DependencyType::ParentChild))
                .unwrap();
        };
        for id in ["cas-a", "cas-b"] {
            add(id, false);
            let mut verdict = Verification::new(format!("ver-{id}"), id.to_string());
            verdict.status = VerificationStatus::Approved;
            verdict.created_at = Utc::now() - chrono::Duration::hours(1);
            verifications.add(&verdict).unwrap();
        }
        let notices = || queue.peek_all(10).unwrap().len();

        // One follow-up over two reviews reaches 0.5, and that crossing is told.
        add("cas-f1", true);
        notify_on_crossing(&cas, "cas-epic");
        let gain = epic_loop_gain(&cas, "cas-epic").unwrap();
        assert_eq!(gain, LoopGain { followups: 1, reviews: 2 });
        assert!(gain.status_line().contains("0.50"), "{}", gain.status_line());
        assert_eq!(notices(), 1);

        // Already tripped: the next follow-up does not repeat it.
        add("cas-f2", true);
        notify_on_crossing(&cas, "cas-epic");
        assert_eq!(notices(), 1);
    }
}
