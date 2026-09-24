//! Session handoffs: one current handoff per project and role (GH #992,
//! cas-0339).
//!
//! A handoff is the note an outgoing session leaves for the next one. It is a
//! memory tagged `handoff` (the long-standing convention, now first class:
//! `memory action=remember entry_type=handoff` stores exactly this), with a
//! `role:<role>` tag naming whose handoff it is. The project is the store it
//! lives in.
//!
//! Handoffs are never merged or overwritten. Saving a new one *supersedes* the
//! previous current handoff for the same role: the old entry keeps its content
//! as history but gains a `superseded` tag and a `superseded-by:<id>` link, its
//! validity ends, and it drops out of the active tier. Session start injects
//! only the newest non-superseded handoff for the session's role, and never
//! ranks handoffs among ordinary memories, so a stale "CURRENT handoff" cannot
//! resurface.

use chrono::{DateTime, Utc};

use cas_types::{Entry, MemoryTier};

/// Tag that marks a memory as a handoff.
pub const HANDOFF_TAG: &str = "handoff";
/// Tag added to a handoff once a newer one replaced it.
pub const SUPERSEDED_TAG: &str = "superseded";
/// Prefix of the tag that links a superseded handoff to its successor.
pub const SUPERSEDED_BY_PREFIX: &str = "superseded-by:";
/// Prefix of the tag that names a handoff's role (`role:supervisor`).
pub const ROLE_TAG_PREFIX: &str = "role:";
/// Role of a handoff that names none. Every handoff written before roles
/// existed was a supervisor's session handoff.
pub const DEFAULT_HANDOFF_ROLE: &str = "supervisor";

/// Whether `entry` is a handoff.
pub fn is_handoff(entry: &Entry) -> bool {
    entry
        .tags
        .iter()
        .any(|tag| tag.trim().eq_ignore_ascii_case(HANDOFF_TAG))
}

/// Lower-cased role, or [`DEFAULT_HANDOFF_ROLE`] when none is given.
pub fn normalize_role(role: Option<&str>) -> String {
    role.map(str::trim)
        .filter(|role| !role.is_empty())
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| DEFAULT_HANDOFF_ROLE.to_string())
}

/// The role a handoff belongs to: its `role:<role>` tag, else the default.
pub fn handoff_role(entry: &Entry) -> String {
    let tagged = entry.tags.iter().find_map(|tag| {
        let tag = tag.trim();
        tag.get(..ROLE_TAG_PREFIX.len())
            .filter(|prefix| prefix.eq_ignore_ascii_case(ROLE_TAG_PREFIX))
            .map(|_| &tag[ROLE_TAG_PREFIX.len()..])
    });
    normalize_role(tagged)
}

/// Whether a handoff is history rather than the current one: superseded by a
/// newer handoff (or marked so by hand before this existed), archived, or past
/// its validity.
pub fn is_superseded(entry: &Entry) -> bool {
    entry.archived
        || entry.is_expired()
        || entry
            .tags
            .iter()
            .any(|tag| tag.trim().eq_ignore_ascii_case(SUPERSEDED_TAG))
        || entry
            .content
            .trim_start()
            .get(..SUPERSEDED_TAG.len())
            .is_some_and(|lead| lead.eq_ignore_ascii_case(SUPERSEDED_TAG))
}

/// The current handoff for `role` among `entries`: the newest handoff of that
/// role that is not superseded. Ties on `created` go to the larger id, so the
/// choice is stable.
pub fn current_handoff<'a>(entries: &'a [Entry], role: &str) -> Option<&'a Entry> {
    let role = normalize_role(Some(role));
    entries
        .iter()
        .filter(|entry| is_handoff(entry) && !is_superseded(entry) && handoff_role(entry) == role)
        .max_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)))
}

/// Make `tags` describe a handoff for `role`: add the `handoff` tag and, when
/// the caller named no role, a `role:<role>` tag. Returns the handoff's role.
pub fn tag_new_handoff(tags: &mut Vec<String>, role: Option<&str>) -> String {
    if !tags
        .iter()
        .any(|tag| tag.trim().eq_ignore_ascii_case(HANDOFF_TAG))
    {
        tags.push(HANDOFF_TAG.to_string());
    }
    let has_role = tags.iter().any(|tag| {
        tag.trim()
            .get(..ROLE_TAG_PREFIX.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(ROLE_TAG_PREFIX))
    });
    if !has_role {
        tags.push(format!("{ROLE_TAG_PREFIX}{}", normalize_role(role)));
    }
    let probe = Entry {
        tags: tags.clone(),
        ..Default::default()
    };
    handoff_role(&probe)
}

/// The handoffs a new handoff `new_id` of `role` supersedes: every other
/// current handoff of the same role.
pub fn handoffs_superseded_by<'a>(
    entries: &'a [Entry],
    new_id: &str,
    role: &str,
) -> Vec<&'a Entry> {
    let role = normalize_role(Some(role));
    entries
        .iter()
        .filter(|entry| {
            entry.id != new_id
                && is_handoff(entry)
                && !is_superseded(entry)
                && handoff_role(entry) == role
        })
        .collect()
}

