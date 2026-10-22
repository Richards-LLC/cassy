use crate::mcp::tools::service::imports::*;
use crate::prompt_revalidation::{
    assignment_solicited_task_id, assignment_stale_task, urgent_assignment_task_id,
};

pub(crate) fn resolve_worker_supervisor_target(
    agent_store: &dyn cas_store::AgentStore,
    factory_session: Option<&str>,
) -> Option<crate::mcp::tools::core::task::lifecycle::supervisor_push::OwningSupervisor> {
    let session = factory_session.map(str::trim).filter(|session| !session.is_empty())?;
    crate::mcp::tools::core::task::lifecycle::supervisor_push::resolve_worker_supervisor(
        agent_store,
        Some(session),
    )
}

fn collapse_message_summary(summary: &str) -> String {
    let mut collapsed = String::with_capacity(summary.len());
    let mut pending_space = false;
    for character in summary.chars() {
        if character.is_control() {
            pending_space |= character.is_whitespace() && !collapsed.is_empty();
            continue;
        }
        if character.is_whitespace() {
            pending_space |= !collapsed.is_empty();
            continue;
        }
        if pending_space {
            collapsed.push(' ');
            pending_space = false;
        }
        collapsed.push(character);
    }
    collapsed
}

fn resolve_inbox_recipient(
    registered_name: Option<String>,
    environment_name: Option<String>,
) -> Option<String> {
    registered_name
        .or(environment_name)
        .filter(|name| !name.trim().is_empty())
}

fn extract_pull_request_number(text: &str) -> Option<u64> {
    for marker in ["pull/", "PR #", "pr #", "Pull Request #"] {
        let mut offset = 0;
        while let Some(relative) = text[offset..].find(marker) {
            let start = offset + relative + marker.len();
            let digits = text[start..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>();
            if !digits.is_empty()
                && let Ok(number) = digits.parse()
            {
                return Some(number);
            }
            offset = start;
        }
    }
    None
}

/// Marker prefixed to any inbox row this poll is handing over for a second
/// time (cas-99d2, GH #127).
///
/// Recipients — human or agent — must be able to tell a repeat from a new
/// instruction mechanically, without reasoning about timestamps. The daemon's
/// teams-inbox writer already treats this exact token as the signal for an
/// *intentional* redelivery (see `daemon::runtime::teams`), so the same marker
/// means the same thing on both delivery channels.
pub(crate) const INBOX_REDELIVERY_MARKER: &str = "[redelivery]";

/// Durable provenance rendered with every queue-backed message (cas-4a27).
///
/// A plain sender prefix cannot distinguish a late spawn task brief from a
/// fresh supervisor reply. The queue already knows the facts a worker needs;
/// keep them visible at the final render boundary instead of asking an agent
/// to reconstruct them from prose and timing. Five minutes is an operational
/// freshness marker, not the 24-hour terminal quarantine TTL: a message can be
/// valid but still worth calling out as late to the recipient.
pub(crate) const MESSAGE_PROVENANCE_STALE_AFTER_SECS: i64 = 5 * 60;

/// Fixed sender namespace the Commander daemon path writes (cas-7f81). The
/// label after it is what the client sent; identity comes from the stamp.
pub(crate) const COMMANDER_SOURCE_PREFIX: &str = "commander:";

/// How a Commander-originated row classifies (cas-7f81, EPIC cas-fbc8).
///
/// `Verified` requires BOTH the `commander:` source namespace and a
/// hub-stamped [`cas_store::QueueOrigin::PairedDevice`] origin. The label is
/// never enough on its own: it is client-typed and a spoofed one must render
/// as `unverified:<label>`, not as the operator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OperatorClass {
    Verified { operator: String, device: String },
    Unverified { label: String },
}

pub(crate) fn operator_class(message: &cas_store::QueuedPrompt) -> Option<OperatorClass> {
    let label = message.source.strip_prefix(COMMANDER_SOURCE_PREFIX)?;
    let verified = message
        .origin
        .as_ref()
        .and_then(cas_store::QueueOrigin::verified_device_id)
        .is_some();
    if !verified {
        return Some(OperatorClass::Unverified {
            label: label.to_owned(),
        });
    }
    // cas-e8df: a stamped row names the operator and device from the
    // credential record; the label is only the fallback for rows written
    // before the operator columns existed.
    if let Some(stamp) = message.operator.as_ref().filter(|stamp| stamp.verified) {
        let device = if stamp.device_label.is_empty() {
            stamp.device_id.clone()
        } else {
            stamp.device_label.clone()
        };
        return Some(OperatorClass::Verified {
            operator: stamp.operator.clone(),
            device,
        });
    }
    let (operator, device) = label
        .rsplit_once('@')
        .map(|(operator, device)| (operator.to_owned(), device.to_owned()))
        .unwrap_or_else(|| (label.to_owned(), "unknown-device".to_owned()));
    Some(OperatorClass::Verified { operator, device })
}

pub(crate) fn queued_message_provenance_at(
    message: &cas_store::QueuedPrompt,
    observed_at: chrono::DateTime<chrono::Utc>,
) -> String {
    let delivery = if message.processed_at.is_some() {
        "replay"
    } else {
        "first"
    };
    let age_secs = (observed_at - message.created_at).num_seconds().max(0);
    match operator_class(message) {
        Some(OperatorClass::Verified { operator, device }) => {
            return format!(
                "[cas #{} operator {operator}@{device} verified {age_secs}s {delivery}]",
                message.id
            );
        }
        Some(OperatorClass::Unverified { label }) => {
            return format!(
                "[cas #{} unverified:{label} {age_secs}s {delivery}]",
                message.id
            );
        }
        None => {}
    }
    let origin = if message.source.eq_ignore_ascii_case("supervisor") {
        "supervisor-authored"
    } else if message.source.eq_ignore_ascii_case("viktor") {
        "viktor"
    } else if message.source.eq_ignore_ascii_case("director")
        && message
            .summary
            .as_deref()
            .is_some_and(|summary| summary.starts_with("Assigned task:"))
    {
        "spawn-boilerplate"
    } else if message.source.eq_ignore_ascii_case("director") {
        "director-generated"
    } else if message.source.starts_with("lifecycle-wake:")
        || message.source.starts_with("lifecycle:")
    {
        // cas-8cbe: the non-waking progress relays (started/ready/closed) are
        // CAS-written too; labelling them agent-authored misread them as a
        // worker talking.
        "lifecycle-relay"
    } else if message.origin == Some(cas_store::QueueOrigin::Daemon)
        && let Some(peer) = crate::cloud::peer_mailbox::parse_envelope(&message.prompt)
    {
        // cas-f9c7: never operator authority; named for what it is.
        // cas-2a5a: a supervisor of another project of the same account
        // names its project.
        let project = (peer.scope == crate::cloud::peer_mailbox::PeerScope::User)
            .then(|| crate::cloud::peers::project_name(&peer.repo))
            .flatten()
            .map(|project| format!(" project={project}"))
            .unwrap_or_default();
        return format!(
            "[cas #{} peer-supervisor {}@{}{project} via-cloud {age_secs}s {delivery}]",
            message.id, peer.sender_name, peer.machine
        );
    } else {
        "agent-authored"
    };
    format!(
        "[cas #{} {} {}s {}]",
        message.id,
        origin,
        age_secs,
        delivery,
    )
}

/// cas-8cbe: the lead of the supervisor's copy of a worker-to-worker message.
const PEER_COPY_LEAD: &str = "Peer worker message copy — from ";

/// The text of the supervisor's copy of a worker peer message.
pub(crate) fn peer_copy_text(from: &str, to: &str, message: &str) -> String {
    format!("{PEER_COPY_LEAD}{from} to {to}.\n\n{message}")
}

/// `(from, to, body)` of a supervisor peer copy built by [`peer_copy_text`].
pub(crate) fn parse_peer_copy(prompt: &str) -> Option<(&str, &str, &str)> {
    let (header, body) = prompt.strip_prefix(PEER_COPY_LEAD)?.split_once(".\n\n")?;
    let (from, to) = header.split_once(" to ")?;
    Some((from, to, body))
}

/// A successful shared delivery claim is the first handoff for this recipient.
/// Queue processing timestamps describe transport work, not earlier model delivery.
pub(crate) fn first_message_provenance(message: &cas_store::QueuedPrompt) -> String {
    let mut claimed = message.clone();
    claimed.processed_at = None;
    queued_message_provenance(&claimed)
}

pub(crate) fn queued_message_provenance(message: &cas_store::QueuedPrompt) -> String {
    queued_message_provenance_at(message, chrono::Utc::now())
}

/// The only safe reply target for a verified Commander row is the operator
/// lane. Keep this exact command beside the row so a supervisor never has to
/// turn the human-readable `From:` label back into a routing target.
pub(crate) fn commander_reply_command(message: &cas_store::QueuedPrompt) -> Option<String> {
    matches!(operator_class(message), Some(OperatorClass::Verified { .. })).then(|| {
        format!(
            "coordination action=message target=operator in_reply_to={} summary=… message=…",
            message.id
        )
    })
}

pub(crate) const COMMANDER_REPLY_CONTRACT_POINTER: &str =
    "Phone reply contract: see `cas-supervisor/references/operator-reply.md`.";

fn commander_reply_framing(message: &cas_store::QueuedPrompt) -> Option<String> {
    commander_reply_command(message)
        .map(|command| format!("Reply with: `{command}`\n{COMMANDER_REPLY_CONTRACT_POINTER}\n"))
}

/// These values are CAS-generated queue-source labels, never agent names.
/// Plain names intentionally stay on the queue-before-register path: a
/// supervisor may send a worker assignment before its registration lands.
pub(crate) fn target_is_reserved_source(target: &str) -> bool {
    let target = target.trim().to_ascii_lowercase();
    target.is_empty()
        || target == "director-generated"
        || target.starts_with("director-generated:")
        || [
            "lifecycle:",
            "lifecycle-wake:",
            "verification-dispatch:",
        ]
        .iter()
        .any(|prefix| target.starts_with(prefix))
}


/// cas-009c: how a row named by `in_reply_to` relates to this send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplyReference {
    /// The counterparty's row to this sender: the reply answers it, and
    /// acknowledges it.
    DirectReply,
    /// A row in the same conversation that this reply cites without
    /// answering: the sender's own earlier row to the counterparty or to
    /// all workers, the counterparty's broadcast, or a row CAS itself wrote
    /// to the sender (lifecycle, verification dispatch, director, delivery
    /// watchdog).
    Thread,
}

/// Classify a prior row against the two parties' names. `counterparty` and
/// `me` list every name each side's rows may carry; empty entries are
/// ignored. `None` means the row belongs to someone else's conversation.
fn classify_reply_reference(
    prior_source: &str,
    prior_target: &str,
    counterparty: &[&str],
    me: &[&str],
) -> Option<ReplyReference> {
    let is = |name: &str, names: &[&str]| {
        names
            .iter()
            .any(|candidate| !candidate.is_empty() && candidate.eq_ignore_ascii_case(name))
    };
    let broadcast = prior_target.eq_ignore_ascii_case("all_workers");
    let cas_generated = target_is_reserved_source(prior_source)
        || ["director", "delivery-watchdog"]
            .iter()
            .any(|source| prior_source.eq_ignore_ascii_case(source));
    if is(prior_source, counterparty) && is(prior_target, me) {
        Some(ReplyReference::DirectReply)
    } else if (is(prior_source, me) && (is(prior_target, counterparty) || broadcast))
        || (is(prior_source, counterparty) && broadcast)
        || (cas_generated && (is(prior_target, me) || broadcast))
    {
        Some(ReplyReference::Thread)
    } else {
        None
    }
}

/// GH #1196: why an operator turn cannot be delivered right now, or `None`
/// when it can or the state is unknown. Delivery needs a Commander client
/// connected to the session's daemon or an enrolled cloud operator feed.
///
/// The daemon records its client count in `loop.json` at the end of each loop
/// pass. The watchdog rewrites the file every 5 s even while the loop is
/// wedged, so the count is current only when the file is fresh
/// ([`STATUS_STALE_SECS`](crate::factory_daemon_health::STATUS_STALE_SECS))
/// and the last pass is recent
/// ([`LOOP_WEDGED_SECS`](crate::factory_daemon_health::LOOP_WEDGED_SECS)).
/// Anything else is unknown, and the turn queues so the daemon can deliver it
/// once it is back. An unreadable cloud binding is unknown too.
///
/// Known limits: a device that attached within the last loop pass (about
/// 5 s) can still be refused; retrying reaches it. A refused send records
/// nothing, so an `in_reply_to` it carried stays unacknowledged.
pub(crate) fn operator_turn_undeliverable(
    cas_root: &std::path::Path,
    factory_session: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    use crate::factory_daemon_health::{LOOP_WEDGED_SECS, STATUS_STALE_SECS};
    let status = crate::factory_daemon_health::read_status(cas_root, factory_session)?;
    let fresh = now.signed_duration_since(status.written_at).num_seconds() <= STATUS_STALE_SECS
        && now.signed_duration_since(status.last_pass_at).num_seconds() <= LOOP_WEDGED_SECS;
    if !fresh || status.commander_clients != Some(0) {
        return None;
    }
    let cloud_feed = cas_store::SqlitePromptQueueStore::open(cas_root)
        .and_then(|store| store.operator_feed_binding())
        .map(|binding| binding.is_some())
        // An unreadable binding is unknown, not absent.
        .unwrap_or(true);
    if cloud_feed {
        return None;
    }
    Some(format!(
        "No Commander device is attached to factory session {factory_session}, and this \
         project has no cloud operator feed, so nothing can deliver this turn. Nothing was \
         queued. Reach the operator in the terminal or another channel; once a Commander \
         device connects, operator sends are delivered normally."
    ))
}

#[cfg(test)]
mod viktor_provenance_tests {
    use super::queued_message_provenance_at;
    use cas_store::{NotificationPriority, QueuedPrompt};

    #[test]
    fn viktor_queue_rows_keep_the_standard_provenance_envelope() {
        let row = QueuedPrompt {
            id: 73,
            source: "viktor".to_string(),
            target: "worker-1".to_string(),
            prompt: "reply".to_string(),
            created_at: chrono::DateTime::parse_from_rfc3339("2026-08-18T20:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            processed_at: None,
            factory_session: Some("factory-1".to_string()),
            summary: Some("Viktor reply".to_string()),
            priority: NotificationPriority::High,
            acked_at: None,
            urgent: false,
            origin: None,
            operator: None,
            recipient_device_id: None,
            kind: None,
            attachments: Vec::new(),
        };

        let observed_at = chrono::DateTime::parse_from_rfc3339("2026-08-18T20:06:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(
            queued_message_provenance_at(&row, observed_at),
            "[cas #73 viktor 360s first]"
        );
    }

    #[test]
    fn provenance_marks_only_rows_past_the_operational_freshness_window() {
        let row = QueuedPrompt {
            id: 74,
            source: "director".to_string(),
            target: "worker-1".to_string(),
            prompt: "assignment".to_string(),
            created_at: chrono::DateTime::parse_from_rfc3339("2026-08-18T20:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            processed_at: None,
            factory_session: Some("factory-1".to_string()),
            summary: Some("Assigned task: cas-test".to_string()),
            priority: NotificationPriority::Normal,
            acked_at: None,
            urgent: false,
            origin: None,
            operator: None,
            recipient_device_id: None,
            kind: None,
            attachments: Vec::new(),
        };
        let observed_at = row.created_at + chrono::Duration::seconds(299);
        let provenance = queued_message_provenance_at(&row, observed_at);
        assert!(provenance.contains("299s"));
        assert!(provenance.contains("spawn-boilerplate"));
        assert!(provenance.contains("first"));
    }

    fn commander_row(origin: Option<cas_store::QueueOrigin>) -> QueuedPrompt {
        QueuedPrompt {
            id: 75,
            source: "commander:Daniel@iphone-15".to_string(),
            target: "supervisor".to_string(),
            prompt: "Status please".to_string(),
            created_at: chrono::DateTime::parse_from_rfc3339("2026-09-10T16:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            processed_at: None,
            factory_session: Some("factory-1".to_string()),
            summary: None,
            priority: NotificationPriority::High,
            acked_at: None,
            urgent: false,
            origin,
            operator: None,
            recipient_device_id: None,
            kind: None,
            attachments: Vec::new(),
        }
    }

    /// cas-7f81: a hub-stamped paired device renders as the operator class —
    /// the one header the skills treat as user authority.
    #[test]
    fn verified_commander_rows_render_the_operator_header() {
        let row = commander_row(Some(cas_store::QueueOrigin::PairedDevice {
            device_id: "dev-42".into(),
        }));
        let observed_at = row.created_at + chrono::Duration::seconds(4);
        assert_eq!(
            queued_message_provenance_at(&row, observed_at),
            "[cas #75 operator Daniel@iphone-15 verified 4s first]"
        );
        let mut replayed = row.clone();
        replayed.processed_at = Some(observed_at);
        assert!(queued_message_provenance_at(&replayed, observed_at).ends_with("4s replay]"));
    }

    /// cas-7f81: the `commander:` label alone never verifies. Unstamped,
    /// unattributed, and even registered-agent stamps all render unverified —
    /// only a paired-device stamp is the operator.
    #[test]
    fn commander_rows_without_a_device_stamp_render_unverified() {
        for origin in [
            None,
            Some(cas_store::QueueOrigin::Unattributed),
            Some(cas_store::QueueOrigin::Daemon),
            Some(cas_store::QueueOrigin::RegisteredAgent {
                agent_id: "agent-1".into(),
            }),
        ] {
            let row = commander_row(origin.clone());
            let observed_at = row.created_at + chrono::Duration::seconds(9);
            assert_eq!(
                queued_message_provenance_at(&row, observed_at),
                "[cas #75 unverified:Daniel@iphone-15 9s first]",
                "origin {origin:?} must not verify"
            );
        }
        let mut agent = commander_row(None);
        agent.source = "supervisor".into();
        assert_eq!(super::operator_class(&agent), None);
        assert!(queued_message_provenance_at(&agent, agent.created_at).contains("supervisor-authored"));
    }

    /// cas-e8df: once the daemon stamps the operator columns, the header
    /// names the credential record, not the `commander:` label — so a label
    /// that disagrees with the stamp cannot rename the operator.
    #[test]
    fn stamped_rows_take_operator_and_device_from_the_columns() {
        let mut row = commander_row(Some(cas_store::QueueOrigin::PairedDevice {
            device_id: "dev-42".into(),
        }));
        row.source = "commander:Mallory@spoofed".into();
        row.operator = Some(cas_store::OperatorStamp {
            operator: "Daniel".into(),
            device_id: "dev-42".into(),
            device_label: "iphone-15".into(),
            scopes: vec!["message:send".into()],
            verified: true,
        });
        assert_eq!(
            queued_message_provenance_at(&row, row.created_at),
            "[cas #75 operator Daniel@iphone-15 verified 0s first]"
        );
        // An unverified stamp never upgrades an unattributed row.
        let mut unverified = commander_row(Some(cas_store::QueueOrigin::Unattributed));
        unverified.operator = Some(cas_store::OperatorStamp {
            operator: "Daniel".into(),
            device_id: String::new(),
            device_label: "phone".into(),
            scopes: Vec::new(),
            verified: false,
        });
        assert_eq!(
            queued_message_provenance_at(&unverified, unverified.created_at),
            "[cas #75 unverified:Daniel@iphone-15 0s first]"
        );
        let mut agent = commander_row(None);
        agent.source = "supervisor".into();
        assert_eq!(super::operator_class(&agent), None);
        assert!(queued_message_provenance_at(&agent, agent.created_at).contains("supervisor-authored"));
    }
}

/// cas-99d2 (GH #127): what to do with one row an inbox poll selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InboxRedelivery {
    /// Never handed to a transport before — render it plainly.
    FirstDelivery,
    /// Already transport-delivered once; render it, marked as a repeat.
    MarkRedelivery,
    /// Already transport-delivered AND the action it solicited demonstrably
    /// happened — re-rendering it verbatim would read as a fresh instruction.
    WithholdConsumed,
}

/// cas-99d2 (GH #127): decide how an inbox poll should render a selected row.
///
/// Pure so the reported shape is testable without a store, a daemon or a clock.
///
/// `already_transport_delivered` is the daemon's own handoff stamp. Note that
/// this is deliberately NOT a reason to drop the row outright: the whole point
/// of the polling API is to surface rows the transport wrote to a file nobody
/// read. What it does establish is that this is not the recipient's first look
/// at the content, which is exactly what the recipient needs told.
///
/// `solicited_action_observed` is real consumption evidence: the state
/// transition the message asked for is already in the store, attributed to this
/// recipient. For an assignment that means the named task has been started by
/// the addressed worker. When a message has both — it was delivered, and what
/// it asked for happened — handing it back verbatim 15 minutes later is a
/// fabricated instruction, which is the reported bug (notification 7112).
pub(crate) fn inbox_redelivery_decision(
    already_transport_delivered: bool,
    solicited_action_observed: bool,
) -> InboxRedelivery {
    if !already_transport_delivered {
        return InboxRedelivery::FirstDelivery;
    }
    if solicited_action_observed {
        return InboxRedelivery::WithholdConsumed;
    }
    InboxRedelivery::MarkRedelivery
}

/// cas-098d (GH #904): how long after its transport handoff a row still reads
/// as the delivery a wake is pointing at. A wake follows the handoff within
/// seconds; a row handed over longer ago than this, still unread in the
/// durable inbox, is a replay of mail the recipient's harness already received.
pub(crate) const INBOX_REPLAY_AFTER_SECS: i64 = MESSAGE_PROVENANCE_STALE_AFTER_SECS;

/// cas-098d (GH #904): a transport-delivered row that an inbox poll lists by
/// id instead of replaying its body: it was handed over longer ago than the
/// replay window, and the recipient has demonstrably had it since (a turn
/// observed carrying it, or the recipient acted after the delivery).
///
/// Replaying those bodies is what buried the message a wake named under a
/// limit's worth of already-handled mail, and made hour-old merge requests,
/// reminders and acks read as live instructions. The row is still claimed and
/// named in the poll's output. A delivery with no such evidence may be one a
/// harness dropped (GH #155), so its body is still handed over in full.
pub(crate) fn inbox_row_is_stale_replay(
    transport_delivered_at: Option<chrono::DateTime<chrono::Utc>>,
    recipient_had_it: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    recipient_had_it
        && transport_delivered_at
            .is_some_and(|at| (now - at).num_seconds() > INBOX_REPLAY_AFTER_SECS)
}

/// The delivery evidence [`inbox_row_is_stale_replay`] asks for: a wake
/// observed carrying the row, or later recipient activity recorded against it.
fn recipient_had_delivery(report: Option<&cas_store::MessageDeliveryReport>) -> bool {
    report.is_some_and(|report| {
        report.wake == cas_store::ObservationStatus::Observed || report.assumed_seen_at.is_some()
    })
}

/// Inbox polling is a delivery surface, not an authority to discard a relay.
/// A failed task read is uncertainty, so only a freshly-read task whose
/// lifecycle occurrence is positively stale may be withheld.
/// Whether inbox_poll withholds `row` as a stale lifecycle relay.
///
/// cas-8cbe: a progress relay (`lifecycle:` source: started, ready again,
/// closed) asks nothing of the supervisor, and a compacted card names it as
/// the read-in-full escape. When the reader names it, it is shown whatever
/// the task did since. Waking relays keep the stale guard even when named.
pub(crate) fn withhold_lifecycle_at_inbox_pop(
    row: &cas_store::QueuedPrompt,
    named: bool,
    task: Option<&cas_types::Task>,
) -> bool {
    use crate::mcp::tools::core::task::lifecycle::supervisor_push::LIFECYCLE_SOURCE_PREFIX;
    if named && row.source.starts_with(LIFECYCLE_SOURCE_PREFIX) {
        return false;
    }
    lifecycle_relay_is_stale_at_inbox_pop(&row.prompt, task)
}

pub(crate) fn lifecycle_relay_is_stale_at_inbox_pop(
    prompt: &str,
    task: Option<&cas_types::Task>,
) -> bool {
    let Some(task) = task else {
        return false;
    };
    matches!(
        crate::prompt_revalidation::revalidate_lifecycle_prompt_against_task(prompt, task),
        crate::prompt_revalidation::LifecyclePromptDecision::SuppressStale { .. }
    )
}

/// cas-7a01 (GH #155): render the wake evidence pair as a sentence.
///
/// The two fields answer different questions and the failure the issue reported
/// lives precisely in their disagreement, so an operator must not have to
/// derive it. `wake_attempt` is what Cassy did; `wake` is whether a turn
/// demonstrably carried the content. `nudge_fired` + `unobserved` is the
/// GH #155 signature and is called out by name — that combination used to be
/// completely invisible.
pub(crate) fn wake_attempt_narrative(
    attempt: cas_store::WakeAttempt,
    wake: cas_store::ObservationStatus,
) -> String {
    use cas_store::{ObservationStatus, WakeAttempt};
    let line = match (attempt, wake) {
        (_, ObservationStatus::Observed) => {
            "wake evidence: this message was injected into a turn on the recipient's side \
             (hook surfacing receipt)."
        }
        (WakeAttempt::Fired, ObservationStatus::Unobserved) => {
            "wake evidence: Cassy DID nudge this recipient's pane, and no turn is recorded as \
             having carried this message — the nudge landed but the harness surfaced nothing \
             (GH #155 signature)."
        }
        (WakeAttempt::Failed, ObservationStatus::Unobserved) => {
            "wake evidence: Cassy ATTEMPTED a wake and it FAILED (see wake_attempt_detail); no \
             recorded turn carried this message, so treat it as NOT delivered to a turn."
        }
        (WakeAttempt::NotAttempted, ObservationStatus::Unobserved) => {
            "wake evidence: Cassy never attempted a wake for this message — the idle gate \
             declined it, or the recipient is on PTY, where the typed payload is the wake. \
             A PTY write is not a turn: if none starts, the delivery watchdog records its \
             nudge or failure here (GH #1153)."
        }
    };
    format!("{line}\n")
}

/// cas-badb (GH #1302): the sender's one-line answer to "did it arrive?".
/// Received means the recipient's own session saw this message: a turn
/// observed carrying it, its own hook rendering it into its turn, its own
/// inbox_poll, or its explicit acknowledgement. A pane handoff or transport
/// claim alone is still pending.
pub(crate) fn recipient_verdict(
    report: &cas_store::MessageDeliveryReport,
    now: chrono::DateTime<chrono::Utc>,
    escalate_after_secs: u64,
) -> String {
    use cas_store::{DeliveryStage, SurfacingSource};
    let how = if report.wake == cas_store::ObservationStatus::Observed {
        Some("its turn started with this message")
    } else {
        match report.recipient_receipt {
            Some(SurfacingSource::ObservedWake) => Some("its turn started with this message"),
            Some(SurfacingSource::HookSurfaced) => Some("this message was shown in its own turn"),
            Some(SurfacingSource::InboxPoll) if report.recipient_poll_settled.is_none() => {
                Some("it read this message with inbox_poll")
            }
            _ => (report.confirmation_source == cas_store::ConfirmationSource::ExplicitAck)
                .then_some("it acknowledged this message"),
        }
    };
    if let Some(how) = how {
        let at = report
            .wake_observed_at
            .or(report.confirmed_at)
            .map(|at| format!(" at {}", at.to_rfc3339()))
            .unwrap_or_default();
        return format!("recipient: RECEIVED by {}: {how}{at}.\n", report.target);
    }
    let waited = (now - report.enqueued_at).num_seconds().max(0);
    if matches!(
        report.stage,
        DeliveryStage::Dropped | DeliveryStage::Suppressed | DeliveryStage::Abandoned
    ) {
        return format!(
            "recipient: NOT RECEIVED by {}: delivery ended ({}) after {waited}s; resend or reach it another way.\n",
            report.target, report.stage
        );
    }
    let escalation = if (waited as u64) < escalate_after_secs {
        format!("you and the operator are told at {escalate_after_secs}s")
    } else {
        "you and the operator have been told".to_string()
    };
    format!(
        "recipient: PENDING for {}: no turn has started with this message after {waited}s          (stage {}, last wake {}, declined wakes {}). Cassy keeps re-waking it every 60s; {escalation}.\n",
        report.target, report.stage, report.wake_attempt, report.wake_gate_declines
    )
}

/// cas-ad92: say whether the body reached the recipient's turn or was only
/// claimed by the daemon's transport. A Claude teams handoff is claimed when
/// the inbox file is written, but Claude Code renders it only at a turn
/// boundary, so "claimed" is not "rendered".
pub(crate) fn recipient_receipt_narrative(
    receipt: Option<cas_store::SurfacingSource>,
    poll_settled: Option<&str>,
) -> String {
    use cas_store::SurfacingSource;
    // cas-6ccb: an inbox_poll that withheld the row, did not replay it, or
    // showed it as a redelivery settled it; the recipient confirmed nothing.
    if let (Some(SurfacingSource::InboxPoll), Some(reason)) = (receipt, poll_settled) {
        return format!(
            "recipient_receipt: {reason} (poll_settled; not an acknowledgement, and not repeated \
             at the next turn).\n"
        );
    }
    let line = match receipt {
        None => return String::new(),
        Some(SurfacingSource::HookSurfaced) => {
            "recipient_receipt: rendered — the body was injected into the recipient's turn \
             (hook_surfaced)."
        }
        Some(SurfacingSource::InboxPoll) => {
            "recipient_receipt: rendered — the recipient read the body with inbox_poll."
        }
        Some(SurfacingSource::ObservedWake) => {
            "recipient_receipt: rendered — Cassy observed the recipient take a turn carrying \
             the body (observed_wake)."
        }
        Some(SurfacingSource::TransportClaimed) => {
            "recipient_receipt: claimed, not rendered — the transport handed the body to the \
             recipient's harness, which has not shown it in a turn yet (transport_claimed)."
        }
        Some(SurfacingSource::TransportDelivered) => {
            "recipient_receipt: handed off, not rendered — still claimable by the recipient's \
             inbox_poll (transport_delivered)."
        }
    };
    format!("{line}\n")
}

/// cas-ac7e (GH #130): the operator-facing warning for a row that claims
/// `stage=delivered` with no recipient-side transport stamp.
///
/// A free function, not an inline condition, because this warning IS the fix's
/// visible surface: it is how an operator tells notification 7183's shape
/// (delivered per the writer, unrecorded per the recipient) from a genuinely
/// corroborated delivery. Three independent clauses have to be right and an
/// inline expression let all three be wrong silently.
///
/// `all_workers` is exempt: a broadcast's per-recipient transport is its
/// broadcast counts, so it legitimately has no single-recipient stamp.
pub(crate) fn recipient_transport_warning(
    stage: cas_store::DeliveryStage,
    target: &str,
    has_recipient_transport: bool,
) -> Option<&'static str> {
    if stage != cas_store::DeliveryStage::Delivered
        || target == "all_workers"
        || has_recipient_transport
    {
        return None;
    }
    Some(
        "recipient_transport: MISSING — this row reports stage=delivered with no \
         per-recipient transport stamp. Either it was delivered before Cassy \
         recorded them, or the stamp and the stage have diverged; treat the \
         delivery as unproven and re-send.\n",
    )
}

const CLAUDE_INTERRUPT_CONFIRM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
const CLAUDE_INTERRUPT_CONFIRM_POLL: std::time::Duration = std::time::Duration::from_millis(100);

fn interrupt_delivery_is_observed(report: &cas_store::MessageDeliveryReport) -> bool {
    matches!(
        report.stage,
        cas_store::DeliveryStage::Delivered | cas_store::DeliveryStage::Confirmed
    ) && report.delivered_at.is_some()
        && report.recipient_transport_at.is_some()
}

fn interrupt_delivery_failure(report: &cas_store::MessageDeliveryReport) -> Option<String> {
    report.stage.is_terminal_non_delivery().then(|| {
        format!(
            "delivery reached terminal stage {}{}",
            report.stage,
            report
                .pending_detail
                .as_deref()
                .map(|detail| format!(": {detail}"))
                .unwrap_or_default()
        )
    })
}

fn interrupt_unconfirmed_message(
    notification_id: i64,
    report: Option<&cas_store::MessageDeliveryReport>,
) -> String {
    let state = report.map_or_else(
        || "no delivery report was readable".to_string(),
        |report| {
            format!(
                "stage={}, wake_attempt={}, recipient_transport={}",
                report.stage,
                report.wake_attempt,
                if report.recipient_transport_at.is_some() {
                    "observed"
                } else {
                    "unobserved"
                }
            )
        },
    );
    format!(
        "Could not confirm Claude interrupt delivery for notification {notification_id} within {}s ({state}). The queue row remains durable for retry, but this call did not silently claim the teammate was interrupted; inspect `coordination action=message_status notification_id={notification_id}` before proceeding.",
        CLAUDE_INTERRUPT_CONFIRM_TIMEOUT.as_secs()
    )
}

async fn wait_for_claude_interrupt_delivery(
    queue: &dyn cas_store::PromptQueueStore,
    notification_id: i64,
) -> std::result::Result<(), String> {
    let deadline = tokio::time::Instant::now() + CLAUDE_INTERRUPT_CONFIRM_TIMEOUT;
    let mut last_report = None;
    loop {
        match queue.message_delivery_report(notification_id) {
            Ok(Some(report)) => {
                if interrupt_delivery_is_observed(&report) {
                    return Ok(());
                }
                if let Some(failure) = interrupt_delivery_failure(&report) {
                    return Err(format!(
                        "Could not interrupt Claude teammate for notification {notification_id}: {failure}"
                    ));
                }
                last_report = Some(report);
            }
            Ok(None) => {}
            Err(error) => {
                return Err(format!(
                    "Could not verify Claude interrupt notification {notification_id}: {error}"
                ));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(interrupt_unconfirmed_message(
                notification_id,
                last_report.as_ref(),
            ));
        }
        tokio::time::sleep(CLAUDE_INTERRUPT_CONFIRM_POLL).await;
    }
}

impl CasService {
    /// cas-207e: the deadline of a Commander ask is a time reminder owned by
    /// the supervisor; `fire_reminder` delivers it as a blocker-class wake.
    fn schedule_ask_deadline(
        &self,
        ask_id: i64,
        secs: i64,
        default: &str,
        factory_session: &str,
    ) -> Result<i64, McpError> {
        let internal = |what: &str, error: String| {
            Self::error(ErrorCode::INTERNAL_ERROR, format!("{what}: {error}"))
        };
        let owner_id = self
            .inner
            .get_agent_id()
            .map_err(|e| internal("Failed to get agent ID", e.to_string()))?;
        let origin_session = std::env::var("CAS_SESSION_ID")
            .ok()
            .filter(|session| !session.trim().is_empty())
            .unwrap_or_else(|| owner_id.clone());
        crate::store::open_reminder_store(&self.inner.cas_root)
            .map_err(|e| internal("Failed to open reminder store", e.to_string()))?
            .create_with_scope(
                &owner_id,
                None,
                &format!(
                    "Operator ask #{ask_id} reached its deadline unanswered. Proceed now with the default: {default}. Then report what you did to the operator."
                ),
                cas_store::ReminderTriggerType::Time,
                Some(chrono::Utc::now() + chrono::Duration::seconds(secs)),
                None,
                Some(&serde_json::json!({ "operator_ask": ask_id })),
                0,
                Some(factory_session),
                Some(&origin_session),
                false,
                None,
            )
            .map_err(|e| internal("Failed to schedule the ask deadline", e.to_string()))
    }

    pub(in crate::mcp::tools::service) async fn message_send(
        &self,
        req: AgentRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::open_prompt_queue_store;

        let mut req = req;

        // cas-6853: broadcast audience policy applies to all_workers only.
        let broadcast_supersede_key = req
            .supersede_key
            .take()
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty());
        let broadcast_sticky = req.sticky.take().unwrap_or(false);
        if (broadcast_supersede_key.is_some() || broadcast_sticky)
            && !req
                .target
                .as_deref()
                .is_some_and(|target| target.trim().eq_ignore_ascii_case("all_workers"))
        {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "supersede_key and sticky apply only to target='all_workers' broadcasts",
            ));
        }

