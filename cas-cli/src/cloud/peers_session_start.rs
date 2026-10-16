//! cas-97ff (GH #1215 §1): a supervisor's SessionStart names the other live
//! supervisors of its repo, so a `/clear` never hides a live peer.
//!
//! The block is rendered from the same [`PeerReport`] that `cas peers` and
//! `coordination action=peers` render ([`discover_peer_report`]), so the two
//! views cannot disagree about who is live, where, or what they claim.

use std::path::Path;
use std::time::Duration;

use super::peers_detail::{ClaimsVisibility, PeerDetail, PeerReport, discover_peer_report};

/// SessionStart waits this long for Cassy Cloud, then reports peers unknown.
/// Bounded separately from the per-request discovery timeout, which applies
/// to each of the report's requests.
pub const SESSION_START_PEERS_DEADLINE: Duration = Duration::from_secs(3);

/// Live peers listed by name; more collapse into a count.
pub const SESSION_START_PEER_ROWS: usize = 5;

/// Hard ceiling on the block, well inside the SessionStart budget.
pub const SESSION_START_PEERS_MAX_BYTES: usize = 1_200;

/// Longest value copied from a peer's cloud metadata.
const FIELD_MAX_CHARS: usize = 40;

/// Longest lookup-failure cause shown on the "unknown" line.
const CAUSE_MAX_CHARS: usize = 120;

/// Section heading.
pub const HEADING: &str = "## Live peer supervisors on this repo (Cassy Cloud)";

/// The rendered block and its compact form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStartPeers {
    pub full: String,
    pub compact: String,
}

/// One line of untrusted cloud metadata: no control characters (so no
/// newline can open a fake section), at most `max` characters.
fn clean(value: &str, max: usize) -> String {
    let mut out: String = value.chars().filter(|c| !c.is_control()).collect();
    if out.chars().count() > max {
        out = out.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
    }
    out
}

/// The peers board, as a published `coordination` call. Its claims and
/// workers view (`detail`) is not on coordination's published surface, so
/// the block points at the CLI for that ([`DETAIL_COMMAND`]).
fn command(tool_prefix: &str) -> String {
    format!("{tool_prefix}coordination action=peers")
}

/// Each peer's claims, workers and branches, from the same report.
pub const DETAIL_COMMAND: &str = "cas peers --detail";

/// The report's lookup failed: one line, never "no peers". `None` when the
/// project does not use Cassy Cloud (not logged in, or no repo identity).
fn unknown_line(report: &PeerReport, tool_prefix: &str) -> Option<Option<String>> {
    match report.status.as_str() {
        "not_logged_in" | "no_project_identity" => Some(None),
        "lookup_failed" => Some(Some(unknown(
            &format!(
                "Cassy Cloud lookup failed: {}",
                report.error.as_deref().unwrap_or("unknown error")
            ),
            tool_prefix,
        ))),
        _ => None,
    }
}

fn unknown(cause: &str, tool_prefix: &str) -> String {
    format!(
        "Live peer supervisors on this repo: unknown ({}). Check with `{}`.",
        clean(cause, CAUSE_MAX_CHARS),
        command(tool_prefix)
    )
}

fn claims(detail: &PeerDetail) -> String {
    match detail.claims_visibility {
        ClaimsVisibility::SameAccount if detail.work.claims_status.state == "ready" => {
            let count = detail.claims.len();
            format!("{count} claimed task{}", if count == 1 { "" } else { "s" })
        }
        ClaimsVisibility::DifferentAccount => "claims private (other account)".into(),
        _ => "claims unknown".into(),
    }
}

fn counts(report: &PeerReport) -> (usize, usize) {
    let live = report.peers.iter().filter(|p| p.supervisor.live).count();
    (live, report.peers.len() - live)
}

