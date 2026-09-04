use crate::mcp::tools::service::imports::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LinkedLifecycleAck {
    pub durable_notification_id: i64,
    pub prompt_id: Option<i64>,
}

/// Acknowledge a lifecycle/death notification through both durable queue
/// identities. The public ID printed in lifecycle envelopes belongs to
/// `supervisor_queue`; daemon redelivery is driven by a linked prompt row with
/// a different autoincrement sequence. Both `message_ack` and `queue_ack` call
/// this bridge so the documented ID cannot confirm an unrelated numeric row.
pub(crate) fn acknowledge_linked_lifecycle_notification(
    cas_root: &std::path::Path,
    notification_id: i64,
) -> Result<Option<LinkedLifecycleAck>, String> {
    let supervisor_queue = crate::store::open_supervisor_queue_store(cas_root)
        .map_err(|error| format!("open supervisor queue: {error}"))?;
    let Some(notification) = supervisor_queue
        .get(notification_id)
        .map_err(|error| format!("lookup durable notification: {error}"))?
    else {
        return Ok(None);
    };
    let dedupe_key = match notification.event_type.as_str() {
        "task_lifecycle" => {
            crate::mcp::tools::core::task::lifecycle::supervisor_push::lifecycle_prompt_dedupe_key(
                notification_id,
            )
        }
        "worker_died" => format!("worker-died-outbox:{notification_id}"),
        // Worker attention wakes use the durable supervisor notification ID
        // in their public `lifecycle-wake:worker-attention:<id>` source, but
        // their outbox key is distinct from a task lifecycle transition.  If
        // this bridge does not recognise them, `message_ack notification_id`
        // acknowledges only the durable row and leaves the linked prompt
        // relay visible in every later worker_status response.
        "worker_idle"
        | "worker_stalled"
        | "worker_delivery_stalled"
        | "worker_unavailable"
        | "supervisor_unread" => {
            format!("worker-attention-outbox:{notification_id}")
        }
        _ => return Ok(None),
    };

    if notification.processed_at.is_none() {
        supervisor_queue
            .ack(notification_id)
            .map_err(|error| format!("ack durable notification: {error}"))?;
    }
    let prompt_queue = crate::store::open_prompt_queue_store(cas_root)
        .map_err(|error| format!("open prompt queue: {error}"))?;
    let prompt_id = prompt_queue
        .ack_by_dedupe_key(&dedupe_key)
        .map_err(|error| format!("ack linked prompt: {error}"))?;

    Ok(Some(LinkedLifecycleAck {
        durable_notification_id: notification_id,
        prompt_id,
    }))
}

impl CasService {
    pub(in crate::mcp::tools::service) async fn queue_notify(
        &self,
        req: AgentRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::{NotificationPriority, open_supervisor_queue_store};

        let supervisor_id = req.supervisor_id.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "supervisor_id required for queue_notify",
            )
        })?;
        let event_type = req.event_type.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "event_type required for queue_notify",
            )
        })?;
        let payload = req.payload.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "payload required for queue_notify",
            )
        })?;

        let priority = match req.priority.as_deref() {
            Some("critical") | Some("0") => NotificationPriority::Critical,
            Some("high") | Some("1") => NotificationPriority::High,
            _ => NotificationPriority::Normal,
        };

        let queue = open_supervisor_queue_store(&self.inner.cas_root).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open queue: {error}"),
            )
        })?;

        let notification_id = queue
            .notify(&supervisor_id, &event_type, &payload, priority)
            .map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to queue notification: {error}"),
                )
            })?;

        Ok(Self::success(format!(
            "Notification queued successfully\n\nID: {notification_id}\nSupervisor: {supervisor_id}\nType: {event_type}\nPriority: {priority:?}"
        )))
    }

    pub(in crate::mcp::tools::service) async fn queue_poll(
        &self,
        req: AgentRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::open_supervisor_queue_store;

        let supervisor_id = req
            .supervisor_id
            .or_else(|| self.inner.get_agent_id().ok())
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "supervisor_id required for queue_poll (or register as an agent first)",
                )
            })?;
        let limit = req.limit.unwrap_or(10);

        let queue = open_supervisor_queue_store(&self.inner.cas_root).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open queue: {error}"),
            )
        })?;

        let notifications = queue.poll(&supervisor_id, limit).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to poll queue: {error}"),
            )
        })?;

        if notifications.is_empty() {
            return Ok(Self::success("No pending notifications"));
        }

        let mut output = format!(
            "Polled {} notification(s) (marked as processed):\n\n",
            notifications.len()
        );
        for notification in &notifications {
            output.push_str(&format!(
                "**[{}]** {} - {:?}\n  Payload: {}\n  Created: {}\n\n",
                notification.id,
                notification.event_type,
                notification.priority,
                notification.payload,
                notification.created_at.format("%H:%M:%S")
            ));
        }

        Ok(Self::success(output))
    }

    pub(in crate::mcp::tools::service) async fn queue_peek(
        &self,
        req: AgentRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::open_supervisor_queue_store;

        let supervisor_id = req
            .supervisor_id
            .or_else(|| self.inner.get_agent_id().ok())
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "supervisor_id required for queue_peek (or register as an agent first)",
                )
            })?;
        let limit = req.limit.unwrap_or(10);

        let queue = open_supervisor_queue_store(&self.inner.cas_root).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open queue: {error}"),
            )
        })?;

        let notifications = queue.peek(&supervisor_id, limit).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to peek queue: {error}"),
            )
        })?;

        if notifications.is_empty() {
            return Ok(Self::success("No pending notifications"));
        }

        let mut output = format!(
            "Peeked {} pending notification(s):\n\n",
            notifications.len()
        );
        for notification in &notifications {
            output.push_str(&format!(
                "**[{}]** {} - {:?}\n  Payload: {}\n  Created: {}\n\n",
                notification.id,
                notification.event_type,
                notification.priority,
                notification.payload,
                notification.created_at.format("%H:%M:%S")
            ));
        }
        output.push_str(
            "Use `queue_poll` to process or `queue_ack` to acknowledge individual notifications.",
        );

        Ok(Self::success(output))
    }

    pub(in crate::mcp::tools::service) async fn queue_ack(
        &self,
        req: AgentRequest,
    ) -> Result<CallToolResult, McpError> {
        let notification_id = req.notification_id.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "notification_id required for queue_ack (lifecycle relay IDs also confirm the linked message delivery row)",
            )
        })?;

        if let Some(linked) =
            acknowledge_linked_lifecycle_notification(&self.inner.cas_root, notification_id)
                .map_err(|error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to acknowledge linked lifecycle notification: {error}"),
                    )
                })?
        {
            return Ok(Self::success(format!(
                "Lifecycle notification {} acknowledged across durable and prompt queues{}",
                linked.durable_notification_id,
                linked
                    .prompt_id
                    .map(|id| format!(" (linked message_id: {id})"))
                    .unwrap_or_else(|| " (linked prompt already terminal or absent)".to_string())
            )));
        }

        use crate::store::open_supervisor_queue_store;

        let queue = open_supervisor_queue_store(&self.inner.cas_root).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open queue: {error}"),
            )
        })?;

        queue.ack(notification_id).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to acknowledge: {error}"),
            )
        })?;

        Ok(Self::success(format!(
            "Notification {notification_id} acknowledged"
        )))
    }
}