        let target = req.target.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "target required (agent name, 'supervisor', or 'all_workers')",
            )
        })?;
        let mut message = req.message.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                format!(
                    "message required — full message body goes in `message`. \
                 Example: {}coordination action=message target=supervisor \
                 summary=\"task blocked\" message=\"cas-abc1 needs ...\"",
                    self.inner.guidance_prefix()
                ),
            )
        })?;
        message = crate::prompt_revalidation::strip_assignment_envelopes(&message).to_owned();
        let summary = req
            .summary
            .map(|summary| collapse_message_summary(&summary))
            .filter(|summary| !summary.is_empty())
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "summary required — a short one-line preview shown in the UI. \
                 Example: summary=\"task blocked on verification\" (required alongside `message`).",
                )
            })?;

        let commander_target = target.starts_with(COMMANDER_SOURCE_PREFIX);
        let commander_selector = target
            .strip_prefix(COMMANDER_SOURCE_PREFIX)
            .map(str::trim)
            .filter(|selector| !selector.is_empty())
            .map(str::to_owned);
        if commander_target && commander_selector.is_none() {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "target='commander:<label>' requires a non-empty Commander label or device",
            ));
        }

        let source = self
            .inner
            .get_agent_id()
            .unwrap_or_else(|_| "unknown".to_string());
        let config = self.inner.load_config();
        // cas-6ee6 (GH #1006 item 1): an over-cap body is refused below unless
        // a supervisor sends it to a factory agent, in which case it spills
        // to an artifact. Identity resolution in between is read-only.
        let over_cap_refusal = crate::mcp::tools::traffic_limits::validate_message_body(
            &source,
            &message,
            &config,
            req.blocker.unwrap_or(false),
            req.merge_request.unwrap_or(false),
            req.task_id.as_deref().unwrap_or("<task-id>"),
        )
        .err();
        // When agent ID lookup fails but CAS_AGENT_NAME is set (factory mode),
        // resolve display_name from the env var so messages show the correct sender.
        let env_agent_name = std::env::var("CAS_AGENT_NAME").ok();
        let agent_from_store = {
            use crate::store::open_agent_store;
            open_agent_store(&self.inner.cas_root).ok().and_then(|store| {
                store.get(&source).ok().or_else(|| {
                    let name = env_agent_name.as_deref()?.trim();
                    if name.is_empty() {
                        return None;
                    }
                    store
                        .list(None)
                        .ok()?
                        .into_iter()
                        .find(|agent| agent.name.eq_ignore_ascii_case(name))
                })
            })
        };
        // The registered row is the explicit identity for this MCP caller.
        // CAS_AGENT_ROLE is only a bootstrap fallback when no row can be
        // resolved; letting it win here can relabel a registered worker as a
        // supervisor when a supervisor-launched test or server shares a
        // process environment.
        let role = agent_from_store
            .as_ref()
            .map(|a| a.role.to_string())
            .or_else(|| std::env::var("CAS_AGENT_ROLE").ok())
            .unwrap_or_else(|| "primary".to_string());
        // Persist this message's classification before folding adds labels
        // and other bodies that no longer look like a pure acknowledgement.
        let quiet_message = req.wake == Some(false)
            || req.kind.as_deref() == Some("receipt")
            || crate::supervisor_message_wakes::pure_ack(&message);
        if quiet_message && role != "worker" {
            message = format!("{}\n{message}", crate::supervisor_message_wakes::QUIET);
        }
        // Resolve cross-machine peers before applying the local inbox cap.
        // Their Cloud body must be readable on the receiving machine: spilling
        // first sent a path on the sender's disk instead of the full text
        // (communication contract P3.6, cas-5c2c). Local messages retain their
        // existing cap/spill policy; peer urgency and identity checks still run.
        if role == "supervisor"
            && let Some(sender) = agent_from_store.as_ref()
            && let Some(sent) = self
                .peer_message_send(
                    sender,
                    &target,
                    &summary,
                    &message,
                    req.in_reply_to,
                    req.urgent.unwrap_or(false),
                    req.kind.as_deref(),
                    req.blocker.unwrap_or(false),
                    req.attachment.as_deref(),
                )
                .await?
        {
            return Ok(sent);
        }

        if let Some(refusal) = over_cap_refusal {
            let to_operator = target.eq_ignore_ascii_case("operator") || commander_target;
            if role != "supervisor" || to_operator {
                return Err(Self::error(ErrorCode::INVALID_PARAMS, refusal));
            }
            let sender = agent_from_store
                .as_ref()
                .map(|agent| agent.name.clone())
                .unwrap_or_else(|| source.clone());
            message = crate::mcp::tools::traffic_limits::spill_message_to_artifact(
                crate::mcp::tools::traffic_limits::message_spill_root(&self.inner.cas_root, &config).as_deref(),
                req.task_id.as_deref(),
                &sender,
                &target,
                &summary,
                &message,
                crate::mcp::tools::traffic_limits::message_body_limit(
                    &config,
                    req.blocker.unwrap_or(false),
                    req.merge_request.unwrap_or(false),
                ),
                chrono::Utc::now(),
            )
            .map_err(|refusal| Self::error(ErrorCode::INVALID_PARAMS, refusal))?;
        }
        // Wake envelopes are message types, not content. Workers can request
        // a typed status/ready, blocker, or merge action through the fields
        // below; literal tags in their message body must not impersonate the
        // server's suffix when the supervisor wake gate parses the row.
        if role == "worker" {
            message = crate::prompt_revalidation::
                neutralize_worker_authored_envelope_tags(&message);
            // Add CAS's quiet marker only after neutralizing caller tags,
            // so it remains a type marker rather than escaped body text.
            if quiet_message {
                message = format!("{}\n{message}", crate::supervisor_message_wakes::QUIET);
            }
        }
        // A shared clone can have two live supervisors and a worker process
        // whose ambient environment was inherited from the wrong harness.
        // The registered caller row is the authoritative owner; the
        // environment remains a compatibility fallback for legacy callers.
        let factory_session = agent_from_store
            .as_ref()
            .and_then(|agent| agent.factory_session.clone())
            .filter(|session| !session.trim().is_empty())
            .or_else(|| {
                std::env::var("CAS_FACTORY_SESSION")
                    .ok()
                    .filter(|session| !session.trim().is_empty())
            });

        let resolve_supervisor_name = || -> Option<String> {
            use crate::store::open_agent_store;
            use cas_types::{AgentRole, AgentStatus};
            let store = open_agent_store(&self.inner.cas_root).ok();
            if role == "worker"
                && let (Some(store), Some(session)) = (store.as_ref(), factory_session.as_deref())
            {
                // Do not let CAS_SUPERVISOR_NAME or roster order override a
                // worker's registered factory owner.
                return resolve_worker_supervisor_target(store.as_ref(), Some(session))
                    .map(|supervisor| supervisor.name);
            }
            if let Ok(name) = std::env::var("CAS_SUPERVISOR_NAME") {
                if !name.trim().is_empty() {
                    return Some(name);
                }
            }
            store?.list(None).ok().and_then(|agents| {
                agents
                    .into_iter()
                    .find(|a| {
                        a.role == AgentRole::Supervisor
                            && (a.status == AgentStatus::Active || a.status == AgentStatus::Idle)
                    })
                    .map(|a| a.name)
            })
        };

        let addressed_logical_supervisor = target.eq_ignore_ascii_case("supervisor");
        let addressed_operator = target.eq_ignore_ascii_case("operator") || commander_target;
        if addressed_operator && role != "supervisor" {
            return Err(Self::error(
                ErrorCode::INVALID_REQUEST,
                "Only a registered supervisor may message target='operator'",
            ));
        }
        let mut peer_supervisor_copy = None;
        let resolved_target = if addressed_operator {
            "operator".to_string()
        } else if role == "worker" {
            if target.eq_ignore_ascii_case("supervisor") {
                resolve_supervisor_name().ok_or_else(|| {
                    // GH #1171: say which supervisors are live, so the worker
                    // can address its own by name instead of parking.
                    let live: Vec<String> = crate::store::open_agent_store(&self.inner.cas_root)
                        .ok()
                        .and_then(|store| store.list(None).ok())
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|agent| {
                            agent.role == cas_types::AgentRole::Supervisor
                                && matches!(
                                    agent.status,
                                    cas_types::AgentStatus::Active | cas_types::AgentStatus::Idle
                                )
                        })
                        .map(|agent| match agent.factory_session.as_deref() {
                            Some(session) if !session.trim().is_empty() => {
                                format!("{} (session {session})", agent.name)
                            }
                            _ => format!("{} (no session)", agent.name),
                        })
                        .collect();
                    let session = factory_session.as_deref().unwrap_or("none");
                    Self::error(
                        ErrorCode::INVALID_REQUEST,
                        if live.is_empty() {
                            format!(
                                "Cannot resolve 'supervisor' for factory session {session}: no supervisor is registered on this clone."
                            )
                        } else {
                            format!(
                                "Cannot resolve 'supervisor' for factory session {session}: none of the live supervisors on this clone belongs to it ({}). Address your supervisor by name.",
                                live.join(", ")
                            )
                        },
                    )
                })?
            } else if target.eq_ignore_ascii_case("all_workers") {
                return Err(Self::error(
                    ErrorCode::INVALID_REQUEST,
                    "Workers cannot broadcast to all_workers",
                ));
            } else {
                let supervisor_name = resolve_supervisor_name();
                // An explicit supervisor name is a valid same-clone recipient
                // whenever that supervisor is live. This is distinct from
                // the logical `supervisor` target, which resolves to the
                // worker's own factory session above.
                let named_supervisor_is_registered = crate::store::open_agent_store(
                    &self.inner.cas_root,
                )
                        .ok()
                        .and_then(|store| store.list(None).ok())
                        .is_some_and(|agents| {
                            agents.iter().any(|agent| {
                                agent.role == cas_types::AgentRole::Supervisor
                                    && (agent.status == cas_types::AgentStatus::Active
                                        || agent.status == cas_types::AgentStatus::Idle)
                                    && agent.name.eq_ignore_ascii_case(&target)
                            })
                        });
                if named_supervisor_is_registered {
                    target
                } else {
                    // cas-5068 / GH #335: peer contact is a narrow,
                    // supervisor-visible collision-warning lane, not general
                    // worker chat. Environment names alone must not grant
                    // cross-session access; both sides are typed store rows.
                    use crate::store::open_agent_store;
                    use cas_types::AgentRole;

                    let session = factory_session.as_deref().ok_or_else(|| {
                        Self::error(
                            ErrorCode::INVALID_REQUEST,
                            "Workers can only message their supervisor or a same-session registered peer; this caller has no named factory session. Use target='supervisor' instead",
                        )
                    })?;
                    let source_agent = agent_from_store.as_ref().filter(|agent| {
                        agent.role == AgentRole::Worker
                            && agent.factory_session.as_deref() == Some(session)
                    }).ok_or_else(|| {
                        Self::error(
                            ErrorCode::INVALID_REQUEST,
                            "Workers can only message their supervisor or a same-session registered peer; this caller is not a registered worker in the current factory session. Use target='supervisor' instead",
                        )
                    })?;
                    let agent_store = open_agent_store(&self.inner.cas_root).map_err(|error| {
                        Self::error(
                            ErrorCode::INTERNAL_ERROR,
                            format!("Failed to resolve peer message scope: {error}"),
                        )
                    })?;
                    let agents = agent_store.list(None).map_err(|error| {
                        Self::error(
                            ErrorCode::INTERNAL_ERROR,
                            format!("Failed to resolve peer message scope: {error}"),
                        )
                    })?;
                    let peer = agents.iter().find(|agent| {
                        agent.role == AgentRole::Worker
                            && agent.id != source_agent.id
                            && agent.name.eq_ignore_ascii_case(&target)
                            && agent.factory_session.as_deref() == Some(session)
                    }).ok_or_else(|| {
                        Self::error(
                            ErrorCode::INVALID_REQUEST,
                            format!(
                                "Workers can only message their supervisor or another registered worker in factory session '{session}'. Use target='supervisor' for '{}'",
                                supervisor_name.unwrap_or_else(|| "<supervisor>".to_string())
                            ),
                        )
                    })?;
                    let supervisor = agents.iter().find(|agent| {
                        agent.role == AgentRole::Supervisor
                            && agent.factory_session.as_deref() == Some(session)
                    }).ok_or_else(|| {
                        Self::error(
                            ErrorCode::INVALID_REQUEST,
                            "Peer messages require a registered supervisor in the same factory session; use target='supervisor' instead",
                        )
                    })?;
                    peer_supervisor_copy = Some(supervisor.name.clone());
                    peer.name.clone()
                }
            }
        } else {
            target
        };

        if role != "worker" && (resolved_target == "owner" || resolved_target.starts_with("inbox:"))
        {
            use crate::store::{
                NotificationPriority, open_agent_store, open_supervisor_queue_store,
            };
            use cas_types::AgentRole;
            use rusqlite::Connection;
            use std::collections::HashSet;

            let display_name = open_agent_store(&self.inner.cas_root)
                .ok()
                .and_then(|store| store.get(&source).ok())
                .map(|agent| {
                    if agent.role == AgentRole::Supervisor {
                        "supervisor".to_string()
                    } else {
                        agent.name
                    }
                })
                .unwrap_or_else(|| source.clone());

            let inbox_id = if resolved_target == "owner" {
                "owner".to_string()
            } else {
                resolved_target
                    .strip_prefix("inbox:")
                    .unwrap_or("owner")
                    .to_string()
            };

            let engaged = (|| -> std::result::Result<bool, rusqlite::Error> {
                let agent_name = std::env::var("CAS_AGENT_NAME").unwrap_or_default();
                if agent_name.is_empty() {
                    return Ok(false);
                }

                let manager = crate::ui::factory::SessionManager::new();
                let sessions = manager
                    .list_sessions()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;

                let session = sessions
                    .into_iter()
                    .find(|session| session.metadata.supervisor.name == agent_name);

                let Some(session) = session else {
                    return Ok(false);
                };

                let mut targets: HashSet<String> = HashSet::new();
                targets.insert(session.metadata.supervisor.name.clone());
                targets.insert("all_workers".to_string());
                for worker in &session.metadata.workers {
                    targets.insert(worker.name.clone());
                }

                if targets.is_empty() {
                    return Ok(false);
                }

                let db_path = self.inner.cas_root.join("cas.db");
                let conn = Connection::open(&db_path)?;

                let mut target_vec: Vec<String> = targets.into_iter().collect();
                target_vec.sort();
                let placeholders = std::iter::repeat_n("?", target_vec.len())
                    .collect::<Vec<_>>()
                    .join(", ");

                let sql = format!(
                    "SELECT 1 FROM prompt_queue WHERE source = ? AND target IN ({placeholders}) LIMIT 1"
                );
                let mut stmt = conn.prepare(&sql)?;

                let mut params: Vec<Box<dyn rusqlite::ToSql>> =
                    Vec::with_capacity(1 + target_vec.len());
                params.push(Box::new("openclaw".to_string()));
                for target in target_vec {
                    params.push(Box::new(target));
                }

                let mut rows = stmt.query(rusqlite::params_from_iter(
                    params.iter().map(|param| param.as_ref()),
                ))?;
                Ok(rows.next()?.is_some())
            })()
            .unwrap_or(false);

            if !engaged {
                return Err(Self::error(
                    ErrorCode::INVALID_REQUEST,
                    "External inbox is not engaged for this session yet. Owner must message this factory session first (via OpenClaw) before agents can reply to 'owner'.",
                ));
            }

            let queue = open_supervisor_queue_store(&self.inner.cas_root).map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to open supervisor queue: {error}"),
                )
            })?;

            let payload = serde_json::json!({
                "schema_version": 1,
                "type": "message",
                "from": display_name,
                "message": message,
            })
            .to_string();

            let notification_id = queue
                .notify(&inbox_id, "message", &payload, NotificationPriority::Normal)
                .map_err(|error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to queue external message: {error}"),
                    )
                })?;

            return Ok(Self::success(format!(
                "External message queued\n\nID: {}\nInbox: {}\nFrom: {} ({})\nMessage: {}",
                notification_id,
                inbox_id,
                display_name,
                role,
                truncate_str(&message, 100)
            )));
        }

        let queue = open_prompt_queue_store(&self.inner.cas_root).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open message queue: {error}"),
            )
        })?;

        let display_name = {
            use crate::store::open_agent_store;
            use cas_types::AgentRole;
            let agent_store = open_agent_store(&self.inner.cas_root).ok();
            agent_store
                .and_then(|store| store.get(&source).ok())
                .map(|agent| {
                    if agent.role == AgentRole::Supervisor {
                        "supervisor".to_string()
                    } else {
                        agent.name
                    }
                })
                .or_else(|| env_agent_name.clone())
                .unwrap_or_else(|| source.clone())
        };

        // A commander:<label> target is a convenience alias for the verified
        // operator lane. Resolve the newest matching Commander row while it
        // is still available even if inbox polling already marked it seen.
        // The reply then follows the row's paired device, never the client
        // supplied label.
        if let Some(selector) = commander_selector.as_deref() {
            let session = factory_session
                .as_deref()
                .filter(|session| !session.trim().is_empty())
                .ok_or_else(|| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!(
                            "target='commander:{selector}' requires an active factory session; \
                             once a verified Commander message exists, call `coordination \
                             action=message target=operator in_reply_to=<notification_id> \
                             summary=\"...\" message=\"...\"`"
                        ),
                    )
                })?;
            let mut supervisor_targets = vec!["supervisor", display_name.as_str()];
            if let Some(name) = env_agent_name.as_deref()
                && !supervisor_targets
                    .iter()
                    .any(|target| target.eq_ignore_ascii_case(name))
            {
                supervisor_targets.push(name);
            }
            if let Some(agent) = agent_from_store.as_ref()
                && !supervisor_targets
                    .iter()
                    .any(|target| target.eq_ignore_ascii_case(&agent.name))
            {
                supervisor_targets.push(agent.name.as_str());
            }
            let prior = queue
                .latest_verified_operator_message(session, selector, &supervisor_targets)
                .map_err(|error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!(
                            "Failed to resolve verified Commander message for \
                             target='commander:{selector}': {error}"
                        ),
                    )
                })?
                .ok_or_else(|| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!(
                            "No verified Commander message from '{selector}' is addressed to \
                             this supervisor. Reply with `coordination action=message \
                             target=operator in_reply_to=<notification_id> summary=\"...\" \
                             message=\"...\"` after a verified Commander row arrives."
                        ),
                    )
                })?;
            if let Some(explicit) = req.in_reply_to
                && explicit != prior.id
            {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "target='commander:{selector}' resolves to notification_id={} \
                         (not in_reply_to={explicit}); use `coordination action=message \
                         target=operator in_reply_to={} summary=\"...\" message=\"...\"`",
                        prior.id, prior.id
                    ),
                ));
            }
            req.in_reply_to = Some(prior.id);
        }

        // Commander turns are a separate recipient lane. They remain
        // supervisor-only; answer turns must point at a live, hub-stamped
        // operator row, while status/receipt/ask/blocker turns may be sent
        // without a preceding operator prompt.
        if addressed_operator {
            if role != "supervisor" {
                return Err(Self::error(
                    ErrorCode::INVALID_REQUEST,
                    "Only a registered supervisor may message target='operator'",
                ));
            }
            let kind = match req.kind.as_deref().unwrap_or("answer") {
                "answer" => crate::ui::factory::OperatorTurnKind::Answer,
                "status" => crate::ui::factory::OperatorTurnKind::Status,
                "receipt" => crate::ui::factory::OperatorTurnKind::Receipt,
                "ask" => crate::ui::factory::OperatorTurnKind::Ask,
                "blocker" => crate::ui::factory::OperatorTurnKind::Blocker,
                other => {
                    return Err(Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!(
                            "invalid operator kind '{other}'; expected answer, status, receipt, ask, or blocker"
                        ),
                    ));
                }
            };
            // cas-207e: an ask may carry a deadline (remind_delay_secs) and the
            // default action it proceeds with (remind_message). At the
            // deadline a blocker-class wake tells the supervisor to proceed.
            let ask_deadline = match (
                req.remind_delay_secs,
                req.remind_message.as_deref().map(str::trim),
            ) {
                (None, None) => None,
                (Some(secs), Some(default))
                    if kind == crate::ui::factory::OperatorTurnKind::Ask
                        && secs > 0
                        && !default.is_empty() =>
                {
                    Some((secs, default.to_string()))
                }
                _ => {
                    return Err(Self::error(
                        ErrorCode::INVALID_PARAMS,
                        "a Commander deadline needs kind=ask, remind_delay_secs > 0 (the deadline) and remind_message (the default action)",
                    ));
                }
            };
            let prior = if kind == crate::ui::factory::OperatorTurnKind::Answer {
                let notification_id = req.in_reply_to.ok_or_else(|| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        "target='operator' kind=answer requires in_reply_to=<Commander notification_id>",
                    )
                })?;
                Some(
                    queue
                        .queued_prompt(notification_id)
                        .map_err(|error| {
                            Self::error(
                                ErrorCode::INTERNAL_ERROR,
                                format!(
                                    "Failed to inspect Commander notification {notification_id}: {error}"
                                ),
                            )
                        })?
                        .ok_or_else(|| {
                            Self::error(
                                ErrorCode::INVALID_PARAMS,
                                format!(
                                    "in_reply_to notification {notification_id} does not exist"
                                ),
                            )
                        })?,
                )
            } else {
                None
            };
            if let Some(prior) = prior.as_ref() {
                let notification_id = req.in_reply_to.expect("answer kind requires reply id");
                if !prior.source.starts_with(COMMANDER_SOURCE_PREFIX) {
                    return Err(Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!(
                            "in_reply_to notification {notification_id} is not a Commander operator message"
                        ),
                    ));
                }
                let expected_targets = [
                    "supervisor",
                    env_agent_name.as_deref().unwrap_or_default(),
                    agent_from_store
                        .as_ref()
                        .map(|agent| agent.name.as_str())
                        .unwrap_or_default(),
                ];
                if !expected_targets
                    .iter()
                    .filter(|target| !target.is_empty())
                    .any(|target| prior.target.eq_ignore_ascii_case(target))
                {
                    return Err(Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!(
                            "in_reply_to notification {notification_id} targets {}, not this supervisor",
                            prior.target
                        ),
                    ));
                }
                if prior
                    .origin
                    .as_ref()
                    .and_then(cas_store::QueueOrigin::verified_device_id)
                    .filter(|device_id| !device_id.trim().is_empty())
                    .is_none()
                {
                    return Err(Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!(
                            "in_reply_to notification {notification_id} has no verified paired-device origin"
                        ),
                    ));
                }
            }
            // cas-e829: a turn belongs to the session it is sent from. The
            // answered row may come from another session, including one that
            // has ended; `in_reply_to` then only refers to it. Inheriting its
            // session would write into a thread nobody is viewing, or queue
            // to a dead session that can never deliver.
            let factory_session = factory_session
                .as_deref()
                .filter(|session| !session.trim().is_empty())
                .ok_or_else(|| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        "target='operator' requires an active factory session",
                    )
                })?;
            let reply_to_session = prior
                .as_ref()
                .and_then(|prior| prior.factory_session.as_deref())
                .map(str::trim)
                .filter(|session| !session.is_empty() && *session != factory_session)
                .map(str::to_owned);
            let device_id = if let Some(prior) = prior.as_ref() {
                prior
                    .origin
                    .as_ref()
                    .and_then(cas_store::QueueOrigin::verified_device_id)
                    .map(str::to_owned)
            } else {
                queue
                    .latest_verified_operator_device(factory_session)
                    .map_err(|error| {
                        Self::error(
                            ErrorCode::INTERNAL_ERROR,
                            format!("Failed to resolve the latest Commander device: {error}"),
                        )
                    })?
            };
            let operator_label = prior.as_ref().and_then(|prior| {
                prior
                    .source
                    .strip_prefix(COMMANDER_SOURCE_PREFIX)
                    .and_then(|label| label.rsplit_once('@').map(|(operator, _)| operator.trim()))
                    .filter(|operator| !operator.is_empty())
                    .map(str::to_owned)
            });
            let attachments = if let Some(artifact_id) = req.attachment.as_deref() {
                let artifact_id = artifact_id.trim();
                if artifact_id.is_empty() {
                    return Err(Self::error(
                        ErrorCode::INVALID_PARAMS,
                        "attachment must be a non-empty published artifact id",
                    ));
                }
                let store = cas_store::SqliteArtifactStore::open(&self.inner.cas_root)
                    .map_err(|error| {
                        Self::error(
                            ErrorCode::INTERNAL_ERROR,
                            format!("Failed to open the artifact store: {error}"),
                        )
                    })?;
                let stored = store.get(artifact_id).map_err(|error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to inspect published artifact '{artifact_id}': {error}"),
                    )
                })?.ok_or_else(|| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!("published artifact '{artifact_id}' does not exist"),
                    )
                })?;
                let artifact = cas_types::ArtifactRef {
                    artifact_id: stored.id,
                    name: stored.name,
                    mime: stored.mime,
                    size_bytes: stored.size_bytes,
                    sha256: stored.sha256,
                };
                artifact.validate().map_err(|error| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!("published artifact '{artifact_id}' is invalid: {error}"),
                    )
                })?;
                vec![artifact]
            } else {
                Vec::new()
            };
            let reply_to = req.in_reply_to;
            let wire_device_id = device_id.clone().unwrap_or_else(|| "*".to_string());
            let payload = serde_json::to_string(&crate::ui::factory::OperatorReplyPayload {
                schema_version: 2,
                reply_to,
                message: message.clone(),
                summary: summary.clone(),
                device_id: wire_device_id.clone(),
                operator_label,
                kind,
                attachments: attachments.clone(),
                reply_to_session,
            })
            .map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to encode Commander reply: {error}"),
                )
            })?;
            // GH #1196: with no Commander client connected to this session's
            // daemon and no enrolled cloud feed, nothing can deliver this
            // turn. Say so now instead of queuing it to expire in 15 min.
            if let Some(reason) =
                operator_turn_undeliverable(&self.inner.cas_root, factory_session, chrono::Utc::now())
            {
                return Err(Self::error(ErrorCode::INVALID_REQUEST, reason));
            }
            let reply = queue
                .record_operator_turn(&cas_store::OperatorTurn {
                    source: "supervisor",
                    target: "operator",
                    prompt: &payload,
                    factory_session: Some(factory_session),
                    metadata: cas_store::OperatorTurnMetadata {
                        summary: Some(summary.as_str()),
                        priority: Some(cas_store::NotificationPriority::Normal),
                        origin: Some(&cas_store::QueueOrigin::Daemon),
                        recipient_device_id: device_id.as_deref(),
                        kind: Some(kind.as_str()),
                        attachments: &attachments,
                        acknowledge_prompt_id: req.in_reply_to,
                        ..Default::default()
                    },
                })
                .map_err(|error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to atomically record Commander reply: {error}"),
                    )
                })?;
            let reply_id = reply.id();
            crate::mcp::tools::service::mutation_receipt::message_committed(reply_id);
            let deadline = match ask_deadline {
                Some((secs, default)) => {
                    let id =
                        self.schedule_ask_deadline(reply_id, secs, &default, factory_session)?;
                    format!(
                        "\nDeadline: reminder #{id} wakes you in {secs}s to proceed with the default; cancel it (remind_cancel remind_id={id}) if the operator answers first"
                    )
                }
                None => String::new(),
            };
            return Ok(Self::success(format!(
                "Commander {} queued\n\nID: {reply_id}\nAnswered notification_id: {}\nDevice: {wire_device_id}\nStatus: queued for {wire_device_id}{deadline}",
                kind.as_str(),
                req.in_reply_to
                    .map_or_else(|| "none".to_string(), |id| id.to_string())
            )));
        }

        // Refuse CAS-generated row-source labels before any queue write.
        // Unregistered names may wait only for a matching pending local spawn.
        let resolved_target_agent = {
            use crate::store::open_agent_store;
            open_agent_store(&self.inner.cas_root)
                .ok()
                .and_then(|store| store.list(None).ok())
                .and_then(|agents| {
                    agents
                        .into_iter()
                        .find(|agent| agent.name.eq_ignore_ascii_case(&resolved_target))
                })
        };
        let target_is_registered = resolved_target.eq_ignore_ascii_case("all_workers")
            || resolved_target.eq_ignore_ascii_case("supervisor")
            || resolved_target.eq_ignore_ascii_case("director")
            || resolved_target_agent.is_some();
        if role != "worker" && target_is_reserved_source(&resolved_target) {
            let mut valid_targets = vec![
                "operator".to_string(),
                "supervisor".to_string(),
                "all_workers".to_string(),
                "director".to_string(),
            ];
            if let Ok(store) = crate::store::open_agent_store(&self.inner.cas_root)
                && let Ok(agents) = store.list(None)
            {
                valid_targets.extend(agents.into_iter().map(|agent| agent.name));
            }
            valid_targets.sort_unstable_by_key(|target| target.to_ascii_lowercase());
            valid_targets.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
            return Err(Self::error(
                ErrorCode::INVALID_REQUEST,
                format!(
                    "Unknown message target '{resolved_target}'. Valid targets: {}. \
                     Nothing was enqueued.",
                    valid_targets
                        .into_iter()
                        .map(|target| format!("'{target}'"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }

        if !target_is_registered
            && !self.message_target_has_pending_spawn(&resolved_target, factory_session.as_deref())?
        {
            return Err(Self::error(
                ErrorCode::INVALID_REQUEST,
                format!("no supervisor or worker named {resolved_target} in this project or its peers. Nothing was enqueued."),
            ));
        }

        // cas-4a27 (GH #334): reply inference intentionally requires a
        // recipient-surfacing receipt, so it cannot turn an unrelated later
        // message into proof that an escalation was read. A supervisor that
        // DID read an escalation needs a precise way to say so. Link this
        // reply to the durable notification explicitly, validate the two
        // endpoints, and render the reference on the worker-facing message.
        // This is strong evidence for exactly one row; the conservative
        // generic reply-inference rule below remains unchanged.
        let explicit_reply_to = req.in_reply_to;
        // Only a direct reply acknowledges the row it names (cas-009c).
        let mut explicit_reply_acks = false;
        if let Some(notification_id) = explicit_reply_to {
            let prior = queue
                .message_delivery_report(notification_id)
                .map_err(|error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to inspect in_reply_to message {notification_id}: {error}"),
                    )
                })?
                .ok_or_else(|| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!("in_reply_to notification {notification_id} does not exist"),
                    )
                })?;
            // cas-009c: both parties under every name a row may carry. A
            // supervisor's rows are stored as the logical `supervisor`
            // (display_name above), whatever name the other side typed.
            let counterparty_is_supervisor = addressed_logical_supervisor
                || resolved_target_agent
                    .as_ref()
                    .is_some_and(|agent| agent.role == cas_types::AgentRole::Supervisor);
            let counterparty = [
                resolved_target.as_str(),
                if counterparty_is_supervisor { "supervisor" } else { "" },
            ];
            let me = [
                display_name.as_str(),
                env_agent_name.as_deref().unwrap_or_default(),
                if role == "supervisor" { "supervisor" } else { "" },
            ];
            let reference =
                classify_reply_reference(&prior.source, &prior.target, &counterparty, &me);
            explicit_reply_acks = reference == Some(ReplyReference::DirectReply);
            match reference {
                Some(ReplyReference::DirectReply) => {
                    message = format!(
                        "[CAS reply: explicitly acknowledges notification_id={notification_id}]\n{message}"
                    );
                }
                Some(ReplyReference::Thread) => {
                    message = format!(
                        "[CAS reply: in thread with notification_id={notification_id}]\n{message}"
                    );
                }
                None => {
                    return Err(Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!(
                            "in_reply_to notification {notification_id} is {} -> {}, which is not in a thread between {resolved_target} and this sender",
                            prior.source, prior.target
                        ),
                    ));
                }
            }
        }

        // P3.8: a worker may explicitly type a status/ready message to its
        // own supervisor as an action wake. Ordinary worker messages remain
        // inbox-only, and the registered same-session endpoints are required
        // before CAS attaches the marker consumed by the wake gate.
        let worker_action_kind = if role == "worker" {
            match req.kind.as_deref() {
                Some("status" | "ready") => {
                    let session = factory_session
                        .as_deref()
                        .filter(|session| !session.trim().is_empty())
                        .ok_or_else(|| {
                            Self::error(
                                ErrorCode::INVALID_REQUEST,
                                "kind=status|ready requires a registered worker and its same-session supervisor",
                            )
                        })?;
                    let registered_worker = agent_from_store.as_ref().is_some_and(|agent| {
                        agent.role == cas_types::AgentRole::Worker
                            && agent.factory_session.as_deref() == Some(session)
                    });
                    let same_session_supervisor =
                        resolved_target_agent.as_ref().is_some_and(|agent| {
                            agent.role == cas_types::AgentRole::Supervisor
                                && agent.factory_session.as_deref() == Some(session)
                                && matches!(
                                    agent.status,
                                    cas_types::AgentStatus::Active | cas_types::AgentStatus::Idle
                                )
                        });
                    if !registered_worker || !same_session_supervisor {
                        return Err(Self::error(
                            ErrorCode::INVALID_REQUEST,
                            "kind=status|ready is only for a registered worker messaging its same-session supervisor",
                        ));
                    }
                    if req.merge_request.unwrap_or(false) || req.blocker.unwrap_or(false) {
                        return Err(Self::error(
                            ErrorCode::INVALID_PARAMS,
                            "kind=status|ready cannot be combined with merge_request=true or blocker=true",
                        ));
                    }
                    Some(if req.kind.as_deref() == Some("status") {
                        crate::prompt_revalidation::WorkerActionKind::Status
                    } else {
                        crate::prompt_revalidation::WorkerActionKind::Ready
                    })
                }
                Some("receipt" | "answer" | "ask") => None,
                Some(other) => {
                    return Err(Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!(
                            "worker messages support status, ready, receipt, answer or ask; status/ready require the same-session supervisor, not '{other}'"
                        ),
                    ));
                }
                None => None,
            }
        } else {
            None
        };
        if let Some(kind) = worker_action_kind {
            message = crate::prompt_revalidation::attach_worker_action_envelope(
                &message,
                kind,
                &display_name,
                req.task_id.as_deref(),
            );
        }

        // cas-bc8c: a genuine merge request receives an immutable envelope so
        // transport can suppress it once its requested tip has landed. That
        // envelope is a message type, not a property of the sender's task
        // state: workers must still be able to report a blocker or rejected
        // close after their only parked branch lands (cas-89e1 / GH #328).
        // Therefore an unmarked worker message remains free-form even if it
        // names the one AwaitingMerge task; only an explicit merge request is
        // eligible for revalidation and suppression.
        if role == "worker" && req.merge_request.unwrap_or(false) {
            use crate::mcp::tools::core::task::repo_context::{
                resolve_repo_context, resolve_repo_context_from_local_root,
            };
            use crate::prompt_revalidation::{
                MergeRequestDecision, MergeRequestEnvelope, attach_merge_request_envelope,
                merge_landed_guidance, merge_request_anchor_advanced_note,
                merge_request_request_changes_guidance, revalidate_merge_request,
                select_merge_request_task,
            };
            use crate::store::open_task_store_local;

            let requested_pr_number = extract_pull_request_number(&message)
                .or_else(|| extract_pull_request_number(&summary));

            let merge_context =
                open_task_store_local(&self.inner.cas_root)
                    .ok()
                    .and_then(|store| {
                        let tasks = store
                            .ids_by_assignees_and_statuses(
                                &[&display_name],
                                &[
                                    cas_types::TaskStatus::Open,
                                    cas_types::TaskStatus::InProgress,
                                    cas_types::TaskStatus::Blocked,
                                    cas_types::TaskStatus::AwaitingMerge,
                                ],
                            )
                            .ok()?
                            .into_iter()
                            .map(|id| store.get(&id))
                            .collect::<std::result::Result<Vec<_>, _>>()
                            .ok()?;
                        let active_task = tasks
                            .iter()
                            .find(|task| {
                                task.status == cas_types::TaskStatus::InProgress
                                    && task.assignee.as_deref().is_some_and(|name| {
                                        name.eq_ignore_ascii_case(&display_name)
                                    })
                            })
                            .map(|task| task.id.clone());
                        Some((
                            select_merge_request_task(
                                &tasks,
                                &display_name,
                                req.task_id.as_deref(),
                            )
                            .cloned(),
                        active_task,
                    ))
                });
            let (merge_task, active_task) = merge_context.unwrap_or((None, None));
            if req.task_id.is_none()
                && let Some(active_task) = active_task.as_deref()
            {
                return Ok(Self::success(format!(
                    "Task {active_task} is in progress. Close it before requesting its merge, or supply task_id for an earlier parked delivery. No merge request was queued."
                )));
            }

            if let Some(task) = merge_task
                && let Some(work_target) = task.deliverables.work_target.as_ref()
                && let Ok(repo) = {
                    match resolve_repo_context_from_local_root(&self.inner.cas_root, work_target) {
                        Ok(repo) => {
                            tracing::debug!(
                                task_id = %task.id,
                                resolution = "local_checkout",
                                repo_root = %repo.repo_root.display(),
                                git_common_dir = %repo.git_common_dir.display(),
                                "merge request revalidation selected explicit local checkout"
                            );
                            Ok(repo)
                        }
                        Err(local_error) => {
                            tracing::debug!(
                                task_id = %task.id,
                                resolution = "local_checkout_miss",
                                error = %local_error,
                                "merge request revalidation falling back to host registry"
                            );
                            match resolve_repo_context(&self.inner.cas_root, work_target) {
                                Ok(repo) => {
                                    tracing::debug!(
                                        task_id = %task.id,
                                        resolution = "host_registry_fallback",
                                        repo_root = %repo.repo_root.display(),
                                        git_common_dir = %repo.git_common_dir.display(),
                                        "merge request revalidation selected host registry checkout"
                                    );
                                    Ok(repo)
                                }
                                Err(error) => {
                                    tracing::debug!(
                                        task_id = %task.id,
                                        resolution = "host_registry_miss",
                                        error = %error,
                                        "merge request revalidation could not resolve checkout"
                                    );
                                    Err(error)
                                }
                            }
                        }
                    }
                }
            {
                // GH #986: the envelope's target tip must describe origin,
                // not whatever this shared repository last saw. Refresh the
                // remote-tracking ref first (bounded, best effort; a repo with
                // no origin just keeps its local ref).
                crate::mcp::tools::core::task::lifecycle::close_ops::fetch_parent_branch_best_effort(
                    &repo.repo_root,
                    &repo.target_branch,
                );
                let branch = crate::prompt_revalidation::merge_request_branch(
                    Some(&task),
                    Some(&repo.repo_root),
                );
                let source_is_fresh = if let Some(branch) = branch.as_deref() {
                    match crate::prompt_revalidation::refresh_merge_request_source(
                        &repo.repo_root,
                        branch,
                        task.delivery_mode,
                    ) {
                        Ok(fresh) => fresh,
                        Err(message) => return Ok(CallToolResult::error(vec![Content::text(message)])),
                    }
                } else { true };
                let branch = if !source_is_fresh {
                    let note = "source freshness unavailable; not checked for already-landed";
                    message = attach_merge_request_envelope(
                        &format!("{message}\n\n{note}"),
                        &MergeRequestEnvelope {
                            task_id: task.id.clone(),
                            branch_tip: task.deliverables.factory_branch_anchor.clone()
                                .or_else(|| branch.as_deref().and_then(|branch| {
                                    crate::prompt_revalidation::resolve_live_branch_tip(&repo.repo_root, branch, None)
                                })).unwrap_or_default(),
                            target_branch: repo.target_branch.clone(),
                            target_branch_tip: crate::prompt_revalidation::resolve_target_branch_tip(
                                &repo.repo_root, &repo.target_branch,
                            ).unwrap_or_default(),
                            anchor_tip: task.deliverables.factory_branch_anchor.clone(),
                            commits_not_on_target_base: 0,
                            pr_number: requested_pr_number,
                            source_freshness_unavailable: Some(note.into()),
                        },
                    );
                    // Preserve the typed wake, but skip cached source/anchor
                    // suppression and automatic anchor advancement below.
                    None
                } else { branch };
                // GH #703: use the live branch tip for a continued unmerged
                // delivery. A parked task's anchor is immutable once its
                // delivery lands or the worker starts another task on this
                // mutable branch; revalidating the live tip then would claim
                // the other task's commits for this one.
                let recorded_anchor = task.deliverables.factory_branch_anchor.clone();
                let other_task_active = active_task
                    .as_deref()
                    .is_some_and(|active_id| active_id != task.id);
                if task.status == cas_types::TaskStatus::AwaitingMerge
                    && source_is_fresh
                    && other_task_active
                    && recorded_anchor.is_none()
                {
                    return Ok(Self::success(format!(
                        "Task {} is parked without an immutable delivery anchor while another task is in progress. No merge request was queued; ask the supervisor to reconcile its delivery.",
                        task.id
                    )));
                }
                let anchor_integrated = source_is_fresh && recorded_anchor.as_deref().is_some_and(|anchor| {
                    matches!(
                        revalidate_merge_request(
                            &repo.repo_root,
                            anchor,
                            &repo.target_branch,
                        ),
                        MergeRequestDecision::AlreadyIntegrated { .. }
                    )
                });
                // A rebase rewrites the old delivery's commit IDs, so its
                // pushed head cannot descend from the recorded anchor. On a
                // fresh, dedicated per-task push branch, with no active
                // sibling task and an unmerged anchor, the branch itself is
                // the delivery identity; report its live origin head instead
                // of freezing the now-retired SHA. Keep the stricter
                // attribution rule for shared worker branches.
                let rebased_task_scoped_branch = task.status
                    == cas_types::TaskStatus::AwaitingMerge
                    && task.delivery_mode == cas_types::DeliveryMode::PushBranch
                    && source_is_fresh
                    && recorded_anchor.is_some()
                    && !other_task_active
                    && !anchor_integrated
                    && task.assignee.as_deref().is_some_and(|assignee| {
                        let expected = format!("factory/{assignee}-{}", task.id);
                        branch.as_deref() == Some(expected.as_str())
                    });
                // cas-ba4a: on shared worker branches, the live tip replaces
                // a parked anchor only when it descends from it with this
                // task's commits alone. A sibling task that is Open or Blocked
                // (no active lease) could otherwise be requested as this delivery.
                let live_tip_continues_delivery = task.status
                    == cas_types::TaskStatus::AwaitingMerge
                    && recorded_anchor.is_some()
                    && branch
                        .as_deref()
                        .and_then(|branch| {
                            crate::prompt_revalidation::resolve_live_branch_tip(
                                &repo.repo_root,
                                branch,
                                recorded_anchor.as_deref(),
                            )
                        })
                        .and_then(|live_tip| {
                            let store = open_task_store_local(&self.inner.cas_root).ok()?;
                            self.inner.eligible_awaiting_merge_advance(
                                store.as_ref(),
                                &task,
                                &repo.repo_root,
                                &repo.target_branch,
                                Some(&live_tip),
                            )
                        })
                        .is_some();
                let frozen_anchor = (task.status == cas_types::TaskStatus::AwaitingMerge
                    && (other_task_active
                        || anchor_integrated
                        || (!live_tip_continues_delivery && !rebased_task_scoped_branch)))
                    .then(|| recorded_anchor.clone())
                    .flatten();
                // GH #1022: only a landed anchor with no other active task can
                // hide same-task commits pushed after the merge.
                let frozen_on_landed_anchor =
                    frozen_anchor.is_some() && anchor_integrated && !other_task_active;
                if let Some(branch) = branch
                    && let Some(branch_tip) = frozen_anchor.or_else(|| {
                        let live = crate::prompt_revalidation::resolve_live_branch_tip(
                            &repo.repo_root,
                            &branch,
                            recorded_anchor.as_deref(),
                        )?;
                        // cas-afa9: a parked delivery is judged by a live tip
                        // only when that tip contains it; a recorded branch
                        // from an earlier cycle is not this delivery.
                        Some(match recorded_anchor.as_deref() {
                            Some(anchor)
                                if task.status == cas_types::TaskStatus::AwaitingMerge
                                    && !rebased_task_scoped_branch =>
                            {
                                crate::prompt_revalidation::merge_request_judged_tip(
                                    &repo.repo_root,
                                    Some(live),
                                    anchor,
                                )
                            }
                            _ => live,
                        })
                    })
                {
                    if task.status != cas_types::TaskStatus::AwaitingMerge
                        && task
                            .deliverables
                            .declined_factory_anchors()
                            .iter()
                            .any(|anchor| anchor == &branch_tip)
                    {
                        return Ok(Self::success(merge_request_request_changes_guidance(
                            &task.id,
                            &branch_tip,
                            task.status,
                        )));
                    }
                    match revalidate_merge_request(
                        &repo.repo_root,
                        &branch_tip,
                        &repo.target_branch,
                    ) {
                        MergeRequestDecision::AlreadyIntegrated { target_tip } => {
                            if frozen_on_landed_anchor
                                && let Some(live_tip) =
                                    crate::prompt_revalidation::resolve_live_branch_tip(
                                        &repo.repo_root,
                                        &branch,
                                        recorded_anchor.as_deref(),
                                    )
                                    .filter(|live_tip| *live_tip != branch_tip)
                                && matches!(
                                    revalidate_merge_request(
                                        &repo.repo_root,
                                        &live_tip,
                                        &repo.target_branch,
                                    ),
                                    MergeRequestDecision::Pending { .. }
                                )
                            {
                                return Ok(Self::success(
                                    crate::prompt_revalidation::merge_request_beyond_landed_anchor_guidance(
                                        &task.id,
                                        &branch_tip,
                                        &live_tip,
                                        &repo.target_branch,
                                    ),
                                ));
                            }
                            return Ok(Self::success(merge_landed_guidance(
                                &task.id,
                                &branch_tip,
                                &repo.target_branch,
                                &target_tip,
                            )));
                        }
                        MergeRequestDecision::Pending { target_tip } => {
                            // cas-749f: composing an envelope does not advance
                            // the durable park or prove the new tip's checks.
                            // The merge gate reads the persisted anchor, so
                            // require close to reconcile it before queueing.
                            if task.status == cas_types::TaskStatus::AwaitingMerge
                                && let Some(anchor) = recorded_anchor.as_deref()
                                && anchor != branch_tip
                            {
                                return Ok(Self::success(format!(
                                    "MERGE REQUEST REQUIRES RE-PARK: {} is parked at {anchor}, but its pushed source is {branch_tip}. No merge request was queued and no anchor was advanced. Re-run `{}task action=close id={}` at {branch_tip} with checks for that tip, then send the merge request again.",
                                    task.id,
                                    crate::mcp::tools::core::guidance::caller_prefix(),
                                    task.id,
                                )));
                            }
                            let anchor_tip = recorded_anchor
                                .as_deref()
                                .filter(|anchor| *anchor != branch_tip)
                                .map(str::to_string);
                            if let Some(previous_anchor) = anchor_tip.as_deref() {
                                message = format!(
                                    "{message}\n\n{}",
                                    merge_request_anchor_advanced_note(
                                        previous_anchor,
                                        &branch_tip,
                                    )
                                );
                            }
                            let commits_not_on_target_base =
                                crate::mcp::tools::core::task::lifecycle::close_ops::count_unmerged_factory_commits(
                                    &repo.repo_root,
                                    &branch_tip,
                                    &target_tip,
                                );
                            if let Some(pr_number) = requested_pr_number {
                                let mut task_with_pr = task.clone();
                                if task_with_pr.deliverables.delivery_pr_number != Some(pr_number) {
                                    task_with_pr.deliverables.delivery_pr_number = Some(pr_number);
                                    if let Ok(store) = open_task_store_local(&self.inner.cas_root) {
                                        if let Err(error) = store.update(&task_with_pr) {
                                            tracing::warn!(
                                                task_id = %task.id,
                                                pr_number,
                                                %error,
                                                "could not record merge-request PR number"
                                            );
                                        }
                                    }
                                }
                            }
                            // cas-2ee2 (GH #1023): the supervisor merged a
                            // PR before reading the QA dispatch that arrived
                            // in the same batch. Lead with the QA state, so
                            // the request is not actionable merge guidance
                            // while its tip has no passed or waived round.
                            if let Some(hold) = crate::qa_pass::merge_request_qa_hold(
                                &self.inner.cas_root,
                                &task,
                                &branch_tip,
                            ) {
                                message = format!("{hold}\n\n{message}");
                            }
                            // cas-8b0e: the in-band review shows the worker's
                            // proof:/ci: lines and renders unsupplied fields
                            // neutrally; a missing merge base keeps the task
                            // metadata with the comparison error.
                            let review = crate::review_body::merge_request_review(
                                &repo.repo_root,
                                &task,
                                &target_tip,
                                &branch_tip,
                                &message,
                            );
                            message = attach_merge_request_envelope(
                                &format!("{message}\n\n{review}"),
                                &MergeRequestEnvelope {
                                    task_id: task.id,
                                    // Live tip; the anchor rides along only
                                    // when it disagrees, so the supervisor
                                    // sees the drift instead of inferring it.
                                    anchor_tip,
                                    branch_tip,
                                    target_branch: repo.target_branch,
                                    target_branch_tip: target_tip,
                                    commits_not_on_target_base,
                                    pr_number: requested_pr_number,
                                    source_freshness_unavailable: None,
                                },
                            );
                        }
                        MergeRequestDecision::Unverifiable => {
                            tracing::warn!(
                                task_id = %task.id,
                                "cas-bc8c: merge request tips could not be verified; delivering free-form message unchanged"
                            );
                        }
                    }
                }
            }
        }

        // cas-8725: a blocker is the other message whose whole purpose is to
        // unblock the supervisor, and it was free text — so it stayed
        // inbox-only and was found by polling. CAS frames it here, on the same
        // terms as the merge request above: the envelope is attached by the
        // SERVER for an explicit message type, so the wake gate never has to
        // read the sender's words. Sending the word "BLOCKER" without the flag
        // still reaches the inbox and still does not wake a pane.
        if req.blocker.unwrap_or(false) {
            message = crate::prompt_revalidation::attach_blocker_envelope(
                &message,
                &display_name,
                req.task_id.as_deref(),
            );
        }

        // Bind supervisor assignments before any halt or enqueue mutation.
        // Prose may open with the receipt for the worker's previous task.
        // Preserve explicit task_id first, otherwise the live assignment;
        // every delivery path reads the same immutable binding.
        if matches!(role.as_str(), "supervisor" | "director")
            && !resolved_target.eq_ignore_ascii_case("all_workers")
            && resolved_target_agent
                .as_ref()
                .is_none_or(|agent| agent.role == cas_types::AgentRole::Worker)
            && crate::prompt_revalidation::is_assignment_prompt(&message)
        {
            let tasks = crate::store::open_task_store_local(&self.inner.cas_root).ok();
            let binding = req
                .task_id
                .clone()
                .filter(|id| !id.trim().is_empty())
                .or_else(|| {
                    let mut assignees = vec![resolved_target.as_str()];
                    if let Some(agent) = resolved_target_agent.as_ref() {
                        assignees.push(&agent.id);
                    }
                    let current = tasks
                        .as_ref()
                        .and_then(|store| {
                            store
                                .ids_by_assignees_and_statuses(
                                    &assignees,
                                    &[
                                        cas_types::TaskStatus::Open,
                                        cas_types::TaskStatus::InProgress,
                                        cas_types::TaskStatus::Blocked,
                                    ],
                                )
                                .ok()
                        })
                        .unwrap_or_default();
                    match current.as_slice() {
                        [id] => {
                            let started = tasks
                                .as_ref()
                                .and_then(|store| store.get(id).ok())
                                .is_some_and(|task| task.status != cas_types::TaskStatus::Open);
                            if started {
                                assignment_solicited_task_id(&message).or_else(|| Some(id.clone()))
                            } else {
                                Some(id.clone())
                            }
                        }
                        [] => assignment_solicited_task_id(&message),
                        // Multiple live assignments cannot prove which task this
                        // message solicits. Do not let incidental prose suppress it.
                        _ => None,
                    }
                });
            message = crate::prompt_revalidation::attach_assignment_envelope(
                &message,
                binding.as_deref(),
            );
            if let Some((task_id, status)) = binding
                .as_deref()
                .and_then(|id| tasks.as_ref()?.get(id).ok())
                .and_then(|task| {
                    assignment_stale_task(
                        &message,
                        task.status,
                        task.assignee.as_deref(),
                        &resolved_target,
                    )
                })
            {
                return Err(Self::error(
                    ErrorCode::INVALID_REQUEST,
                    format!(
                        "Assignment refused for {task_id}: task is {status}. No message was queued. To bind the intended assignment, pass task_id=<id>."
                    ),
                ));
            }
        }

        // cas-6913: a message addressed to a not-yet-registered worker name
        // (the common spawn-then-immediately-assign sequence) sits in the
        // queue until that name shows up in the agent store. Check
        // registration state so the compact receipt can add this exception.
        // `all_workers` is a broadcast, not a single-target claim.
        //
        // cas-73c8: `director` is a permanent team member (TeamsManager
        // init_team_config) and the source of inbound teammate messages, but
        // is not an agent_store registration. Treat it as always registered
        // so outbound replies after an inbound director message are not
        // reported as "not yet registered".
        let urgent = req.urgent.unwrap_or(false);
        // Urgent messages break the target's in-flight turn, so they must jump
        // the queue ahead of any backlog: force Critical priority when urgent
        // and no explicit priority was given.
        let priority = req.priority.as_deref().map(|p| match p {
            "critical" | "0" => cas_store::NotificationPriority::Critical,
            "high" | "1" => cas_store::NotificationPriority::High,
            _ => cas_store::NotificationPriority::Normal,
        });
        let priority = if urgent && priority.is_none() {
            Some(cas_store::NotificationPriority::Critical)
        } else {
            priority
        };

        if urgent && !target_is_registered {
            return Err(Self::error(
                ErrorCode::INVALID_REQUEST,
                format!(
                    "Could not interrupt '{resolved_target}': target is not registered, so no live pane can be interrupted"
                ),
            ));
        }

        // cas-b269 review 2: halt fan-out is session-scoped, authorized by
        // AgentRole::Supervisor|Director (and display fallback), fail-closed
        // on store errors, generation-stamped, and all-or-none with enqueue
        // (compensate halt writes if enqueue fails).
        let mut halt_compensation: Vec<(String, std::collections::HashMap<String, String>)> =
            Vec::new();
        let mut halt_bindings: Vec<(String, u64)> = Vec::new();
        {
            use crate::mcp::tools::core::task::lifecycle::stale_close_guard::{
                HaltWorkerCandidate, apply_halt_metadata, bind_halt_to_task_if_generation,
                halt_targets_for_urgent, is_merge_reclose_exempt_urgent, may_source_role_set_halt,
                may_source_set_halt, next_halt_generation, session_scoped_worker_names,
                should_persist_urgent_halt,
            };
            use crate::store::{open_agent_store, open_task_store};
            use cas_types::{AgentRole, TaskStatus};

            // Prefer typed role from agent store when available.
            let source_role_for_halt = agent_from_store
                .as_ref()
                .map(|a| a.role.to_string())
                .unwrap_or_else(|| role.clone());
            let source_authorized = agent_from_store
                .as_ref()
                .map(|a| may_source_role_set_halt(a.role))
                .unwrap_or_else(|| may_source_set_halt(&display_name, &source_role_for_halt));

            if urgent
                && (source_authorized || may_source_set_halt(&display_name, &source_role_for_halt))
            {
                let agent_store = open_agent_store(&self.inner.cas_root).map_err(|e| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!(
                            "Failed to open agent store for urgent halt (cas-b269, fail closed): {e}"
                        ),
                    )
                })?;
                let agents = agent_store.list(None).map_err(|e| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!(
                            "Failed to list agents for urgent halt (cas-b269, fail closed): {e}"
                        ),
                    )
                })?;

                let worker_candidates: Vec<HaltWorkerCandidate> = agents
                    .iter()
                    .filter(|a| a.role == AgentRole::Worker)
                    .map(|a| HaltWorkerCandidate {
                        name: a.name.clone(),
                        factory_session: a.factory_session.clone(),
                    })
                    .collect();
                let session_workers =
                    session_scoped_worker_names(&worker_candidates, factory_session.as_deref());

                // cas-1145: an urgent assignment/recovery message is also a
                // halt request, so its named task must be real and owned by
                // the addressed worker before either metadata or queue state
                // is touched. This closes the reserved-but-never-created id
                // race while preserving ordinary urgent stop messages.
                let assignment_task_id = urgent_assignment_task_id(&message);
                if let Some(task_id) = assignment_task_id.as_deref() {
                    let Some(target_agent) = resolved_target_agent.as_ref().filter(|agent| {
                        agent.role == AgentRole::Worker
                            && agent.name.eq_ignore_ascii_case(&resolved_target)
                    }) else {
                        return Err(Self::error(
                            ErrorCode::INVALID_PARAMS,
                            format!(
                                "Urgent assignment rejected for task {task_id}: target '{resolved_target}' is not a single registered worker"
                            ),
                        ));
                    };
                    let task_store = open_task_store(&self.inner.cas_root).map_err(|error| {
                        Self::error(
                            ErrorCode::INTERNAL_ERROR,
                            format!(
                                "Urgent assignment rejected for task {task_id}: task store unavailable: {error}"
                            ),
                        )
                    })?;
                    let task = task_store.get(task_id).map_err(|error| {
                        tracing::warn!(
                            task_id = %task_id,
                            target = %resolved_target,
                            error = %error,
                            "cas-1145: urgent assignment named a task that does not exist"
                        );
                        Self::error(
                            ErrorCode::INVALID_PARAMS,
                            format!(
                                "Urgent assignment rejected: task {task_id} does not exist; no halt or queue row was emitted"
                            ),
                        )
                    })?;
                    let owns_task = task.assignee.as_deref().is_some_and(|owner| {
                        owner.eq_ignore_ascii_case(&target_agent.name)
                            || owner.eq_ignore_ascii_case(&target_agent.id)
                    });
                    if !owns_task {
                        tracing::warn!(
                            task_id = %task_id,
                            target = %target_agent.name,
                            assignee = ?task.assignee,
                            "cas-1145: urgent assignment named a task not owned by its target"
                        );
                        return Err(Self::error(
                            ErrorCode::INVALID_PARAMS,
                            format!(
                                "Urgent assignment rejected: task {task_id} is not assigned to worker {}",
                                target_agent.name
                            ),
                        ));
                    }
                }

                // cas-126b: an urgent "MERGE DONE → re-close now" hand-off both
                // wakes the parked worker AND (before this guard) armed
                // halt_task_work — deadlocking the very re-close it asks for.
                // Skip the halt fan-out when this urgent send is close/verify
                // guidance that references, as a bounded token, a task that is
                // (a) currently AwaitingMerge AND (b) assigned to THIS urgent's
                // target worker. The assignee binding is a scope/authorization
                // gate: an urgent to worker B must not skip halt because its
                // text happens to name worker A's parked task. The exemption
                // only skips the halt flag — the message is still
                // enqueued+injected, and the factory-branch merge gate in
                // close_ops remains the sole authority on close success, so a
                // re-close sent before the merge is visible still rejects with
                // MERGE REQUIRED (never a false success). Fail closed — if the
                // task store can't be read, keep the original halt behavior.
                let reclose_exempt = match open_task_store(&self.inner.cas_root)
                    .ok()
                    .and_then(|ts| ts.list(Some(TaskStatus::AwaitingMerge)).ok())
                {
                    Some(tasks) => {
                        let target_awaiting_ids: Vec<String> = tasks
                            .into_iter()
                            .filter(|t| {
                                t.assignee
                                    .as_deref()
                                    .map(|a| a.eq_ignore_ascii_case(&resolved_target))
                                    .unwrap_or(false)
                            })
                            .map(|t| t.id)
                            .collect();
                        is_merge_reclose_exempt_urgent(&message, &target_awaiting_ids)
                    }
                    None => false,
                };

                if reclose_exempt {
                    tracing::debug!(
                        target = %resolved_target,
                        "cas-126b: skipping halt_task_work for merge-complete re-close urgent"
                    );
                } else if should_persist_urgent_halt(
                    urgent,
                    &display_name,
                    &source_role_for_halt,
                    &resolved_target,
                    &session_workers,
                ) {
                    let targets = halt_targets_for_urgent(&resolved_target, &session_workers);
                    let halt_generation = next_halt_generation();
                    for target_name in &targets {
                        // Match by name + session so same-name cross-session
                        // peers are not halted.
                        let Some(mut agent) = agents
                            .iter()
                            .find(|a| {
                                a.role == AgentRole::Worker
                                    && a.name.eq_ignore_ascii_case(target_name)
                                    && a.visible_to_factory_session(factory_session.as_deref())
                            })
                            .cloned()
                        else {
                            continue;
                        };
                        halt_compensation.push((agent.id.clone(), agent.metadata.clone()));
                        apply_halt_metadata(&mut agent.metadata, halt_generation);
                        if let Some(task_id) = assignment_task_id.as_deref() {
                            let _ = bind_halt_to_task_if_generation(
                                &mut agent.metadata,
                                halt_generation,
                                task_id,
                            );
                        }
                        if let Err(e) = agent_store.update(&agent) {
                            // Compensate any prior successful writes.
                            for (id, prev) in halt_compensation.drain(..) {
                                if let Ok(mut a) = agent_store.get(&id) {
                                    a.metadata = prev;
                                    let _ = agent_store.update(&a);
                                }
                            }
                            return Err(Self::error(
                                ErrorCode::INTERNAL_ERROR,
                                format!(
                                    "Failed to persist halt_task_work for {target_name} \
                                     before urgent enqueue (cas-b269): {e}"
                                ),
                            ));
                        }
                        halt_bindings.push((agent.id.clone(), halt_generation));
                    }
                }
            } else if urgent
                && !source_authorized
                && !may_source_set_halt(&display_name, &source_role_for_halt)
            {
                tracing::debug!(
                    source = %display_name,
                    role = %source_role_for_halt,
                    "cas-b269: ignoring halt for unauthorized source"
                );
            }
        }

        // cas-f9e8 telemetry: measure the wall-clock spent inside the DB
        // insert and log it alongside the caller-visible message id, so a
        // future investigator can bisect whether stalls live in send-side
        // persistence, daemon wake, daemon poll, or downstream inject. Logged
        // at debug so normal sessions stay quiet; enable via
        // `RUST_LOG=cas::coordination=debug`.
        // cas-d9a8: the server-stamped sender. This is the REGISTRY ID of the
        // agent CAS resolved from this MCP session — deliberately not
        // `display_name`, not `row_source`, and not `source`'s "unknown"
        // fallback, all of which are labels a caller can influence or a failed
        // lookup can invent. When no registered row resolves, the row goes out
        // unstamped and keeps today's inbox-only behaviour: failing to
        // identify a sender must never be worth a pane wake.
        let queue_origin =
            agent_from_store
                .as_ref()
                .map(|agent| cas_store::QueueOrigin::RegisteredAgent {
                    agent_id: agent.id.clone(),
                });
        // Local cap/spill and reply framing must not discard the sender's opt-out.
        if (req.wake == Some(false) || req.kind.as_deref()==Some("receipt"))
            && !crate::supervisor_message_wakes::quiet(&message) {
            message=format!("{}\n{message}",crate::supervisor_message_wakes::QUIET);
        }
        let enqueue_started = std::time::Instant::now();
        let peer_copy_message = peer_supervisor_copy
            .as_ref()
            .map(|_| peer_copy_text(&display_name, &resolved_target, &message));
        let (message_id, duplicate_suppressed, supervisor_copy_id) =
            if let Some(supervisor) = peer_supervisor_copy.as_deref() {
                let session = factory_session
                    .as_deref()
                    .expect("peer scope requires a factory session");
                match queue.enqueue_worker_peer_with_supervisor_copy(
                    &display_name,
                    &resolved_target,
                    supervisor,
                    &message,
                    peer_copy_message.as_deref().expect("peer copy text"),
                    session,
                    Some(summary.as_str()),
                    priority,
                    urgent,
                    queue_origin.as_ref(),
                ) {
                    Ok(ids) => (ids.recipient_id, false, Some(ids.supervisor_copy_id)),
                    Err(error) => {
                        return Err(Self::error(
                            ErrorCode::INVALID_REQUEST,
                            format!("Could not queue scoped peer message: {error}"),
                        ));
                    }
                }
            } else {
                // cas-15f2: stamp the row with the RECIPIENT's session, not the
                // sender's, so the recipient's daemon and inbox can select it.
                let row_session = row_factory_session(
                    &resolved_target,
                    resolved_target_agent.as_ref(),
                    factory_session.as_deref(),
                );
                // cas-5087: and stamp it with a source the recipient's wake
                // gate can resolve — see `row_source`.
                let row_source = row_source(
                    &display_name,
                    agent_from_store.as_ref(),
                    resolved_target_agent.as_ref(),
                );
                let enqueue_result = if worker_action_kind.is_some() {
                    let action_session = row_session
                        .as_deref()
                        .expect("validated worker action has a same-session supervisor");
                    let action_origin = queue_origin
                        .as_ref()
                        .expect("validated worker action has a registered origin");
                    queue.enqueue_worker_action_with_outcome(
                        &row_source,
                        &resolved_target,
                        &message,
                        &display_name,
                        action_session,
                        Some(summary.as_str()),
                        priority,
                        urgent,
                        action_origin,
                    )
                } else {
                    queue.enqueue_urgent_with_outcome(
                        &row_source,
                        &resolved_target,
                        &message,
                        row_session.as_deref(),
                        Some(summary.as_str()),
                        priority,
                        urgent,
                        queue_origin.as_ref(),
                    )
                };
                let enqueue_outcome = match enqueue_result {
                    Ok(id) => id,
                    Err(error) => {
                        // Compensate halt fan-out so we never leave halt without the
                        // corresponding urgent message (all-or-none).
                        if !halt_compensation.is_empty() {
                            use crate::store::open_agent_store;
                            if let Ok(agent_store) = open_agent_store(&self.inner.cas_root) {
                                for (id, prev) in halt_compensation.drain(..) {
                                    if let Ok(mut a) = agent_store.get(&id) {
                                        a.metadata = prev;
                                        let _ = agent_store.update(&a);
                                    }
                                }
                            }
                        }
                        return Err(Self::error(
                            ErrorCode::INTERNAL_ERROR,
                            format!("Failed to queue message: {error}"),
                        ));
                    }
                };
                let duplicate_suppressed = matches!(
                    enqueue_outcome,
                    cas_store::EnqueueOutcome::SuppressedDuplicate(_)
                );
                (enqueue_outcome.id(), duplicate_suppressed, None)
            };

        crate::mcp::tools::service::mutation_receipt::message_committed(message_id);
        // cas-6853 (communication contract v2, P2.4/P3.7): a broadcast's
        // audience policy. Stamped on the committed row; older same-key
        // broadcasts still awaiting transport are withdrawn.
        if broadcast_supersede_key.is_some() || broadcast_sticky {
            queue
                .stamp_broadcast_policy(
                    message_id,
                    broadcast_supersede_key.as_deref(),
                    broadcast_sticky,
                )
                .map_err(|error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!(
                            "Broadcast {message_id} queued but its supersede_key/sticky policy failed: {error}"
                        ),
                    )
                })?;
        }
        if let Some(notification_id) = explicit_reply_to.filter(|_| explicit_reply_acks) {
            queue.ack(notification_id).map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!(
                        "Reply {message_id} queued but failed to confirm in_reply_to notification {notification_id}: {error}"
                    ),
                )
            })?;
        }

        // cas-85fd: an urgent halt is a course-correction exchange, not a
        // permanent worker state. Once the row exists, bind every halt this
        // send armed to its exact prompt id. A later urgent has a newer
        // generation and is deliberately left untouched by this write.
        if !halt_bindings.is_empty() {
            use crate::mcp::tools::core::task::lifecycle::stale_close_guard::bind_halt_to_prompt_if_generation;
            use crate::store::open_agent_store;
            if let Ok(agent_store) = open_agent_store(&self.inner.cas_root) {
                for (agent_id, generation) in &halt_bindings {
                    let Ok(mut agent) = agent_store.get(agent_id) else {
                        continue;
                    };
                    if bind_halt_to_prompt_if_generation(
                        &mut agent.metadata,
                        *generation,
                        message_id,
                    ) {
                        if let Err(error) = agent_store.update(&agent) {
                            tracing::warn!(
                                agent_id,
                                message_id,
                                error = %error,
                                "could not bind urgent halt to its prompt; task start remains the recovery path"
                            );
                        }
                    }
                }
            }
        }

        // cas-6ad2: sending a response is a recipient-side consumption signal
        // for prior messages from that counterparty. The old explicit
        // message_ack API was never invoked by factory prompts, leaving every
        // acted-on message stuck at Delivered/AwaitingAck.
        //
        // cas-99d2 (GH #126): "a reply happened" is NOT on its own evidence
        // that any particular earlier message was consumed. The store now
        // requires the reply to post-date the message's transport handoff AND a
        // surfacing receipt to exist for it — so this call needs the reply's
        // own enqueue instant, read back from the row just written rather than
        // sampled as "now" (which would drift past the true enqueue time by
        // however long the surrounding bookkeeping takes and could sweep in a
        // message delivered in that window).
        //
        // Supervisors have two queue identities: outbound source
        // `"supervisor"` and their generated pane/display name as an inbound
        // target. Workers use their display name both ways. Include both alias
        // shapes and advance only already transport-delivered rows.
        let mut recipient_aliases = vec![display_name.as_str()];
        if let Some(name) = env_agent_name.as_deref()
            && !recipient_aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(name))
        {
            recipient_aliases.push(name);
        }
        let resolved_target_role = resolved_target_agent.as_ref().map(|agent| agent.role);
        let resolved_target_cli = resolved_target_agent
            .as_ref()
            .map(|agent| crate::mcp::tools::service::factory_ops::worker_cli_from_agent(agent));
        let target_is_supervisor = addressed_logical_supervisor
            || resolved_target_role == Some(cas_types::AgentRole::Supervisor);
        let mut counterparty_aliases = vec![resolved_target.as_str()];
        if role == "worker"
            && target_is_supervisor
            && !counterparty_aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case("supervisor"))
        {
            counterparty_aliases.push("supervisor");
        }
        let reply_enqueued_at = queue
            .message_delivery_report(message_id)
            .ok()
            .flatten()
            .map(|report| report.enqueued_at)
            .unwrap_or_else(chrono::Utc::now);
        if let Err(error) = queue.ack_delivered_for_recipient(
            &recipient_aliases,
            &counterparty_aliases,
            factory_session.as_deref(),
            reply_enqueued_at,
        ) {
            tracing::warn!(
                message_id,
                source = %display_name,
                target_agent = %resolved_target,
                error = %error,
                "failed to confirm prior delivered messages after recipient response"
            );
        }

        // cas-85fd: release only the halt bound to the urgent this reply
        // answers. cas-4a8e1 (GH #1064): any reply to the supervisor written
        // after that urgent reached the worker discharges it — an interrupt
        // is usually answered with an ordinary message, not message_ack, and
        // requiring `Confirmed` left the halt to veto an unrelated close long
        // after the exchange ended. Do not clear a legacy/unbound halt, a
        // newer halt that replaced this exchange, or one whose urgent the
        // worker could not yet have seen.
        if role == "worker" && target_is_supervisor {
            use crate::mcp::tools::core::task::lifecycle::stale_close_guard::{
                clear_halt_metadata, halt_prompt_id, reply_discharges_halt,
            };
            use crate::store::open_agent_store;
            if let Ok(agent_store) = open_agent_store(&self.inner.cas_root)
                && let Ok(mut agent) = agent_store.get(&source)
                && let Some(prompt_id) = halt_prompt_id(&agent.metadata)
                && queue
                    .message_delivery_report(prompt_id)
                    .ok()
                    .flatten()
                    .is_some_and(|report| reply_discharges_halt(&report, reply_enqueued_at))
            {
                clear_halt_metadata(&mut agent.metadata);
                if let Err(error) = agent_store.update(&agent) {
                    tracing::warn!(
                        agent_id = %source,
                        prompt_id,
                        error = %error,
                        "could not release answered urgent halt"
                    );
                }
            }
        }

        let persist_latency_ms = enqueue_started.elapsed().as_secs_f64() * 1000.0;
        tracing::debug!(
            target: "cas::coordination",
            stage = "enqueue",
            channel = "prompt_queue",
            message_id,
            source = %display_name,
            target_agent = %resolved_target,
            priority = ?priority,
            duplicate_suppressed,
            persist_ms = persist_latency_ms,
            "prompt_queue enqueue resolved"
        );

        // Notify daemon that prompt queue has new data (best-effort)
        if !duplicate_suppressed {
            let notify_started = std::time::Instant::now();
            let notify_outcome = cas_factory::notify_daemon(&self.inner.cas_root);
            let notify_latency_ms = notify_started.elapsed().as_secs_f64() * 1000.0;
            match notify_outcome {
                Ok(()) => {
                    tracing::debug!(
                        target: "cas::coordination",
                        stage = "notify",
                        channel = "prompt_queue",
                        message_id,
                        notify_ms = notify_latency_ms,
                        "daemon wakeup signal sent"
                    );
                }
                Err(ref e) => {
                    // Kept as debug because this is expected when the daemon is
                    // not running (e.g. `cas serve` standalone sessions).
                    tracing::debug!(
                        target: "cas::coordination",
                        stage = "notify",
                        channel = "prompt_queue",
                        message_id,
                        notify_ms = notify_latency_ms,
                        error = %e,
                        "daemon wakeup signal failed (daemon may not be running)"
                    );
                }
            }
        }

        // cas-71d9 (GH #269): an urgent write to a Claude teammate is not a
        // successful interrupt until the daemon's pane-output probe records a
        // recipient-side transport receipt. Wait through that bounded probe
        // window and fail explicitly when no observation arrives. The queue
        // row remains durable, so a late retry is still possible, but the
        // caller can no longer mistake enqueue success for a broken wait state.
        if urgent && resolved_target_cli == Some(cas_mux::SupervisorCli::Claude) {
            wait_for_claude_interrupt_delivery(&*queue, message_id)
                .await
                .map_err(|message| Self::error(ErrorCode::INTERNAL_ERROR, message))?;
        }

        // P1.6: the send result is a compact current-stage receipt. The
        // detailed delivery report stays available through message_status;
        // only actionable exceptions expand the send result.
        // cas-d5ab: with a daemon listening, give it its first pass so a busy
        // recipient's deferral is named at send time, not 50 ms later. A
        // native-extension recipient's row is never decided by the daemon,
        // so it is not waited on. A backlog is not a reason to skip: the wait
        // is bounded and says "no decision yet" when it runs out (review N1:
        // a project-wide pending count skipped every real send).
        let native_recipient = resolved_target_agent.as_ref().is_some_and(|agent| {
            agent.metadata.get("native_extension").is_some_and(|value| value == "true")
        });
        let awaits_decision = target_is_registered
            && !urgent
            && !duplicate_suppressed
            && !native_recipient
            && daemon_accepts_notify(&self.inner.cas_root);
        let (send_report, no_decision_yet) = if awaits_decision {
            wait_for_send_outcome(&*queue, message_id).await
        } else {
            (queue.message_delivery_report(message_id).ok().flatten(), false)
        };
        let delivery_stage = send_report
            .as_ref()
            .map(|report| report.stage)
            .unwrap_or(cas_store::DeliveryStage::Enqueued);
        let stage_text = delivery_stage.to_string();
        let report_detail = send_report
            .as_ref()
            .and_then(|report| {
                report.pending_detail.as_deref().or_else(|| {
                    report.pending_reason.map(|reason| reason.as_str())
                })
            })
            .unwrap_or("inspect message_status for the current delivery detail");
        // The task can move on between preflight and the daemon's first pass.
        // A withdrawal is a refused send, never a successful queue receipt.
        if delivery_stage == cas_store::DeliveryStage::Suppressed
            && send_report.as_ref().is_some_and(|report| {
                report.pending_reason == Some(cas_store::PendingReason::SupersededStale)
            })
            && let Some(task_id) = assignment_solicited_task_id(&message)
        {
            return Err(Self::error(
                ErrorCode::INVALID_REQUEST,
                format!(
                    "Assignment refused for {task_id}: {report_detail}. Message #{message_id} was withdrawn before delivery. To bind the intended assignment, pass task_id=<id>."
                ),
            ));
        }
        let supervisor_copy_suffix = supervisor_copy_id
            .map(|id| format!("; supervisor copy #{id}"))
            .unwrap_or_default();
        let receipt = if duplicate_suppressed {
            format!(
                "suppressed #{message_id} → {resolved_target} (duplicate; no new row)\
                 {supervisor_copy_suffix}"
            )
        } else if !target_is_registered {
            format!(
                "queued #{message_id} → {resolved_target} ({stage_text})\n\
                 Target is not registered yet; the row will deliver when it registers. \
                 Check message_status notification_id={message_id} for its latest stage. \
                 {supervisor_copy_suffix}"
            )
        } else if urgent {
            format!(
                "queued #{message_id} → {resolved_target} (interrupt-and-redirect)\
                 {supervisor_copy_suffix}"
            )
        } else {
            match delivery_stage {
                // cas-d5ab (P1.6): name the deferral and where it surfaces.
                cas_store::DeliveryStage::Gated => format!(
                    "deferred #{message_id} → {resolved_target}: {}{supervisor_copy_suffix}",
                    deferral_reason(
                        send_report.as_ref().and_then(|report| report.wake_attempt_detail.as_deref()),
                        send_report.as_ref().and_then(|report| report.pending_detail.as_deref()),
                        resolved_target_cli == Some(cas_mux::SupervisorCli::Claude),
                    )
                ),
                cas_store::DeliveryStage::Delivered
                | cas_store::DeliveryStage::AssumedSeen
                | cas_store::DeliveryStage::Confirmed => format!(
                    "delivered #{message_id} → {resolved_target} ({stage_text}){supervisor_copy_suffix}"
                ),
                cas_store::DeliveryStage::Dropped | cas_store::DeliveryStage::Abandoned => {
                    format!(
                        "failed #{message_id} → {resolved_target} ({stage_text}: {report_detail}).\
                         Check message_status notification_id={message_id} for details.\
                         {supervisor_copy_suffix}"
                    )
                }
                cas_store::DeliveryStage::Suppressed => format!(
                    "suppressed #{message_id} → {resolved_target} ({report_detail}).\
                     Check message_status notification_id={message_id} for details.\
                     {supervisor_copy_suffix}"
                ),
                cas_store::DeliveryStage::PartiallyDelivered => format!(
                    "partial #{message_id} → {resolved_target} ({report_detail}).\
                     Check message_status notification_id={message_id} for details.\
                     {supervisor_copy_suffix}"
                ),
                // Review L6: the settle window ended before the daemon decided.
                _ if no_decision_yet => format!(
                    "queued #{message_id} → {resolved_target} (no decision yet)\
                     {supervisor_copy_suffix}"
                ),
                _ => format!(
                    "queued #{message_id} → {resolved_target} ({stage_text})\
                     {supervisor_copy_suffix}"
                ),
            }
        };

        let message_id_text = message_id.to_string();
        let enqueue_result = if duplicate_suppressed {
            "suppressed_duplicate"
        } else {
            "created"
        };
        let _ = crate::hooks::handlers::session_hygiene::append_factory_session_event(
            &self.inner.cas_root,
            "coordination_message",
            &[
                ("message_id", &message_id_text),
                ("source", &display_name),
                ("target", &resolved_target),
                ("summary", &summary),
                ("urgent", if urgent { "true" } else { "false" }),
                ("enqueue_result", enqueue_result),
            ],
        );

        Ok(Self::success(receipt))
    }

    pub(in crate::mcp::tools::service) async fn inbox_poll(
        &self,
        req: AgentRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::{open_agent_store, open_prompt_queue_store};

        let agent_id = self.inner.get_agent_id().ok();
        let registered_agent = agent_id.as_deref().and_then(|id| {
            open_agent_store(&self.inner.cas_root)
                .ok()
                .and_then(|store| store.get(id).ok())
        });
        let recipient = resolve_inbox_recipient(
            registered_agent.as_ref().map(|agent| agent.name.clone()),
            std::env::var("CAS_AGENT_NAME").ok(),
        )
        .ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_REQUEST,
                "inbox_poll requires a registered agent identity",
            )
        })?;
        let factory_session = std::env::var("CAS_FACTORY_SESSION")
            .ok()
            .filter(|session| !session.trim().is_empty())
            .or_else(|| {
                registered_agent
                    .as_ref()
                    .and_then(|agent| agent.factory_session.clone())
            });
        let limit = req.limit.unwrap_or(10).min(100);

        let queue = open_prompt_queue_store(&self.inner.cas_root).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open prompt queue: {error}"),
            )
        })?;
        // The store records recipient-seen state in the same transaction that
        // selects the rows, before this MCP response is handed back. That
        // prevents concurrent duplicate delivery; a later poll does not replay
        // a row. cas-6ccb (GH #1215): a row polled but never acknowledged with
        // message_ack is shown once more by the next turn's hook (the
        // `inbox_poll` receipt, older than that turn, with no ack), so a row
        // polled mid-turn and not acted on is not lost. Daemon transport state
        // remains independent.
        // cas-3bf1 (GH #176): poll every alias this agent answers to, not just
        // its registered pane name. A supervisor answers to two, and rows
        // addressed to the logical `supervisor` alias were previously
        // UNREACHABLE from here — the unseen predicate matches
        // `q.target = ?alias OR q.target = 'all_workers'`, and `supervisor` was
        // never passed as an alias, so supervisor-addressed mail was visible
        // only via the turn-start hook. Measured on the live queue before this
        // change: 40 of 50 `supervisor`-addressed rows never receipted, against
        // 15 of 59 for the pane name.
        let aliases = crate::harness_policy::inbox_aliases(
            &recipient,
            registered_agent
                .as_ref()
                .is_some_and(|agent| agent.role == cas_types::AgentRole::Supervisor),
        );
        // cas-ad92: a pointer wake names its notification id. Hand that row
        // over even while the daemon's transport still holds it claimed:
        // Claude Code shows the teams copy only at the next turn boundary,
        // and without hooks (cas-0f5b) this poll is the woken agent's only
        // way to read the body now. Only rows addressed to this caller match.
        let named: Vec<i64> = req.notification_id.into_iter().collect();
        let mut messages: Vec<cas_store::QueuedPrompt> = Vec::new();
        for alias in aliases.iter().filter(|_| !named.is_empty()) {
            for row in queue
                .poll_named_for_recipient(alias, factory_session.as_deref(), &named)
                .map_err(|error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to poll recipient inbox: {error}"),
                    )
                })?
            {
                if !messages.iter().any(|existing| existing.id == row.id) {
                    messages.push(row);
                }
            }
        }
        // cas-8cbe (GH #123): named reads are the recipient's explicit request
        // for a retained row. Surface that id before the stale quarantine
        // sweep; otherwise the sweep terminally abandons it before the
        // store's NamedRead eligibility can return it. Automatic polling
        // below still has its own stale cutoff, and the sweep quarantines
        // every other stale pending row before that automatic drain.
        // cas-d047 (GH #69): a freshly spawned worker must never be handed a
        // months-old item addressed to a recycled name. The daemon sweeps
        // these too, but a worker can poll in a session with no daemon tick
        // yet (or none at all), so quarantine on the read path as well — the
        // store keeps the row and its forensics, it just stops being
        // deliverable, and every withheld row is named in the log.
        match queue.expire_stale_pending(cas_store::PROMPT_QUEUE_STALE_TTL_SECS) {
            Ok(stale) => {
                for row in &stale {
                    tracing::warn!(
                        target: "cas::coordination",
                        stage = "stale_quarantine",
                        prompt_id = row.id,
                        source = %row.source,
                        target_agent = %row.target,
                        created_at = %row.created_at.to_rfc3339(),
                        age_secs = (chrono::Utc::now() - row.created_at).num_seconds(),
                        recipient = %recipient,
                        "cas-d047: withheld stale queue item from inbox delivery"
                    );
                }
            }
            Err(error) => {
                tracing::warn!(
                    target: "cas::coordination",
                    stage = "stale_quarantine",
                    %error,
                    "cas-d047: stale queue sweep failed; stale rows remain filtered out of this poll"
                );
            }
        }
        for alias in &aliases {
            let remaining = limit.saturating_sub(messages.len());
            if remaining == 0 {
                break;
            }
            let found = queue
                .poll_unseen_for_recipient(alias, factory_session.as_deref(), remaining)
                .map_err(|error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to poll recipient inbox: {error}"),
                    )
                })?;
            for row in found {
                // A broadcast comes back from BOTH aliases; handing it to the
                // caller twice in one response is the duplicate this task must
                // not create while removing the cross-turn one.
                if messages.iter().any(|existing| existing.id == row.id) {
                    continue;
                }
                messages.push(row);
            }
        }
        // Retire every polled row for the whole identity, so the turn-start
        // hook does not re-inject what this poll just handed over.
        crate::harness_policy::mirror_receipts_across_aliases(&*queue, &messages, &aliases);

        if messages.is_empty() {
            return Ok(Self::success(format!("No unread messages for {recipient}")));
        }

        // cas-99d2 (GH #127): a row the daemon already handed to this
        // recipient's transport is not new mail, and the poll used to render it
        // byte-identically with no way to tell. Classify each row before
        // rendering: withhold one whose solicited transition already happened,
        // and mark the rest as repeats.
        let task_store = crate::store::open_task_store_local(&self.inner.cas_root).ok();
        let mut rendered = 0usize;
        // Every withholding path below follows one policy: first identify the
        // typed action the row requests, then require fresh positive evidence
        // that action is moot. Missing/unreadable state delivers. A withheld
        // row is always retained here with its notification id and an
        // operator-readable reason; no suppression may disappear silently.
        let mut withheld: Vec<(i64, String)> = Vec::new();
        let mut redelivered = 0usize;
        let mut shown_redeliveries: Vec<i64> = Vec::new();
        // cas-098d (GH #904): older transport deliveries are claimed and named,
        // not replayed.
        let mut replays: Vec<(i64, String)> = Vec::new();
        let now = chrono::Utc::now();
        let mut body = String::new();
        for message in &messages {
            // A supervisor can claim a queue row directly through inbox_poll,
            // bypassing the daemon's transport-time check. Re-run the exact
            // merge-request predicate here so an invalidated delivery anchor
            // cannot become actionable merely because this is the first
            // delivery surface the supervisor used.
            let stale_merge_request = crate::prompt_revalidation::parse_merge_request_envelope(
                &message.prompt,
            )
            .and_then(|envelope| {
                let store = task_store.as_ref()?;
                let task = store.get(&envelope.task_id).ok()?;
                use crate::mcp::tools::core::task::repo_context::resolve_repo_context;
                let repo_root = task
                    .deliverables
                    .work_target
                    .as_ref()
                    .and_then(|work_target| {
                        resolve_repo_context(&self.inner.cas_root, work_target).ok()
                    })
                    .map(|repo| repo.repo_root)
                    .unwrap_or_else(|| {
                        self.inner
                            .cas_root
                            .parent()
                            .unwrap_or(&self.inner.cas_root)
                            .to_path_buf()
                    });
                let git = crate::prompt_revalidation::revalidate_merge_request(
                    &repo_root,
                    &envelope.branch_tip,
                    &envelope.target_branch,
                );
                (!matches!(
                    crate::prompt_revalidation::merge_request_delivery_decision(
                        Some(&task),
                        &envelope,
                        &git,
                    ),
                    crate::prompt_revalidation::MergeRequestDelivery::Deliver
                ))
                .then_some(envelope.task_id)
            });
            if let Some(task_id) = stale_merge_request {
                tracing::info!(
                    target: "cas::coordination",
                    stage = "inbox_withheld_stale_merge_request",
                    prompt_id = message.id,
                    recipient = %recipient,
                    task_id = %task_id,
                    "withheld a merge request whose delivery anchor no longer holds at inbox pop"
                );
                withheld.push((
                    message.id,
                    format!("merge request for {task_id}, already done"),
                ));
                continue;
            }

            // Lifecycle rows have the same direct-poll bypass. In particular,
            // a queued MERGE REQUIRED relay must not survive request_changes,
            // cancel, reset, or a completed merge merely because no daemon
            // tick ran before the supervisor polled.
            let lifecycle_task = crate::prompt_revalidation::parse_lifecycle_envelope(
                &message.prompt,
            )
            .and_then(|envelope| {
                task_store
                    .as_ref()
                    .and_then(|store| store.get(&envelope.task_id).ok())
            });
            let stale_lifecycle = withhold_lifecycle_at_inbox_pop(
                message,
                named.contains(&message.id),
                lifecycle_task.as_ref(),
            );
            if stale_lifecycle {
                let task_id = crate::prompt_revalidation::parse_lifecycle_envelope(&message.prompt)
                    .map(|envelope| envelope.task_id)
                    .unwrap_or_default();
                tracing::info!(
                    target: "cas::coordination",
                    stage = "inbox_withheld_stale_merge_relay",
                    prompt_id = message.id,
                    recipient = %recipient,
                    task_id = %task_id,
                    "withheld a stale lifecycle relay at inbox pop"
                );
                withheld.push((
                    message.id,
                    format!("lifecycle relay for {task_id}, already done"),
                ));
                continue;
            }
            let solicited_task = if match queue.is_abandonment_recovery(message.id) {
                Ok(recovery) => recovery,
                Err(error) => {
                    tracing::warn!(prompt_id = message.id, %error, "could not classify abandonment recovery");
                    false
                }
            } {
                None
            } else {
                assignment_solicited_task_id(&message.prompt)
            };
            let stale_assignment = match (&solicited_task, task_store.as_ref()) {
                (Some(task_id), Some(store)) => store.get(task_id).ok().and_then(|task| {
                    assignment_stale_task(
                        &message.prompt,
                        task.status,
                        task.assignee.as_deref(),
                        &recipient,
                    )
                }),
                _ => None,
            };
            if let Some((task_id, status)) = stale_assignment {
                tracing::info!(
                    target: "cas::coordination",
                    stage = "inbox_withheld_stale_assignment",
                    prompt_id = message.id,
                    recipient = %recipient,
                    task_id = %task_id,
                    status = %status,
                    "cas-7c1a: withheld a queued assignment whose task state makes its start instruction stale"
                );
                withheld.push((
                    message.id,
                    format!("assignment for {task_id}, already started: {status}"),
                ));
                continue;
            }
            // The transition an assignment solicits: the ADDRESSED recipient
            // started that task. `assignee` must match — another worker
            // starting the task says nothing about whether this recipient ever
            // saw the message.
            let solicited_action_observed = match (&solicited_task, task_store.as_ref()) {
                (Some(task_id), Some(store)) => store
                    .get(task_id)
                    .ok()
                    .map(|task| {
                        task.status != cas_types::TaskStatus::Open
                            && task
                                .assignee
                                .as_deref()
                                .is_some_and(|assignee| assignee.eq_ignore_ascii_case(&recipient))
                    })
                    .unwrap_or(false),
                _ => false,
            };
            match inbox_redelivery_decision(
                message.processed_at.is_some(),
                solicited_action_observed,
            ) {
                InboxRedelivery::WithholdConsumed => {
                    let task_id = solicited_task.unwrap_or_default();
                    tracing::info!(
                        target: "cas::coordination",
                        stage = "inbox_withheld_consumed",
                        prompt_id = message.id,
                        recipient = %recipient,
                        task_id = %task_id,
                        "cas-99d2: withheld an already-delivered assignment whose solicited \
                         task start is already recorded for this recipient"
                    );
                    withheld.push((
                        message.id,
                        format!("assignment for {task_id}, already done"),
                    ));
                    continue;
                }
                InboxRedelivery::MarkRedelivery
                    if !named.contains(&message.id)
                        && inbox_row_is_stale_replay(
                        message.processed_at,
                        recipient_had_delivery(
                            queue
                                .message_delivery_report(message.id)
                                .ok()
                                .flatten()
                                .as_ref(),
                        ),
                        now,
                    ) =>
                {
                    let age_mins = message
                        .processed_at
                        .map(|at| (now - at).num_minutes())
                        .unwrap_or_default();
                    replays.push((
                        message.id,
                        format!(
                            "{} from {}, delivered {age_mins} min ago",
                            message.summary.as_deref().unwrap_or("no summary"),
                            message.source
                        ),
                    ));
                    continue;
                }
                InboxRedelivery::MarkRedelivery => {
                    redelivered += 1;
                    // cas-6ccb: shown again in full here; no third showing.
                    shown_redeliveries.push(message.id);
                    body.push_str(&format!(
                        "**[{}] From: {} — {INBOX_REDELIVERY_MARKER} (already delivered {})**\n\
                        Summary: {}\n{}\n{}Message: {}\n\n",
                        message.id,
                        message.source,
                        message
                            .processed_at
                            .map(|at| at.to_rfc3339())
                            .unwrap_or_else(|| "earlier".to_string()),
                        message.summary.as_deref().unwrap_or("(no summary)"),
                        queued_message_provenance(message),
                        commander_reply_framing(message).unwrap_or_default(),
                        message.prompt,
                    ));
                }
                InboxRedelivery::FirstDelivery => {
                    body.push_str(&format!(
                        "**[{}] From: {}**\nSummary: {}\n{}\n{}Message: {}\n\n",
                        message.id,
                        message.source,
                        message.summary.as_deref().unwrap_or("(no summary)"),
                        queued_message_provenance(message),
                        commander_reply_framing(message).unwrap_or_default(),
                        message.prompt,
                    ));
                }
            }
            rendered += 1;
        }
        // cas-6ccb (GH #1215): a row this poll withheld as already done, or
        // named without replaying it, is settled now, so the next turn's
        // repeat of unacknowledged polled rows never brings back its body.
        // No ack and no stage change: the recipient confirmed none of these.
        let settled: Vec<(i64, String)> = withheld
            .iter()
            .map(|(id, reason)| (*id, format!("withheld by inbox_poll: {reason}")))
            .chain(replays.iter().map(|(id, what)| {
                (*id, format!("not replayed by inbox_poll: {what}"))
            }))
            .chain(shown_redeliveries.iter().map(|id| {
                (*id, "shown again by inbox_poll as an earlier delivery".to_string())
            }))
            .collect();
        if let Err(error) = queue.settle_polled_rows(&aliases, &settled) {
            tracing::warn!(
                target: "cas::coordination",
                recipient = %recipient,
                %error,
                "cas-6ccb: could not settle withheld inbox rows; they may be repeated once"
            );
        }

        let replay_note = (!replays.is_empty()).then(|| {
            format!(
                "Not replayed: {} older message(s) your transport delivered more than {} min ago \
                 and you have had since: {}",
                replays.len(),
                INBOX_REPLAY_AFTER_SECS / 60,
                replays
                    .iter()
                    .map(|(id, what)| format!("{id} ({what})"))
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
        if rendered == 0
            && withheld.is_empty()
            && let Some(note) = replay_note.as_deref()
        {
            return Ok(Self::success(format!(
                "No new messages for {recipient}. {note}."
            )));
        }
        if rendered == 0 {
            let ids = withheld
                .iter()
                .map(|(id, reason)| format!("{id} ({reason})"))
                .collect::<Vec<_>>()
                .join(", ");
            return Ok(Self::success(format!(
                "No unread messages for {recipient} — withheld {} message(s) already \
                 done: {ids}{}",
                withheld.len(),
                replay_note
                    .as_deref()
                    .map(|note| format!(". {note}"))
                    .unwrap_or_default()
            )));
        }

        let mut output = format!(
            "Pulled {rendered} unread message(s) for {recipient} (inbox claim: marked seen \
             now; a direct message you neither message_ack nor answer is shown once more at \
             your next turn, except broadcasts and rows withheld or marked already delivered \
             here; daemon transport delivery is unchanged)"
        );
        if redelivered > 0 {
            output.push_str(&format!(
                ". {redelivered} marked {INBOX_REDELIVERY_MARKER}: already handed to your \
                 transport once — treat as a duplicate unless you never acted on it"
            ));
        }
        if !withheld.is_empty() {
            output.push_str(&format!(
                ". Withheld {} message(s) already done: {}",
                withheld.len(),
                withheld
                    .iter()
                    .map(|(id, reason)| format!("{id} ({reason})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(note) = replay_note {
            output.push_str(&format!(". {note}"));
        }
        output.push_str(":\n\n");
        output.push_str(&body);

        Ok(Self::success(output))
    }

    pub(in crate::mcp::tools::service) async fn message_ack(
        &self,
        req: AgentRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::open_prompt_queue_store;

        let notification_id = req.notification_id.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "notification_id required for message_ack (ordinary prompt message ID, or the durable notification ID printed in a lifecycle relay)",
            )
        })?;

        // cas-20ac: lifecycle envelopes print supervisor_queue IDs, not
        // prompt_queue IDs. Resolve that lane first; blindly acking the same
        // integer in prompt_queue can confirm an unrelated row while the
        // lifecycle relay remains pending and is injected again.
        if let Some(linked) = super::supervisor_queue::acknowledge_linked_lifecycle_notification(
            &self.inner.cas_root,
            notification_id,
        )
        .map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to acknowledge linked lifecycle notification: {error}"),
            )
        })? {
            return Ok(Self::success(format!(
                "Lifecycle message {} acknowledged across durable and prompt queues{}; exact-notification redelivery is now terminal.",
                linked.durable_notification_id,
                linked
                    .prompt_id
                    .map(|id| format!(" (linked message_id: {id})"))
                    .unwrap_or_else(|| " (linked prompt already terminal or absent)".to_string())
            )));
        }

        let queue = open_prompt_queue_store(&self.inner.cas_root).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open prompt queue: {error}"),
            )
        })?;

        // Historical terminal relays carry only their `lifecycle-wake:<id>`
        // marker; that ID is neither a durable notification nor a prompt row.
        if let Some(prompt_id) = queue.ack_lifecycle_wake(notification_id).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to acknowledge lifecycle relay: {error}"),
            )
        })? {
            return Ok(Self::success(format!(
                "Lifecycle relay {notification_id} acknowledged (prompt message_id: {prompt_id}); it will no longer replay in worker_status."
            )));
        }

        queue.ack(notification_id).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to acknowledge message: {error}"),
            )
        })?;

        // cas-85fd/cas-dcf2: a reply after an inbox drain is intentionally
        // only `assumed_seen`, so it cannot release an urgent-stop halt. An
        // explicit acknowledgement of the *bound* urgent is the authoritative
        // receipt instead. Release no other (including newer) halt.
        use crate::mcp::tools::core::task::lifecycle::stale_close_guard::{
            clear_halt_metadata, halt_prompt_id,
        };
        if let Ok(agent_id) = self.inner.get_agent_id()
            && let Ok(agent_store) = self.inner.open_agent_store()
            && let Ok(mut agent) = agent_store.get(&agent_id)
            && halt_prompt_id(&agent.metadata) == Some(notification_id)
            && matches!(
                queue.message_status(notification_id),
                Ok(Some(cas_store::MessageStatus::Confirmed))
            )
        {
            clear_halt_metadata(&mut agent.metadata);
            if let Err(error) = agent_store.update(&agent) {
                tracing::warn!(
                    agent_id = %agent_id,
                    prompt_id = notification_id,
                    error = %error,
                    "could not release explicitly acknowledged urgent halt"
                );
            }
        }

        // cas-45c4 (GH #102): say what an ack actually proves. It is the
        // caller's claim that it received this message — not evidence Cassy
        // observed the content being surfaced, and not a guarantee for anyone
        // else's copy of a broadcast.
        Ok(Self::success(format!(
            "Message {notification_id} acknowledged by this session (confirmation_source: \
             explicit_ack). This records YOUR claim to have received it; Cassy does not \
             independently observe that the content was surfaced. Use message_status to see \
             transport handoff, wake/reaction observations, and confirmation provenance \
             separately."
        )))
    }

    pub(in crate::mcp::tools::service) async fn message_status_query(
        &self,
        req: AgentRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::open_prompt_queue_store;

        let queue = open_prompt_queue_store(&self.inner.cas_root).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open prompt queue: {error}"),
            )
        })?;
        let notification_id = match req.notification_id {
            Some(id) => id,
            None => {
                let Some(given) = req.id.clone().filter(|id| !id.trim().is_empty()) else {
                    return Err(Self::error(
                        ErrorCode::INVALID_PARAMS,
                        "notification_id required for message_status (the prompt queue message ID), \
                         or id=<peer message id> for a message sent to a peer supervisor",
                    ));
                };
                // cas-f9c7: `id` names a message sent to a peer supervisor on
                // another machine; its receipt lives in the Cassy Cloud peer
                // mailbox. cas-6021: a number this queue holds is its own
                // message, answered here rather than as a peer 404.
                // cas-6021 review: a local read error is reported as one,
                // not hidden behind a peer lookup's unrelated failure.
                let local = match given.trim().parse::<i64>() {
                    Ok(id) => queue
                        .message_delivery_report(id)
                        .map_err(|error| {
                            Self::error(
                                ErrorCode::INTERNAL_ERROR,
                                format!("Failed to query message status: {error}"),
                            )
                        })?
                        .map(|_| id),
                    Err(_) => None,
                };
                match local {
                    Some(local) => local,
                    None => return self.peer_message_status(given).await,
                }
            }
        };

        // cas-2c5f: stage-based report is additive; legacy status string is
        // preserved on the first line for older clients/scripts.
        let report = queue
            .message_delivery_report(notification_id)
            .map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to query message status: {error}"),
                )
            })?;

        match report {
            Some(mut r) => {
                enrich_report_from_harness_artifact(&self.inner.cas_root, &mut r);
                // cas-893c AC2: `delivered_at` is only transport handoff (the
                // teams-inbox write / PTY inject succeeding). cas-4fb9 adds
                // artifact-backed wake/reaction observations where the
                // recipient harness exposes them; a missing artifact remains
                // `Unobserved`. `confirmed_at` is still the only field that means
                // "the recipient told us it got this" (an explicit
                // `message_ack`), which most recipients never call. So the
                // honest "how long has this been undelivered" clock runs
                // from `enqueued_at` until `confirmed_at`, not until
                // `delivered_at` — a message can sit "delivered" (transport
                // succeeded) but functionally unread for a long time, which
                // is exactly the failure mode this task exists to surface.
                let now = chrono::Utc::now();
                let undelivered_after_secs = if r.confirmed_at.is_none() {
                    Some((now - r.enqueued_at).num_seconds().max(0))
                } else {
                    None
                };

                let mut json_value = serde_json::to_value(&r).unwrap_or_else(|_| {
                    serde_json::json!({
                        "id": r.id,
                        "legacy_status": r.legacy_status,
                        "stage": r.stage,
                    })
                });
                if let Some(obj) = json_value.as_object_mut() {
                    obj.insert(
                        "undelivered_after_secs".to_string(),
                        match undelivered_after_secs {
                            Some(secs) => serde_json::Value::Number(secs.into()),
                            None => serde_json::Value::Null,
                        },
                    );
                }
                let json = serde_json::to_string_pretty(&json_value).unwrap_or_else(|_| {
                    format!(
                        "{{\"id\":{},\"legacy_status\":\"{}\",\"stage\":\"{}\"}}",
                        r.id, r.legacy_status, r.stage
                    )
                });
                let reason = r
                    .pending_reason
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "none".into());
                let undelivered_line = match undelivered_after_secs {
                    Some(secs) => format!(
                        "undelivered_after: {secs}s (not yet confirmed received — \
                         transport handoff succeeding is not the same as the recipient \
                         reading it; escalate if this is climbing and the target is idle)\n"
                    ),
                    // cas-45c4 (GH #102): an ack inferred from a later reply is
                    // evidence the recipient took a turn, NOT that this
                    // message's content was surfaced to it. Reporting both the
                    // same way is what let status claim a confirmation the
                    // recipient never made.
                    None => format!(
                        "undelivered_after: n/a (confirmation_source: {}{})\n",
                        r.confirmation_source,
                        if r.confirmation_source.is_recipient_claim() {
                            " — the recipient's own claim about this message"
                        } else {
                            " — Cassy inferred this from later activity; the recipient never \
                             claimed to have read THIS message"
                        }
                    ),
                };
                // cas-ac7e (GH #130): stage=delivered used to be the writer's
                // unchecked claim about itself. Say out loud when the
                // recipient side cannot corroborate it, instead of reporting
                // "delivered" and leaving the operator to discover from the
                // recipient that nothing arrived.
                let transport_line = recipient_transport_warning(
                    r.stage,
                    &r.target,
                    r.recipient_transport_at.is_some(),
                )
                .unwrap_or("");
                // cas-7a01 (GH #155): `wake: unobserved` used to be a
                // hardcoded constant with no backing column — three incidents
                // read it and learned nothing, because it could not tell "Cassy
                // never nudged this recipient" from "Cassy nudged it and the
                // harness started a turn without surfacing the message".
                // `wake_attempt` is the daemon's own record of which of those
                // happened; `wake` remains recipient-side evidence.
                let wake_attempt_line = wake_attempt_narrative(r.wake_attempt, r.wake);
                let receipt_line = recipient_receipt_narrative(
                    r.recipient_receipt,
                    r.recipient_poll_settled.as_deref(),
                );
                // cas-15f2: a row that no daemon can select used to read
                // "awaiting_delivery" for a full 15 minutes and then flip to
                // abandoned, so the operator watching it had no way to tell
                // "in flight" from "already doomed" — two supervisors waited on
                // exactly this. State the deadline while it is still pending,
                // and say the reason out loud once it is terminal.
                let handoff = if r.stage == cas_store::DeliveryStage::Gated
                    && r.pending_reason == Some(cas_store::PendingReason::GatedNotReady)
                {
                    queue
                        .inbox_handoff_state(notification_id)
                        .map_err(|error| {
                            Self::error(
                                ErrorCode::INTERNAL_ERROR,
                                format!("Failed to query deferred inbox status: {error}"),
                            )
                        })?
                } else {
                    cas_store::InboxHandoffState::NotHeld
                };
                let pending_deadline_line = pending_deadline_narrative(
                    r.stage,
                    r.pending_reason,
                    r.pending_detail.as_deref(),
                    undelivered_after_secs,
                    handoff,
                );
                let factory = crate::config::Config::load(&self.inner.cas_root)
                    .unwrap_or_default()
                    .factory();
                let escalate_after_secs = if r.urgent || r.priority <= 1 {
                    factory.delivery_stalled_priority_secs
                } else {
                    factory.delivery_stalled_normal_secs
                };
                let verdict_line = recipient_verdict(&r, now, escalate_after_secs);
                let wake_target = if r.target == "supervisor" {
                    crate::store::open_agent_store(&self.inner.cas_root).ok()
                        .and_then(|store| store.list(None).ok())
                        .and_then(|agents| agents.into_iter().filter(|agent| agent.role == cas_types::AgentRole::Supervisor
                            && agent.factory_session == r.factory_session).max_by_key(|agent| agent.registered_at))
                        .map(|agent| agent.name).unwrap_or_else(|| r.target.clone())
                } else { r.target.clone() };
                let wake_cost_line = crate::supervisor_message_wakes::report(&self.inner.cas_root, &wake_target)
                    .unwrap_or_else(|error| format!("directed_message_wake_cost: unavailable ({error})\n"));
                Ok(Self::success(format!(
                    "Message {notification_id} status: {}\n\
                     {verdict_line}\
                     {wake_cost_line}\
                     stage: {}  pending_reason: {}  wake: {}  wake_attempt: {}  wake_gate_declines: {}  \
                     reaction: {}  confirmation_source: {}\n\
                     {wake_attempt_line}\
                     {receipt_line}\
                     {transport_line}\
                     {pending_deadline_line}\
                     {undelivered_line}\
                     {json}",
                    r.legacy_status,
                    r.stage,
                    reason,
                    r.wake,
                    r.wake_attempt,
                    r.wake_gate_declines,
                    r.reaction,
                    r.confirmation_source
                )))
            }
            None => Ok(Self::success(format!(
                "Message {notification_id} not found"
            ))),
        }
    }
}