/// The SessionStart block for `report`: `None` when the cloud has no live
/// peer of this repo, or the project does not use Cassy Cloud at all; one
/// "unknown" line when the lookup failed. Never claims there are no peers.
pub fn render_session_start_peers(report: &PeerReport, tool_prefix: &str) -> Option<String> {
    if let Some(line) = unknown_line(report, tool_prefix) {
        return line;
    }
    let (live, stale) = counts(report);
    if live == 0 {
        return None;
    }
    let mut out = String::from(HEADING);
    for detail in report.peers.iter().filter(|p| p.supervisor.live).take(SESSION_START_PEER_ROWS) {
        let peer = &detail.supervisor;
        // cas-d58d: name the account each peer runs as.
        out.push_str(&format!(
            "\n- {} on {}, owner {}: heartbeat {}s ago, focus {}, {}",
            clean(&peer.name, FIELD_MAX_CHARS),
            clean(&peer.machine, FIELD_MAX_CHARS),
            clean(&super::peers::owner_label(peer), FIELD_MAX_CHARS),
            peer.heartbeat_age_secs,
            clean(peer.focus.as_deref().unwrap_or("none"), FIELD_MAX_CHARS),
            claims(detail),
        ));
    }
    // cas-a131: same files in flight into the same target as a peer.
    for overlap in super::peer_merge::render_overlaps(&report.overlaps) {
        out.push('\n');
        out.push_str(&overlap);
    }
    if live > SESSION_START_PEER_ROWS {
        out.push_str(&format!("\n+{} more live", live - SESSION_START_PEER_ROWS));
    }
    if stale > 0 {
        out.push_str(&format!("\n{stale} stale (seen in the last 24h, not live)"));
    }
    if report.status == "partial" {
        out.push_str("\nClaims could not be read for every peer.");
    }
    out.push_str(&format!(
        "\nTheir claims and work: `{DETAIL_COMMAND}`. Coordinate before taking the same tasks."
    ));
    Some(out)
}

/// Compact form for the SessionStart budget: counts and the command.
pub fn compact_session_start_peers(report: &PeerReport, tool_prefix: &str) -> Option<String> {
    if let Some(line) = unknown_line(report, tool_prefix) {
        return line;
    }
    let (live, stale) = counts(report);
    (live > 0).then(|| {
        format!(
            "{HEADING}\n{live} live, {stale} stale; run `{DETAIL_COMMAND}`"
        )
    })
}

/// Look the peers up for a supervisor's SessionStart, within
/// [`SESSION_START_PEERS_DEADLINE`]. The lookup runs on its own thread; a
/// cloud that does not answer in time is reported as unknown, never as
/// "no peers".
pub fn session_start_peers(
    cas_root: &Path,
    self_id: Option<&str>,
    tool_prefix: &str,
) -> Option<SessionStartPeers> {
    // Projects that do not reach Cassy Cloud never start a lookup thread.
    let logged_in = super::CloudConfig::load_from_cas_dir(cas_root)
        .is_ok_and(|config| config.is_logged_in());
    if !logged_in || super::resolve_canonical_id(cas_root).is_none() {
        return None;
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    let (root, self_id_owned) = (cas_root.to_path_buf(), self_id.map(str::to_string));
    std::thread::Builder::new()
        .name("cas-session-start-peers".into())
        .spawn(move || {
            let _ = sender.send(discover_peer_report(&root, self_id_owned.as_deref(), true, None));
        })
        .ok()?;
    let report = match receiver.recv_timeout(SESSION_START_PEERS_DEADLINE) {
        Ok(Ok(report)) => report,
        Ok(Err(error)) => {
            let line = unknown(&format!("Cassy Cloud lookup failed: {error}"), tool_prefix);
            return Some(SessionStartPeers {
                compact: line.clone(),
                full: line,
            });
        }
        Err(_) => {
            let line = unknown(
                &format!(
                    "Cassy Cloud did not answer within {}s",
                    SESSION_START_PEERS_DEADLINE.as_secs()
                ),
                tool_prefix,
            );
            return Some(SessionStartPeers {
                compact: line.clone(),
                full: line,
            });
        }
    };
    let full = render_session_start_peers(&report, tool_prefix)?;
    let compact = compact_session_start_peers(&report, tool_prefix).unwrap_or_else(|| full.clone());
    Some(SessionStartPeers { full, compact })
}

/// [`session_start_peers`] for a SessionStart of `role`: supervisors only.
/// Workers and unknown roles get nothing and make no request.
pub fn session_start_peers_for_role(
    role: Option<&str>,
    cas_root: &Path,
    self_id: Option<&str>,
    tool_prefix: &str,
) -> Option<SessionStartPeers> {
    if role != Some("supervisor") {
        return None;
    }
    session_start_peers(cas_root, self_id, tool_prefix)
}

#[cfg(test)]
#[path = "peers_session_start_tests.rs"]
mod tests;