#[cfg(test)]
mod cas_20ac_ack_tests {
    use super::*;
    use crate::store::{
        NotificationPriority, open_prompt_queue_store, open_supervisor_queue_store,
    };
    use cas_store::EnqueueIdempotentResult;

    /// The embedded durable ID and the prompt row ID intentionally diverge.
    /// Acknowledging the exact visible notification must confirm BOTH lanes,
    /// never an unrelated prompt with a coincident numeric ID.
    #[test]
    fn visible_lifecycle_notification_id_acks_its_linked_prompt() {
        let temp = tempfile::tempdir().expect("tempdir");
        let prompt_queue = open_prompt_queue_store(temp.path()).expect("prompt queue");
        let supervisor_queue = open_supervisor_queue_store(temp.path()).expect("supervisor queue");

        // Force the two table sequences apart and leave prompt id=1 unrelated.
        let unrelated = prompt_queue
            .enqueue("worker", "supervisor", "unrelated ordinary message")
            .expect("unrelated prompt");
        assert_eq!(unrelated, 1);
        let durable_id = supervisor_queue
            .notify(
                "supervisor-id",
                "worker_died",
                "{}",
                NotificationPriority::Critical,
            )
            .expect("durable notice");
        assert_eq!(durable_id, 1);
        let linked_prompt_id = match prompt_queue
            .enqueue_idempotent(
                "lifecycle-wake:worker-died:1",
                "supervisor",
                "<worker-died worker_id=\"w\" worker_name=\"lost\" incident=\"i\" notification_id=\"1\">\nHeld at death: none\nParked back to Open: none\n</worker-died>",
                None,
                Some("worker died: lost"),
                Some(NotificationPriority::Critical),
                "worker-died-outbox:1",
                None,
            )
            .expect("linked prompt")
        {
            EnqueueIdempotentResult::Created(id)
            | EnqueueIdempotentResult::AlreadyExists(id) => id,
        };
        assert_eq!(linked_prompt_id, 2);

        let ack = acknowledge_linked_lifecycle_notification(temp.path(), durable_id)
            .expect("ack bridge")
            .expect("lifecycle row");
        assert_eq!(ack.prompt_id, Some(linked_prompt_id));
        assert!(
            supervisor_queue
                .get(durable_id)
                .expect("lookup")
                .expect("row")
                .processed_at
                .is_some()
        );

        let unrelated_report = prompt_queue
            .message_delivery_report(unrelated)
            .expect("unrelated report")
            .expect("unrelated row");
        assert!(
            unrelated_report.confirmed_at.is_none(),
            "numeric collision must not acknowledge the unrelated prompt"
        );
        let linked_report = prompt_queue
            .message_delivery_report(linked_prompt_id)
            .expect("linked report")
            .expect("linked row");
        assert!(
            linked_report.confirmed_at.is_some(),
            "linked lifecycle prompt must be terminal after message_ack/queue_ack"
        );
        assert_eq!(
            linked_report.confirmation_source,
            cas_store::ConfirmationSource::ExplicitAck
        );

        // Shared helper is idempotent, matching both public ACK actions.
        let repeated = acknowledge_linked_lifecycle_notification(temp.path(), durable_id)
            .expect("repeat ack")
            .expect("lifecycle row");
        assert_eq!(repeated.prompt_id, Some(linked_prompt_id));
    }