/// Enrich the store's deliberately conservative report with real harness
/// records. Failure at any lookup/parse step leaves both stages unobserved.
///
/// The delivery timestamp is only an ordering floor: it never becomes an
/// observation itself. A non-Unobserved value requires a concrete record in
/// the target worker's resolved artifact, and the record timestamp + path are
/// attached to the report as provenance.
fn enrich_report_from_harness_artifact(
    cas_root: &std::path::Path,
    report: &mut cas_store::MessageDeliveryReport,
) {
    use cas_store::ObservationStatus;

    let Some(delivered_at) = report.delivered_at else {
        return;
    };
    let Ok(store) = crate::store::open_agent_store(cas_root) else {
        return;
    };
    let Ok(agents) = store.list(None) else {
        return;
    };
    let Some(agent) = agents.into_iter().find(|agent| {
        (agent.name == report.target || agent.id == report.target)
            && report
                .factory_session
                .as_ref()
                .is_none_or(|session| agent.factory_session.as_ref() == Some(session))
    }) else {
        return;
    };
    let cli = crate::mcp::tools::service::factory_ops::worker_cli_from_agent(&agent);
    if cli == cas_mux::SupervisorCli::OpenCode {
        let session_id = agent.cc_session_id.as_deref().unwrap_or(&agent.id);
        let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
        let Some(observation) = crate::mcp::tools::service::opencode_liveness::observe(
            cas_root,
            session_id,
            now_ms,
            crate::mcp::tools::service::agent_liveness::agent_process_is_alive(&agent),
        ) else {
            return;
        };
        let Some(mapped_session) =
            crate::mcp::tools::service::opencode_liveness::mapped_session_id(&observation)
        else {
            return;
        };
        let evidence_prefix = format!("OpenCode mapped session {mapped_session}");
        if let Some(at) = observation
            .state
            .last_activity_at
            .and_then(|at| chrono::DateTime::<chrono::Utc>::from_timestamp_millis(at as i64))
            .filter(|at| *at >= delivered_at)
        {
            report.wake = ObservationStatus::Observed;
            report.wake_observed_at = Some(at);
            report.wake_evidence = Some(format!(
                "{evidence_prefix} plugin activity signal at {}",
                at.to_rfc3339()
            ));
        }
        if let Some(at) = observation
            .state
            .last_tool
            .as_ref()
            .and_then(|tool| tool.completed_at)
            .and_then(|at| chrono::DateTime::<chrono::Utc>::from_timestamp_millis(at as i64))
            .filter(|at| *at >= delivered_at)
        {
            report.reaction = ObservationStatus::Observed;
            report.reaction_observed_at = Some(at);
            report.reaction_evidence = Some(format!(
                "{evidence_prefix} completed tool attribution at {}",
                at.to_rfc3339()
            ));
        }
        return;
    }
    let Some(path) =
        crate::mcp::tools::service::factory_ops::worker_transcript_path_for_agent(cas_root, &agent)
    else {
        return;
    };
    let observations = crate::mcp::tools::service::harness_observation::observations_after_delivery(
        &path,
        cli,
        delivered_at,
        &report.prompt,
    );
    if let Some(wake) = observations.wake {
        report.wake = ObservationStatus::Observed;
        report.wake_observed_at = Some(wake.at);
        report.wake_evidence = Some(wake.evidence);
    }
    if let Some(reaction) = observations.reaction {
        report.reaction = ObservationStatus::Observed;
        report.reaction_observed_at = Some(reaction.at);
        report.reaction_evidence = Some(reaction.evidence);
    }
}

