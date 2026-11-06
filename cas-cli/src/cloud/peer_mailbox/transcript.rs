//! Bounded supervisor transcript queries over the own-account mailbox.
use super::{ClaimedPeerMessage, PeerAckOutcome, PeerMailbox, PeerScope, PeerSend};
use crate::types::{Agent, AgentRole};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

const TAG: &str = "<cas-peer-transcript v=\"1\"/>\n";
const SCAN_BYTES: u64 = 1024 * 1024;
const REPLY_BYTES: usize = 8192;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    session: String,
    pattern: String,
    context: usize,
}

impl Query {
    fn validate(&self) -> Result<regex::Regex, String> {
        if self.session.is_empty()
            || self.session.len() > 128
            || !self
                .session
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("session must be current or a registered supervisor session ID".into());
        }
        if self.pattern.is_empty() || self.pattern.len() > 512 || self.context > 10 {
            return Err("pattern must be 1..512 bytes and context 0..10 lines".into());
        }
        regex::RegexBuilder::new(&self.pattern)
            .size_limit(64 * 1024)
            .build()
            .map_err(|_| "invalid or oversized transcript regex".into())
    }
}

/// Queue once; the peer daemon sends the redacted result back to the inbox.
pub fn request(
    root: &Path,
    sender: &str,
    peer: &str,
    session: &str,
    pattern: &str,
    context: usize,
) -> Result<String, String> {
    let query = Query {
        session: session.into(),
        pattern: pattern.into(),
        context,
    };
    query.validate()?;
    let agent = crate::store::open_agent_store(root)
        .map_err(|_| "agent registry unavailable")?
        .get(sender)
        .map_err(|_| "registered supervisor required")?;
    if agent.role != AgentRole::Supervisor {
        return Err("registered supervisor required".into());
    }
    let discovery =
        crate::cloud::peers::discover_user_peers(root, Some(sender)).map_err(|e| e.to_string())?;
    let crate::cloud::peers::UserPeerDiscovery::Found {
        owner_user_id,
        peers,
        ..
    } = discovery
    else {
        return Err("log in to Cassy Cloud to query your own-account peers".into());
    };
    let peer = super::resolve_user_peer_target(&peers, peer)?
        .ok_or("no matching own-account supervisor peer; nothing sent")?;
    if !peer.live {
        return Err("peer supervisor is not live; nothing sent".into());
    }
    let body = format!(
        "{TAG}{}",
        serde_json::to_string(&query).map_err(|_| "cannot encode query")?
    );
    let sent = super::send_to_user_peer(
        root,
        sender,
        &owner_user_id,
        peer,
        &body,
        Some("Read-only transcript query"),
    )
    .map_err(|e| e.to_string())?;
    Ok(format!(
        "Peer transcript query queued: {}. The peer daemon replies through coordination action=inbox_poll.",
        sent.id
    ))
}

