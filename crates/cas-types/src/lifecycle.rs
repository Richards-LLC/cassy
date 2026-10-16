//! Typed task lifecycle relay envelope shared by transport and recipient drains.

use crate::TaskStatus;
use chrono::{DateTime, Utc};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleEnvelope {
    pub task_id: String,
    pub new_status: TaskStatus,
    pub occurrence: DateTime<Utc>,
    /// The lifecycle kind is optional for compatibility with envelopes from
    /// before the relay supersession contract was added.
    pub transition: Option<String>,
    /// Merge-boundary tip captured when an awaiting-merge/close-rejected relay
    /// was emitted. A later close cycle must not revive the old relay.
    pub branch_tip: Option<String>,
}

fn xml_attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!(" {name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

/// Parse a lifecycle relay without interpreting its notification id as a task id.
pub fn parse_lifecycle_envelope(prompt: &str) -> Option<LifecycleEnvelope> {
    let tag_end = prompt.find('>')?;
    let tag = &prompt[..tag_end];
    if !tag.starts_with("<task-lifecycle ") {
        return None;
    }
    let task_id = xml_attribute(tag, "task_id")?.to_string();
    let new_status = TaskStatus::from_str(xml_attribute(tag, "new")?).ok()?;
    let occurrence = DateTime::parse_from_rfc3339(xml_attribute(tag, "occurrence")?)
        .ok()?
        .with_timezone(&Utc);
    Some(LifecycleEnvelope {
        task_id,
        new_status,
        occurrence,
        transition: xml_attribute(tag, "transition").map(str::to_string),
        branch_tip: xml_attribute(tag, "branch_tip").map(str::to_string),
    })
}