/// Demote `entry` to history because `new_id` replaced it. Its content, title
/// and provenance are kept; it gains the `superseded` and
/// `superseded-by:<new_id>` tags, its validity ends at `now`, and it leaves
/// the active tier (a pinned handoff stops being pinned).
pub fn mark_superseded(entry: &mut Entry, new_id: &str, now: DateTime<Utc>) {
    if !entry
        .tags
        .iter()
        .any(|tag| tag.trim().eq_ignore_ascii_case(SUPERSEDED_TAG))
    {
        entry.tags.push(SUPERSEDED_TAG.to_string());
    }
    let link = format!("{SUPERSEDED_BY_PREFIX}{new_id}");
    if !entry.tags.contains(&link) {
        entry.tags.push(link);
    }
    entry.valid_until = Some(entry.valid_until.map_or(now, |until| until.min(now)));
    if matches!(
        entry.memory_tier,
        MemoryTier::InContext | MemoryTier::Working
    ) {
        entry.memory_tier = MemoryTier::Cold;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn handoff(id: &str, minutes_ago: i64, tags: &[&str], content: &str) -> Entry {
        Entry {
            id: id.to_string(),
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
            content: content.to_string(),
            created: Utc::now() - Duration::minutes(minutes_ago),
            ..Default::default()
        }
    }

    #[test]
    fn the_newest_unsuperseded_handoff_per_role_is_current() {
        let entries = vec![
            handoff(
                "old",
                90,
                &["summary", "handoff"],
                "CURRENT handoff from yesterday",
            ),
            handoff(
                "new",
                10,
                &["handoff", "role:supervisor"],
                "CURRENT handoff today",
            ),
            handoff("worker", 5, &["handoff", "role:worker"], "worker handoff"),
            handoff("note", 1, &["summary"], "not a handoff"),
        ];
        assert_eq!(
            current_handoff(&entries, "supervisor").map(|e| e.id.as_str()),
            Some("new")
        );
        assert_eq!(
            current_handoff(&entries, "Worker").map(|e| e.id.as_str()),
            Some("worker")
        );
        assert_eq!(current_handoff(&entries, "director"), None);
    }

    #[test]
    fn superseded_and_legacy_superseded_handoffs_are_never_current() {
        let mut marked = handoff("marked", 1, &["handoff"], "newest but superseded");
        mark_superseded(&mut marked, "later", Utc::now() - Duration::seconds(1));
        let entries = vec![
            handoff("keep", 30, &["handoff"], "the current one"),
            marked,
            handoff(
                "legacy",
                2,
                &["handoff"],
                "SUPERSEDED by handoff-2026-08-07-post-cas-a6fa: ...",
            ),
            Entry {
                archived: true,
                ..handoff("archived", 3, &["handoff"], "archived")
            },
        ];
        assert_eq!(
            current_handoff(&entries, "supervisor").map(|e| e.id.as_str()),
            Some("keep")
        );
    }

    #[test]
    fn a_new_handoff_supersedes_only_current_handoffs_of_its_role() {
        let entries = vec![
            handoff("sup-a", 60, &["handoff"], "untagged, so supervisor"),
            handoff(
                "sup-b",
                30,
                &["handoff", "role:supervisor"],
                "tagged supervisor",
            ),
            handoff("wrk", 20, &["handoff", "role:worker"], "worker"),
            handoff("sup-new", 0, &["handoff", "role:supervisor"], "the new one"),
        ];
        let ids: Vec<_> = handoffs_superseded_by(&entries, "sup-new", "supervisor")
            .into_iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(ids, ["sup-a", "sup-b"]);
    }

    #[test]
    fn superseding_keeps_content_as_history_and_leaves_the_active_tier() {
        let now = Utc::now();
        let mut entry = Entry {
            memory_tier: MemoryTier::InContext,
            title: Some("Handoff 2026-09-23".to_string()),
            ..handoff(
                "old",
                60,
                &["handoff", "role:supervisor"],
                "CURRENT handoff body",
            )
        };
        mark_superseded(&mut entry, "new", now);
        mark_superseded(&mut entry, "new", now);
        assert_eq!(entry.content, "CURRENT handoff body");
        assert_eq!(entry.title.as_deref(), Some("Handoff 2026-09-23"));
        assert_eq!(
            entry.tags,
            [
                "handoff",
                "role:supervisor",
                "superseded",
                "superseded-by:new"
            ]
        );
        assert_eq!(entry.valid_until, Some(now));
        assert_eq!(entry.memory_tier, MemoryTier::Cold);
        assert!(is_superseded(&entry));
    }

    #[test]
    fn tagging_a_new_handoff_adds_the_kind_and_the_session_role_once() {
        let mut tags = vec!["summary".to_string()];
        assert_eq!(tag_new_handoff(&mut tags, Some(" Worker ")), "worker");
        assert_eq!(tags, ["summary", "handoff", "role:worker"]);
        assert_eq!(tag_new_handoff(&mut tags, Some("supervisor")), "worker");
        assert_eq!(tags, ["summary", "handoff", "role:worker"]);

        let mut untagged = Vec::new();
        assert_eq!(tag_new_handoff(&mut untagged, None), "supervisor");
        assert_eq!(untagged, ["handoff", "role:supervisor"]);
    }
}