/// How long a never-selected row waits before the poison sweep abandons it
/// (`PROMPT_RETRY_MAX_AGE_SECS` in cas-store).
const PROMPT_ABANDON_DEADLINE_SECS: i64 = 15 * 60;

/// Say whether a still-pending row is in flight or already doomed, and name the
/// reason once it is terminal (cas-15f2).
///
/// The old status text reported `stage=enqueued / pending_reason=awaiting_delivery`
/// identically for a row about to be delivered and for one no daemon could ever
/// select. Two supervisors watched that line for 15 minutes before it flipped to
/// `abandoned`, which is the whole "pending forever with no reason" complaint.
fn pending_deadline_narrative(
    stage: cas_store::DeliveryStage,
    pending_reason: Option<cas_store::PendingReason>,
    pending_detail: Option<&str>,
    undelivered_after_secs: Option<i64>,
    handoff: cas_store::InboxHandoffState,
) -> String {
    use cas_store::DeliveryStage;
    // cas-d1659: a row parked for a busy recipient is neither doomed nor on
    // the 15-minute selection deadline. Say what will actually happen to it.
    if stage == DeliveryStage::Gated
        && pending_reason == Some(cas_store::PendingReason::AwaitingBusyRecipient)
    {
        if let Some(detail)=pending_detail.filter(|detail|detail.starts_with("directed supervisor message ")
            || detail.starts_with("quiet acknowledgement or wake=false")) {
            return format!("supervisor wake policy: still pending — {detail}; readable at the next turn start or tool call\n");
        }
        return format!(
            "busy recipient: still pending — wake retries stopped while the recipient works; \
             the message surfaces at its next turn start or tool call — {}\n",
            pending_detail.unwrap_or("no detail was recorded")
        );
    }
    // GatedNotReady also covers composer retries. Only the durable Teams
    // handoff marker promises tool-boundary surfacing without a retry deadline.
    if stage == DeliveryStage::Gated
        && pending_reason == Some(cas_store::PendingReason::GatedNotReady)
    {
        let detail = pending_detail.unwrap_or("no detail was recorded");
        return if handoff == cas_store::InboxHandoffState::HeldTeamsInbox {
            format!(
                "held inbox: still pending — surfaces at its next tool call or turn start; \
                 does not expire while the recipient stays in the session — {detail}\n"
            )
        } else {
            format!(
                "delivery retry: still pending — no held inbox handoff is recorded; \
                 transport retries (for example behind an operator draft) are abandoned \
                 once the {PROMPT_ABANDON_DEADLINE_SECS}s retry budget runs out — {detail}\n"
            )
        };
    }
    match stage {
        DeliveryStage::Abandoned => format!(
            "undeliverable: this row was abandoned and will never be delivered — {}\n",
            pending_detail.unwrap_or("no reason was recorded")
        ),
        DeliveryStage::Enqueued | DeliveryStage::Gated | DeliveryStage::Selected => {
            let Some(waited) = undelivered_after_secs else {
                return String::new();
            };
            let remaining = PROMPT_ABANDON_DEADLINE_SECS - waited;
            if remaining > 0 {
                format!(
                    "delivery deadline: abandoned as undeliverable in {remaining}s if no daemon \
                     selects it (waited {waited}s of {PROMPT_ABANDON_DEADLINE_SECS}s)\n"
                )
            } else {
                format!(
                    "delivery deadline: PAST ({waited}s waited, limit \
                     {PROMPT_ABANDON_DEADLINE_SECS}s) — the sweep will mark this undeliverable; \
                     it is not still in flight\n"
                )
            }
        }
        _ => String::new(),
    }
}

