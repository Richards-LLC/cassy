//! Fleet operations (fleet-operations brief S1, cas-566b).

/// The message an operator's "ask the supervisor to merge" sends: the task,
/// the branch it is parked on and the tip the operator saw.
pub(crate) fn request_merge_text(
    task_id: &str,
    title: &str,
    branch: Option<&str>,
    tip: Option<&str>,
) -> String {
    format!(
        "Operator request from Commander: please merge {task_id} ({title}).\n\
         Branch: {}\nTip: {}\n\
         It is awaiting merge. Merge it into its epic, or reply with what blocks it.",
        branch.unwrap_or("<not recorded>"),
        tip.unwrap_or("<not recorded>"),
    )
}