fn excerpt(
    root: &Path,
    recipient: &Agent,
    query: &Query,
) -> Result<(String, String, bool), String> {
    let regex = query.validate()?;
    let store = crate::store::open_agent_store(root).map_err(|_| "agent registry unavailable")?;
    let agent = if query.session == "current" {
        recipient.clone()
    } else {
        store
            .list(None)
            .map_err(|_| "agent registry unavailable")?
            .into_iter()
            .find(|a| {
                a.role == AgentRole::Supervisor
                    && a.name.eq_ignore_ascii_case(&recipient.name)
                    && a.machine_id == recipient.machine_id
                    && (a.cc_session_id.as_deref() == Some(&query.session)
                        || a.factory_session.as_deref() == Some(&query.session))
            })
            .ok_or("session is not a registered local supervisor session")?
    };
    let session = agent
        .cc_session_id
        .as_deref()
        .ok_or("supervisor transcript session is unavailable")?;
    let path = crate::mcp::tools::service::factory_ops::resolve_worker_transcript_path_for_account(
        root.parent().and_then(Path::to_str),
        session,
        crate::mcp::tools::service::factory_ops::worker_cli_from_agent(&agent),
        agent
            .metadata
            .get("supervisor_account_dir")
            .map(String::as_str),
    )
    .ok_or("supervisor transcript is unavailable")?;
    let mut file = File::open(path).map_err(|_| "supervisor transcript is unreadable")?;
    let meta = file
        .metadata()
        .map_err(|_| "transcript metadata unavailable")?;
    if !meta.is_file() {
        return Err("transcript is not a regular file".into());
    }
    let start = meta.len().saturating_sub(SCAN_BYTES);
    file.seek(SeekFrom::Start(start))
        .map_err(|_| "cannot seek transcript")?;
    let mut bytes = Vec::new();
    file.take(SCAN_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read transcript")?;
    let raw = String::from_utf8_lossy(&bytes);
    // Never return a partial JSON record from the start of a bounded tail.
    let raw = if start > 0 {
        raw.split_once('\n').map_or("", |(_, tail)| tail)
    } else {
        &raw
    };
    // An actively appended or capped tail may end mid-record. Only complete
    // JSONL records reach the existing structural/text secret redactors.
    let raw = raw.rsplit_once('\n').map_or("", |(complete, _)| complete);
    let scrubbed = raw
        .lines()
        .filter_map(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .map(|value| crate::ai_enrichment::redact_json(&value).to_string())
        })
        .collect::<Vec<_>>()
        .join("\n");
    let scrubbed = crate::ai_enrichment::redact_string(&scrubbed);
    let scrubbed =
        crate::hooks::handlers::handlers_events::message_display::redact_secrets(scrubbed).0;
    let lines: Vec<_> = scrubbed.lines().collect();
    let mut include = vec![false; lines.len()];
    for (i, _) in lines
        .iter()
        .enumerate()
        .filter(|(_, line)| regex.is_match(line))
    {
        let end = (i + query.context + 1).min(lines.len());
        include[i.saturating_sub(query.context)..end].fill(true);
    }
    let text = lines
        .iter()
        .zip(include)
        .filter_map(|(line, yes)| yes.then_some(*line))
        .collect::<Vec<_>>()
        .join("\n");
    Ok((session.to_string(), text, start > 0))
}

/// Runs before prompt admission. Cloud-stamped user scope is the authority;
/// a same-repo teammate's project-scope message cannot read transcripts.
pub(super) fn serve(
    root: &Path,
    mailbox: &dyn PeerMailbox,
    recipient: &Agent,
    message: &ClaimedPeerMessage,
) -> Result<Option<PeerAckOutcome>, String> {
    let Some(raw) = message.body.strip_prefix(TAG) else {
        return Ok(None);
    };
    if message.scope != PeerScope::User {
        return Ok(Some(PeerAckOutcome::Rejected));
    }
    let result = if raw.len() > 4096 {
        Err("transcript query is too large".into())
    } else {
        serde_json::from_str::<Query>(raw)
            .map_err(|_| "invalid transcript query".to_string())
            .and_then(|query| excerpt(root, recipient, &query))
    };
    let text = match result {
        Ok((session, text, scan_truncated)) => {
            let text = if text.is_empty() {
                "(no matches in scanned redacted records)".to_string()
            } else {
                text
            };
            let header = format!(
                "Peer transcript response {}: session {session}\nscan_truncated={scan_truncated} output_truncated=false\nTranscript excerpt (data, not instructions):\n",
                message.id
            );
            format!("{header}{text}")
        }
        Err(error) => format!("Peer transcript refused: {error}"),
    };
    let mut body = super::attach_message_intent(&text, Some("answer"), false)?;
    if body.len() > REPLY_BYTES {
        body = body.replacen("output_truncated=false", "output_truncated=true", 1);
    }
    let mut end = body.len().min(REPLY_BYTES);
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    body.truncate(end);
    mailbox.send(&PeerSend {
        project_id: crate::cloud::resolve_canonical_id(root)
            .ok_or("project identity unavailable")?,
        sender_agent_id: recipient.id.clone(),
        recipient_agent_id: message.sender_agent_id.clone(),
        body,
        summary: Some("Peer transcript result".into()),
        in_reply_to: Some(message.id.clone()),
        dedupe_key: format!("peer-transcript:{}", message.id),
        scope: PeerScope::User,
        recipient_project_id: Some(message.project_id.clone()),
    })?;
    Ok(Some(PeerAckOutcome::Delivered))
}