/// cas-d5ab (P1.6): the one-line reason a gated send names.
///
/// The wake decision's own detail is read first: the daemon's generic hold
/// stamp ("wake deferred — pane busy, tool call in flight, operator
/// composing, or wake slot taken") lists every reason and names none (review
/// H2). A Claude recipient sees a held message at its next tool call or turn
/// start (cas-a1ed); a PTY recipient gets it typed when its pane is free.
fn deferral_reason(wake_detail: Option<&str>, pending_detail: Option<&str>, claude_recipient: bool) -> String {
    fn named(detail: &str) -> Option<&'static str> {
        if detail.starts_with("wake deferred") {
            return Some("recipient busy");
        }
        if detail.contains("tool call in flight") {
            Some("recipient mid tool call")
        } else if detail.contains("not been silent long enough") {
            Some("recipient busy")
        } else if detail.contains("unsubmitted draft") {
            Some("operator is typing in the recipient's pane")
        } else if detail.contains("not ready for injection") {
            Some("recipient pane not ready yet")
        } else if detail.contains("wake slot") {
            Some("another wake went first this pass")
        } else {
            None
        }
    }
    let why = wake_detail
        .and_then(named)
        .or_else(|| pending_detail.and_then(named))
        .map(str::to_string)
        .or_else(|| {
            wake_detail
                .or(pending_detail)
                .map(|detail| detail.trim_start_matches("wake_declined_by_policy: ").to_string())
        })
        .unwrap_or_else(|| "held by the wake gate".to_string());
    let surfaces = if claude_recipient {
        "surfaces at its next tool call or turn start"
    } else {
        "delivered when its pane is free"
    };
    format!("{why}; {surfaces}")
}

/// cas-d5ab (P1.6): how long a send waits for the daemon's first decision on
/// its row. The daemon decides within tens of milliseconds of the notify;
/// tests give a loaded machine more room.
#[cfg(not(test))]
const SEND_OUTCOME_SETTLE: std::time::Duration = std::time::Duration::from_millis(1000);
#[cfg(test)]
const SEND_OUTCOME_SETTLE: std::time::Duration = std::time::Duration::from_secs(10);
const SEND_OUTCOME_POLL: std::time::Duration = std::time::Duration::from_millis(20);

/// Whether the daemon has made its first decision on a fresh row: handed it
/// over, held it, or stated why it waits (review M4). A row nobody has looked
/// at reports `awaiting_delivery`; native-extension recipients, whose rows
/// keep that reason, are not waited on at all. The wake attempt is stamped
/// before the hold, so it is not the signal.
fn send_outcome_settled(report: &cas_store::MessageDeliveryReport) -> bool {
    !matches!(
        report.stage,
        cas_store::DeliveryStage::Enqueued | cas_store::DeliveryStage::Selected
    ) || report
        .pending_reason
        .is_some_and(|reason| reason != cas_store::PendingReason::AwaitingDelivery)
}

/// Whether a daemon is listening on the notify socket. A stale socket left by
/// a killed daemon refuses the datagram, and `notify_daemon` treats that as
/// success (review M3).
fn daemon_accepts_notify(cas_root: &std::path::Path) -> bool {
    let Ok(socket) = std::os::unix::net::UnixDatagram::unbound() else {
        return false;
    };
    let _ = socket.set_nonblocking(true);
    match socket.send_to(&[1u8], cas_factory::notify_socket_path(cas_root)) {
        Ok(_) => true,
        Err(error) => error.kind() == std::io::ErrorKind::WouldBlock,
    }
}

/// The row's report once the daemon has decided it, and whether the settle
/// window ended first.
async fn wait_for_send_outcome(
    queue: &dyn cas_store::PromptQueueStore,
    message_id: i64,
) -> (Option<cas_store::MessageDeliveryReport>, bool) {
    let deadline = tokio::time::Instant::now() + SEND_OUTCOME_SETTLE;
    loop {
        let report = queue.message_delivery_report(message_id).ok().flatten();
        if report.as_ref().is_none_or(send_outcome_settled) {
            return (report, false);
        }
        if tokio::time::Instant::now() >= deadline {
            return (report, true);
        }
        tokio::time::sleep(SEND_OUTCOME_POLL).await;
    }
}

/// Which `factory_session` a queued row must carry (cas-15f2).
///
/// The row is stamped with the **recipient's** session, never the sender's.
/// Every downstream filter — the daemon's `peek_for_targets`, `inbox_poll`'s
/// `drain_unseen_for_recipient`, turn-start surfacing, and the `worker_status`
/// unseen counter — selects on `factory_session = <observer's own session>`.
/// Stamping at enqueue with the target's session is therefore the single change
/// that makes all four see the row, and it keeps every cross-session isolation
/// test true: the row genuinely belongs to the recipient's session, so nothing
/// leaks into a session that was not addressed.
///
/// Before this, `message_send` stamped the sender's `CAS_FACTORY_SESSION` while
/// validating the target against a session-blind name lookup, so a supervisor
/// addressing a supervisor in another session got an optimistic "enqueued
/// (target is registered)" for a row no daemon could ever select: the sender's
/// daemon matched the session but not its roster, the recipient's daemon
/// matched its roster but not the session. The row sat at
/// `stage=enqueued / awaiting_delivery` until the sender-side 15-minute poison
/// sweep abandoned it as `abandoned_unknown_target`.
///
/// Logical fan-out names stay sender-scoped: `all_workers` is a broadcast to
/// *this* factory, and `supervisor` / `director` resolve within the caller's own
/// session. Only a concrete registered agent redirects the stamp.
/// Which `source` a queued row must carry (cas-5087).
///
/// A supervisor's outbound rows are stamped `source="supervisor"` — the right
/// answer for a worker, which has exactly one and reads "from supervisor" as
/// its chain of command. It is the WRONG answer for a supervisor recipient:
/// the peer-supervisor wake allowance (cas-15f2) resolves `source` against the
/// agent roster by name, and no live supervisor is *named* "supervisor"
/// (`noble-lynx-44`, `fast-kestrel-6`), so that lookup always failed and the
/// wake could never fire. Routing the row to the right session then delivered
/// it into an inbox nobody was woken to read — the polling-discovery failure
/// the wake slice exists to end.
///
/// So when both ends are supervisors, name the sender. It is also the only
/// legible answer on a clone with two of them, where "from supervisor" does
/// not say WHICH. Worker-facing rows are untouched.
fn row_source(
    display_name: &str,
    sender: Option<&cas_types::Agent>,
    target_agent: Option<&cas_types::Agent>,
) -> String {
    use cas_types::AgentRole;
    let both_supervisors = sender.is_some_and(|agent| agent.role == AgentRole::Supervisor)
        && target_agent.is_some_and(|agent| agent.role == AgentRole::Supervisor);
    match sender.filter(|_| both_supervisors) {
        Some(sender) if !sender.name.trim().is_empty() => sender.name.clone(),
        _ => display_name.to_string(),
    }
}

fn row_factory_session(
    resolved_target: &str,
    target_agent: Option<&cas_types::Agent>,
    sender_session: Option<&str>,
) -> Option<String> {
    if matches!(
        resolved_target.to_ascii_lowercase().as_str(),
        "all_workers" | "supervisor" | "director"
    ) {
        return sender_session.map(str::to_owned);
    }
    target_agent
        .and_then(|agent| agent.factory_session.clone())
        .filter(|session| !session.trim().is_empty())
        .or_else(|| sender_session.map(str::to_owned))
}

#[cfg(test)]
mod pending_deadline_tests {
    use super::pending_deadline_narrative;
    use cas_store::{DeliveryStage, InboxHandoffState};

    /// The 15 minutes two supervisors spent watching "awaiting_delivery" must
    /// now say how much of the budget is left.
    #[test]
    fn a_pending_row_states_how_long_before_it_is_abandoned() {
        let line = pending_deadline_narrative(
            DeliveryStage::Enqueued,
            None,
            None,
            Some(60),
            InboxHandoffState::NotHeld,
        );
        assert!(
            line.contains("abandoned as undeliverable in 840s"),
            "{line}"
        );
        assert!(line.contains("waited 60s of 900s"), "{line}");
    }

    /// Past the deadline the row is doomed, not in flight. Saying "still
    /// pending" there is the claimed-green shape this task exists to remove.
    /// cas-a1ed: messages 4189870, 4189912 and 4189924 were held because
    /// their busy recipients' panes were never silent. Status said "PAST — the
    /// sweep will mark this undeliverable" although no sweep abandons a row
    /// held for a live recipient, so a supervisor read a delivery as lost.
    #[test]
    fn a_row_held_for_a_busy_recipient_is_not_on_the_abandonment_deadline_cas_a1ed() {
        for waited in [60, 1200] {
            let line = pending_deadline_narrative(
                DeliveryStage::Gated,
                Some(cas_store::PendingReason::GatedNotReady),
                Some(
                    "wake deferred — pane busy, tool call in flight, operator composing, or wake slot taken",
                ),
                Some(waited),
                InboxHandoffState::HeldTeamsInbox,
            );
            assert!(line.contains("still pending"), "{line}");
            assert!(line.contains("next tool"), "{line}");
            assert!(!line.contains("abandoned as undeliverable"), "{line}");
            assert!(!line.contains("PAST"), "{line}");
            assert!(!line.contains("retry budget"), "{line}");
        }
    }

    #[test]
    fn a_row_past_the_deadline_says_it_is_not_in_flight() {
        let line = pending_deadline_narrative(
            DeliveryStage::Enqueued,
            None,
            None,
            Some(1200),
            InboxHandoffState::NotHeld,
        );
        assert!(line.contains("PAST"), "{line}");
        assert!(line.contains("not still in flight"), "{line}");
    }

    #[test]
    fn an_abandoned_row_names_its_reason_instead_of_reporting_progress() {
        let line = pending_deadline_narrative(
            DeliveryStage::Abandoned,
            None,
            Some("target no longer belongs to factory session"),
            Some(1800),
            InboxHandoffState::NotHeld,
        );
        assert!(line.contains("undeliverable"), "{line}");
        assert!(
            line.contains("target no longer belongs to factory session"),
            "{line}"
        );
        assert!(line.contains("will never be delivered"), "{line}");
    }

    #[test]
    fn an_abandoned_row_without_a_recorded_reason_says_so() {
        let line = pending_deadline_narrative(
            DeliveryStage::Abandoned,
            None,
            None,
            None,
            InboxHandoffState::NotHeld,
        );
        assert!(line.contains("no reason was recorded"), "{line}");
    }

    /// cas-d1659: a busy recipient's parked row is still pending and will
    /// surface; it is neither abandoned nor on the selection deadline.
    #[test]
    fn a_busy_recipient_row_says_it_is_pending_for_the_next_turn() {
        let line = pending_deadline_narrative(
            DeliveryStage::Gated,
            Some(cas_store::PendingReason::AwaitingBusyRecipient),
            Some("recipient busy: wake budget spent after 3 attempts"),
            Some(1200),
            InboxHandoffState::NotHeld,
        );
        assert!(line.contains("busy recipient: still pending"), "{line}");
        assert!(line.contains("next turn start or tool call"), "{line}");
        assert!(!line.contains("never be delivered"), "{line}");
        assert!(!line.contains("PAST"), "{line}");
        for detail in ["directed supervisor message burst debounce", "directed supervisor message held by sender-pair wake cap"] {
            let policy=pending_deadline_narrative(DeliveryStage::Gated,Some(cas_store::PendingReason::AwaitingBusyRecipient),Some(detail),Some(1200),InboxHandoffState::NotHeld);
            assert!(policy.contains("supervisor wake policy"),"{policy}");
            assert!(!policy.contains("busy recipient"),"a policy hold is not evidence the recipient works: {policy}");
        }

    }

    #[test]
    fn a_delivered_row_adds_no_deadline_noise() {
        assert!(
            pending_deadline_narrative(
                DeliveryStage::Delivered,
                None,
                None,
                Some(30),
                InboxHandoffState::NotHeld
            )
            .is_empty()
        );
    }
}


#[cfg(test)]
mod cross_session_routing_tests {
    use super::{row_factory_session, row_source};
    use cas_types::{Agent, AgentRole};

    fn agent_in(name: &str, session: Option<&str>) -> Agent {
        let mut agent = Agent::new(format!("id-{name}"), name.to_string());
        agent.factory_session = session.map(str::to_owned);
        agent
    }

    fn supervisor_in(name: &str, session: Option<&str>) -> Agent {
        let mut agent = agent_in(name, session);
        agent.role = AgentRole::Supervisor;
        agent
    }

    /// cas-5087: the wake gate resolves `source` against the roster BY NAME,
    /// and no live supervisor is named "supervisor" — so a collapsed source
    /// made the cas-15f2 peer wake unreachable in production. Both ends
    /// supervisors: name the sender.
    #[test]
    fn a_supervisor_to_supervisor_row_names_the_sending_supervisor() {
        let sender = supervisor_in("young-raven-93", Some("cas-src-young-raven-93"));
        let target = supervisor_in("noble-lynx-44", Some("cas-src-vivid-sparrow-8"));

        assert_eq!(
            row_source("supervisor", Some(&sender), Some(&target)),
            "young-raven-93",
            "a row no daemon can attribute to a registered supervisor cannot wake a pane"
        );
    }

    /// Worker-facing rows keep reading "from supervisor": a worker has exactly
    /// one, and that string is its chain of command.
    #[test]
    fn a_worker_facing_row_still_reads_as_from_supervisor() {
        let sender = supervisor_in("young-raven-93", Some("cas-src-young-raven-93"));
        let worker = agent_in("swift-fox", Some("cas-src-young-raven-93"));

        assert_eq!(
            row_source("supervisor", Some(&sender), Some(&worker)),
            "supervisor"
        );
        assert_eq!(row_source("supervisor", Some(&sender), None), "supervisor");
        assert_eq!(
            row_source("swift-fox", Some(&worker), Some(&sender)),
            "swift-fox",
            "a worker's own name is already its display name"
        );
    }

    /// The cas-15f2 regression: supervisor A in session A messages supervisor B
    /// in session B. The row must belong to B, or no daemon ever selects it.
    #[test]
    fn a_message_to_an_agent_in_another_session_is_stamped_with_the_recipients_session() {
        let target = agent_in("noble-lynx-44", Some("cas-src-vivid-sparrow-8"));

        let session = row_factory_session(
            "noble-lynx-44",
            Some(&target),
            Some("cas-src-young-raven-93"),
        );

        assert_eq!(session.as_deref(), Some("cas-src-vivid-sparrow-8"));
    }

    #[test]
    fn a_same_session_message_is_unchanged() {
        let target = agent_in("daring-marten-11", Some("cas-src-young-raven-93"));

        let session = row_factory_session(
            "daring-marten-11",
            Some(&target),
            Some("cas-src-young-raven-93"),
        );

        assert_eq!(session.as_deref(), Some("cas-src-young-raven-93"));
    }

    /// `all_workers` means "this factory's workers". Redirecting it to some
    /// other session's roster would be a broadcast into a factory the caller
    /// does not run.
    #[test]
    fn logical_fanout_names_stay_scoped_to_the_sender() {
        for name in ["all_workers", "supervisor", "director", "ALL_WORKERS"] {
            let stray = agent_in(name, Some("cas-src-other-session"));
            let session = row_factory_session(name, Some(&stray), Some("cas-src-mine"));
            assert_eq!(
                session.as_deref(),
                Some("cas-src-mine"),
                "{name} must not redirect the stamp"
            );
        }
    }

    #[test]
    fn an_unregistered_or_sessionless_target_keeps_the_senders_session() {
        assert_eq!(
            row_factory_session("ghost", None, Some("cas-src-mine")).as_deref(),
            Some("cas-src-mine")
        );
        let legacy = agent_in("legacy", None);
        assert_eq!(
            row_factory_session("legacy", Some(&legacy), Some("cas-src-mine")).as_deref(),
            Some("cas-src-mine")
        );
        let blank = agent_in("blank", Some("   "));
        assert_eq!(
            row_factory_session("blank", Some(&blank), Some("cas-src-mine")).as_deref(),
            Some("cas-src-mine")
        );
    }

    #[test]
    fn a_sessionless_sender_still_routes_to_the_recipients_session() {
        let target = agent_in("noble-lynx-44", Some("cas-src-vivid-sparrow-8"));
        assert_eq!(
            row_factory_session("noble-lynx-44", Some(&target), None).as_deref(),
            Some("cas-src-vivid-sparrow-8")
        );
    }
}

#[cfg(test)]
mod inbox_poll_identity_tests {
    use super::{
        enrich_report_from_harness_artifact, interrupt_unconfirmed_message,
        lifecycle_relay_is_stale_at_inbox_pop, recipient_transport_warning,
        withhold_lifecycle_at_inbox_pop,
        extract_pull_request_number, resolve_inbox_recipient,
    };
    use cas_store::DeliveryStage;

    #[test]
    fn claude_interrupt_timeout_is_an_explicit_failure_surface() {
        let message = interrupt_unconfirmed_message(269, None);
        assert!(message.contains("Could not confirm Claude interrupt delivery"));
        assert!(message.contains("notification 269"));
        assert!(message.contains("did not silently claim"));
        assert!(message.contains("message_status notification_id=269"));
    }

    /// cas-7a01 (GH #155): the combination that used to be completely
    /// invisible — Cassy nudged the pane and no turn ever carried the message —
    /// must be named, not left for an operator to infer from two fields.
    #[test]
    fn a_fired_nudge_with_no_surfacing_is_called_out() {
        let line = super::wake_attempt_narrative(
            cas_store::WakeAttempt::Fired,
            cas_store::ObservationStatus::Unobserved,
        );
        assert!(line.contains("DID nudge"), "{line}");
        assert!(line.contains("#155"), "{line}");
    }

    /// cas-badb (GH #1302): the sender sees received or pending. A pane
    /// handoff is still pending; the recipient's own session seeing the
    /// message is received.
    #[test]
    fn message_status_tells_the_sender_received_or_pending_cas_badb() {
        use cas_store::{PromptQueueStore, SurfacingSource};
        let temp = tempfile::tempdir().unwrap();
        let queue = cas_store::SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        let id = queue.enqueue_with_session("worker-a", "zen-hound", "please review", "session").unwrap();
        let verdict = |now| {
            let report = queue.message_delivery_report(id).unwrap().unwrap();
            super::recipient_verdict(&report, now, 300)
        };
        let now = chrono::Utc::now();
        let pending = verdict(now);
        assert!(pending.starts_with("recipient: PENDING for zen-hound"), "{pending}");
        assert!(pending.contains("told at 300s"), "{pending}");
        assert!(
            verdict(now + chrono::Duration::seconds(600)).contains("have been told"),
            "after the escalation threshold the sender knows the alert went out"
        );
        queue.record_recipient_surfaced(id, "zen-hound", SurfacingSource::TransportDelivered).unwrap();
        assert!(verdict(now).starts_with("recipient: PENDING"), "a pane handoff is not a read");
        queue.record_recipient_surfaced(id, "zen-hound", SurfacingSource::HookSurfaced).unwrap();
        let received = verdict(now);
        assert!(received.starts_with("recipient: RECEIVED by zen-hound"), "{received}");
    }

    /// cas-ad92: status must tell a body rendered into a turn from one the
    /// transport only claimed.
    #[test]
    fn recipient_receipt_separates_rendered_from_claimed() {
        use cas_store::SurfacingSource;
        let rendered = super::recipient_receipt_narrative(Some(SurfacingSource::HookSurfaced), None);
        let claimed = super::recipient_receipt_narrative(Some(SurfacingSource::TransportClaimed), None);
        assert!(rendered.contains("rendered —"), "{rendered}");
        assert!(claimed.contains("claimed, not rendered"), "{claimed}");
        assert!(super::recipient_receipt_narrative(None, None).is_empty());
        // cas-6ccb: a settled poll receipt says why, and that it is no ack.
        let settled = super::recipient_receipt_narrative(
            Some(SurfacingSource::InboxPoll),
            Some("withheld by inbox_poll: merge request for cas-1, already done"),
        );
        assert!(settled.starts_with("recipient_receipt: withheld by inbox_poll:"), "{settled}");
        assert!(settled.contains("not an acknowledgement"), "{settled}");
    }

