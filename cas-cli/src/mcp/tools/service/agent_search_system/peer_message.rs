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
            // cas-d58d: under `own`, a reply to another account's supervisor
            // of this repo is refused like a new message.
            if envelope.scope == peer_mailbox::PeerScope::Project
                && crate::cloud::peers::same_repo_scope(&cas_root)
                    == crate::config::SameRepoScope::Own
            {
                let (root, self_id) = (cas_root.clone(), sender.id.clone());
                let discovery =
                    tokio::task::spawn_blocking(move || discover_peers(&root, Some(&self_id)))
                        .await
                        .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
                let own = match discovery {
                    Ok(PeerDiscovery::Found { peers, .. }) => {
                        peers.iter().any(|peer| peer.agent_id == envelope.sender_agent_id)
                    }
                    _ => false,
                };
                if !own {
                    return Err(Self::error(
                        ErrorCode::INVALID_REQUEST,
                        format!(
                            "{}@{} is not a supervisor of your own Cassy Cloud account (or its \
                             account could not be confirmed); this project's \
                             peers.same_repo_scope is own, so only your own supervisors are \
                             messaged. Nothing was sent.",
                            envelope.sender_name, envelope.machine
                        ),
                    ));
                }
            }
            // GH #1319: a reply to a supervisor that has since ended (or whose
            // harness is logged out) goes to its successor, or is refused.
            // Best effort: an unreadable directory leaves the reply addressed
            // to the original sender, as before.
            let redirect = if envelope.scope == peer_mailbox::PeerScope::Project {
                let (root, self_id) = (cas_root.clone(), sender.id.clone());
                let discovery =
                    tokio::task::spawn_blocking(move || discover_peers(&root, Some(&self_id)))
                        .await
                        .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
                match discovery {
                    Ok(PeerDiscovery::Found { peers, ended, .. }) => {
                        let original = peers
                            .iter()
                            .find(|peer| peer.agent_id == envelope.sender_agent_id)
                            .map(|peer| (peer, false))
                            .or_else(|| {
                                ended
                                    .iter()
                                    .find(|peer| peer.agent_id == envelope.sender_agent_id)
                                    .map(|peer| (peer, true))
                            });
                        match original {
                            Some((original, was_ended)) => {
                                match Self::slot_holder(&peers, original, was_ended)? {
                                    (Some(successor), Some(note)) => Some((successor, note)),
                                    _ => None,
                                }
                            }
                            None => None,
                        }
                    }
                    _ => None,
                }
            } else {
                None
            };
            let (sender_id, sender_name) = (sender.id.clone(), sender.name.clone());
            let message = self.peer_body(message, attachment).await?;
            // cas-2a5a: a reply across projects is advisory too.
            let blocker = blocker && envelope.scope == peer_mailbox::PeerScope::Project;
            let (body, summary) = (
                peer_mailbox::attach_message_intent(&message, kind, blocker),
                summary.to_string(),
            );
            let (successor, note) = redirect.unzip();
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
                    successor.as_ref(),
                )
            })
            .await
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?
            .map_err(|error| Self::error(ErrorCode::INVALID_REQUEST, error))?;
            let note = note.map(|note| format!(" {note}")).unwrap_or_default();
            return Ok(reply.map(|reply| {
                Self::success(format!(
                    "Reply sent to peer supervisor {} via Cassy Cloud (peer message {}, {}); \
                     message {row_id} acknowledged.{note} Check delivery with coordination \
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
        let (peers, hidden, workers, ended) = match discovery {
            Ok(PeerDiscovery::Found {
                peers,
                hidden,
                workers,
                ended,
                ..
            }) => (peers, hidden, workers, ended),
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
        // cas-d58d: under `peers.same_repo_scope = own`, a name that is
        // another account's supervisor of this repo is refused, never
        // delivered and never tried as a cross-project or local target.
        let addressed = match crate::cloud::peers::resolve_same_repo_target(&peers, &hidden, &wanted)
            .map_err(|ambiguous| Self::error(ErrorCode::INVALID_PARAMS, ambiguous))?
        {
            crate::cloud::peers::SameRepoTarget::Peer(peer) => Some((peer.clone(), false)),
            crate::cloud::peers::SameRepoTarget::Refused(reason) => {
                return Err(Self::error(ErrorCode::INVALID_REQUEST, reason));
            }
            // GH #1319: a supervisor of this repo whose session ended.
            crate::cloud::peers::SameRepoTarget::NotFound => {
                crate::cloud::peers::ended_target(&ended, &wanted).map(|old| (old.clone(), true))
            }
        };
        let (peer, redirect) = match addressed {
            Some((addressed, ended)) => {
                let (successor, note) = Self::slot_holder(&peers, &addressed, ended)?;
                (Some(successor.unwrap_or(addressed)), note)
            }
            None => (None, None),
        };
        // cas-9f53: a live worker in a peer's session takes instructions only
        // from its own supervisor. Name that supervisor instead of "no such
        // worker", so a lane takeover has a channel.
        if peer.is_none()
            && let Some(worker) = workers.iter().find(|worker| worker.matches(&wanted))
        {
            let owner = &worker.supervisor;
            return Err(Self::error(
                ErrorCode::INVALID_REQUEST,
                format!(
                    "{}@{} is a worker in peer supervisor {}@{}'s factory session ({}); peer \
                     workers take messages only from their own supervisor. Message target={}@{} \
                     to relay it. Nothing was enqueued.",
                    worker.name,
                    worker.machine,
                    owner.name,
                    owner.machine,
                    owner.session.as_deref().unwrap_or("unknown"),
                    owner.name,
                    owner.machine
                ),
            ));
        }
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
        let redirect = redirect.map(|note| format!(" {note}")).unwrap_or_default();
        Ok(Some(Self::success(format!(
            "Message sent to peer supervisor {}@{} via Cassy Cloud (peer message {}, {}): delivered to {}@{} mailbox.{redirect}{staleness} \
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

    /// GH #1319: route a resolved target to whoever reads its mailbox. Returns
    /// the successor (if any) and the note telling the sender so; a target
    /// nobody can read is a typed refusal, never a delivered send.
    fn slot_holder(
        peers: &[crate::cloud::peers::Peer],
        addressed: &crate::cloud::peers::Peer,
        ended: bool,
    ) -> Result<(Option<crate::cloud::peers::Peer>, Option<String>), McpError> {
        use crate::cloud::peers::SlotHolder;
        match crate::cloud::peers::slot_holder(peers, addressed, ended) {
            SlotHolder::Addressed => Ok((None, None)),
            SlotHolder::Successor { peer, note } => Ok((Some(peer.clone()), Some(note))),
            SlotHolder::Refused(reason) => Err(Self::error(ErrorCode::INVALID_REQUEST, reason)),
        }
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
    /// Cassy Cloud user. Every failure is reported (GH #1319); `explicit` (a
    /// `supervisor:` target) only changes how an unknown name is described.
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
        // GH #1319: by here the target is no local agent and no supervisor of
        // this repo, so a failed lookup or an unknown name is the answer, not
        // a reason to fall through to a local "no supervisor or worker" error.
        let (owner, peers, ended) = match discovery {
            Ok(UserPeerDiscovery::Found {
                owner_user_id,
                peers,
                ended,
            }) => (owner_user_id, peers, ended),
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
            unchecked => {
                let why = match unchecked {
                    Err(error) => error.to_string(),
                    _ => "not logged in to Cassy Cloud".to_string(),
                };
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "no supervisor or worker named {target} in this project or its peers; \
                         your account's other projects could not be checked ({why}). Nothing \
                         was sent."
                    ),
                ));
            }
        };
        let addressed = match peer_mailbox::resolve_user_peer_target(&peers, target)
            .map_err(|ambiguous| Self::error(ErrorCode::INVALID_PARAMS, ambiguous))?
        {
            Some(peer) => Some((peer.clone(), false)),
            // Historical sessions use the same selectors as live account
            // peers, including project and project@machine. Multiple ended
            // sessions are history, so name the most recently seen match;
            // slot_holder still decides whether a live successor is safe.
            None => ended
                .iter()
                .filter(|old| {
                    peer_mailbox::resolve_user_peer_target(std::slice::from_ref(*old), target)
                        .is_ok_and(|peer| peer.is_some())
                })
                .max_by_key(|old| old.last_heartbeat)
                .map(|old| (old.clone(), true)),
        };
        let Some((addressed, was_ended)) = addressed else {
            let reason = if explicit {
                format!("'{target}' names no supervisor of your Cassy Cloud account")
            } else {
                format!(
                    "no supervisor or worker named {target} in this project or its peers: it is \
                     no local agent, and Cassy Cloud lists no live, recent or ended supervisor \
                     of this repo or your account by that name"
                )
            };
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!(
                    "{reason} (list them with coordination action=peers scope=all). Nothing \
                     was sent."
                ),
            ));
        };
        let (successor, redirect) = Self::slot_holder(&peers, &addressed, was_ended)?;
        let peer = successor.unwrap_or(addressed);
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
        let redirect = redirect.map(|note| format!(" {note}")).unwrap_or_default();
        Ok(Some(Self::success(format!(
            "Message sent to supervisor {}@{} of your project {} via Cassy Cloud (peer message \
             {}, {}).{redirect} It is advisory: never urgent, never the operator. Check delivery \
             with coordination action=message_status id={}.",
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

