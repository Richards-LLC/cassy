//! cas-f9c7: `coordination action=message` to a peer supervisor of this repo
//! on another machine, through the Cassy Cloud peer mailbox.
//!
//! cas-2a5a: `target=supervisor:<project>` (or a name no same-repo peer
//! has) reaches any supervisor of the caller's own Cassy Cloud user in
//! another project, never another user's. Same rules: advisory only.
//!
//! Only a registered supervisor may send. A peer message is never urgent and
//! never operator-authored, and it is only ever delivered to a supervisor, so
//! it cannot interrupt a pane or spawn or direct workers (cas-604d).

use crate::cloud::peer_mailbox::{self, PeerMailbox};
use crate::cloud::peers::{PeerDiscovery, UserPeerDiscovery, discover_peers, discover_user_peers};
use crate::mcp::tools::service::imports::*;

/// The local-only targets that never route to a peer.
const LOCAL_TARGETS: &[&str] = &["operator", "supervisor", "all_workers", "director", "owner"];

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
        kind: Option<&str>,
        blocker: bool,
        attachment: Option<&str>,
    ) -> Result<Option<CallToolResult>, McpError> {
        if LOCAL_TARGETS
            .iter()
            .any(|local| target.eq_ignore_ascii_case(local))
            || target.starts_with("commander:")
            || target.starts_with("inbox:")
            || super::message::target_is_reserved_source(target)
        {
            return Ok(None);
        }
        let cas_root = self.inner.cas_root.clone();
        let explicit_account_peer = target
            .get(..11)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("supervisor:"));
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
            let message = self.peer_body(message, attachment).await?;
            // cas-2a5a: a reply across projects is advisory too.
            let blocker = blocker && envelope.scope == peer_mailbox::PeerScope::Project;
            let (body, summary) = (
                peer_mailbox::attach_message_intent(&message, kind, blocker),
                summary.to_string(),
            );
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

        if explicit_account_peer {
            if urgent {
                return Err(refuse_urgent());
            }
            let message = self.peer_body(message, attachment).await?;
            return self
                .user_peer_message_send(sender, target, summary, &message, kind, true)
                .await;
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
            if self.message_target_has_pending_spawn(target, sender.factory_session.as_deref())? {
                return Ok(None);
            }
        }

        let (root, self_id, wanted) = (cas_root.clone(), sender.id.clone(), target.to_string());
        let discovery = tokio::task::spawn_blocking(move || discover_peers(&root, Some(&self_id)))
            .await
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        let peers = match discovery {
            Ok(PeerDiscovery::Found { peers, .. }) => peers,
            Ok(PeerDiscovery::NotLoggedIn) => {
                return Err(Self::error(
                    ErrorCode::INVALID_REQUEST,
                    format!(
                        "Could not resolve message target '{wanted}' through Cassy Cloud: no token \
                         (not logged in; `cas login`). Nothing was enqueued."
                    ),
                ));
            }
            Ok(PeerDiscovery::NoProjectIdentity) => {
                return Err(Self::error(
                    ErrorCode::INVALID_REQUEST,
                    format!(
                        "Could not resolve message target '{wanted}' through Cassy Cloud: this project has no canonical id. Nothing was enqueued."
                    ),
                ));
            }
            Err(error) => {
                return Err(Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!(
                        "Could not look up peer supervisors for '{wanted}': {error}. Nothing was enqueued."
                    ),
                ));
            }
        };
        let peer = peer_mailbox::resolve_peer_target(&peers, &wanted)
            .map_err(|ambiguous| Self::error(ErrorCode::INVALID_PARAMS, ambiguous))?
            .cloned();
        let Some(peer) = peer else {
            // cas-2a5a: not a supervisor of this repo; perhaps one of
            // another project of the same user. Unknown stays local.
            if urgent {
                return Ok(None);
            }
            let message = self.peer_body(message, attachment).await?;
            return self
                .user_peer_message_send(sender, target, summary, &message, kind, false)
                .await;
        };
        if peer
            .session
            .as_deref()
            .is_none_or(|session| session.trim().is_empty())
        {
            return Err(Self::error(
                ErrorCode::INVALID_REQUEST,
                format!(
                    "{}@{} is not running a factory daemon, so it cannot receive peer messages",
                    peer.name, peer.machine
                ),
            ));
        }
        if urgent {
            return Err(refuse_urgent());
        }
        let message = self.peer_body(message, attachment).await?;
        let (sender_id, body, summary) = (
            sender.id.clone(),
            peer_mailbox::attach_message_intent(&message, kind, blocker),
            summary.to_string(),
        );
        let to = peer.clone();
        let receipt = tokio::task::spawn_blocking(move || {
            peer_mailbox::send_to_peer(&cas_root, &sender_id, &to, &body, Some(&summary), None)
        })
        .await
        .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?
        .map_err(|error| {
            Self::error(
                ErrorCode::INVALID_REQUEST,
                format!(
                    "Could not deliver to {}@{} mailbox: {error}",
                    peer.name, peer.machine
                ),
            )
        })?;
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
            "Message sent to peer supervisor {}@{} via Cassy Cloud (peer message {}, {}): delivered to {}@{} mailbox.{staleness} \
             Check delivery with coordination action=message_status id={}.",
            peer.name,
            peer.machine,
            receipt.id,
            receipt.status,
            peer.name,
            peer.machine,
            receipt.id
        ))))
    }

    /// cas-2a5a: `message` with the pointer `attachment` names (a published
    /// artifact or a file in this repo), resolved only when sending to a peer.
    async fn peer_body(&self, message: &str, attachment: Option<&str>) -> Result<String, McpError> {
        let Some(raw) = attachment.map(str::trim).filter(|raw| !raw.is_empty()) else {
            return Ok(message.to_string());
        };
        let (root, raw) = (self.inner.cas_root.clone(), raw.to_string());
        let pointer =
            tokio::task::spawn_blocking(move || peer_mailbox::resolve_attachment(&root, &raw))
                .await
                .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?
                .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, error))?;
        Ok(peer_mailbox::attach_pointers(message, &[pointer]))
    }

    /// cas-2a5a: send to a supervisor of another project of the caller's own
    /// Cassy Cloud user. `explicit` (a `supervisor:` target) reports every
    /// failure; otherwise an unresolved target falls back to the local path.
    /// Never urgent, and a blocker is carried only as an ask: the recipient
    /// supervisor decides.
    async fn user_peer_message_send(
        &self,
        sender: &cas_types::Agent,
        target: &str,
        summary: &str,
        message: &str,
        kind: Option<&str>,
        explicit: bool,
    ) -> Result<Option<CallToolResult>, McpError> {
        let cas_root = self.inner.cas_root.clone();
        let (root, self_id) = (cas_root.clone(), sender.id.clone());
        let discovery =
            tokio::task::spawn_blocking(move || discover_user_peers(&root, Some(&self_id)))
                .await
                .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        let (owner, peers) = match discovery {
            Ok(UserPeerDiscovery::Found {
                owner_user_id,
                peers,
            }) => (owner_user_id, peers),
            Ok(UserPeerDiscovery::NotLoggedIn) if explicit => {
                return Err(Self::error(
                    ErrorCode::INVALID_REQUEST,
                    format!(
                        "Could not resolve '{target}': not logged in to Cassy Cloud (`cas \
                         login`). Nothing was sent."
                    ),
                ));
            }
            Err(error) if explicit => {
                return Err(Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!(
                        "Could not look up your account's supervisors for '{target}': {error}. \
                         Nothing was sent."
                    ),
                ));
            }
            _ => return Ok(None),
        };
        let peer = peer_mailbox::resolve_user_peer_target(&peers, target)
            .map_err(|ambiguous| Self::error(ErrorCode::INVALID_PARAMS, ambiguous))?
            .cloned();
        let Some(peer) = peer else {
            if !explicit {
                return Ok(None);
            }
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!(
                    "'{target}' names no supervisor of your Cassy Cloud account (list them \
                     with coordination action=peers scope=all). Nothing was sent."
                ),
            ));
        };
        let body = peer_mailbox::attach_message_intent(message, kind, false);
        let (sender_id, summary_text, to) = (sender.id.clone(), summary.to_string(), peer.clone());
        let receipt = tokio::task::spawn_blocking(move || {
            peer_mailbox::send_to_user_peer(
                &cas_root,
                &sender_id,
                &owner,
                &to,
                &body,
                Some(&summary_text),
            )
        })
        .await
        .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?
        .map_err(|error| {
            Self::error(
                ErrorCode::INVALID_REQUEST,
                format!(
                    "Could not deliver to {}@{} mailbox: {error}",
                    peer.name, peer.machine
                ),
            )
        })?;
        Ok(Some(Self::success(format!(
            "Message sent to supervisor {}@{} of your project {} via Cassy Cloud (peer message \
             {}, {}). It is advisory: never urgent, never the operator. Check delivery with \
             coordination action=message_status id={}.",
            peer.name,
            peer.machine,
            peer.project_name().unwrap_or_else(|| "unknown".into()),
            receipt.id,
            receipt.status,
            receipt.id
        ))))
    }

    /// Queue-on-registration is reserved for names CAS is actually spawning.
    pub(in crate::mcp::tools::service) fn message_target_has_pending_spawn(
        &self,
        target: &str,
        factory_session: Option<&str>,
    ) -> Result<bool, McpError> {
        crate::store::open_spawn_queue_store(&self.inner.cas_root)
            .and_then(|store| Ok(store.has_pending_spawn(target, factory_session)?))
            .map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Could not check pending local spawn for '{target}': {error}"),
                )
            })
    }

    /// `message_status id=<peer message id>`: the cloud delivery receipt of a
    /// message sent to a peer supervisor.
    pub(in crate::mcp::tools::service) async fn peer_message_status(
        &self,
        id: String,
    ) -> Result<CallToolResult, McpError> {
        let cas_root = self.inner.cas_root.clone();
        let wanted = id.clone();
        let text = tokio::task::spawn_blocking(move || -> Result<String, String> {
            if let Some(status) = peer_mailbox::outbox_status_text(&cas_root, &id)? {
                return Ok(status);
            }
            let status = peer_mailbox::http_mailbox(&cas_root)
                .ok_or_else(|| "not logged in to Cassy Cloud".to_string())?
                .status(&id)?;
            Ok(format!(
                "Peer message {}: {}{} (delivery attempts: {})",
                status.id,
                status.status,
                status
                    .delivered_at
                    .as_deref()
                    .map(|at| format!(" at {at}"))
                    .unwrap_or_default(),
                status.attempts
            ))
        })
        .await
        .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?
        .map_err(|error| {
            let visibility = if error.starts_with("peer message status refused (404)") {
                " is not visible to this account (or does not exist)"
            } else {
                " status could not be read"
            };
            Self::error(
                ErrorCode::INVALID_REQUEST,
                format!("Peer message {wanted}{visibility}: {error}"),
            )
        })?;
        Ok(Self::success(text))
    }
}