    /// The three wake-attempt states must read differently. If two of them
    /// render the same sentence, the split this task exists to create is
    /// cosmetic.
    #[test]
    fn every_wake_attempt_state_reads_differently() {
        use cas_store::{ObservationStatus, WakeAttempt};
        let lines: Vec<String> = [
            WakeAttempt::Fired,
            WakeAttempt::Failed,
            WakeAttempt::NotAttempted,
        ]
        .into_iter()
        .map(|a| super::wake_attempt_narrative(a, ObservationStatus::Unobserved))
        .collect();
        let mut unique = lines.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            3,
            "wake states collapse to the same text: {lines:?}"
        );
    }

    /// A confirmed surfacing outranks whatever the nudge did: the message
    /// demonstrably reached a turn.
    #[test]
    fn an_observed_wake_reports_the_surfacing_regardless_of_the_nudge() {
        use cas_store::{ObservationStatus, WakeAttempt};
        for attempt in [
            WakeAttempt::Fired,
            WakeAttempt::Failed,
            WakeAttempt::NotAttempted,
        ] {
            let line = super::wake_attempt_narrative(attempt, ObservationStatus::Observed);
            assert!(line.contains("injected into a turn"), "{attempt}: {line}");
        }
    }

    /// cas-ac7e (GH #130): the 7183 shape — delivered per the writer, with
    /// nothing on the recipient's side to corroborate it — must be called out.
    #[test]
    fn a_delivered_row_without_a_recipient_stamp_warns() {
        let warning = recipient_transport_warning(DeliveryStage::Delivered, "fast-cobra-90", false);
        assert!(
            warning.is_some_and(|w| w.contains("MISSING")),
            "silently reporting stage=delivered here is exactly the blind spot #130 reported"
        );
    }

    #[test]
    fn a_corroborated_delivery_does_not_warn() {
        assert_eq!(
            recipient_transport_warning(DeliveryStage::Delivered, "fast-cobra-90", true),
            None
        );
    }

    #[test]
    fn a_broadcast_is_exempt_from_the_recipient_stamp_warning() {
        assert_eq!(
            recipient_transport_warning(DeliveryStage::Delivered, "all_workers", false),
            None,
            "a broadcast's per-recipient transport is its broadcast counts; warning here              would fire on every healthy broadcast and train operators to ignore it"
        );
    }

    #[test]
    fn a_row_that_never_reached_delivered_does_not_warn() {
        for stage in [
            DeliveryStage::Enqueued,
            DeliveryStage::Selected,
            DeliveryStage::Gated,
            DeliveryStage::Confirmed,
        ] {
            assert_eq!(
                recipient_transport_warning(stage, "worker-1", false),
                None,
                "{stage} has made no delivery claim to contradict"
            );
        }
    }

    #[test]
    fn registered_identity_precedes_environment_fallback() {
        assert_eq!(
            resolve_inbox_recipient(
                Some("registered-worker".to_string()),
                Some("env-worker".to_string()),
            ),
            Some("registered-worker".to_string())
        );
    }

    #[test]
    fn environment_identity_is_used_when_registration_is_unavailable() {
        assert_eq!(
            resolve_inbox_recipient(None, Some("env-worker".to_string())),
            Some("env-worker".to_string())
        );
        assert_eq!(resolve_inbox_recipient(None, Some("  ".to_string())), None);
    }

    #[test]
    fn merge_request_pr_number_extraction_accepts_urls_and_labels() {
        assert_eq!(
            extract_pull_request_number("queued https://github.test/org/repo/pull/932"),
            Some(932)
        );
        assert_eq!(extract_pull_request_number("PR #933 merged"), Some(933));
        assert_eq!(extract_pull_request_number("no pull request here"), None);
    }

    /// cas-8cbe: a compacted progress relay's card names `inbox_poll
    /// notification_id=N`; that named read shows the event even after the
    /// task moved on. A parked relay that went stale stays withheld.
    #[test]
    fn a_named_progress_relay_is_shown_after_its_task_moves_on() {
        let relay = |source: &str, transition: &str, new: &str| cas_store::QueuedPrompt {
            id: 7,
            source: source.to_string(),
            target: "supervisor".to_string(),
            prompt: format!(
                "<task-lifecycle transition=\"{transition}\" task_id=\"cas-f9b7\" old=\"open\" \
                 new=\"{new}\" actor=\"w\" notification_id=\"1\" \
                 occurrence=\"2026-10-17T15:17:19Z\">\nTask cas-f9b7 — t\n</task-lifecycle>"
            ),
            created_at: chrono::Utc::now(),
            processed_at: None,
            factory_session: None,
            summary: None,
            priority: cas_store::NotificationPriority::Normal,
            acked_at: None,
            urgent: false,
            origin: Some(cas_store::QueueOrigin::Daemon),
            operator: None,
            recipient_device_id: None,
            kind: None,
            attachments: Vec::new(),
        };
        let mut task = cas_types::Task::new("cas-f9b7".into(), "t".into());
        task.status = cas_types::TaskStatus::Closed;
        let started = relay("lifecycle:1", "task_started", "in_progress");
        let parked = relay("lifecycle-wake:2", "task_awaiting_merge", "awaiting_merge");
        assert!(withhold_lifecycle_at_inbox_pop(&started, false, Some(&task)));
        assert!(!withhold_lifecycle_at_inbox_pop(&started, true, Some(&task)));
        assert!(withhold_lifecycle_at_inbox_pop(&parked, true, Some(&task)));
    }

    #[test]
    fn unreadable_task_never_withholds_a_lifecycle_relay_at_inbox_pop() {
        let prompt = "<task-lifecycle transition=\"task_awaiting_merge\" task_id=\"cas-live\" old=\"in_progress\" new=\"awaiting_merge\" actor=\"worker\" notification_id=\"1\" occurrence=\"2026-08-14T14:00:00Z\">\nMERGE REQUIRED\n</task-lifecycle>";
        assert!(
            !lifecycle_relay_is_stale_at_inbox_pop(prompt, None),
            "a read failure is uncertainty, not evidence that a live relay is moot"
        );
    }

    #[test]
    fn message_report_is_enriched_only_by_target_codex_rollout_records() {
        use cas_store::ObservationStatus;

        let _lock = crate::hooks::test_env_lock();
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().join("project");
        std::fs::create_dir_all(&cas_root).unwrap();
        let codex_home = temp.path().join("codex-home");
        let sessions = codex_home.join("sessions/2099/01/01");
        std::fs::create_dir_all(&sessions).unwrap();
        // Exercise the live legacy-row shape: factory registration has no
        // clone_path metadata, so status resolves the convention worktree.
        let clone_path = cas_root.join("worktrees/worker-a");
        std::fs::create_dir_all(&clone_path).unwrap();
        let rollout = sessions.join("rollout-live-worker-session.jsonl");
        std::fs::write(
            &rollout,
            format!(
                concat!(
                    "{{\"timestamp\":\"2099-01-01T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"cwd\":{:?},\"originator\":\"codex-tui\",\"source\":\"cli\"}}}}\n",
                    "{{\"timestamp\":\"2099-01-01T00:00:02Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_started\",\"turn_id\":\"observed-turn\"}}}}\n",
                    "{{\"timestamp\":\"2099-01-01T00:00:02.500Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"Message from supervisor: act\"}}],\"internal_chat_message_metadata_passthrough\":{{\"turn_id\":\"observed-turn\"}}}}}}\n",
                    "{{\"timestamp\":\"2099-01-01T00:00:03Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"internal_chat_message_metadata_passthrough\":{{\"turn_id\":\"observed-turn\"}}}}}}\n"
                ),
                clone_path.display().to_string()
            ),
        )
        .unwrap();

        let old_codex_home = std::env::var("CODEX_HOME").ok();
        unsafe { std::env::set_var("CODEX_HOME", &codex_home) };

        let agent_store = crate::store::open_agent_store(&cas_root).unwrap();
        let mut agent =
            cas_types::Agent::new("live-worker-session".to_string(), "worker-a".to_string());
        agent.role = cas_types::AgentRole::Worker;
        agent.factory_session = Some("factory-1".to_string());
        agent
            .metadata
            .insert("worker_cli".to_string(), "codex".to_string());
        agent_store.register(&agent).unwrap();

        let queue = crate::store::open_prompt_queue_store(&cas_root).unwrap();
        let message_id = queue
            .enqueue_with_session("supervisor", "worker-a", "act", "factory-1")
            .unwrap();
        queue.mark_transport_delivered(message_id).unwrap();
        let mut report = queue.message_delivery_report(message_id).unwrap().unwrap();
        assert_eq!(report.wake, ObservationStatus::Unobserved);
        assert!(
            serde_json::to_value(&report)
                .unwrap()
                .get("prompt")
                .is_none()
        );

        enrich_report_from_harness_artifact(&cas_root, &mut report);

        unsafe {
            match old_codex_home {
                Some(value) => std::env::set_var("CODEX_HOME", value),
                None => std::env::remove_var("CODEX_HOME"),
            }
        }
        assert_eq!(report.wake, ObservationStatus::Observed);
        assert_eq!(report.reaction, ObservationStatus::Observed);
        assert!(report.wake_evidence.as_deref().is_some_and(|evidence| {
            evidence.contains("task_started") && evidence.contains("rollout-live-worker")
        }));
    }

    #[test]
    fn message_report_uses_opencode_mapping_for_wake_and_reaction() {
        use cas_mux::{
            OpenCodeSessionEvent, OpenCodeSessionEventKind, OpenCodeSessionState,
            persist_opencode_session_state,
        };
        use cas_store::ObservationStatus;

        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().join("project");
        std::fs::create_dir_all(&cas_root).unwrap();
        let clone_path = cas_root.join("worktrees/opencode-worker");
        std::fs::create_dir_all(&clone_path).unwrap();
        let session_id = "opencode-opencode-worker";
        let event_at = chrono::Utc::now().timestamp_millis().max(0) as u64;
        let mut state = OpenCodeSessionState::new(session_id, clone_path.display().to_string());
        state.apply(OpenCodeSessionEvent {
            at: event_at,
            kind: OpenCodeSessionEventKind::RootCreated {
                session_id: "ses_mapped-worker".to_string(),
                directory: clone_path.display().to_string(),
            },
        });
        state.apply(OpenCodeSessionEvent {
            at: event_at + 1,
            kind: OpenCodeSessionEventKind::ToolBefore {
                session_id: "ses_mapped-worker".to_string(),
                name: "bash".to_string(),
                call_id: Some("call-1".to_string()),
            },
        });
        state.apply(OpenCodeSessionEvent {
            at: event_at + 2,
            kind: OpenCodeSessionEventKind::ToolAfter {
                session_id: "ses_mapped-worker".to_string(),
                call_id: Some("call-1".to_string()),
                success: true,
            },
        });
        persist_opencode_session_state(
            &cas_mux::opencode_session_state_path(&cas_root, session_id),
            &state,
        )
        .unwrap();

        let agent_store = crate::store::open_agent_store(&cas_root).unwrap();
        let mut agent =
            cas_types::Agent::new(session_id.to_string(), "opencode-worker".to_string());
        agent.role = cas_types::AgentRole::Worker;
        agent.factory_session = Some("factory-1".to_string());
        agent
            .metadata
            .insert("worker_cli".to_string(), "opencode".to_string());
        agent_store.register(&agent).unwrap();

        let queue = crate::store::open_prompt_queue_store(&cas_root).unwrap();
        let message_id = queue
            .enqueue_with_session("supervisor", "opencode-worker", "act", "factory-1")
            .unwrap();
        queue.mark_transport_delivered(message_id).unwrap();
        let mut report = queue.message_delivery_report(message_id).unwrap().unwrap();
        report.delivered_at = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
            event_at.saturating_sub(1_000) as i64,
        );

        enrich_report_from_harness_artifact(&cas_root, &mut report);

        assert_eq!(report.wake, ObservationStatus::Observed);
        assert_eq!(report.reaction, ObservationStatus::Observed);
        assert!(
            report
                .wake_evidence
                .as_deref()
                .is_some_and(|evidence| evidence.contains("ses_mapped-worker"))
        );
        assert!(
            report
                .reaction_evidence
                .as_deref()
                .is_some_and(|evidence| evidence.contains("completed tool attribution"))
        );
        assert_eq!(report.wake_observed_at, report.reaction_observed_at);
    }
}

#[cfg(test)]
mod cas99d2_redelivery_tests {
    use super::{
        INBOX_REDELIVERY_MARKER, INBOX_REPLAY_AFTER_SECS, InboxRedelivery, inbox_redelivery_decision,
        inbox_row_is_stale_replay,
    };
    use crate::prompt_revalidation::assignment_solicited_task_id;

    /// The literal text of notification 7112 (supervisor hand-written dispatch).
    #[test]
    fn the_real_7112_assignment_names_its_solicited_task() {
        let prompt = "You are assigned task cas-7587 (P2 bug, epic cas-b0c7). Run \
                      `mcp__cas__task action=show id=cas-7587` then \
                      `mcp__cas__task action=start id=cas-7587`.\n\nScope: GH #122 …";
        assert_eq!(
            assignment_solicited_task_id(prompt).as_deref(),
            Some("cas-7587")
        );
    }

    /// The director's generated assignment prompt shape.
    #[test]
    fn the_generated_assignment_prompt_names_its_solicited_task() {
        let prompt = "You have been assigned a new task:\n\
                      Task ID: cas-99d2\n\
                      Title: Message confirmation truth\n\n\
                      Start working: mcp__cas__task action=start id=cas-99d2";
        assert_eq!(
            assignment_solicited_task_id(prompt).as_deref(),
            Some("cas-99d2")
        );
    }

    /// A message that merely mentions a task id is not an assignment. Treating
    /// it as one would let unrelated task progress silence real instructions.
    #[test]
    fn non_assignment_messages_solicit_nothing() {
        for prompt in [
            "factory/fierce-crow-25 is merged into the epic branch. Re-run close for cas-bcfb.",
            "status on cas-7587? your last note was 20 minutes ago",
            "Reviewed the diff for cas-7587 myself — nice work.",
            "stand down and shut down cleanly",
        ] {
            assert_eq!(
                assignment_solicited_task_id(prompt),
                None,
                "must not be read as an assignment: {prompt}"
            );
        }
    }

    /// An assignment phrase with no task-shaped id yields nothing rather than
    /// a bogus id that would be looked up and (not) found.
    #[test]
    fn an_assignment_without_a_task_id_solicits_nothing() {
        assert_eq!(
            assignment_solicited_task_id("You have been assigned a new task, details to follow"),
            None
        );
    }

    #[test]
    fn redelivery_decision_covers_the_three_cases() {
        assert_eq!(
            inbox_redelivery_decision(false, false),
            InboxRedelivery::FirstDelivery
        );
        assert_eq!(
            inbox_redelivery_decision(true, false),
            InboxRedelivery::MarkRedelivery,
            "a repeat with no consumption evidence is still delivered, but marked"
        );
        assert_eq!(
            inbox_redelivery_decision(true, true),
            InboxRedelivery::WithholdConsumed,
            "GH #127: delivered once AND the solicited action happened"
        );
        assert_eq!(
            inbox_redelivery_decision(false, true),
            InboxRedelivery::FirstDelivery,
            "a row never handed to a transport must be delivered even if the task \
             happens to have moved — the recipient has provably not seen this text"
        );
    }

    #[test]
    fn a_delivery_older_than_the_replay_window_is_listed_not_replayed_cas_098d() {
        let now = chrono::Utc::now();
        let long_ago = Some(now - chrono::Duration::minutes(40));
        assert!(
            !inbox_row_is_stale_replay(None, true, now),
            "never delivered is new mail"
        );
        assert!(
            !inbox_row_is_stale_replay(Some(now - chrono::Duration::seconds(4)), true, now),
            "the delivery a wake just named is handed over in full"
        );
        assert!(
            !inbox_row_is_stale_replay(
                Some(now - chrono::Duration::seconds(INBOX_REPLAY_AFTER_SECS)),
                true,
                now
            ),
            "the window's edge still counts as recent"
        );
        assert!(inbox_row_is_stale_replay(long_ago, true, now));
        assert!(
            !inbox_row_is_stale_replay(long_ago, false, now),
            "an old delivery the recipient never demonstrably had may be a dropped one (GH #155): replay it"
        );
    }

    /// The marker must match the token the daemon's teams-inbox writer already
    /// recognises as an intentional redelivery, so the two channels agree.
    #[test]
    // pin: The inbox producer and teams redelivery consumer exchange this exact wire token.
    fn the_marker_matches_the_teams_inbox_redelivery_token() {
        assert_eq!(INBOX_REDELIVERY_MARKER, "[redelivery]");
    }
}

