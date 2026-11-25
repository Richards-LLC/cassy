//! Durable assignment dispatch shared by task mutations and the director.

use cas_types::{AgentRole, Task, TaskStatus, TaskType};
use std::path::Path;

/// Task staffing never creates verifier authority. Sealed daemon verifiers
/// receive their dispatch directly, rather than accepting an ordinary task.
// gate[G-156 assign]: Review assignment requires verdict authority
pub(crate) fn ensure_review_assignment(
    dispatch_id: Option<&str>,
    role: Option<AgentRole>,
) -> Result<(), String> {
    let Some(dispatch_id) = dispatch_id else { return Ok(()) };
    if role == Some(AgentRole::Supervisor) { return Ok(()) }
    Err(crate::gates::tag("G-156", format!(
        "Review dispatch {dispatch_id} cannot be assigned to an ordinary worker or an unverified identity. \
         Use Cassy's daemon-owned verifier on this exact dispatch, or have a registered supervisor \
         record the verdict with verification action=add dispatch_id={dispatch_id}. \
         Staffing a task does not grant verifier authority."
    )))
}

/// The one-line form of cas-writing-tests's "Fewest tests" principle, carried once by
/// every task start (cas-5a52).
pub(crate) const TEST_PRINCIPLE: &str = "Elegant tests, not copious: write the fewest tests that fail without the change, one per behaviour, \
     through the real path, replaying the real incident; extend an existing test before adding one, and fix or delete a flaky test \
     instead of retrying it (cas-writing-tests, Fewest tests).";

/// cas-5bf1: what a supervisor's brief, request_changes reason or acceptance
/// criteria may not ask for under cas-writing-tests ("Fewest tests", "assert
/// outcomes, not wording"). The phrases live in that skill's
/// `references/brief-test-phrases.txt`, which the skill owns and ships; this reads
/// them. A hit warns and never refuses; a negation in the clause before the ask
/// ("no message text assertions") is the rule itself, never a violation.
// gate[N-106 notice]: A brief asks to assert message or output text
const TEXT_ASSERTION_CODE: &str = "N-106";
// gate[N-107 notice]: A brief asks for a test per item
const PER_ITEM_TEST_CODE: &str = "N-107";

const BRIEF_TEST_PHRASES: &str = include_str!("builtins/skills/cas-writing-tests/references/brief-test-phrases.txt");

/// The phrase file's `[section]` as one case-insensitive pattern.
fn phrase_section(section: &str) -> regex::Regex {
    let mut current = "";
    let patterns: Vec<&str> = BRIEF_TEST_PHRASES
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter(|line| match line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            Some(name) => { current = name; false }
            None => current == section,
        })
        .collect();
    regex::Regex::new(&format!("(?i){}", patterns.iter().map(|pattern| format!("(?:{pattern})")).collect::<Vec<_>>().join("|")))
        .expect("brief-test-phrases.txt holds valid patterns")
}

static PHRASES: std::sync::LazyLock<[(&'static str, regex::Regex); 2]> = std::sync::LazyLock::new(|| {
    [(TEXT_ASSERTION_CODE, phrase_section(TEXT_ASSERTION_CODE)), (PER_ITEM_TEST_CODE, phrase_section(PER_ITEM_TEST_CODE))]
});
static NEGATION: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| phrase_section("negation"));
static CLAUSE_BOUNDARY: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| phrase_section("clause-boundary"));

/// The warning codes `text` earns, each at most once, in the file's order. A
/// match is forgiven only by a negation in its own clause before it: from the
/// previous clause boundary to the match start.
pub(crate) fn test_policy_warnings(text: &str) -> Vec<&'static str> {
    let mut codes = Vec::new();
    for sentence in text.split(['.', ';', '\n']) {
        for (code, pattern) in PHRASES.iter() {
            let asks = pattern.find_iter(sentence).any(|found| {
                let before = &sentence[..found.start()];
                let clause = CLAUSE_BOUNDARY.find_iter(before).last().map_or(before, |boundary| &before[boundary.end()..]);
                !NEGATION.is_match(clause)
            });
            if asks && !codes.contains(code) {
                codes.push(*code);
            }
        }
    }
    codes
}

/// The warnings appended to a response, empty when `text` complies. Each names
/// its rule through its code; the request is never refused.
pub(crate) fn test_policy_note(text: &str) -> String {
    test_policy_warnings(text)
        .into_iter()
        .map(|code| match code {
            TEXT_ASSERTION_CODE => crate::gates::tag("N-106", "\nWarning: this asks to assert message or output text; cas-writing-tests rules out wording assertions. Nothing was refused."),
            _ => crate::gates::tag("N-107", "\nWarning: this asks for a test per item; cas-writing-tests (Fewest tests) extends one existing test. Nothing was refused."),
        })
        .collect()
}

