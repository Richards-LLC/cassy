//! cas-f9c7: `coordination action=message` to a peer supervisor of this repo
//! on another machine, through the Cassy Cloud peer mailbox.
//!
//! Only a registered supervisor may send. A peer message is never urgent and
//! never operator-authored, and it is only ever delivered to a supervisor, so
//! it cannot interrupt a pane or spawn or direct workers (cas-604d).

use crate::cloud::peer_mailbox::{self, PeerMailbox};
use crate::cloud::peers::{PeerDiscovery, discover_peers};
use crate::mcp::tools::service::imports::*;

/// The local-only targets that never route to a peer.
const LOCAL_TARGETS: &[&str] = &["operator", "supervisor", "all_workers"];

impl CasService {
    /// Route a supervisor's message to a peer supervisor when it addresses
    /// one: a reply (`in_reply_to` naming a delivered peer message) or a
    /// target that is not a local agent but names a peer of this repo.
    /// `Ok(None)` leaves the message to the local path.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::mcp::tools::service) async fn peer_message_send(
        &self,
        sender: &cas_types::Agent,
        target: &str,
        summary: &str,
        message: &str,
        in_reply_to: Option<i64>,
        urgent: bool,
    ) -> Result<Option<CallToolResult>, McpError> {
        if LOCAL_TARGETS
            .iter()
            .any(|local| target.eq_ignore_ascii_case(local))
            || target.starts_with("commander:")
        {
            return Ok(None);
        }
        let cas_root = self.inner.cas_root.clone();
        let refuse_urgent = || {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "a message to a peer supervisor on another machine cannot be urgent: peers \
                 never interrupt each other's panes",
            )
        };

        // A reply to a delivered peer message goes back to its sender.
        if let Some(row_id) = in_reply_to {
            let queue = crate::store::open_prompt_queue_store(&cas_root).map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to open prompt queue: {error}"),
                )
            })?;
            let peer_row = queue
                .queued_prompt(row_id)
                .ok()
                .flatten()
                .filter(|row| row.origin == Some(cas_store::QueueOrigin::Daemon))
                .and_then(|row| peer_mailbox::parse_envelope(&row.prompt));
            let Some(envelope) = peer_row else {
                return Ok(None);
            };
            let addressed = target.eq_ignore_ascii_case(&envelope.sender_name)
                || target.eq_ignore_ascii_case(&format!(
                    "{}@{}",
                    envelope.sender_name, envelope.machine
                ));
            if !addressed {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "in_reply_to={row_id} is a message from peer supervisor {}@{}; reply \
                         with target={}@{}",
                        envelope.sender_name,
                        envelope.machine,
                        envelope.sender_name,
                        envelope.machine
                    ),
                ));
            }
            if urgent {
                return Err(refuse_urgent());
            }
            let (sender_id, sender_name) = (sender.id.clone(), sender.name.clone());
            let (body, summary) = (message.to_string(), summary.to_string());
            let reply = tokio::task::spawn_blocking(move || {
                let queue = crate::store::open_prompt_queue_store(&cas_root)
                    .map_err(|error| format!("could not open the prompt queue: {error}"))?;
                peer_mailbox::reply_to_peer_row(
                    &cas_root,
                    queue.as_ref(),
                    row_id,
                    &sender_id,
                    &sender_name,
                    &body,
                    Some(&summary),
                )
            })
            .await
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?
            .map_err(|error| Self::error(ErrorCode::INVALID_REQUEST, error))?;
            return Ok(reply.map(|reply| {
                Self::success(format!(
                    "Reply sent to peer supervisor {} via Cassy Cloud (peer message {}, {}); \
                     message {row_id} acknowledged. Check delivery with coordination \
                     action=message_status id={}.",
                    reply.to, reply.receipt.id, reply.receipt.status, reply.receipt.id
                ))
            }));
        }

        // A target naming a local agent stays local.
        let qualified = target.contains('@');
        if !qualified {
            let local = crate::store::open_agent_store(&cas_root)
                .ok()
                .and_then(|store| store.list(None).ok())
                .is_some_and(|agents| {
                    agents
                        .iter()
                        .any(|agent| agent.name.eq_ignore_ascii_case(target))
                });
            if local {
                return Ok(None);
            }
        }

        let (root, self_id, wanted) = (cas_root.clone(), sender.id.clone(), target.to_string());
        let discovery = tokio::task::spawn_blocking(move || discover_peers(&root, Some(&self_id)))
            .await
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        let peers = match discovery {
            Ok(PeerDiscovery::Found { peers, .. }) => peers,
            Ok(PeerDiscovery::NotLoggedIn) if qualified => {
                return Err(Self::error(
                    ErrorCode::INVALID_REQUEST,
                    format!(
                        "'{wanted}' names a peer supervisor, but this project is not logged in \
                         to Cassy Cloud (`cas login`)"
                    ),
                ));
            }
            Ok(_) => return Ok(None),
            Err(error) if qualified => {
                return Err(Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Could not look up peer supervisors: {error}"),
                ));
            }
            Err(_) => return Ok(None),
        };
        let peer = peer_mailbox::resolve_peer_target(&peers, &wanted)
            .map_err(|ambiguous| Self::error(ErrorCode::INVALID_PARAMS, ambiguous))?
            .cloned();
        let Some(peer) = peer else {
            return Ok(None);
        };
        if urgent {
            return Err(refuse_urgent());
        }
        let (sender_id, body, summary) =
            (sender.id.clone(), message.to_string(), summary.to_string());
        let to = peer.clone();
        let receipt = tokio::task::spawn_blocking(move || {
            peer_mailbox::send_to_peer(&cas_root, &sender_id, &to, &body, Some(&summary), None)
        })
        .await
        .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?
        .map_err(|error| Self::error(ErrorCode::INVALID_REQUEST, error))?;
        let staleness = if peer.live {
            String::new()
        } else {
            format!(
                " The peer's last heartbeat was {}s ago; it will receive this when its daemon \
                 next claims.",
                peer.heartbeat_age_secs
            )
        };
        Ok(Some(Self::success(format!(
            "Message sent to peer supervisor {}@{} via Cassy Cloud (peer message {}, {}).{staleness} \
             Check delivery with coordination action=message_status id={}.",
            peer.name, peer.machine, receipt.id, receipt.status, receipt.id
        ))))
    }

    /// `message_status id=<peer message id>`: the cloud delivery receipt of a
    /// message sent to a peer supervisor.
    pub(in crate::mcp::tools::service) async fn peer_message_status(
        &self,
        id: String,
    ) -> Result<CallToolResult, McpError> {
        let cas_root = self.inner.cas_root.clone();
        let status = tokio::task::spawn_blocking(move || {
            peer_mailbox::http_mailbox(&cas_root)
                .ok_or_else(|| "not logged in to Cassy Cloud".to_string())?
                .status(&id)
        })
        .await
        .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?
        .map_err(|error| Self::error(ErrorCode::INVALID_REQUEST, error))?;
        Ok(Self::success(format!(
            "Peer message {}: {}{} (delivery attempts: {})",
            status.id,
            status.status,
            status
                .delivered_at
                .as_deref()
                .map(|at| format!(" at {at}"))
                .unwrap_or_default(),
            status.attempts
        )))
    }
}