/// Regression coverage for GH #328. A task can remain AwaitingMerge briefly
/// after its branch lands, precisely when a worker may need supervisor help to
/// escape a rejected close gate. Only a typed merge request may be withheld in
/// that state; the same worker's ordinary escalation must still be queued.
#[cfg(test)]
mod cas_89e1_post_merge_message_type_tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use cas_types::{Agent, AgentRole, Task, TaskStatus, WorkTarget};
    use rmcp::model::RawContent;
    use std::path::Path;
    use std::process::Command;

    #[tokio::test(flavor = "current_thread")]
    async fn cas_f17b_successor_reads_abandoned_content() {
        use crate::ui::factory::daemon::runtime::queue_and_events::read_row_facts;
        use cas_store::{HOOK_SURFACE_BUDGET_CHARS, HookDrain, QueueOrigin};
        let mut env = TestEnvGuard::temp_home();
        env.set("CAS_FACTORY_SESSION", "new-factory");
        env.set("CAS_AGENT_NAME", "zealous-tiger");
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        let agents = crate::store::open_agent_store(&root).unwrap();
        let queue = crate::store::open_prompt_queue_store(&root).unwrap();
        let tasks = crate::store::open_task_store_local(&root).unwrap();
        let mut old_task = Task::new("cas-0f17".into(), "Old assignment".into());
        old_task.status = TaskStatus::Closed;
        tasks.add(&old_task).unwrap();
        let mut sender = Agent::new("original-sender".into(), "vivid-dolphin".into());
        sender.role = AgentRole::Supervisor;
        sender.factory_session = Some("old-factory".into());
        agents.register(&sender).unwrap();
        let bodies = [
            "You are assigned task cas-0f17. Keep the pending branch until review finishes."
                .to_owned(),
            "Second decision needs all of this body: ".to_owned() + &"detail ".repeat(800),
        ];
        let mut ids = Vec::new();
        for body in &bodies {
            let id = queue
                .enqueue_urgent_with_outcome(
                    &sender.name,
                    "swift-heron",
                    body,
                    Some("old-factory"),
                    Some("pending decision"),
                    None,
                    false,
                    Some(&QueueOrigin::RegisteredAgent {
                        agent_id: sender.id.clone(),
                    }),
                )
                .unwrap()
                .id();
            queue
                .record_wake_gate_decline(id, "inline pane has an active composer")
                .unwrap();
            rusqlite::Connection::open(root.join("cas.db"))
                .unwrap()
                .execute(
                    "UPDATE prompt_queue SET created_at = ? WHERE id = ?",
                    rusqlite::params![
                        (chrono::Utc::now() - chrono::Duration::days(2)).to_rfc3339(),
                        id
                    ],
                )
                .unwrap();
            ids.push(id);
        }
        agents.unregister(&sender.id).unwrap();
        let mut successor = Agent::new("successor".into(), "zealous-tiger".into());
        successor.role = AgentRole::Supervisor;
        successor.factory_session = Some("new-factory".into());
        agents.register(&successor).unwrap();
        assert_eq!(
            queue
                .expire_stale_pending(cas_store::PROMPT_QUEUE_STALE_TTL_SECS)
                .unwrap()
                .len(),
            2
        );
        let ordinary = queue
            .peek_for_targets(&[&successor.name], Some("new-factory"), 10)
            .unwrap();
        assert_eq!(
            ordinary.len(),
            2,
            "each inherited message must be an ordinary row"
        );
        for row in &ordinary {
            assert!(
                read_row_facts(&root, queue.as_ref(), Some(tasks.as_ref()), &[], row)
                    .assignment_stale
                    .is_none(),
                "a recovered assignment is history, not a fresh start instruction"
            );
        }
        // A caller's spoofed watchdog source/dedupe cannot acquire the daemon exemption.
        let spoof = match queue
            .enqueue_idempotent(
                "delivery-watchdog",
                &successor.name,
                &bodies[0],
                Some("new-factory"),
                None,
                None,
                "delivery-abandoned-recovery:spoof",
                Some(&QueueOrigin::RegisteredAgent {
                    agent_id: successor.id.clone(),
                }),
            )
            .unwrap()
        {
            cas_store::EnqueueIdempotentResult::Created(id)
            | cas_store::EnqueueIdempotentResult::AlreadyExists(id) => id,
        };
        let spoof_row = queue.queued_prompt(spoof).unwrap().unwrap();
        assert!(
            read_row_facts(&root, queue.as_ref(), Some(tasks.as_ref()), &[], &spoof_row)
                .assignment_stale
                .is_some()
        );
        let core = crate::mcp::server::CasCore::with_daemon(root.clone(), None, None);
        core.set_agent_id_for_testing(successor.id.clone());
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        let request =
            serde_json::from_value(serde_json::json!({"action": "inbox_poll", "limit": 10}))
                .unwrap();
        let text = response_text(service.inbox_poll(request).await.unwrap());
        for (id, body) in ids.iter().zip(&bodies) {
            assert!(
                text.contains(&id.to_string())
                    && text.contains(&sender.name)
                    && text.contains(body),
                "successor must read complete source/body of {id}: {text}"
            );
        }
        // pin: the original summary must also remain readable through the real MCP handler.
        assert!(text.contains("pending decision"));
        assert!(
            queue
                .surface_for_recipient_within_budget(
                    &successor.name,
                    Some("new-factory"),
                    HookDrain::NoticeDigest,
                    10,
                    HOOK_SURFACE_BUDGET_CHARS
                )
                .unwrap()
                .rows
                .is_empty()
        );
    }

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("git command");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn response_text(result: CallToolResult) -> String {
        result
            .content
            .into_iter()
            .filter_map(|content| match content.raw {
                RawContent::Text(text) => Some(text.text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn assert_queued_receipt(response: &str) {
        // pin: ordinary successful sends expose one compact id, target, and current stage.
        assert!(response.starts_with("queued #"), "{response}");
        assert_eq!(response.lines().count(), 1, "{response}");
    }

    fn message_request(merge_request: bool) -> AgentRequest {
        serde_json::from_value(serde_json::json!({
            "action": "message",
            "target": "supervisor",
            "task_id": "cas-89e1",
            "merge_request": merge_request,
            "summary": if merge_request { "ready to merge" } else { "close gate rejected" },
            "message": if merge_request {
                "Fresh worker delivery; please merge the branch."
            } else {
                "My close was rejected after the merge landed; please help unblock the gate."
            },
        }))
        .expect("static message request")
    }

    fn message_request_to(target: &str) -> AgentRequest {
        serde_json::from_value(serde_json::json!({
            "action": "message",
            "target": target,
            "summary": "shared-clone routing regression",
            "message": "Route this message to the intended live supervisor.",
        }))
        .expect("static routing message request")
    }

    /// cas-66c9: a card's explicit read-in-full command must reach a retained
    /// row even when the ordinary inbox TTL has elapsed; the same poll must
    /// still keep other stale rows out of automatic delivery.
    #[tokio::test(flavor = "current_thread")]
    async fn named_inbox_read_survives_stale_ttl_without_auto_surfacing_stale_rows() {
        let mut env = TestEnvGuard::temp_home();
        env.set("CAS_AGENT_ROLE", "supervisor");
        env.set("CAS_FACTORY_SESSION", "factory-a");
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        let agents = core.open_agent_store().expect("agent store");
        let mut supervisor = Agent::new("supervisor-id".into(), "supervisor".into());
        supervisor.role = AgentRole::Supervisor;
        supervisor.factory_session = Some("factory-a".into());
        agents.register(&supervisor).expect("register supervisor");
        core.set_agent_id_for_testing(supervisor.id.clone());

        let queue = crate::store::open_prompt_queue_store(&cas_root).expect("prompt queue");
        let full_body = "retained merge decision details";
        let named = queue
            .enqueue_with_session("worker-a", "supervisor", full_body, "factory-a")
            .expect("enqueue named row");
        let automatic_body = "stale row must stay out of automatic delivery";
        let automatic = queue
            .enqueue_with_session("worker-b", "supervisor", automatic_body, "factory-a")
            .expect("enqueue automatic control row");
        rusqlite::Connection::open(cas_root.join("cas.db"))
            .expect("database")
            .execute(
                "UPDATE prompt_queue SET created_at = ?1 WHERE id IN (?2, ?3)",
                rusqlite::params![
                    (chrono::Utc::now() - chrono::Duration::hours(25)).to_rfc3339(),
                    named,
                    automatic,
                ],
            )
            .expect("age both rows past the stale TTL");
        let quarantined = queue
            .expire_stale_pending(cas_store::PROMPT_QUEUE_STALE_TTL_SECS)
            .expect("simulate the daemon stale sweep");
        assert_eq!(
            quarantined.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![named, automatic],
            "the daemon has already retained and quarantined both stale rows"
        );

        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        let response = response_text(
            service
                .inbox_poll(
                    serde_json::from_value(serde_json::json!({
                        "action": "inbox_poll",
                        "notification_id": named,
                    }))
                    .unwrap(),
                )
                .await
                .expect("named read succeeds"),
        );
        assert!(response.contains(full_body), "{response}");
        assert!(!response.contains(automatic_body), "{response}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn post_merge_suppression_requires_the_explicit_merge_request_type() {
        let mut env = TestEnvGuard::temp_home();
        crate::store::known_repos::ensure_host_schema().expect("host repo schema");
        let host_collision = tempfile::tempdir().expect("host collision repo");
        let collision_repo = host_collision.path();
        git(collision_repo, &["init", "-q", "-b", "main"]);
        std::fs::create_dir(collision_repo.join(".cas")).expect("collision Cassy directory");
        std::fs::write(
            collision_repo.join(".cas/config.toml"),
            "[project]\ncanonical_id = \"cas-89e1-message-test\"\n",
        )
        .expect("collision Cassy config");
        crate::store::known_repos::register_repo_strict(collision_repo)
            .expect("register host collision");

        let project = tempfile::tempdir().expect("temporary project");
        let repo = project.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.email", "cas-test@example.invalid"]);
        git(repo, &["config", "user.name", "Cassy Test"]);
        std::fs::create_dir(repo.join(".cas")).expect("Cassy directory");
        std::fs::write(
            repo.join(".cas/config.toml"),
            "[project]\ncanonical_id = \"cas-89e1-message-test\"\n",
        )
        .expect("Cassy config");
        std::fs::write(repo.join("base.txt"), "base\n").expect("base file");
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "base"]);
        git(repo, &["branch", "fresh-base", "main"]);
        git(repo, &["checkout", "-qb", "factory/worker-a"]);
        std::fs::write(repo.join("delivery.txt"), "delivery\n").expect("delivery file");
        git(repo, &["add", "delivery.txt"]);
        git(repo, &["commit", "-qm", "delivery"]);
        let parked_anchor = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(repo)
                .output()
                .expect("parked anchor")
                .stdout,
        )
        .expect("anchor sha")
        .trim()
        .to_string();
        git(repo, &["checkout", "-q", "main"]);
        git(
            repo,
            &["merge", "--no-ff", "factory/worker-a", "-m", "merge"],
        );

        let cas_root = repo.join(".cas");
        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        let agents = core.open_agent_store().expect("agent store");
        let mut worker = Agent::new("worker-id".to_string(), "worker-a".to_string());
        worker.role = AgentRole::Worker;
        agents.register(&worker).expect("register worker");
        let mut supervisor = Agent::new("supervisor-id".to_string(), "supervisor".to_string());
        supervisor.role = AgentRole::Supervisor;
        agents.register(&supervisor).expect("register supervisor");
        core.set_agent_id_for_testing(worker.id.clone());

        let tasks = core.open_task_store().expect("task store");
        let mut task = Task::new(
            "cas-89e1".to_string(),
            "post-merge message type".to_string(),
        );
        task.status = TaskStatus::AwaitingMerge;
        task.assignee = Some(worker.name.clone());
        task.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:cas-89e1-message-test".to_string(),
            target_branch: "main".to_string(),
        });
        task.deliverables.parked_branch = Some("factory/worker-a".to_string());
        task.deliverables.factory_branch_anchor = Some(parked_anchor.clone());
        tasks.add(&task).expect("add parked task");

        // The fixture starts hermetic even when the test binary was launched
        // by a supervisor. Re-introduce a conflicting ambient role only after
        // registration to prove the persisted worker role wins at the MCP
        // message boundary.
        assert!(std::env::var_os("CAS_AGENT_ROLE").is_none());
        env.set("CAS_AGENT_ROLE", "supervisor");

        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core.clone(), None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core.clone());

        let escalation = response_text(
            service
                .message_send(message_request(false))
                .await
                .expect("ordinary escalation is delivered"),
        );
        assert!(
            !escalation.contains("Merge already landed"),
            "an untyped escalation must not be suppressed: {escalation}"
        );
        let queue = crate::store::open_prompt_queue_store(&cas_root).expect("prompt queue");
        let queued = queue.poll_all(10).expect("queued escalation");
        assert_eq!(queued.len(), 1, "ordinary escalation must reach the queue");
        let report = queue
            .message_delivery_report(queued[0].id)
            .expect("read send stage")
            .expect("send receipt row");
        // pin: ordinary sends return only their current queue id, target, and stage.
        assert_eq!(
            escalation,
            format!("queued #{} → {} ({})", report.id, report.target, report.stage)
        );
        assert!(
            crate::prompt_revalidation::parse_merge_request_envelope(&queued[0].prompt).is_none(),
            "ordinary escalation must not acquire a merge envelope: {:?}",
            queued[0].prompt
        );

        let stale_merge = response_text(
            service
                .message_send(message_request(true))
                .await
                .expect("stale merge request receives guidance"),
        );
        assert!(
            stale_merge.contains("Merge already landed"),
            "an explicitly typed stale merge request must still be suppressed: {stale_merge}"
        );

        // The worker starts task B on the reused factory branch, now based on
        // unrelated main work. A's immutable delivery is still the old SHA.
        let mut task_b = Task::new("cas-task-b".to_string(), "next task".to_string());
        task_b.status = TaskStatus::InProgress;
        task_b.assignee = Some(worker.name.clone());
        tasks.add(&task_b).expect("add active task B");
        git(repo, &["checkout", "-q", "factory/worker-a"]);
        git(repo, &["reset", "--hard", "fresh-base"]);
        std::fs::write(repo.join("task-b.txt"), "unrelated task B\n").unwrap();
        git(repo, &["add", "task-b.txt"]);
        git(repo, &["commit", "-qm", "task B"]);

        let parked_request = response_text(
            service
                .message_send(message_request(true))
                .await
                .expect("parked A request is checked against A's anchor"),
        );
        assert!(
            parked_request.contains("Merge already landed"),
            "task B's tip must not become A's merge request: {parked_request}"
        );
        assert_eq!(
            tasks.get(&task.id).unwrap().deliverables.factory_branch_anchor,
            Some(parked_anchor)
        );
        let mut implicit_request = message_request(true);
        implicit_request.task_id = None;
        let implicit_response = response_text(
            service
                .message_send(implicit_request)
                .await
                .expect("implicit request with active B receives guidance"),
        );
        assert!(
            implicit_response.contains("No merge request was queued"),
            "implicit request must not target parked A: {implicit_response}"
        );

        // Real MCP -> Git/store -> queue boundary: recorded review metadata
        // is human text beside the unchanged immutable merge envelope.
        task_b.status = TaskStatus::AwaitingMerge;
        task_b.risk = vec![cas_types::TaskRisk::Concurrency];
        task_b.door = Some(cas_types::TaskDoor::TwoWay);
        task_b.notes = "qa-bundle: /proof/task-b/qa/bundle.json".into();
        task_b.deliverables.work_target = task.deliverables.work_target.clone();
        task_b.deliverables.parked_branch = Some("factory/worker-a".into());
        tasks.update(&task_b).expect("park B with metadata");
        let mut request = message_request(true);
        request.task_id = Some(task_b.id.clone());
        let response = response_text(service.message_send(request).await.expect("fresh B request"));
        // pin: this request must return its queued state once the worker asks for merge.
        assert_queued_receipt(&response);
        let rows = crate::store::open_prompt_queue_store(&cas_root).unwrap().poll_all(10).unwrap();
        let (row, envelope) = rows.iter().find_map(|row| {
            let envelope = crate::prompt_revalidation::parse_merge_request_envelope(&row.prompt)?;
            (envelope.task_id == task_b.id).then_some((row, envelope))
        }).expect("B's typed request reaches queue");
        assert_eq!(envelope.target_branch, "main");
        // pin: the supervisor sees the exact merge-request disposition in the queued row.
        assert!(row.prompt.contains("Fresh worker delivery; please merge the branch."));
        // pin: merge requests preserve the task risk label in their context.
        assert!(row.prompt.contains("**Risk:** concurrency"));
        // pin: merge requests preserve the task lifecycle gate in their context.
        assert!(row.prompt.contains("**Door:** two-way"));
        // pin: exact QA bundle paths remain attached to the merge request.
        assert!(row.prompt.contains("/proof/task-b/qa/bundle.json"));
        // pin: file names from the task diff remain byte-preserved in the request.
        assert!(row.prompt.contains("A\ttask-b.txt"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unreviewed_reopen_merge_request_reaches_supervisor_cas_1cc7() {
        let mut env = TestEnvGuard::temp_home();
        env.set("CAS_AGENT_ROLE", "supervisor");
        crate::store::known_repos::ensure_host_schema().unwrap();
        let project = tempfile::tempdir().unwrap();
        let repo = project.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.email", "cas-test@example.invalid"]);
        git(repo, &["config", "user.name", "Cassy Test"]);
        let root = repo.join(".cas");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("config.toml"),
            "[project]\ncanonical_id = \"cas-unreviewed-message-test\"\n",
        )
        .unwrap();
        std::fs::write(repo.join("base.txt"), "base\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "base"]);
        git(
            repo,
            &["checkout", "-qb", "factory/worker-a-cas-unreviewed-message"],
        );
        std::fs::write(repo.join("delivery.txt"), "unreviewed delivery\n").unwrap();
        git(repo, &["add", "delivery.txt"]);
        git(
            repo,
            &["commit", "-qm", "fix(cas-unreviewed-message): delivery"],
        );
        let tip = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(repo)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        let core = crate::mcp::server::CasCore::with_daemon(root.clone(), None, None);
        let agents = core.open_agent_store().unwrap();
        agents.init().unwrap();
        agents
            .register(&Agent::new_with_role(
                "worker-id".into(),
                "worker-a".into(),
                AgentRole::Worker,
            ))
            .unwrap();
        agents
            .register(&Agent::new_with_role(
                "supervisor-id".into(),
                "supervisor".into(),
                AgentRole::Supervisor,
            ))
            .unwrap();
        core.set_agent_id_for_testing("supervisor-id".into());
        let tasks = core.open_task_store().unwrap();
        tasks.init().unwrap();
        let mut task = Task::new(
            "cas-unreviewed-message".into(),
            "Unreviewed premature close".into(),
        );
        task.status = TaskStatus::Closed;
        task.assignee = Some("worker-a".into());
        task.deliverables.factory_branch_anchor = Some(tip.clone());
        task.deliverables.parked_branch = Some("factory/worker-a-cas-unreviewed-message".into());
        task.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:cas-unreviewed-message-test".into(),
            target_branch: "main".into(),
        });
        tasks.add(&task).unwrap();
        core.cas_task_reopen(Parameters(
            serde_json::from_value(
                serde_json::json!({"id":task.id,"reason":"Unmerged tip was never reviewed"}),
            )
            .unwrap(),
        ))
        .await
        .unwrap();
        let core = crate::mcp::server::CasCore::with_daemon(root.clone(), None, None);
        core.set_agent_id_for_testing("worker-id".into());
        env.set("CAS_AGENT_ROLE", "worker");
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core.clone(), None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core.clone());
        let mut request = message_request(true);
        request.task_id = Some(task.id.clone());
        let response = response_text(service.message_send(request).await.unwrap());
        assert_queued_receipt(&response);
        let rows = crate::store::open_prompt_queue_store(&root)
            .unwrap()
            .peek_all(20)
            .unwrap();
        let (row, envelope) = rows
            .iter()
            .find_map(|row| {
                crate::prompt_revalidation::parse_merge_request_envelope(&row.prompt)
                    .map(|envelope| (row, envelope))
            })
            .expect("typed merge request is durable");
        assert_eq!(row.target, "supervisor");
        assert_eq!(response, format!("queued #{} → supervisor (enqueued)", row.id));
        assert_eq!(envelope.branch_tip, tip);
        let reopened = tasks.get(&task.id).unwrap();
        let git = crate::prompt_revalidation::queued_merge_request_decision(
            repo,
            Some(&reopened),
            &envelope,
        );
        assert_eq!(
            crate::prompt_revalidation::merge_request_delivery_decision(
                Some(&reopened),
                &envelope,
                &git
            ),
            crate::prompt_revalidation::MergeRequestDelivery::Deliver
        );
        let inbox_core = crate::mcp::server::CasCore::with_daemon(root.clone(), None, None);
        inbox_core.set_agent_id_for_testing("supervisor-id".into());
        #[cfg(feature = "mcp-proxy")]
        let inbox_service = CasService::new(inbox_core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let inbox_service = CasService::new(inbox_core);
        let inbox = response_text(
            inbox_service
                .inbox_poll(
                    serde_json::from_value(serde_json::json!({"action":"inbox_poll"})).unwrap(),
                )
                .await
                .unwrap(),
        );
        assert!(inbox.contains("Fresh worker delivery"), "{inbox}");
        assert!(inbox.contains(&tip), "{inbox}");
    }

    /// cas-b3ab: a worker whose default `factory/<name>` branch already landed
    /// resumes a task on its per-task branch `factory/<name>-<task>` with
    /// unmerged commits. Its merge request is judged by that branch and
    /// queued, never suppressed as "already landed" by the other task's branch.
    #[tokio::test(flavor = "current_thread")]
    async fn merge_request_on_a_per_task_branch_is_not_judged_by_the_merged_default_branch() {
        let _env = TestEnvGuard::temp_home();
        crate::store::known_repos::ensure_host_schema().expect("host repo schema");
        let project = tempfile::tempdir().expect("temporary project");
        let repo = project.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.email", "cas-test@example.invalid"]);
        git(repo, &["config", "user.name", "Cassy Test"]);
        std::fs::create_dir(repo.join(".cas")).expect("Cassy directory");
        std::fs::write(
            repo.join(".cas/config.toml"),
            "[project]\ncanonical_id = \"cas-b3ab-message-test\"\n",
        )
        .expect("Cassy config");
        std::fs::write(repo.join("base.txt"), "base\n").expect("base file");
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "base"]);
        // The worker's previous task landed from its default branch.
        git(repo, &["checkout", "-qb", "factory/worker-a"]);
        std::fs::write(repo.join("earlier.txt"), "earlier task\n").unwrap();
        git(repo, &["add", "earlier.txt"]);
        git(repo, &["commit", "-qm", "cas-0001 earlier task"]);
        git(repo, &["checkout", "-q", "main"]);
        git(
            repo,
            &[
                "merge",
                "--no-ff",
                "factory/worker-a",
                "-m",
                "merge earlier",
            ],
        );
        // This task lives on its own, unmerged, per-task branch.
        git(repo, &["checkout", "-qb", "factory/worker-a-cas-b3ab"]);
        std::fs::write(repo.join("delivery.txt"), "this task\n").unwrap();
        git(repo, &["add", "delivery.txt"]);
        git(repo, &["commit", "-qm", "cas-b3ab delivery"]);
        let rev_parse = || {
            String::from_utf8(
                Command::new("git")
                    .args(["rev-parse", "HEAD"])
                    .current_dir(repo)
                    .output()
                    .expect("rev-parse")
                    .stdout,
            )
            .unwrap()
            .trim()
            .to_string()
        };
        // MERGE REQUIRED parked this tip; a conflict sent the task back to
        // work, retiring that anchor, and the worker pushed a rebased fix.
        let retired_anchor = rev_parse();
        std::fs::write(repo.join("delivery.txt"), "this task, rebased\n").unwrap();
        git(repo, &["commit", "-qam", "cas-b3ab rebase fix"]);
        let per_task_tip = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(repo)
                .output()
                .expect("per-task tip")
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        git(repo, &["checkout", "-q", "main"]);

        let cas_root = repo.join(".cas");
        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        let agents = core.open_agent_store().expect("agent store");
        let mut worker = Agent::new("worker-id".to_string(), "worker-a".to_string());
        worker.role = AgentRole::Worker;
        agents.register(&worker).expect("register worker");
        let mut supervisor = Agent::new("supervisor-id".to_string(), "supervisor".to_string());
        supervisor.role = AgentRole::Supervisor;
        agents.register(&supervisor).expect("register supervisor");
        core.set_agent_id_for_testing(worker.id.clone());

        // Resumed after a MERGE REQUIRED park: back in progress, no parked branch.
        let tasks = core.open_task_store().expect("task store");
        let mut task = Task::new("cas-b3ab".to_string(), "per-task branch".to_string());
        task.status = TaskStatus::InProgress;
        task.assignee = Some(worker.name.clone());
        task.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:cas-b3ab-message-test".to_string(),
            target_branch: "main".to_string(),
        });
        task.deliverables.historical_factory_branch_anchors = vec![retired_anchor];
        tasks.add(&task).expect("add task");

        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core.clone(), None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core.clone());
        let mut request = message_request(true);
        request.task_id = Some(task.id.clone());
        let response = response_text(service.message_send(request).await.expect("merge request"));
        assert!(
            !response.contains("Merge already landed"),
            "another task's merged branch must not suppress this request: {response}"
        );
        assert_queued_receipt(&response);
        let rows = crate::store::open_prompt_queue_store(&cas_root)
            .unwrap()
            .poll_all(10)
            .unwrap();
        let envelope = rows
            .iter()
            .find_map(|row| crate::prompt_revalidation::parse_merge_request_envelope(&row.prompt))
            .unwrap_or_else(|| panic!("typed merge request reaches the queue: {response}"));
        assert_eq!(envelope.branch_tip, per_task_tip);
        assert_eq!(
            crate::prompt_revalidation::merge_request_branch(Some(&task), Some(repo)).as_deref(),
            Some("factory/worker-a-cas-b3ab"),
            "the per-task branch is preferred over the merged default branch"
        );
    }

    async fn stale_source_merge_request_gh1179(offline: bool, local_merge: bool, status: TaskStatus) {
        let _env = TestEnvGuard::temp_home();
        let origin = tempfile::tempdir().unwrap();
        git(origin.path(), &["init", "-q", "--bare"]);
        let project = tempfile::tempdir().unwrap();
        let repo = project.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.email", "cas-test@example.invalid"]);
        git(repo, &["config", "user.name", "Cassy Test"]);
        std::fs::create_dir(repo.join(".cas")).unwrap();
        std::fs::write(repo.join(".cas/config.toml"),
            "[project]\ncanonical_id = \"cas-gh1179-message-test\"\n").unwrap();
        std::fs::write(repo.join("base.txt"), "base\n").unwrap();
        git(repo, &["add", "base.txt"]);
        git(repo, &["commit", "-qm", "base"]);
        git(repo, &["remote", "add", "origin", origin.path().to_str().unwrap()]);
        let branch = "factory/worker-a-cas-874e";
        git(repo, &["checkout", "-qb", branch]);
        std::fs::write(repo.join("earlier.txt"), "integrated predecessor\n").unwrap();
        git(repo, &["add", "earlier.txt"]);
        git(repo, &["commit", "-qm", "earlier delivery"]);
        let sha = |reference: &str| crate::git_evidence::resolve_branch_sha(repo, reference).unwrap();
        let stale_tip = sha("HEAD");
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["merge", "-q", "--no-ff", branch, "-m", "merge predecessor"]);
        let target_tip = sha("main");
        git(repo, &["push", "-q", "origin", "main", branch]);
        git(repo, &["checkout", "-q", branch]);
        std::fs::write(repo.join("delivery.txt"), "unmerged delivery\n").unwrap();
        git(repo, &["add", "delivery.txt"]);
        git(repo, &["commit", "-qm", "fix(cas-874e): live published delivery"]);
        let published_tip = sha("HEAD");
        git(repo, &["push", "-q", "origin", branch]);
        git(repo, &["checkout", "-q", "main"]);
        // Another checkout pushed the continuation; both refs cached here
        // still name the already-integrated predecessor.
        git(repo, &["update-ref", &format!("refs/heads/{branch}"), &stale_tip]);
        git(repo, &["update-ref", &format!("refs/remotes/origin/{branch}"), &stale_tip]);
        assert!(!crate::git_evidence::git_commit_is_ancestor(repo, &published_tip, "main"));
        if offline {
            git(repo, &["remote", "set-url", "origin", project.path().join("missing-origin").to_str().unwrap()]);
        }

        let cas_root = repo.join(".cas");
        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        let agents = core.open_agent_store().unwrap();
        let mut worker = Agent::new("worker-id".into(), "worker-a".into());
        worker.role = AgentRole::Worker;
        agents.register(&worker).unwrap();
        let mut supervisor = Agent::new("supervisor-id".into(), "supervisor".into());
        supervisor.role = AgentRole::Supervisor;
        agents.register(&supervisor).unwrap();
        core.set_agent_id_for_testing(worker.id.clone());
        let tasks = core.open_task_store().unwrap();
        let mut task = Task::new("cas-874e".into(), "live origin source".into());
        task.status = status;
        if status == TaskStatus::InProgress {
            task.deliverables.historical_factory_branch_anchors = vec![stale_tip.clone()];
        }
        if local_merge {
            task.delivery_mode = cas_types::DeliveryMode::LocalMerge;
        }
        task.assignee = Some(worker.name);
        task.deliverables.parked_branch = Some(branch.into());
        task.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:cas-gh1179-message-test".into(), target_branch: "main".into(),
        });
        tasks.add(&task).unwrap();
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        let mut request = message_request(true);
        request.task_id = Some(task.id.clone());
        let response = response_text(service.message_send(request).await.unwrap());
        if local_merge {
            assert!(response.contains("Merge already landed"), "{response}");
            return;
        }
        assert!(!response.contains("Merge already landed"), "{response}");
        if offline {
            assert_queued_receipt(&response);
            let rows = crate::store::open_prompt_queue_store(&cas_root).unwrap().poll_all(10).unwrap();
            let envelope = rows.iter().find_map(|row|
                crate::prompt_revalidation::parse_merge_request_envelope(&row.prompt)
            ).expect("offline request retains its typed supervisor wake");
            let note = "source freshness unavailable; not checked for already-landed";
            assert_eq!(envelope.source_freshness_unavailable.as_deref(), Some(note));
            assert!(rows.iter().any(|row| row.prompt.contains(note)));
            assert_eq!(crate::prompt_revalidation::queued_merge_request_decision(
                repo, Some(&task), &envelope,
            ), crate::prompt_revalidation::MergeRequestDecision::Unverifiable);
            let cached_landed = crate::prompt_revalidation::MergeRequestDecision::AlreadyIntegrated {
                target_tip: target_tip.clone(),
            };
            assert_eq!(crate::prompt_revalidation::merge_request_delivery_decision(
                Some(&task), &envelope, &cached_landed,
            ), crate::prompt_revalidation::MergeRequestDelivery::Deliver);
            let mut closed = task.clone();
            closed.status = TaskStatus::Closed;
            assert!(matches!(crate::prompt_revalidation::merge_request_delivery_decision(
                Some(&closed), &envelope, &cached_landed,
            ), crate::prompt_revalidation::MergeRequestDelivery::SuppressResolved { .. }));
            let inbox_core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
            inbox_core.set_agent_id_for_testing("supervisor-id".into());
            #[cfg(feature = "mcp-proxy")]
            let inbox = CasService::new(inbox_core, None);
            #[cfg(not(feature = "mcp-proxy"))]
            let inbox = CasService::new(inbox_core);
            let inbox = response_text(inbox.inbox_poll(serde_json::from_value(
                serde_json::json!({"action": "inbox_poll"}),
            ).unwrap()).await.unwrap());
            assert!(inbox.contains("Fresh worker delivery"), "{inbox}");
            assert!(inbox.contains(note), "{inbox}");
        } else {
            assert_queued_receipt(&response);
            let rows = crate::store::open_prompt_queue_store(&cas_root).unwrap().poll_all(10).unwrap();
            let envelope = rows.iter().find_map(|row|
                crate::prompt_revalidation::parse_merge_request_envelope(&row.prompt)
            ).expect("live source merge request reaches supervisor queue");
            assert_eq!(envelope.branch_tip, published_tip);
            assert_eq!(envelope.target_branch_tip, target_tip);
            // Once that exact published delivery lands, suppression is valid.
            git(repo, &["merge", "-q", "--no-ff", &published_tip, "-m", "merge current delivery"]);
            git(repo, &["push", "-q", "origin", "main"]);
            let mut request = message_request(true);
            request.task_id = Some("cas-874e".into());
            let landed = response_text(service.message_send(request).await.unwrap());
            assert!(landed.contains("Merge already landed"), "{landed}");
            assert!(landed.contains(&published_tip), "{landed}");
        }
    }

    /// A parked per-task branch can be rebased and force-pushed after the
    /// merge request anchor was recorded. A message cannot stand in for the
    /// exact-tip checks at close or advertise an anchor the store never wrote.
    #[tokio::test(flavor = "current_thread")]
    async fn merge_request_rebase_force_push_requires_repark_before_envelope_cas749f() {
        let _env = TestEnvGuard::temp_home();
        crate::store::known_repos::ensure_host_schema().expect("host repo schema");
        let origin = tempfile::tempdir().expect("bare origin");
        git(origin.path(), &["init", "-q", "--bare"]);
        let project = tempfile::tempdir().expect("temporary project");
        let repo = project.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.email", "cas-test@example.invalid"]);
        git(repo, &["config", "user.name", "Cassy Test"]);
        std::fs::create_dir(repo.join(".cas")).expect("Cassy directory");
        std::fs::write(
            repo.join(".cas/config.toml"),
            "[project]\ncanonical_id = \"cas-4a65-message-test\"\n",
        )
        .expect("Cassy config");
        std::fs::write(repo.join("base.txt"), "base\n").expect("base file");
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "base"]);
        let base = crate::git_evidence::resolve_branch_sha(repo, "HEAD").expect("base SHA");
        git(repo, &["remote", "add", "origin", origin.path().to_str().unwrap()]);
        git(repo, &["push", "-q", "origin", "main"]);

        let branch = "factory/worker-a-cas-4a65";
        git(repo, &["checkout", "-qb", branch]);
        std::fs::write(repo.join("delivery.txt"), "parked task delivery\n")
            .expect("delivery file");
        git(repo, &["add", "delivery.txt"]);
        git(repo, &["commit", "-qm", "cas-4a65 delivery"]);
        let old_anchor = crate::git_evidence::resolve_branch_sha(repo, "HEAD")
            .expect("pre-rebase anchor");
        git(repo, &["push", "-q", "origin", branch]);

        // Advance the target independently, then rewrite the parked task
        // commit onto it just as a worker does before force-pushing a rebase.
        git(repo, &["checkout", "-q", "main"]);
        std::fs::write(repo.join("main-only.txt"), "new target base\n")
            .expect("updated target file");
        git(repo, &["add", "main-only.txt"]);
        git(repo, &["commit", "-qm", "advance target before rebase"]);
        git(repo, &["push", "-q", "origin", "main"]);
        git(repo, &["checkout", "-q", branch]);
        git(repo, &["rebase", "--onto", "main", &base]);
        let live_tip = crate::git_evidence::resolve_branch_sha(repo, "HEAD")
            .expect("rebased source SHA");
        git(repo, &["push", "-q", "--force", "origin", branch]);
        assert_ne!(old_anchor, live_tip, "rebase must replace the parked SHA");
        assert!(
            !crate::git_evidence::git_commit_is_ancestor(repo, &old_anchor, &live_tip),
            "rebased source must no longer descend from the stored anchor"
        );

        let cas_root = repo.join(".cas");
        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        let agents = core.open_agent_store().expect("agent store");
        let mut worker = Agent::new("worker-id".into(), "worker-a".into());
        worker.role = AgentRole::Worker;
        agents.register(&worker).expect("register worker");
        let mut supervisor = Agent::new("supervisor-id".into(), "supervisor".into());
        supervisor.role = AgentRole::Supervisor;
        agents.register(&supervisor).expect("register supervisor");
        core.set_agent_id_for_testing(worker.id.clone());
        let tasks = core.open_task_store().expect("task store");
        let mut task = Task::new("cas-4a65".into(), "rebased delivery".into());
        task.status = TaskStatus::AwaitingMerge;
        task.assignee = Some(worker.name);
        task.deliverables.factory_branch_anchor = Some(old_anchor.clone());
        task.deliverables.parked_branch = Some(branch.into());
        task.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:cas-4a65-message-test".into(),
            target_branch: "main".into(),
        });
        tasks.add(&task).expect("add parked task");

        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        let mut request = message_request(true);
        request.task_id = Some(task.id.clone());
        let response = response_text(service.message_send(request).await.expect("merge request"));
        assert!(response.contains("RE-PARK") && response.contains(&live_tip)
            && response.contains("task action=close id=cas-4a65"), "{response}");
        assert_eq!(tasks.get(&task.id).unwrap().deliverables.factory_branch_anchor.as_deref(), Some(old_anchor.as_str()));
        let rows = crate::store::open_prompt_queue_store(&cas_root)
            .expect("prompt queue")
            .poll_all(10)
            .expect("queued messages");
        assert!(!rows.iter().any(|row| {
            crate::prompt_revalidation::parse_merge_request_envelope(&row.prompt)
                .is_some_and(|envelope| envelope.task_id == task.id)
        }), "a mergeable envelope cannot be queued ahead of its persisted park");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merge_request_refreshes_stale_source_before_landed_suppression_gh1179() {
        stale_source_merge_request_gh1179(false, false, TaskStatus::AwaitingMerge).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merge_request_failed_source_refresh_never_claims_landed_gh1179() {
        stale_source_merge_request_gh1179(true, false, TaskStatus::AwaitingMerge).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn local_merge_request_keeps_local_authority_with_offline_origin_gh1179() {
        stale_source_merge_request_gh1179(true, true, TaskStatus::AwaitingMerge).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reopened_merge_request_refreshes_stale_historical_tip_gh1179() {
        stale_source_merge_request_gh1179(false, false, TaskStatus::InProgress).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reopened_merge_request_offline_still_queues_gh1179() {
        stale_source_merge_request_gh1179(true, false, TaskStatus::InProgress).await;
    }

    #[test]
    fn unsafe_merge_request_source_still_refuses_gh1179() {
        let repo = tempfile::tempdir().unwrap();
        let error = crate::prompt_revalidation::refresh_merge_request_source(
            repo.path(), "-unsafe", cas_types::DeliveryMode::LocalMerge,
        ).expect_err("unsafe source names must refuse before any Git probe");
        assert!(error.contains("unsafe source branch"), "{error}");
    }

    /// GH #734: a worker in one factory session must not inherit the newest
    /// supervisor from a sibling session on the same clone. Explicit names
    /// remain valid same-clone recipients, and their rows belong to the named
    /// supervisor's session.
    #[tokio::test(flavor = "current_thread")]
    async fn shared_clone_worker_messages_follow_the_registered_owner_session() {
        let _env = TestEnvGuard::with_optional_vars(&[
            // Deliberately inherit the sibling session too: persisted worker
            // ownership must outrank both stale ambient routing variables.
            ("CAS_FACTORY_SESSION", Some("factory-b")),
            ("CAS_SUPERVISOR_NAME", Some("supervisor-b")),
            ("CAS_AGENT_ROLE", None),
        ]);
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        let agents = core.open_agent_store().expect("agent store");

        let mut worker = Agent::new("worker-id".to_string(), "worker-a".to_string());
        worker.role = AgentRole::Worker;
        worker.factory_session = Some("factory-a".to_string());
        agents.register(&worker).expect("register worker");
        let mut supervisor_a = Agent::new("supervisor-a-id".to_string(), "supervisor-a".to_string());
        supervisor_a.role = AgentRole::Supervisor;
        supervisor_a.factory_session = Some("factory-a".to_string());
        agents.register(&supervisor_a).expect("register owning supervisor");
        let mut supervisor_b = Agent::new("supervisor-b-id".to_string(), "supervisor-b".to_string());
        supervisor_b.role = AgentRole::Supervisor;
        supervisor_b.factory_session = Some("factory-b".to_string());
        agents.register(&supervisor_b).expect("register sibling supervisor");
        core.set_agent_id_for_testing(worker.id.clone());

        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core.clone(), None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core.clone());

        let owner_result = service
            .message_send(message_request_to("supervisor"))
            .await
            .expect("owner message is delivered");
        assert_queued_receipt(&response_text(owner_result));
        let explicit_result = service
            .message_send(message_request_to("supervisor-b"))
            .await
            .expect("explicit live supervisor message is delivered");
        assert_queued_receipt(&response_text(explicit_result));

        let rows = crate::store::open_prompt_queue_store(&cas_root)
            .expect("prompt queue")
            .poll_all(10)
            .expect("queued messages");
        assert_eq!(rows.len(), 2);
        let owner_row = rows
            .iter()
            .find(|row| row.target == "supervisor-a")
            .expect("logical supervisor target resolves to factory-a owner");
        assert_eq!(owner_row.factory_session.as_deref(), Some("factory-a"));
        let explicit_row = rows
            .iter()
            .find(|row| row.target == "supervisor-b")
            .expect("explicit supervisor target remains accepted");
        assert_eq!(explicit_row.factory_session.as_deref(), Some("factory-b"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn worker_status_and_ready_kinds_are_scoped_to_the_same_session_supervisor() {
        let _env = TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        let agents = core.open_agent_store().expect("agent store");

        let mut worker = Agent::new("worker-id".to_string(), "worker-a".to_string());
        worker.role = AgentRole::Worker;
        worker.factory_session = Some("factory-a".to_string());
        agents.register(&worker).expect("register worker");
        let mut supervisor_a =
            Agent::new("supervisor-a-id".to_string(), "supervisor-a".to_string());
        supervisor_a.role = AgentRole::Supervisor;
        supervisor_a.factory_session = Some("factory-a".to_string());
        agents
            .register(&supervisor_a)
            .expect("register owning supervisor");
        let mut supervisor_b =
            Agent::new("supervisor-b-id".to_string(), "supervisor-b".to_string());
        supervisor_b.role = AgentRole::Supervisor;
        supervisor_b.factory_session = Some("factory-b".to_string());
        agents
            .register(&supervisor_b)
            .expect("register sibling supervisor");
        core.set_agent_id_for_testing(worker.id.clone());

        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core.clone(), None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core.clone());

        for (kind, expected) in [
            (
                "status",
                crate::prompt_revalidation::WorkerActionKind::Status,
            ),
            ("ready", crate::prompt_revalidation::WorkerActionKind::Ready),
        ] {
            let mut request = message_request_to("supervisor");
            request.kind = Some(kind.to_string());
            if kind == "status" {
                request.summary = Some("status\nsummary\r\nline\tend".into());
            }
            let response = response_text(
                service
                    .message_send(request)
                    .await
                    .expect("same-session worker action is queued"),
            );
            assert_queued_receipt(&response);
            let rows = crate::store::open_prompt_queue_store(&cas_root)
                .unwrap()
                .poll_all(10)
                .unwrap();
            let row = rows.last().expect("typed worker row");
            if kind == "status" {
                assert_eq!(
                    row.summary.as_deref(),
                    Some("status summary line end"),
                    "message summaries remain one line and contain no controls"
                );
            }
            assert_eq!(row.target, "supervisor-a");
            assert_eq!(
                crate::prompt_revalidation::parse_worker_action_envelope(&row.prompt)
                    .map(|envelope| envelope.kind),
                Some(expected)
            );
        }

        for (kind,wake,body) in [
            ("receipt",None,"The delivery was received."),
            ("answer",Some(false),"A field report that can wait."),
        ] {
            let mut request=message_request_to("supervisor");
            request.kind=Some(kind.into());request.wake=wake;request.message=Some(body.into());
            service.message_send(request).await.expect("quiet message remains deliverable");
            let rows=crate::store::open_prompt_queue_store(&cas_root).unwrap().poll_all(10).unwrap();
            let row=rows.iter().find(|row|row.prompt.contains(body))
                .expect("quiet body retained");
            assert!(crate::supervisor_message_wakes::quiet(&row.prompt),"{kind}: {}",row.prompt);
        }

        let mut cross_session = message_request_to("supervisor-b");
        cross_session.kind = Some("status".to_string());
        let error = service
            .message_send(cross_session)
            .await
            .expect_err("typed status cannot target a sibling session");
        assert!(
            error.message.contains("same-session supervisor"),
            "{error:?}"
        );
    }

    /// cas-6920: classify each acknowledgement before its labels and other
    /// messages become part of the queued body.
    #[tokio::test(flavor = "current_thread")]
    async fn folded_worker_acknowledgements_remain_inbox_only_cas_6920() {
        let _env = TestEnvGuard::temp_home();
        let root = tempfile::tempdir().unwrap();
        let cas_root = root.path().join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();
        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        let worker = register_gh_1171_fleet(&core, false);
        let agents = core.open_agent_store().unwrap();
        let mut supervisor = agents.get("supervisor-a-id").unwrap();
        supervisor.factory_session = worker.factory_session.clone();
        agents.update(&supervisor).unwrap();
        core.set_agent_id_for_testing(worker.id.clone());
        let service = gh_1171_service(&core);
        for body in ["ok", "thanks"] {
            let mut request = message_request_to("supervisor");
            request.kind = Some("status".into());
            request.message = Some(body.into());
            service.message_send(request).await.unwrap();
        }
        let queue = crate::store::open_prompt_queue_store(&cas_root).unwrap();
        let rows = queue.peek_all(10).unwrap();
        assert_eq!(rows.len(), 1, "acknowledgements still coalesce");
        let prompt = &rows[0].prompt;
        assert!(prompt.contains("ok") && prompt.contains("thanks"), "{prompt}");
        assert!(crate::supervisor_message_wakes::quiet(prompt),
            "folded acknowledgements must wait for the next turn: {prompt}");
    }

    /// GH #1171: a worker's `target=supervisor` failed with "Cannot resolve
    /// 'supervisor'" while its supervisor was live and messaging it, because
    /// that supervisor's row carried no factory session and a second
    /// supervisor shared the clone. Resolution stays session-scoped: the
    /// worker's session metadata names its supervisor, a sole sessionless
    /// live supervisor is accepted, and another stamped session's supervisor
    /// is never chosen.
    fn register_gh_1171_fleet(core: &crate::mcp::server::CasCore, with_b: bool) -> Agent {
        let agents = core.open_agent_store().expect("agent store");
        let mut worker = Agent::new("worker-id".to_string(), "worker-a".to_string());
        worker.role = AgentRole::Worker;
        worker.factory_session = Some("factory-a".to_string());
        agents.register(&worker).expect("register worker");
        let mut supervisor_a = Agent::new("supervisor-a-id".to_string(), "supervisor-a".to_string());
        supervisor_a.role = AgentRole::Supervisor;
        // Registered from a process without CAS_FACTORY_SESSION.
        supervisor_a.factory_session = None;
        agents.register(&supervisor_a).expect("register unstamped supervisor");
        if with_b {
            let mut supervisor_b =
                Agent::new("supervisor-b-id".to_string(), "supervisor-b".to_string());
            supervisor_b.role = AgentRole::Supervisor;
            supervisor_b.factory_session = Some("factory-b".to_string());
            agents.register(&supervisor_b).expect("register sibling supervisor");
        }
        worker
    }

    fn gh_1171_service(core: &crate::mcp::server::CasCore) -> CasService {
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core.clone(), None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core.clone());
        service
    }

    #[tokio::test(flavor = "current_thread")]
    async fn worker_supervisor_target_follows_session_metadata_to_an_unstamped_supervisor_gh_1171()
    {
        let env = TestEnvGuard::temp_home();
        let sessions = env.home().join(".cas").join("sessions");
        std::fs::create_dir_all(&sessions).expect("sessions dir");
        std::fs::write(
            sessions.join("factory-a.json"),
            r#"{"name":"factory-a","supervisor":{"name":"supervisor-a"},"workers":[]}"#,
        )
        .expect("session metadata");
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        let worker = register_gh_1171_fleet(&core, true);
        core.set_agent_id_for_testing(worker.id.clone());

        let result = gh_1171_service(&core)
            .message_send(message_request_to("supervisor"))
            .await
            .expect("target=supervisor resolves while the owning supervisor is live");
        assert_queued_receipt(&response_text(result));
        let rows = crate::store::open_prompt_queue_store(&cas_root)
            .expect("prompt queue")
            .poll_all(10)
            .expect("queued messages");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target, "supervisor-a", "never the sibling session's supervisor");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn worker_supervisor_target_accepts_the_sole_sessionless_live_supervisor_gh_1171() {
        let _env = TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        let worker = register_gh_1171_fleet(&core, true);
        core.set_agent_id_for_testing(worker.id.clone());

        gh_1171_service(&core)
            .message_send(message_request_to("supervisor"))
            .await
            .expect("the only sessionless live supervisor owns the worker");
        let rows = crate::store::open_prompt_queue_store(&cas_root)
            .expect("prompt queue")
            .poll_all(10)
            .expect("queued messages");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target, "supervisor-a");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn worker_supervisor_target_never_borrows_another_sessions_supervisor_gh_1171() {
        let _env = TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        let agents = core.open_agent_store().expect("agent store");
        let mut worker = Agent::new("worker-id".to_string(), "worker-a".to_string());
        worker.role = AgentRole::Worker;
        worker.factory_session = Some("factory-a".to_string());
        agents.register(&worker).expect("register worker");
        let mut supervisor_b = Agent::new("supervisor-b-id".to_string(), "supervisor-b".to_string());
        supervisor_b.role = AgentRole::Supervisor;
        supervisor_b.factory_session = Some("factory-b".to_string());
        agents.register(&supervisor_b).expect("register sibling supervisor");
        core.set_agent_id_for_testing(worker.id.clone());

        let error = gh_1171_service(&core)
            .message_send(message_request_to("supervisor"))
            .await
            .expect_err("another session's supervisor is not this worker's");
        assert!(error.message.contains("Cannot resolve 'supervisor'"), "{}", error.message);
        assert!(
            error.message.contains("supervisor-b"),
            "the refusal names the live supervisors that can be addressed by name: {}",
            error.message
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn message_requires_a_non_empty_summary() {
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let core = crate::mcp::server::CasCore::with_daemon(cas_root, None, None);

        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);

        let request: AgentRequest = serde_json::from_value(serde_json::json!({
            "action": "message",
            "target": "supervisor",
            "summary": "   ",
            "message": "body",
        }))
        .expect("blank-summary message request");
        let error = service
            .message_send(request)
            .await
            .expect_err("blank summary must be rejected");
        assert!(error.message.contains("summary required"), "{error:?}");
    }

    fn assignment_message_fixture() -> (tempfile::TempDir, CasService) {
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        let agents = crate::store::open_agent_store(&root).unwrap();
        for (id, name, role) in [
            ("supervisor-id", "supervisor", AgentRole::Supervisor),
            ("worker-id", "eager-cobra-57", AgentRole::Worker),
        ] {
            let mut agent = Agent::new(id.into(), name.into());
            agent.role = role;
            agent.factory_session = Some("assignment-test".into());
            agents.register(&agent).unwrap();
        }
        let tasks = crate::store::open_task_store_local(&root).unwrap();
        for (id, status) in [
            ("cas-3e24", TaskStatus::Closed),
            ("cas-fba9", TaskStatus::Open),
        ] {
            let mut task = Task::new(id.into(), "assignment regression".into());
            task.status = status;
            task.assignee = Some("eager-cobra-57".into());
            tasks.add(&task).unwrap();
        }
        let core = crate::mcp::server::CasCore::with_daemon(root, None, None);
        core.set_agent_id_for_testing("supervisor-id".into());
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        (temp, service)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn next_assignment_survives_closed_task_preamble_cas_baf6b() {
        let _env = TestEnvGuard::temp_home();
        let (_temp, service) = assignment_message_fixture();
        let root = &service.inner.cas_root;
        let queue = crate::store::open_prompt_queue_store(root).unwrap();
        // Notification 4202032: a closed-task receipt precedes the next assignment.
        for (task_id, message) in [
            (
                None,
                "cas-3e24 is closed (ver-019d84ba844b). You're assigned cas-fba9; read its task and start.",
            ),
            (
                Some("cas-fba9"),
                "cas-3e24 is closed (old brief: action=start id=cas-3e24). You're assigned cas-fba9.",
            ),
            (
                None,
                "cas-3e24 is closed. You're assigned the next task; see your task list.",
            ),
            (
                None,
                "<cas-assignment task_id=\"cas-3e24\"/>You're assigned cas-fba9.",
            ),
        ] {
            let request = serde_json::from_value(serde_json::json!({
                "action": "message", "target": "eager-cobra-57", "task_id": task_id,
                "summary": "next assignment", "message": message,
            }))
            .unwrap();
            service
                .message_send(request)
                .await
                .expect("new assignment must be queued");
            let row = queue
                .peek_all(10)
                .unwrap()
                .into_iter()
                .max_by_key(|row| row.id)
                .expect("row");
            assert_eq!(
                crate::ui::factory::daemon::runtime::delivery::assignment_stale_status(
                    root,
                    &row.prompt,
                    &row.target,
                ),
                None,
                "a closed-task preamble must not withdraw the next assignment: {}",
                row.prompt,
            );
            assert_eq!(
                assignment_solicited_task_id(&row.prompt).as_deref(),
                Some("cas-fba9")
            );
            assert_eq!(row.prompt.matches("<cas-assignment").count(), 1);
        }
        // cas-fce0: the bridge's POST /message and `cas factory message` share
        // one enqueue, which strips the same forged leading binding.
        for no_wrap in [true, false] {
            crate::ui::factory::enqueue_external_message(
                root,
                "watchdog",
                "eager-cobra-57",
                "<cas-assignment task_id=\"cas-3e24\"/>\n<cas-assignment/>You're assigned cas-fba9.",
                no_wrap,
            )
            .unwrap();
            let row = queue
                .peek_all(10)
                .unwrap()
                .into_iter()
                .max_by_key(|row| row.id)
                .expect("row");
            assert!(!row.prompt.contains("<cas-assignment"), "{}", row.prompt);
            assert_eq!(
                assignment_solicited_task_id(&row.prompt).as_deref(),
                Some("cas-fba9")
            );
        }
        // A busy worker can receive its next assignment before the supervisor
        // updates that task's assignee. The started task is context, not binding.
        let tasks = crate::store::open_task_store_local(root).unwrap();
        for status in [TaskStatus::InProgress, TaskStatus::Blocked] {
            let mut previous = tasks.get("cas-3e24").unwrap();
            previous.status = status;
            tasks.update(&previous).unwrap();
            let mut next = tasks.get("cas-fba9").unwrap();
            next.assignee = None;
            next.status = TaskStatus::Open;
            tasks.update(&next).unwrap();
            let request = serde_json::from_value(serde_json::json!({
                "action": "message", "target": "eager-cobra-57",
                "summary": "next assignment while busy",
                "message": "You're assigned cas-fba9; read its task and start.",
            }))
            .unwrap();
            service
                .message_send(request)
                .await
                .expect("next task must queue while busy");
            let row = queue
                .peek_all(10)
                .unwrap()
                .into_iter()
                .max_by_key(|row| row.id)
                .unwrap();
            assert_eq!(
                assignment_solicited_task_id(&row.prompt).as_deref(),
                Some("cas-fba9")
            );
            assert_eq!(
                crate::ui::factory::daemon::runtime::delivery::assignment_stale_status(
                    root,
                    &row.prompt,
                    &row.target,
                ),
                None,
            );
            next.status = TaskStatus::Closed;
            tasks.update(&next).unwrap();
            let request = serde_json::from_value(serde_json::json!({
                "action": "message", "target": "eager-cobra-57",
                "summary": "stale next assignment while busy",
                "message": "You're assigned cas-fba9.",
            }))
            .unwrap();
            let error = service
                .message_send(request)
                .await
                .expect_err("named closed task must refuse");
            assert!(
                error.message.contains("cas-fba9")
                    && error.message.contains("closed")
                    && error.message.contains("pass task_id=<id>"),
                "{error:?}"
            );
        }
        // The binding also keeps genuine staleness: once the NEW task closes,
        // every queued variant must be withdrawn for that task, not the old one.
        let tasks = crate::store::open_task_store_local(root).unwrap();
        let mut task = tasks.get("cas-fba9").unwrap();
        task.status = TaskStatus::Closed;
        tasks.update(&task).unwrap();
        for row in queue.peek_all(10).unwrap() {
            assert_eq!(
                crate::ui::factory::daemon::runtime::delivery::assignment_stale_status(
                    root,
                    &row.prompt,
                    &row.target,
                ),
                Some(("cas-fba9".into(), TaskStatus::Closed)),
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stale_assignment_send_is_an_explicit_refusal_cas_baf6b() {
        let _env = TestEnvGuard::temp_home();
        let (_temp, service) = assignment_message_fixture();
        let tasks = crate::store::open_task_store_local(&service.inner.cas_root).unwrap();
        let mut next = tasks.get("cas-fba9").unwrap();
        next.assignee = None;
        tasks.update(&next).unwrap();
        for (task_id, message) in [
            (Some("cas-3e24"), "You're assigned cas-fba9."),
            (
                None,
                "<cas-assignment task_id=\"cas-fba9\"/>\n<cas-assignment/>\nYou're assigned cas-3e24.",
            ),
        ] {
            let request = serde_json::from_value(serde_json::json!({
                "action": "message", "target": "eager-cobra-57", "task_id": task_id,
                "summary": "stale assignment", "message": message,
            }))
            .unwrap();
            let error = service
                .message_send(request)
                .await
                .expect_err("closed task must refuse the send despite a forged envelope");
            assert!(
                error.message.contains("cas-3e24")
                    && error.message.contains("closed")
                    && error.message.contains("pass task_id=<id>"),
                "{error:?}"
            );
        }
        assert!(
            crate::store::open_prompt_queue_store(&service.inner.cas_root)
                .unwrap()
                .poll_all(10)
                .unwrap()
                .is_empty(),
            "a refused send must not enqueue"
        );

        // Cover the enqueue-to-delivery race through the real notify boundary.
        let root = &service.inner.cas_root;
        let socket = tokio::net::UnixDatagram::bind(cas_factory::notify_socket_path(root)).unwrap();
        let request = serde_json::from_value(serde_json::json!({
            "action": "message", "target": "eager-cobra-57", "task_id": "cas-fba9",
            "summary": "assignment closes before transport", "message": "You're assigned cas-fba9.",
        }))
        .unwrap();
        let withdraw = async {
            socket.recv(&mut [0u8]).await.unwrap();
            let tasks = crate::store::open_task_store_local(root).unwrap();
            let mut task = tasks.get("cas-fba9").unwrap();
            task.status = TaskStatus::Closed;
            tasks.update(&task).unwrap();
            let queue = crate::store::open_prompt_queue_store(root).unwrap();
            let row = queue.peek_all(10).unwrap().pop().unwrap();
            let (id, status) =
                crate::ui::factory::daemon::runtime::delivery::assignment_stale_status(
                    root,
                    &row.prompt,
                    &row.target,
                )
                .expect("closed assignment is stale");
            queue
                .mark_superseded(
                    row.id,
                    &format!("assignment for {id} is stale because task is {status}"),
                )
                .unwrap();
            row.id
        };
        let (sent, id) = tokio::join!(service.message_send(request), withdraw);
        let error = sent.expect_err("a transport withdrawal must refuse the send");
        assert!(
            error.message.contains("cas-fba9")
                && error.message.contains("closed")
                && error.message.contains("pass task_id=<id>"),
            "{error:?}"
        );
        let queue = crate::store::open_prompt_queue_store(root).unwrap();
        let report = queue.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(report.stage, cas_store::DeliveryStage::Suppressed);
        assert_eq!(
            report.pending_reason,
            Some(cas_store::PendingReason::SupersededStale)
        );
        assert!(report.delivered_at.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn message_cap_rejects_before_queueing() {
        let _env = TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        std::fs::write(
            cas_root.join("config.toml"),
            "[factory]\nmessage_max_chars = 5\n",
        )
        .expect("write test factory config");

        let agents = crate::store::open_agent_store(&cas_root).expect("agent store");
        agents.init().expect("agent store init");
        let mut worker = Agent::new("worker-id".to_string(), "worker-a".to_string());
        worker.role = AgentRole::Worker;
        agents.register(&worker).expect("register worker");
        let mut supervisor = Agent::new("supervisor-id".to_string(), "supervisor".to_string());
        supervisor.role = AgentRole::Supervisor;
        agents.register(&supervisor).expect("register supervisor");

        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        core.set_agent_id_for_testing(worker.id);
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);

        let request: AgentRequest = serde_json::from_value(serde_json::json!({
            "action": "message",
            "target": "supervisor",
            "task_id": "cas-449b",
            "summary": "short summary",
            "message": "123456",
        }))
        .expect("over-cap message request");
        let error = service
            .message_send(request)
            .await
            .expect_err("over-cap message must be rejected");
        assert!(error.message.contains("limit is 5 characters"), "{error:?}");
        assert!(
            error
                .message
                .contains("[factory] artifacts_root/<project-key>/cas-449b/<name>.md"),
            "{error:?}"
        );

        let rows = crate::store::open_prompt_queue_store(&cas_root)
            .expect("prompt queue")
            .poll_all(10)
            .expect("queued messages");
        assert!(rows.is_empty(), "rejected message must not be queued: {rows:?}");
    }

    /// cas-6ee6 (GH #1006 item 1): a supervisor ruling over the cap is
    /// delivered to the worker as its head plus the path of the artifact
    /// holding the full text, instead of being refused.
    #[tokio::test(flavor = "current_thread")]
    async fn supervisor_over_cap_message_spills_to_artifact_and_is_delivered_cas_6ee6() {
        let _env = TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let artifacts = temp.path().join("artifacts");
        std::fs::write(
            cas_root.join("config.toml"),
            format!(
                "[factory]\nmessage_max_chars = 300\nartifacts_root = \"{}\"\n",
                artifacts.display()
            ),
        )
        .expect("write test factory config");

        let agents = crate::store::open_agent_store(&cas_root).expect("agent store");
        agents.init().expect("agent store init");
        let mut worker = Agent::new("worker-id".to_string(), "worker-a".to_string());
        worker.role = AgentRole::Worker;
        agents.register(&worker).expect("register worker");
        let mut supervisor = Agent::new("supervisor-id".to_string(), "supervisor".to_string());
        supervisor.role = AgentRole::Supervisor;
        agents.register(&supervisor).expect("register supervisor");

        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        core.set_agent_id_for_testing(supervisor.id);
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);

        let ruling = format!("RULING: {}", "keep the contract clause as written. ".repeat(30));
        assert!(ruling.chars().count() > 300);
        let request: AgentRequest = serde_json::from_value(serde_json::json!({
            "action": "message",
            "target": "worker-a",
            "task_id": "cas-6ee6",
            "summary": "ruling",
            "message": ruling,
        }))
        .expect("over-cap supervisor message request");
        service
            .message_send(request)
            .await
            .expect("an over-cap supervisor message is delivered, not refused");

        let spilled: Vec<_> = std::fs::read_dir(
            crate::config::project_factory_artifacts_root(&cas_root, &artifacts).join("cas-6ee6"),
        )
        .expect("spill directory under artifacts_root/<project-key>/<task>")
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(spilled.len(), 1, "{spilled:?}");
        assert!(
            std::fs::read_to_string(&spilled[0]).unwrap().contains(&ruling),
            "the artifact holds the full text"
        );
        let rows = crate::store::open_prompt_queue_store(&cas_root)
            .expect("prompt queue")
            .poll_all(10)
            .expect("queued messages");
        let row = rows
            .iter()
            .find(|row| row.target == "worker-a")
            .unwrap_or_else(|| panic!("message queued for the worker: {rows:?}"));
        // pin: the artifact spill envelope keeps this exact marker at the inline head.
        assert!(row.prompt.contains("RULING: keep the contract"), "{}", row.prompt);
        assert!(
            row.prompt.contains(&spilled[0].display().to_string()),
            "delivered message names the artifact: {}",
            row.prompt
        );
        // pin: the full ruling must stay out of the inline prompt after spilling.
        assert!(!row.prompt.contains(&ruling), "only the head is delivered inline");
    }

    #[test]
    fn verified_commander_rows_print_the_exact_operator_reply_command() {
        let mut row = cas_store::QueuedPrompt {
            id: 3139,
            source: "commander:Daniel@Soundwave".into(),
            target: "supervisor".into(),
            prompt: "What is status?".into(),
            created_at: chrono::Utc::now(),
            processed_at: None,
            factory_session: Some("factory-1".into()),
            summary: Some("status".into()),
            priority: cas_store::NotificationPriority::Normal,
            acked_at: None,
            urgent: false,
            origin: Some(cas_store::QueueOrigin::PairedDevice {
                device_id: "device-1".into(),
            }),
            operator: None,
            recipient_device_id: None,
            kind: None,
            attachments: Vec::new(),
        };
        assert_eq!(
            commander_reply_command(&row).as_deref(),
            Some("coordination action=message target=operator in_reply_to=3139 summary=… message=…")
        );
        assert_eq!(
            super::commander_reply_framing(&row).as_deref(),
            Some(
                "Reply with: `coordination action=message target=operator in_reply_to=3139 summary=… message=…`\nPhone reply contract: see `cas-supervisor/references/operator-reply.md`.\n",
            )
        );
        row.origin = None;
        assert_eq!(commander_reply_command(&row), None);
        assert_eq!(super::commander_reply_framing(&row), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn commander_label_routes_to_latest_verified_operator_row() {
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", None),
            ("CAS_AGENT_NAME", None),
            ("CAS_FACTORY_SESSION", None),
            ("CAS_SUPERVISOR_NAME", None),
        ]);
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let agents = crate::store::open_agent_store(&cas_root).expect("agent store");
        let mut supervisor = Agent::new("supervisor-id".to_string(), "supervisor".to_string());
        supervisor.role = AgentRole::Supervisor;
        supervisor.factory_session = Some("factory-commander-alias".to_string());
        agents.register(&supervisor).expect("register supervisor");
        let queue = crate::store::open_prompt_queue_store(&cas_root).expect("prompt queue");
        let commander = queue
            .enqueue_operator_message(
                "commander:Daniel@Soundwave",
                "supervisor",
                "What is status?",
                Some("factory-commander-alias"),
                Some("status"),
                Some(cas_store::NotificationPriority::Normal),
                false,
                None,
                &cas_store::OperatorStamp {
                    operator: "Daniel".into(),
                    device_id: "device-1".into(),
                    device_label: "Soundwave".into(),
                    scopes: vec!["message:send".into()],
                    verified: true,
                },
            )
            .expect("queue Commander message")
            .id();

        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        core.set_agent_id_for_testing(supervisor.id);
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        let request: AgentRequest = serde_json::from_value(serde_json::json!({
            "action": "message",
            "target": "commander:Daniel@Soundwave",
            "summary": "status response",
            "message": "Three workers are running; nothing is awaiting merge.",
        }))
        .expect("commander alias request");
        let response = response_text(
            service
                .message_send(request)
                .await
                .expect("commander alias queues successfully"),
        );
        assert!(response.contains(&format!("Answered notification_id: {commander}")), "{response}");
        assert!(response.contains("Device: device-1"), "{response}");
        let rows = queue
            .peek_operator_replies("factory-commander-alias", 10)
            .expect("read operator replies");
        let reply = rows.first().expect("operator reply row");
        assert_eq!(reply.recipient_device_id.as_deref(), Some("device-1"));
        let payload: crate::ui::factory::OperatorReplyPayload =
            serde_json::from_str(&reply.prompt).expect("operator reply payload");
        assert_eq!(payload.reply_to, Some(commander));
        assert_eq!(payload.kind, crate::ui::factory::OperatorTurnKind::Answer);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unknown_supervisor_target_is_refused_before_queueing() {
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", None),
            ("CAS_AGENT_NAME", None),
            ("CAS_FACTORY_SESSION", None),
            ("CAS_SUPERVISOR_NAME", None),
        ]);
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let agents = crate::store::open_agent_store(&cas_root).expect("agent store");
        let mut supervisor = Agent::new("supervisor-id".to_string(), "supervisor".to_string());
        supervisor.role = AgentRole::Supervisor;
        supervisor.factory_session = Some("factory-unknown-target".to_string());
        agents.register(&supervisor).expect("register supervisor");
        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        core.set_agent_id_for_testing(supervisor.id);
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        let request: AgentRequest = serde_json::from_value(serde_json::json!({
            "action": "message",
            "target": "lifecycle:manual",
            "summary": "should fail",
            "message": "This must never enter the queue.",
        }))
        .expect("unknown target request");
        let error = service
            .message_send(request)
            .await
            .expect_err("unknown target must be rejected");
        assert!(error.message.contains("Unknown message target"), "{error:?}");
        assert!(error.message.contains("'supervisor'"), "{error:?}");
        assert!(error.message.contains("'all_workers'"), "{error:?}");
        let rows = crate::store::open_prompt_queue_store(&cas_root)
            .expect("prompt queue")
            .peek_all(10)
            .expect("peek queue");
        assert!(rows.is_empty(), "unknown target must not enqueue: {rows:?}");
    }

    #[test]
    fn plain_names_are_distinct_from_reserved_row_sources() {
        for target in [
            "",
            "  ",
            "lifecycle:manual",
            "lifecycle-wake:42",
            "verification-dispatch:vd-1",
            "director-generated",
            "director-generated:task-1",
        ] {
            assert!(super::target_is_reserved_source(target), "{target:?}");
        }
        for target in [
            "not-born-yet",
            "lifecycle-worker",
            "verification-dispatcher",
        ] {
            assert!(!super::target_is_reserved_source(target), "{target}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn supervisor_operator_reply_preserves_paired_device_thread_route() {
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", None),
            ("CAS_AGENT_NAME", None),
            ("CAS_FACTORY_SESSION", None),
            ("CAS_SUPERVISOR_NAME", None),
        ]);
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let agents = crate::store::open_agent_store(&cas_root).expect("agent store");
        let mut supervisor = Agent::new("supervisor-id".to_string(), "supervisor".to_string());
        supervisor.role = AgentRole::Supervisor;
        supervisor.factory_session = Some("factory-operator".to_string());
        agents.register(&supervisor).expect("register supervisor");

        let queue = crate::store::open_prompt_queue_store(&cas_root).expect("prompt queue");
        let commander = queue
            .enqueue_urgent_with_outcome(
                "commander:Daniel@phone-7",
                "supervisor",
                "Please confirm the deployment.",
                Some("factory-operator"),
                Some("deployment status"),
                Some(cas_store::NotificationPriority::Normal),
                false,
                Some(&cas_store::QueueOrigin::PairedDevice {
                    device_id: "phone-7".to_string(),
                }),
            )
            .expect("queue Commander message")
            .id();

        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        core.set_agent_id_for_testing(supervisor.id);
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        let request: AgentRequest = serde_json::from_value(serde_json::json!({
            "action": "message",
            "target": "operator",
            "in_reply_to": commander,
            "summary": "deployment confirmed",
            "message": "The deployment is confirmed and healthy.",
        }))
        .expect("operator reply request");

        let response = response_text(
            service
                .message_send(request)
                .await
                .expect("supervisor reply queues successfully"),
        );
        assert!(response.contains("queued for phone-7"), "{response}");

        let rows = queue
            .peek_operator_replies("factory-operator", 10)
            .expect("read queued operator replies");
        let reply = rows.first().expect("operator reply row");
        assert_eq!(reply.source, "supervisor");
        assert_eq!(reply.factory_session.as_deref(), Some("factory-operator"));
        assert_eq!(reply.recipient_device_id.as_deref(), Some("phone-7"));
        let payload: crate::ui::factory::OperatorReplyPayload =
            serde_json::from_str(&reply.prompt).expect("operator reply payload");
        assert_eq!(payload.reply_to, Some(commander));
        assert_eq!(payload.device_id, "phone-7");
        assert_eq!(payload.message, "The deployment is confirmed and healthy.");
        assert_eq!(payload.schema_version, 2);
        assert_eq!(payload.kind, crate::ui::factory::OperatorTurnKind::Answer);
        assert!(payload.attachments.is_empty());
        assert_eq!(payload.reply_to_session, None, "same-session answer quotes nothing");
        let local = cas_store::SqlitePromptQueueStore::open(&cas_root).unwrap();
        let event = local.operator_delivery_event(reply.id).unwrap().unwrap();
        let snapshot: serde_json::Value = serde_json::from_str(&event.payload_snapshot).unwrap();
        assert_eq!(snapshot["prompt"], reply.prompt);
        assert_eq!(snapshot["recipient_device_id"], "phone-7");
        assert_eq!(snapshot["kind"], "answer");
        assert_eq!(snapshot["acknowledge_prompt_id"], commander);
        assert_eq!(event.audience_state, "unenrolled");
        assert!(queue.queued_prompt(commander).unwrap().unwrap().acked_at.is_some());
    }

    /// cas-e829: an answer belongs to the session it is sent from. A row from
    /// an earlier, ended session is only referenced; nothing queues to it.
    #[tokio::test(flavor = "current_thread")]
    async fn supervisor_answer_to_an_earlier_session_stays_in_its_own_session() {
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", None),
            ("CAS_AGENT_NAME", None),
            ("CAS_FACTORY_SESSION", None),
            ("CAS_SUPERVISOR_NAME", None),
        ]);
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let agents = crate::store::open_agent_store(&cas_root).expect("agent store");
        let mut ended = Agent::new("old-supervisor-id".to_string(), "wise-lion-31".to_string());
        ended.role = AgentRole::Supervisor;
        ended.status = cas_types::AgentStatus::Shutdown;
        ended.factory_session = Some("acct-wise-lion-31".to_string());
        agents.register(&ended).expect("register ended supervisor");
        let mut supervisor = Agent::new("supervisor-id".to_string(), "rapid-gazelle-52".to_string());
        supervisor.role = AgentRole::Supervisor;
        supervisor.factory_session = Some("acct-rapid-gazelle-52".to_string());
        agents.register(&supervisor).expect("register supervisor");

        let queue = crate::store::open_prompt_queue_store(&cas_root).expect("prompt queue");
        let earlier = queue
            .enqueue_urgent_with_outcome(
                "commander:Daniel@phone-7",
                "supervisor",
                "Did the import finish?",
                Some("acct-wise-lion-31"),
                Some("import"),
                Some(cas_store::NotificationPriority::Normal),
                false,
                Some(&cas_store::QueueOrigin::PairedDevice {
                    device_id: "phone-7".to_string(),
                }),
            )
            .expect("queue earlier-session Commander message")
            .id();

        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        core.set_agent_id_for_testing(supervisor.id);
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        let request: AgentRequest = serde_json::from_value(serde_json::json!({
            "action": "message",
            "target": "operator",
            "in_reply_to": earlier,
            "summary": "import finished",
            "message": "The import finished overnight.",
        }))
        .expect("operator reply request");
        service
            .message_send(request)
            .await
            .expect("cross-session answer queues in the sender's session");

        assert!(
            queue
                .peek_operator_replies("acct-wise-lion-31", 10)
                .expect("ended session replies")
                .is_empty(),
            "nothing queues to the ended session"
        );
        let rows = queue
            .peek_operator_replies("acct-rapid-gazelle-52", 10)
            .expect("current session replies");
        let reply = rows.first().expect("answer in the sender's session");
        assert_eq!(reply.factory_session.as_deref(), Some("acct-rapid-gazelle-52"));
        assert_eq!(reply.recipient_device_id.as_deref(), Some("phone-7"));
        let payload: crate::ui::factory::OperatorReplyPayload =
            serde_json::from_str(&reply.prompt).expect("operator reply payload");
        assert_eq!(payload.reply_to, Some(earlier), "the earlier turn is still referenced");
        assert_eq!(payload.reply_to_session.as_deref(), Some("acct-wise-lion-31"));
    }

    /// cas-e829: a supervisor with no session of its own cannot borrow the
    /// answered row's session; the answer is refused rather than queued to a
    /// session it was not sent from.
    #[tokio::test(flavor = "current_thread")]
    async fn sessionless_supervisor_answer_does_not_borrow_the_answered_session() {
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", None),
            ("CAS_AGENT_NAME", None),
            ("CAS_FACTORY_SESSION", None),
            ("CAS_SUPERVISOR_NAME", None),
        ]);
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let agents = crate::store::open_agent_store(&cas_root).expect("agent store");
        let mut supervisor = Agent::new("supervisor-id".to_string(), "supervisor".to_string());
        supervisor.role = AgentRole::Supervisor;
        supervisor.factory_session = None;
        agents.register(&supervisor).expect("register supervisor");

        let queue = crate::store::open_prompt_queue_store(&cas_root).expect("prompt queue");
        let asked = queue
            .enqueue_urgent_with_outcome(
                "commander:Daniel@phone-7",
                "supervisor",
                "Status?",
                Some("factory-elsewhere"),
                Some("status"),
                Some(cas_store::NotificationPriority::Normal),
                false,
                Some(&cas_store::QueueOrigin::PairedDevice {
                    device_id: "phone-7".to_string(),
                }),
            )
            .expect("queue Commander message")
            .id();

        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        core.set_agent_id_for_testing(supervisor.id);
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        let request: AgentRequest = serde_json::from_value(serde_json::json!({
            "action": "message",
            "target": "operator",
            "in_reply_to": asked,
            "summary": "status",
            "message": "All green.",
        }))
        .expect("operator reply request");
        let error = service
            .message_send(request)
            .await
            .expect_err("a sessionless answer is refused");
        assert!(error.message.contains("requires an active factory session"), "{error:?}");
        assert!(
            queue
                .peek_operator_replies("factory-elsewhere", 10)
                .expect("answered session replies")
                .is_empty()
        );
    }

    /// GH #1196: with the session's daemon reporting no Commander client and
    /// no cloud operator feed, an operator send fails at once and queues
    /// nothing; with a client connected, or no daemon status at all
    /// (unknown), it queues as before.
    #[tokio::test(flavor = "current_thread")]
    async fn gh_1196_operator_send_with_no_commander_attached_fails_at_once() {
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", None),
            ("CAS_AGENT_NAME", None),
            ("CAS_FACTORY_SESSION", None),
            ("CAS_SUPERVISOR_NAME", None),
        ]);
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let agents = crate::store::open_agent_store(&cas_root).expect("agent store");
        let mut supervisor = Agent::new("supervisor-id".to_string(), "supervisor".to_string());
        supervisor.role = AgentRole::Supervisor;
        supervisor.factory_session = Some("factory-no-commander".to_string());
        agents.register(&supervisor).expect("register supervisor");
        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        core.set_agent_id_for_testing(supervisor.id);
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        let request = || -> AgentRequest {
            serde_json::from_value(serde_json::json!({
                "action": "message",
                "target": "operator",
                "kind": "status",
                "summary": "build status",
                "message": "The build is green.",
            }))
            .expect("operator request")
        };
        let write_clients = |clients: Option<u64>| {
            let now = chrono::Utc::now();
            crate::factory_daemon_health::write_status(
                &cas_root,
                &crate::factory_daemon_health::DaemonLoopStatus {
                    pid: 1,
                    session: "factory-no-commander".into(),
                    written_at: now,
                    last_pass_at: now,
                    phase: "idle".into(),
                    passes: 1,
                    pending_spawns: 0,
                    in_flight_spawn: None,
                    in_flight_started_at: None,
                    loop_thread_wait: None,
                    helpers_killed: Vec::new(),
                    last_reset: None,
                    max_pass_ms: None,
                    p99_pass_ms: None,
                    window_passes: 0,
                    passes_over_100ms: 0,
                    phase_latency: Default::default(),
                    slow_steps: Vec::new(),
                    wedge: None,
                    job_lanes: Default::default(),
                    lane_violations: 0,
                    lane_violation_sites: Vec::new(),
                    frames: Default::default(),
                    frames_coalesced: 0,
                    frame_bytes_p50: None,
                    frame_window_ms: 0,
                    frames_per_s: None,
                    commander_clients: clients,
                },
            )
            .expect("write loop.json");
        };
        let queued = || {
            crate::store::open_prompt_queue_store(&cas_root)
                .expect("prompt queue")
                .peek_operator_replies("factory-no-commander", 10)
                .expect("read operator replies")
                .len()
        };

        write_clients(Some(0));
        let error = service
            .message_send(request())
            .await
            .expect_err("no Commander attached: refused at send time");
        assert!(
            error.message.contains("No Commander device is attached to factory session factory-no-commander"),
            "{error:?}"
        );
        assert!(error.message.contains("Nothing was queued"), "{error:?}");
        assert_eq!(queued(), 0, "nothing is queued to expire");

        write_clients(Some(1));
        let response = response_text(service.message_send(request()).await.expect("queued"));
        assert!(response.contains("Commander status queued"), "{response}");
        assert_eq!(queued(), 1);

        std::fs::remove_file(crate::factory_daemon_health::status_path(
            &cas_root,
            "factory-no-commander",
        ))
        .expect("remove loop.json");
        let response = response_text(service.message_send(request()).await.expect("queued"));
        assert!(response.contains("Commander status queued"), "unknown state still queues: {response}");
    }

    /// GH #1196 review M4/L8: each branch where the Commander state is not
    /// known to be "no device" lets the turn queue.
    #[test]
    fn gh_1196_operator_refusal_needs_a_known_live_state() {
        use crate::factory_daemon_health::{LOOP_WEDGED_SECS, STATUS_STALE_SECS};
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let store = cas_store::SqlitePromptQueueStore::open(&cas_root).expect("prompt queue");
        cas_store::PromptQueueStore::init(&store).expect("init prompt queue");
        let session = "factory-refusal";
        let written = chrono::Utc::now();
        let write = |cas_root: &std::path::Path, last_pass_at: chrono::DateTime<chrono::Utc>| {
            let mut status: crate::factory_daemon_health::DaemonLoopStatus =
                serde_json::from_value(serde_json::json!({
                    "pid": 1,
                    "session": session,
                    "written_at": written,
                    "last_pass_at": last_pass_at,
                    "phase": "idle",
                    "passes": 1,
                    "pending_spawns": 0,
                }))
                .expect("loop status");
            status.commander_clients = Some(0);
            crate::factory_daemon_health::write_status(cas_root, &status).expect("write loop.json");
        };
        let check = |now| operator_turn_undeliverable(&cas_root, session, now);

        write(&cas_root, written);
        assert!(check(written).is_some(), "fresh, live, no device, no feed: refused");
        let stale = written + chrono::Duration::seconds(STATUS_STALE_SECS + 1);
        assert!(check(stale).is_none(), "stale loop.json is unknown");

        write(&cas_root, written - chrono::Duration::seconds(LOOP_WEDGED_SECS + 1));
        assert!(check(written).is_none(), "a wedged loop's client count is unknown");

        write(&cas_root, written);
        store
            .bind_operator_feed(&cas_store::OperatorFeedBinding {
                account_id: "acct-1".into(),
                machine_id: "mch-1".into(),
                hub_id: "hub-1".into(),
                project_id: "proj-1".into(),
                bound_at: "2026-10-15T19:00:00Z".into(),
            })
            .expect("bind feed");
        assert!(check(written).is_none(), "a cloud-bound project always queues");

        let bad_root = temp.path().join("unreadable").join(".cas");
        std::fs::create_dir_all(&bad_root).expect("second Cassy root");
        std::fs::write(bad_root.join("cas.db"), b"not a sqlite database").expect("corrupt db");
        write(&bad_root, written);
        assert!(
            operator_turn_undeliverable(&bad_root, session, written).is_none(),
            "an unreadable binding is unknown, not absent"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn supervisor_can_send_unprompted_status_to_all_leased_devices() {
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", None),
            ("CAS_AGENT_NAME", None),
            ("CAS_FACTORY_SESSION", None),
            ("CAS_SUPERVISOR_NAME", None),
        ]);
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let agents = crate::store::open_agent_store(&cas_root).expect("agent store");
        let mut supervisor = Agent::new("supervisor-id".to_string(), "supervisor".to_string());
        supervisor.role = AgentRole::Supervisor;
        supervisor.factory_session = Some("factory-unprompted".to_string());
        agents.register(&supervisor).expect("register supervisor");
        let artifacts = cas_store::SqliteArtifactStore::open(&cas_root).expect("artifact store");
        artifacts
            .record_local(&cas_store::NewArtifact {
                id: "artifact-1".into(),
                task_id: "cas-ad6e".into(),
                name: "status.pdf".into(),
                mime: "application/pdf".into(),
                size_bytes: 42,
                sha256: "ab".repeat(32),
            })
            .expect("published artifact");

        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        core.set_agent_id_for_testing(supervisor.id);
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        let request: AgentRequest = serde_json::from_value(serde_json::json!({
            "action": "message",
            "target": "operator",
            "kind": "status",
            "summary": "build status",
            "message": "The build is green.",
            "attachment": "artifact-1",
        }))
        .expect("unprompted operator request");

        let response = response_text(
            service
                .message_send(request)
                .await
                .expect("unprompted status queues successfully"),
        );
        assert!(response.contains("Commander status queued"), "{response}");
        assert!(response.contains("Device: *"), "{response}");

        let rows = crate::store::open_prompt_queue_store(&cas_root)
            .expect("prompt queue")
            .peek_operator_replies("factory-unprompted", 10)
            .expect("read queued operator replies");
        let reply = rows.first().expect("unprompted reply row");
        assert_eq!(reply.recipient_device_id, None);
        let payload: crate::ui::factory::OperatorReplyPayload =
            serde_json::from_str(&reply.prompt).expect("operator reply payload");
        assert_eq!(payload.reply_to, None);
        assert_eq!(payload.device_id, "*");
        assert_eq!(payload.kind, crate::ui::factory::OperatorTurnKind::Status);
        assert_eq!(payload.attachments.len(), 1);
        assert_eq!(payload.attachments[0].artifact_id, "artifact-1");
        assert_eq!(payload.attachments[0].name, "status.pdf");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn supervisor_unprompted_turn_uses_latest_verified_commander_device() {
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", None),
            ("CAS_AGENT_NAME", None),
            ("CAS_FACTORY_SESSION", None),
            ("CAS_SUPERVISOR_NAME", None),
        ]);
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let agents = crate::store::open_agent_store(&cas_root).expect("agent store");
        let mut supervisor = Agent::new("supervisor-id".to_string(), "supervisor".to_string());
        supervisor.role = AgentRole::Supervisor;
        supervisor.factory_session = Some("factory-latest-device".to_string());
        agents.register(&supervisor).expect("register supervisor");

        let queue = crate::store::open_prompt_queue_store(&cas_root).expect("prompt queue");
        let operator = cas_store::OperatorStamp {
            operator: "Daniel".into(),
            device_id: "phone-9".into(),
            device_label: "laptop".into(),
            scopes: vec!["message:send".into()],
            verified: true,
        };
        queue
            .enqueue_operator_message(
                "commander:Daniel@laptop",
                "supervisor",
                "Status please",
                Some("factory-latest-device"),
                Some("status"),
                Some(cas_store::NotificationPriority::Normal),
                false,
                None,
                &operator,
            )
            .expect("verified Commander row");

        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        core.set_agent_id_for_testing(supervisor.id);
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        let request: AgentRequest = serde_json::from_value(serde_json::json!({
            "action": "message",
            "target": "operator",
            "kind": "ask",
            "summary": "need approval",
            "message": "Can you approve this release?",
        }))
        .expect("unprompted operator request");

        let response = response_text(
            service
                .message_send(request)
                .await
                .expect("unprompted ask queues successfully"),
        );
        assert!(response.contains("queued for phone-9"), "{response}");
        let rows = queue
            .peek_operator_replies("factory-latest-device", 10)
            .expect("read queued operator replies");
        let reply = rows.first().expect("routed ask row");
        assert_eq!(reply.recipient_device_id.as_deref(), Some("phone-9"));
        let payload: crate::ui::factory::OperatorReplyPayload =
            serde_json::from_str(&reply.prompt).expect("operator reply payload");
        assert_eq!(payload.reply_to, None);
        assert_eq!(payload.device_id, "phone-9");
        assert_eq!(payload.kind, crate::ui::factory::OperatorTurnKind::Ask);
    }

    /// cas-207e: an ask with a deadline and a default schedules the
    /// supervisor's proceed wake; half a deadline is refused before any write.
    #[tokio::test(flavor = "current_thread")]
    async fn operator_ask_with_deadline_schedules_the_proceed_wake_cas_207e() {
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", None),
            ("CAS_AGENT_NAME", None),
            ("CAS_FACTORY_SESSION", None),
            ("CAS_SUPERVISOR_NAME", None),
        ]);
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let agents = crate::store::open_agent_store(&cas_root).expect("agent store");
        let mut supervisor = Agent::new("supervisor-id".to_string(), "supervisor".to_string());
        supervisor.role = AgentRole::Supervisor;
        supervisor.factory_session = Some("factory-ask-deadline".to_string());
        agents.register(&supervisor).expect("register supervisor");
        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        core.set_agent_id_for_testing(supervisor.id);
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        let ask = |extra: serde_json::Value| -> AgentRequest {
            let mut request = serde_json::json!({
                "action": "message",
                "target": "operator",
                "kind": "ask",
                "summary": "approve the cut",
                "message": "Cut 3.54 now?",
            });
            request
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            serde_json::from_value(request).expect("operator ask request")
        };

        let refused = service
            .message_send(ask(serde_json::json!({ "remind_delay_secs": 900 })))
            .await
            .expect_err("a deadline without a default is refused");
        assert!(refused.message.contains("remind_message"), "{refused:?}");

        let response = response_text(
            service
                .message_send(ask(serde_json::json!({
                    "remind_delay_secs": 900,
                    "remind_message": "cut 3.54 from release-flow/d-354",
                })))
                .await
                .expect("ask with a deadline queues"),
        );
        assert!(response.contains("Deadline: reminder #"), "{response}");
        let reminders = crate::store::open_reminder_store(&cas_root)
            .expect("reminder store")
            .list_pending("supervisor-id")
            .expect("pending reminders");
        assert_eq!(reminders.len(), 1);
        let reminder = &reminders[0];
        assert_eq!(reminder.trigger_type, cas_store::ReminderTriggerType::Time);
        assert!(
            reminder
                .message
                .contains("Proceed now with the default: cut 3.54 from release-flow/d-354")
        );
        assert!(
            reminder
                .trigger_filter
                .as_ref()
                .is_some_and(|f| f.get("operator_ask").is_some())
        );
        let due_in = reminder.trigger_at.unwrap() - chrono::Utc::now();
        assert!(
            due_in > chrono::Duration::seconds(890) && due_in <= chrono::Duration::seconds(900)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn non_supervisor_cannot_send_operator_turns() {
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", None),
            ("CAS_AGENT_NAME", None),
            ("CAS_FACTORY_SESSION", None),
            ("CAS_SUPERVISOR_NAME", None),
        ]);
        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let agents = crate::store::open_agent_store(&cas_root).expect("agent store");
        let mut worker = Agent::new("worker-id".to_string(), "worker".to_string());
        worker.role = AgentRole::Worker;
        agents.register(&worker).expect("register worker");

        let core = crate::mcp::server::CasCore::with_daemon(cas_root, None, None);
        core.set_agent_id_for_testing(worker.id);
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        let request: AgentRequest = serde_json::from_value(serde_json::json!({
            "action": "message",
            "target": "operator",
            "kind": "blocker",
            "summary": "not allowed",
            "message": "This must not be routed.",
        }))
        .expect("operator request");
        let error = service
            .message_send(request)
            .await
            .expect_err("worker operator turn must be rejected");
        assert!(error.message.contains("Only a registered supervisor"), "{error:?}");
    }
}