/// Carried by every assignment and spawn brief. It points at the principle
/// rather than repeating it, since the task start the worker runs next
/// carries it (cas-d9db).
pub(crate) const WORKER_ASSIGNMENT_TEST_POLICY: &str = concat!(
    "Fewest tests: cas-writing-tests (task start states the principle). Assignment authorizes and requires those ",
    "tests covering the lines touched, which you run yourself per cas-worker ",
    "references/discipline.md. The full suite runs once on the landing tree: in the merge queue, else CI, else one supervisor run per epic. ",
    "Docs-only and no-code tasks prove their acceptance check instead (e.g. ",
    "markdownlint, `cas codemap status`, a published artifact); don't add code tests to them. ",
    "Your close parks the delivery; the merge verdict closes it. Never re-close, spawn a verifier or ask anyone to close for you."
);

/// Queue the assignment once for the current worker in its factory session. This
/// does not depend on the task appearing in the director's focused snapshot.
pub(crate) fn enqueue(cas_dir: &Path, task: &Task) -> crate::Result<Option<i64>> {
    enqueue_with_queue_occurrence(cas_dir, task, None, false, "")
}

pub(crate) fn enqueue_queued(cas_dir: &Path, task: &Task, occurrence: u64, initial: bool, repair_brief: &str) -> crate::Result<Option<i64>> {
    enqueue_with_queue_occurrence(cas_dir, task, Some(occurrence), initial, repair_brief)
}

// gate-audit[initial]: selector Initial enqueue is not a repair round; this flag chooses prompt identity and reuse, and fresh task leases own G-172 admission.
fn enqueue_with_queue_occurrence(cas_dir: &Path, task: &Task, occurrence: Option<u64>, initial: bool, repair_brief: &str) -> crate::Result<Option<i64>> {
    if !matches!(task.status, TaskStatus::Open | TaskStatus::InProgress)
        || matches!(task.task_type, TaskType::Epic | TaskType::Gate)
    {
        return Ok(None);
    }
    let Some(assignee) = task.assignee.as_deref() else {
        return Ok(None);
    };
    let agents = crate::store::open_agent_store(cas_dir)?.list(None)?;
    let worker = agents
        .iter()
        .filter(|agent| {
            agent.name.eq_ignore_ascii_case(assignee) || agent.id.eq_ignore_ascii_case(assignee)
        })
        .max_by_key(|agent| agent.registered_at)
        .filter(|agent| {
            agent.role == AgentRole::Worker
                && agent.is_alive()
                && chrono::Utc::now()
                    .signed_duration_since(agent.last_heartbeat)
                    .num_seconds()
                    <= cas_types::DEFAULT_HEARTBEAT_TIMEOUT_SECS
        });
    if let Some(worker) = worker {
        ensure_review_assignment(task.review_dispatch_id.as_deref(), Some(worker.role))
            .map_err(crate::error::CasError::Other)?;
    }
    // Queue admission precedes registration and optional prompt policy. A tail
    // is delivered by the ordered handoff, never as an immediate start prompt.
    if occurrence.is_none()
        && crate::worker_task_queue::admit_assignment(cas_dir, task, worker)?
    {
        return Ok(None);
    }
    let prompts = crate::config::Config::load(cas_dir)?.orchestration().auto_prompt;
    if occurrence.is_none() && (!prompts.enabled || !prompts.on_task_assigned) {
        return Ok(None);
    }
    let Some(worker) = worker else {
        return Ok(None);
    };
    let Some(session) = worker.factory_session.as_deref() else {
        return Ok(None);
    };
    // Respect hard start gates and external blockers through the canonical
    // ready policy. Ordinary blocks edges allow parallel preparation.
    if task.status == TaskStatus::Open
        && !crate::store::open_task_store(cas_dir)?
            .list_ready()?
            .iter()
            .any(|ready| ready.id == task.id)
    {
        return Ok(None);
    }
    let cli = crate::mcp::tools::service::factory_ops::worker_cli_from_agent(worker);
    let prefix = cli.backend().capabilities().tool_prefix;
    let mut text = format!(
        "You have been assigned a new task:\nTask ID: {}\nTitle: {}\n\n\
         View full details: {prefix}task action=show id={}\n\
         Start working: {prefix}task action=start id={} brief=true\n\
         Successful task action=start is authoritative assignment acceptance; no prose ACK is required.\n\
         {}\n\
         While working, post progress notes with {prefix}task action=notes.\n\
         If blocked, add a blocker note, set status=blocked, then send {prefix}coordination action=message target=supervisor blocker=true summary=\"...\" message=\"...\".",
        task.id,
        task.title,
        task.id,
        task.id,
        WORKER_ASSIGNMENT_TEST_POLICY,
    );
    if occurrence.is_some() && task.status == TaskStatus::InProgress {
        let branch = task.deliverables.factory_branch_start.as_ref()
            .map(|start| start.branch.as_str()).unwrap_or("see task details");
        text = format!("You have been assigned a new task:\nTask ID: {}\nTitle: {}\n\nCassy started this task automatically from your queue. Branch: {branch}. No assignment ACK is needed.\n\nView full details: {prefix}task action=show id={}\n\n{}\n\nWhile working, post progress notes with {prefix}task action=notes id={}.", task.id, task.title, task.id, WORKER_ASSIGNMENT_TEST_POLICY, task.id);
    }
    text.push_str(&crate::finding_triage::focus_brief(cas_dir));
    text.push_str(repair_brief);
    if occurrence.is_some() && task.status == TaskStatus::InProgress {
        text = crate::prompt_revalidation::attach_queue_continuation(&text, &task.id);
    }
    let key = match occurrence {
        Some(occurrence) if !initial => format!("task-queue-assignment:{}:{}:{occurrence}", task.id, worker.id),
        _ => assignment_key(session, &worker.name, &task.id),
    };
    let queue = crate::store::open_prompt_queue_store(cas_dir)?;
    let mut result = queue.enqueue_idempotent(
        "task-assignment",
        &worker.name,
        &text,
        Some(session),
        Some(&format!("Assigned task: {}", task.id)),
        Some(cas_store::NotificationPriority::High),
        &key,
        Some(&cas_store::QueueOrigin::Daemon),
    )?;
    let mut upgraded = false;
    if initial && task.status == TaskStatus::InProgress
        && let cas_store::EnqueueIdempotentResult::AlreadyExists(id) = result
        && !queue.recipient_handed_off(id, &worker.name)?
        && let Some(row) = queue.queued_prompt(id)?
        && crate::prompt_revalidation::assignment_stale_task(
            &row.prompt, task.status, task.assignee.as_deref(), &worker.name,
        ).is_some()
    {
        // An Open assignment may still be waiting when its queue head starts.
        // Preserve the brief, but make that same pending row a continuation.
        let branch = task.deliverables.factory_branch_start.as_ref()
            .map(|start| start.branch.as_str()).unwrap_or("see task details");
        let continued = crate::prompt_revalidation::attach_queue_continuation(
            &format!("{}\n\nCassy started this task automatically from your queue. Branch: {branch}. No assignment ACK is needed.{repair_brief}",
                crate::prompt_revalidation::strip_assignment_envelopes(&row.prompt)), &task.id,
        );
        upgraded = queue.rewrite_pending(id, &continued, Some(&format!("Assigned task: {}", task.id)))?;
        if !upgraded {
            // A stale row can already be terminal at recovery. A fresh queue
            // occurrence remains actionable without rewriting delivery history.
            result = queue.enqueue_idempotent(
                "task-assignment", &worker.name, &text, Some(session),
                Some(&format!("Assigned task: {}", task.id)), Some(cas_store::NotificationPriority::High),
                &format!("task-queue-assignment:{}:{}:{}", task.id, worker.id, occurrence.unwrap_or_default()),
                Some(&cas_store::QueueOrigin::Daemon),
            )?;
        }
    }
    if upgraded || matches!(result, cas_store::EnqueueIdempotentResult::Created(_)) {
        if let Err(error) = cas_factory::notify_daemon(cas_dir) {
            tracing::debug!(%error, "assignment queued; daemon wake datagram unavailable");
        }
    }
    Ok(Some(match result {
        cas_store::EnqueueIdempotentResult::Created(id)
        | cas_store::EnqueueIdempotentResult::AlreadyExists(id) => id,
    }))
}