    /// Worker attention relays use a different event type and source spelling
    /// than task lifecycle/death relays.  The printed durable ID must still
    /// acknowledge the linked prompt, otherwise a consumed idle notice is
    /// replayed forever by `worker_status`.
    #[test]
    fn visible_worker_attention_id_acks_its_linked_prompt() {
        let temp = tempfile::tempdir().expect("tempdir");
        let prompt_queue = open_prompt_queue_store(temp.path()).expect("prompt queue");
        let supervisor_queue = open_supervisor_queue_store(temp.path()).expect("supervisor queue");

        let durable_id = supervisor_queue
            .notify(
                "supervisor-id",
                "worker_delivery_stalled",
                r#"{"worker":"quiet-ibis"}"#,
                NotificationPriority::High,
            )
            .expect("durable notice");
        let prompt_id = match prompt_queue
            .enqueue_idempotent(
                &format!("lifecycle-wake:worker-attention:{durable_id}"),
                "supervisor",
                &format!(
                    "<worker-attention kind=\"worker_delivery_stalled\" worker=\"quiet-ibis\" notification_id=\"{durable_id}\">\\ndelivery stalled\\n</worker-attention>"
                ),
                None,
                Some("worker_delivery_stalled: quiet-ibis"),
                Some(NotificationPriority::High),
                &format!("worker-attention-outbox:{durable_id}"),
                None,
            )
            .expect("linked prompt")
        {
            EnqueueIdempotentResult::Created(id) | EnqueueIdempotentResult::AlreadyExists(id) => id,
        };
        prompt_queue
            .mark_undelivered_lifecycle_relay(prompt_id, Some("worker shut down before delivery"))
            .expect("terminal relay");
        assert_eq!(
            prompt_queue
                .list_undelivered_lifecycle_relays(10)
                .expect("terminal relay list")
                .len(),
            1,
            "the banner must count the exact relay before its printed ID is acknowledged"
        );

        let ack = acknowledge_linked_lifecycle_notification(temp.path(), durable_id)
            .expect("ack bridge")
            .expect("worker attention lifecycle row");
        assert_eq!(ack.prompt_id, Some(prompt_id));
        assert!(
            prompt_queue
                .message_delivery_report(prompt_id)
                .expect("report")
                .expect("linked row")
                .confirmed_at
                .is_some(),
            "the linked relay must be terminal after its printed durable ID is acknowledged"
        );
        let reopened = open_prompt_queue_store(temp.path()).expect("reopen prompt queue");
        assert!(
            reopened
                .list_undelivered_lifecycle_relays(10)
                .expect("reopened terminal relay list")
                .is_empty(),
            "an acknowledged worker-attention relay must remain absent after a store reopen"
        );
    }

    #[test]
    fn every_worker_attention_kind_acks_its_linked_prompt() {
        for kind in [
            "worker_idle",
            "worker_stalled",
            "worker_delivery_stalled",
            "worker_unavailable",
        ] {
            let temp = tempfile::tempdir().expect("tempdir");
            let prompt_queue = open_prompt_queue_store(temp.path()).expect("prompt queue");
            let supervisor_queue =
                open_supervisor_queue_store(temp.path()).expect("supervisor queue");
            let durable_id = supervisor_queue
                .notify("supervisor-id", kind, "{}", NotificationPriority::High)
                .expect("durable notice");
            let prompt_id = match prompt_queue
                .enqueue_idempotent(
                    &format!("lifecycle-wake:worker-attention:{durable_id}"),
                    "supervisor",
                    &format!(
                        "<worker-attention kind=\"{kind}\" worker=\"quiet-ibis\" notification_id=\"{durable_id}\">\\nrelay\\n</worker-attention>"
                    ),
                    None,
                    Some(&format!("{kind}: quiet-ibis")),
                    Some(NotificationPriority::High),
                    &format!("worker-attention-outbox:{durable_id}"),
                    None,
                )
                .expect("linked prompt")
            {
                EnqueueIdempotentResult::Created(id) | EnqueueIdempotentResult::AlreadyExists(id) => id,
            };

            let ack = acknowledge_linked_lifecycle_notification(temp.path(), durable_id)
                .expect("ack bridge")
                .expect("attention event must be bridged");
            assert_eq!(ack.prompt_id, Some(prompt_id), "{kind}");
        }
    }
}