/// cas-d5ab (communication contract v2, P1.6): how a send names the row's
/// current outcome. The end-to-end `ct2_p1_send_returns_named_outcome` drives
/// the real daemon (`ui::factory::daemon::runtime::busy_claude_delivery_tests`);
/// these pin the wording against the daemon's production stamp text.
#[cfg(test)]
mod cas_d5ab_send_outcome_tests {
    use super::*;
    use cas_store::PromptQueueStore as _;

    /// The daemon's generic hold stamp (queue_and_events.rs, `wake_deferred`).
    const GENERIC_HOLD: &str =
        "wake deferred — pane busy, tool call in flight, operator composing, or wake slot taken";

    #[test]
    fn a_deferral_names_the_wake_decision_not_the_generic_hold() {
        let cases = [
            ("wake_declined_by_policy: recipient has a tool call in flight", true, "recipient mid tool call; surfaces at its next tool call or turn start"),
            ("wake_declined_by_policy: pane has not been silent long enough", true, "recipient busy; surfaces at its next tool call or turn start"),
            ("wake_declined_by_policy: no transcript evidence and the pane has not been silent long enough", true, "recipient busy; surfaces at its next tool call or turn start"),
            ("wake_declined_by_policy: operator has an unsubmitted draft in the composer", false, "operator is typing in the recipient's pane; delivered when its pane is free"),
            ("wake_declined_by_policy: no wake slot left this pass (a supervisor wake already fired)", true, "another wake went first this pass; surfaces at its next tool call or turn start"),
        ];
        for (wake_detail, claude, expected) in cases {
            assert_eq!(deferral_reason(Some(wake_detail), Some(GENERIC_HOLD), claude), expected, "{wake_detail}");
        }
        assert_eq!(
            deferral_reason(None, Some(GENERIC_HOLD), true),
            "recipient busy; surfaces at its next tool call or turn start",
            "the generic hold alone names no single cause"
        );
        assert_eq!(
            deferral_reason(None, Some("pane not ready for injection"), false),
            "recipient pane not ready yet; delivered when its pane is free"
        );
    }

    #[test]
    fn a_stale_notify_socket_is_not_a_listening_daemon() {
        let root = tempfile::tempdir().unwrap();
        assert!(!daemon_accepts_notify(root.path()), "no socket");
        let path = cas_factory::notify_socket_path(root.path());
        drop(std::os::unix::net::UnixDatagram::bind(&path).unwrap());
        assert!(path.exists());
        assert!(!daemon_accepts_notify(root.path()), "a killed daemon's socket refuses");
        let _listening = std::os::unix::net::UnixDatagram::bind({
            std::fs::remove_file(&path).unwrap();
            &path
        })
        .unwrap();
        assert!(daemon_accepts_notify(root.path()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_row_the_daemon_set_aside_ends_the_wait_but_a_fresh_one_does_not() {
        let root = tempfile::tempdir().unwrap();
        let queue = cas_store::SqlitePromptQueueStore::open(root.path()).unwrap();
        queue.init().unwrap();
        let waiting = queue.enqueue_with_session("supervisor", "worker", "ping", "s").unwrap();
        queue
            .record_pending_reason(
                waiting,
                cas_store::PendingReason::AdapterRetryable,
                Some("transport busy; retrying"),
            )
            .unwrap();
        let started = std::time::Instant::now();
        let (report, timed_out) = wait_for_send_outcome(&queue, waiting).await;
        assert!(!timed_out && report.is_some());
        assert!(started.elapsed() < std::time::Duration::from_millis(200), "a stated wait settles at once");
        assert!(!send_outcome_settled(
            &queue
                .message_delivery_report(queue.enqueue_with_session("supervisor", "w", "fresh", "s").unwrap())
                .unwrap()
                .unwrap()
        ));
    }
}