/// Startup, registration repair and ordinary assignment share one durable row.
/// A queue successor uses its own occurrence key, so a later continuation is
/// never mistaken for the original launch instruction.
pub(crate) fn assignment_key(session: &str, worker: &str, task: &str) -> String {
    format!("task-assignment:{session}:{worker}:{task}")
}

/// A launched process is a new recipient even under a reused name. Rows its
/// predecessor received must not swallow this process's assignment, so the
/// shared key is scoped to one launch. Call once per launch, after it starts.
pub(crate) fn release_received_assignment_keys(
    cas_dir: &Path, session: &str, worker: &str,
) -> crate::Result<usize> {
    Ok(crate::store::open_prompt_queue_store(cas_dir)?.release_received_dedupe_keys(
        worker, session, &format!("task-assignment:{session}:{worker}:"),
    )?)
}

pub(crate) fn enqueue_spawn_message(
    cas_dir: &Path, session: &str, worker: &str, task: &str, message: &str,
) -> crate::Result<i64> {
    let result = crate::store::open_prompt_queue_store(cas_dir)?.enqueue_idempotent(
        "task-assignment", worker, message, Some(session),
        Some(&format!("Assigned task: {task}")), Some(cas_store::NotificationPriority::High),
        &assignment_key(session, worker, task), Some(&cas_store::QueueOrigin::Daemon),
    )?;
    Ok(match result {
        cas_store::EnqueueIdempotentResult::Created(id)
        | cas_store::EnqueueIdempotentResult::AlreadyExists(id) => id,
    })
}
